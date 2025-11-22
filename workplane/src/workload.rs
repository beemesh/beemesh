use std::sync::{Arc, RwLock};

use anyhow::{Result, anyhow};
use serde_json::{Map, Value};
use tokio::task::JoinHandle;
use tokio::{select, sync::watch, time::interval};
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::discovery::ServiceRecord;
use crate::manifest::{self, WorkloadKind, WorkloadManifest};
use crate::network::Network;
use crate::raft::{LeadershipUpdate, RaftManager, RaftRole};
use crate::selfheal::SelfHealer;
use crate::streams::{RPCRequest, RPCResponse, register_stream_handler, send_request};

pub struct Workload {
    cfg: Config,
    pub network: Network,
    pub raft: Option<Arc<RwLock<RaftManager>>>,
    selfhealer: Option<SelfHealer>,
    manifest: Arc<Vec<u8>>,
    manifest_spec: Arc<WorkloadManifest>,
    leader_state: Arc<RwLock<Option<String>>>,
    leader_monitor: Option<LeaderMonitor>,
}

impl Workload {
    pub fn new(cfg: Config, manifest: Vec<u8>) -> Result<Self> {
        if cfg.private_key.is_empty() {
            return Err(anyhow!("config: private key is required"));
        }
        if cfg.workload_name.is_empty() || cfg.pod_name.is_empty() {
            return Err(anyhow!("config: workload and pod name required"));
        }
        let mut cfg = cfg;
        cfg.apply_defaults();

        let manifest_spec = Arc::new(manifest::parse_workload_manifest(&manifest)?);

        if manifest_spec.replicas > 0 {
            cfg.replicas = manifest_spec.replicas;
        }

        let network = Network::new(&cfg)?;
        cfg.peer_id_str = Some(network.peer_id());

        let manifest = Arc::new(manifest);

        let leader_state = Arc::new(RwLock::new(None));
        let handler_network = network.clone();
        let local_peer_id = handler_network.peer_id();
        let leader_state_for_handler = leader_state.clone();
        register_stream_handler(Arc::new(move |remote: &ServiceRecord, req: RPCRequest| {
            if req.method.eq_ignore_ascii_case("healthz") {
                let mut body = Map::new();
                body.insert("remote".to_string(), Value::String(remote.peer_id.clone()));
                if let Some(pod) = &remote.pod_name {
                    body.insert("pod".to_string(), Value::String(pod.clone()));
                }
                return RPCResponse {
                    ok: true,
                    error: None,
                    body,
                };
            }

            let leader = leader_state_for_handler
                .read()
                .expect("leader state")
                .clone();
            let leader_matches = leader
                .as_ref()
                .map(|leader_id| leader_id == &local_peer_id)
                .unwrap_or(false);
            if req.leader_only && !leader_matches {
                if let Some(leader_id) = leader {
                    if let Some(target) = handler_network
                        .find_service_peers()
                        .into_iter()
                        .find(|record| record.peer_id == leader_id)
                    {
                        debug!(target = %target.peer_id, method = %req.method, "proxying follower request to leader");
                        match send_request(&target, req.clone()) {
                            Ok(resp) => return resp,
                            Err(err) => {
                                warn!(error = %err, method = %req.method, "failed to proxy request to leader");
                                crate::increment_counter!("workplane.raft.follower_rejections");
                                return RPCResponse {
                                    ok: false,
                                    error: Some("leader unreachable".to_string()),
                                    body: Map::new(),
                                };
                            }
                        }
                    } else {
                        warn!(leader = %leader_id, "no WDHT record for leader to proxy request");
                    }
                }
                crate::increment_counter!("workplane.raft.follower_rejections");
                return RPCResponse {
                    ok: false,
                    error: Some("not leader".to_string()),
                    body: Map::new(),
                };
            }

            let mut body = Map::new();
            body.insert(
                "handled_by".to_string(),
                Value::String(local_peer_id.clone()),
            );
            RPCResponse {
                ok: true,
                error: None,
                body,
            }
        }));

        let mut raft = None;
        if manifest_spec.kind == WorkloadKind::StatefulSet {
            let mut manager = RaftManager::new();
            let peer_ids = network
                .find_service_peers()
                .into_iter()
                .map(|p| p.peer_id)
                .collect::<Vec<_>>();
            let update = manager.bootstrap_if_needed(&network.peer_id(), &peer_ids);
            *leader_state.write().expect("leader state") = update.leader_id.clone();
            raft = Some(Arc::new(RwLock::new(manager)));
        } else {
            *leader_state.write().expect("leader state") = Some(network.peer_id());
        }

        let selfhealer = Some(SelfHealer::new(
            network.clone(),
            cfg.clone(),
            manifest.clone(),
            manifest_spec.clone(),
        ));

        Ok(Self {
            cfg,
            network,
            raft,
            selfhealer,
            manifest,
            manifest_spec,
            leader_state,
            leader_monitor: None,
        })
    }

    pub fn start(&mut self) {
        self.network.start();
        if let Some(sh) = self.selfhealer.as_mut() {
            sh.start();
        }
        if self.is_stateful() {
            self.start_leader_monitor();
        }
    }

    pub async fn close(&mut self) {
        if let Some(monitor) = self.leader_monitor.take() {
            monitor.stop().await;
        }
        if let Some(sh) = self.selfhealer.as_mut() {
            sh.stop().await;
        }
        self.network.shutdown().await;
        info!("workload shut down");
    }

    pub fn manifest(&self) -> Arc<Vec<u8>> {
        self.manifest.clone()
    }

    pub fn is_stateful(&self) -> bool {
        self.manifest_spec.kind == WorkloadKind::StatefulSet
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    pub fn manifest_spec(&self) -> &WorkloadManifest {
        self.manifest_spec.as_ref()
    }

    fn start_leader_monitor(&mut self) {
        if self.leader_monitor.is_some() {
            return;
        }
        let Some(raft) = self.raft.clone() else {
            return;
        };

        let monitor = LeaderMonitor::new(
            self.cfg.clone(),
            self.network.clone(),
            raft,
            self.leader_state.clone(),
        );
        self.leader_monitor = Some(monitor);
    }
}

struct LeaderMonitor {
    stop_tx: watch::Sender<bool>,
    handle: JoinHandle<()>,
}

impl LeaderMonitor {
    fn new(
        cfg: Config,
        network: Network,
        raft: Arc<RwLock<RaftManager>>,
        leader_state: Arc<RwLock<Option<String>>>,
    ) -> Self {
        let (tx, mut rx) = watch::channel(false);
        let interval_duration = cfg.replica_check_interval;
        let handle = tokio::spawn(async move {
            let mut ticker = interval(interval_duration);
            ticker.tick().await;
            loop {
                select! {
                    _ = ticker.tick() => {
                        let records = network.find_service_peers();
                        let relevant: Vec<ServiceRecord> = records
                            .into_iter()
                            .filter(|record| record.workload_id == cfg.workload_id())
                            .collect();
                        let update = {
                            let mut guard = raft.write().expect("raft lock");
                            guard.update_from_records(&relevant)
                        };
                        {
                            let mut guard = leader_state.write().expect("leader state");
                            *guard = update.leader_id.clone();
                        }
                        LeaderMonitor::apply_network_update(&network, &update);
                    }
                    changed = rx.changed() => {
                        if changed.is_ok() && *rx.borrow() {
                            break;
                        }
                    }
                }
            }
        });
        Self {
            stop_tx: tx,
            handle,
        }
    }

    async fn stop(self) {
        let _ = self.stop_tx.send(true);
        let _ = self.handle.await;
    }

    fn apply_network_update(network: &Network, update: &LeadershipUpdate) {
        if update.role_changed {
            match update.local_role {
                RaftRole::Leader => {
                    if let Err(err) = network.update_leader_endpoint(true, update.epoch) {
                        warn!(error = %err, "failed to advertise leader endpoint");
                    }
                }
                _ => {
                    if let Err(err) = network.update_leader_endpoint(false, update.epoch) {
                        warn!(error = %err, "failed to withdraw leader endpoint");
                    }
                }
            }
        } else if update.changed && update.local_role != RaftRole::Leader {
            // Ensure followers retract any stale leader advertisement.
            if let Err(err) = network.update_leader_endpoint(false, update.epoch) {
                warn!(error = %err, "failed to keep follower endpoint inactive");
            }
        }
    }
}

use std::sync::Arc;

use anyhow::{Result, anyhow};
use serde_json::{Map, Value};
use tracing::info;

use crate::config::Config;
use crate::discovery::ServiceRecord;
use crate::manifest::{self, WorkloadKind, WorkloadManifest};
use crate::network::Network;
use crate::raft::RaftManager;
use crate::self_heal::SelfHealer;
use crate::streams::{RPCRequest, RPCResponse, register_stream_handler};

pub struct Workload {
    cfg: Config,
    pub network: Network,
    pub raft: Option<RaftManager>,
    self_healer: Option<SelfHealer>,
    manifest: Arc<Vec<u8>>,
    manifest_spec: Arc<WorkloadManifest>,
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

        register_stream_handler(Arc::new(|remote: &ServiceRecord, req: RPCRequest| {
            if req.method.eq_ignore_ascii_case("healthz") {
                let mut body = Map::new();
                body.insert("remote".to_string(), Value::String(remote.peer_id.clone()));
                if let Some(pod) = &remote.pod_name {
                    body.insert("pod".to_string(), Value::String(pod.clone()));
                }
                RPCResponse {
                    ok: true,
                    error: None,
                    body,
                }
            } else {
                RPCResponse {
                    ok: false,
                    error: Some("unknown method".to_string()),
                    body: Map::new(),
                }
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
            manager.bootstrap_if_needed(&network.peer_id(), &peer_ids);
            raft = Some(manager);
        }

        let self_healer = Some(SelfHealer::new(
            network.clone(),
            cfg.clone(),
            manifest.clone(),
            manifest_spec.clone(),
        ));

        Ok(Self {
            cfg,
            network,
            raft,
            self_healer,
            manifest,
            manifest_spec,
        })
    }

    pub fn start(&mut self) {
        self.network.start();
        if let Some(sh) = self.self_healer.as_mut() {
            sh.start();
        }
    }

    pub async fn close(&mut self) {
        if let Some(sh) = self.self_healer.as_mut() {
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
}

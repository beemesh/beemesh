use std::collections::HashSet;
use std::sync::{Arc, Mutex, RwLock};

use anyhow::{Context, Result, anyhow};
use libp2p::identity::Keypair;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::interval;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::config::Config;
use crate::discovery::{self, ServiceRecord};

#[derive(Clone)]
pub struct Network {
    inner: Arc<NetworkInner>,
}

struct NetworkInner {
    cfg: Config,
    local_record: RwLock<ServiceRecord>,
    heartbeat: Mutex<Option<HeartbeatHandle>>,
    policy: PolicyEngine,
}

struct HeartbeatHandle {
    stop_tx: watch::Sender<bool>,
    join_handle: JoinHandle<()>,
}

#[derive(Clone)]
struct PolicyEngine {
    allow_cross_namespace: bool,
    allowed: HashSet<String>,
    denied: HashSet<String>,
}

impl Network {
    pub fn new(cfg: &Config) -> Result<Self> {
        if cfg.private_key.is_empty() {
            return Err(anyhow!("config: private key is required"));
        }
        if cfg.workload_name.is_empty() || cfg.pod_name.is_empty() {
            return Err(anyhow!("config: workload name and pod name are required"));
        }

        let keypair = Keypair::from_protobuf_encoding(&cfg.private_key)
            .context("decode libp2p private key")?;
        let peer_id = keypair.public().to_peer_id().to_string();

        let mut caps = serde_json::Map::new();
        caps.insert(
            "namespace".into(),
            serde_json::Value::String(cfg.namespace.clone()),
        );
        caps.insert(
            "workload".into(),
            serde_json::Value::String(cfg.workload_name.clone()),
        );
        caps.insert(
            "pod".into(),
            serde_json::Value::String(cfg.pod_name.clone()),
        );
        if let Some(ord) = cfg.ordinal() {
            caps.insert("ordinal".into(), serde_json::json!(ord));
        }

        let record = ServiceRecord {
            workload_id: cfg.workload_id(),
            namespace: cfg.namespace.clone(),
            workload_name: cfg.workload_name.clone(),
            peer_id,
            pod_name: Some(cfg.pod_name.clone()),
            ordinal: cfg.ordinal(),
            addrs: cfg.listen_addrs.clone(),
            caps,
            version: 1,
            ts: current_millis(),
            nonce: Uuid::new_v4().to_string(),
            ready: false,
            healthy: false,
        };

        Ok(Self {
            inner: Arc::new(NetworkInner {
                cfg: cfg.clone(),
                local_record: RwLock::new(record),
                heartbeat: Mutex::new(None),
                policy: PolicyEngine::from_config(cfg),
            }),
        })
    }

    pub fn start(&self) {
        if let Err(err) = self.refresh_heartbeat() {
            warn!(error = %err, "failed to publish initial service record");
        }

        let mut guard = self.inner.heartbeat.lock().expect("heartbeat lock");
        if guard.is_some() {
            return;
        }

        let (tx, mut rx) = watch::channel(false);
        let interval_duration = self.inner.cfg.heartbeat_interval;
        let network = self.clone();
        let join_handle = tokio::spawn(async move {
            let mut ticker = interval(interval_duration);
            ticker.tick().await;
            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        if let Err(err) = network.refresh_heartbeat() {
                            warn!(error = %err, "failed to refresh WDHT record");
                        }
                    }
                    changed = rx.changed() => {
                        if changed.is_ok() && *rx.borrow() {
                            break;
                        }
                    }
                }
            }
            debug!("network heartbeat stopped");
        });

        *guard = Some(HeartbeatHandle {
            stop_tx: tx,
            join_handle,
        });
    }

    pub async fn shutdown(&self) {
        if let Some(handle) = self.take_heartbeat() {
            let _ = handle.stop_tx.send(true);
            let _ = handle.join_handle.await;
        }

        let record = self.local_record();
        discovery::remove(&record.workload_id, &record.peer_id);
    }

    pub fn update_local_status(&self, healthy: bool, ready: bool) -> Result<()> {
        let mut changed = false;
        {
            let mut record = self.inner.local_record.write().expect("record write lock");
            if record.healthy != healthy {
                record.healthy = healthy;
                changed = true;
            }
            if record.ready != ready {
                record.ready = ready;
                changed = true;
            }
            if changed {
                record.version = record.version.saturating_add(1);
                record.ts = current_millis();
                record.nonce = Uuid::new_v4().to_string();
            }
        }

        if changed {
            self.publish_current_record()?;
        }
        Ok(())
    }

    pub fn update_leader_endpoint(&self, active: bool, epoch: u64) -> Result<()> {
        let mut should_publish = false;
        {
            let mut record = self.inner.local_record.write().expect("record write lock");
            let mut changed = false;
            if record.ready != active {
                record.ready = active;
                changed = true;
            }
            let leader_value = serde_json::Value::Bool(active);
            if record.caps.get("leader") != Some(&leader_value) {
                record.caps.insert("leader".into(), leader_value);
                changed = true;
            }
            let epoch_value = serde_json::Value::Number(serde_json::Number::from(epoch));
            if record.caps.get("leader_epoch") != Some(&epoch_value) {
                record.caps.insert("leader_epoch".into(), epoch_value);
                changed = true;
            }
            if changed {
                record.version = record.version.saturating_add(1);
                record.ts = current_millis();
                record.nonce = Uuid::new_v4().to_string();
                should_publish = true;
            }
        }

        if should_publish {
            if active {
                info!(epoch, "advertising leader endpoint");
                crate::increment_counter!("workplane.network.leader_advertisements");
            } else {
                info!(epoch, "withdrawing leader endpoint");
                crate::increment_counter!("workplane.network.leader_withdrawals");
            }
            self.publish_current_record()?;
        }
        Ok(())
    }

    pub fn find_service_peers(&self) -> Vec<ServiceRecord> {
        let namespace = self.inner.cfg.namespace.clone();
        let workload = self.inner.cfg.workload_name.clone();
        let local_id = self.inner.cfg.workload_id();
        discovery::list(&namespace, &workload)
            .into_iter()
            .filter(|record| {
                record.workload_id == local_id || self.inner.policy.allows(&namespace, record)
            })
            .collect()
    }

    pub fn peer_id(&self) -> String {
        self.local_record().peer_id
    }

    fn refresh_heartbeat(&self) -> Result<()> {
        {
            let mut record = self.inner.local_record.write().expect("record write lock");
            record.version = record.version.saturating_add(1);
            record.ts = current_millis();
            record.nonce = Uuid::new_v4().to_string();
        }
        self.publish_current_record()
    }

    fn publish_current_record(&self) -> Result<()> {
        let record = self.local_record();
        if !discovery::put(record.clone(), self.inner.cfg.dht_ttl) {
            return Err(anyhow!("rejected WDHT update due to stale clock or order"));
        }
        Ok(())
    }

    fn local_record(&self) -> ServiceRecord {
        self.inner
            .local_record
            .read()
            .expect("record read lock")
            .clone()
    }

    fn take_heartbeat(&self) -> Option<HeartbeatHandle> {
        self.inner.heartbeat.lock().expect("heartbeat lock").take()
    }
}

impl PolicyEngine {
    fn from_config(cfg: &Config) -> Self {
        Self {
            allow_cross_namespace: cfg.allow_cross_namespace,
            allowed: cfg.allowed_workloads.iter().cloned().collect(),
            denied: cfg.denied_workloads.iter().cloned().collect(),
        }
    }

    fn allows(&self, local_namespace: &str, record: &ServiceRecord) -> bool {
        let workload_id = format!("{}/{}", record.namespace, record.workload_name);
        if self.denied.contains(&workload_id) {
            return false;
        }
        if !self.allow_cross_namespace && record.namespace != local_namespace {
            return self.allowed.contains(&workload_id);
        }
        if self.allowed.is_empty() {
            return true;
        }
        self.allowed.contains(&workload_id)
    }
}

fn current_millis() -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    now.as_millis() as i64
}

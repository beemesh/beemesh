use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use reqwest::Client;
use serde_json::json;
use tokio::task::JoinHandle;
use tokio::time::interval;
use tokio::{select, sync::watch};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::config::Config;
use crate::discovery::ServiceRecord;
use crate::manifest::WorkloadManifest;
use crate::network::Network;
use crate::task::Task;

pub struct SelfHealer {
    network: Network,
    config: Config,
    manifest: Arc<Vec<u8>>,
    manifest_spec: Arc<WorkloadManifest>,
    http: Client,
    stop_tx: Option<watch::Sender<bool>>,
    handle: Option<JoinHandle<()>>,
}

impl SelfHealer {
    pub fn new(
        network: Network,
        config: Config,
        manifest: Arc<Vec<u8>>,
        manifest_spec: Arc<WorkloadManifest>,
    ) -> Self {
        let timeout = config.health_probe_timeout;
        Self {
            network,
            config,
            manifest,
            manifest_spec,
            http: Client::builder()
                .timeout(timeout)
                .build()
                .expect("reqwest client"),
            stop_tx: None,
            handle: None,
        }
    }

    pub fn start(&mut self) {
        if self.handle.is_some() {
            return;
        }
        let (tx, mut rx) = watch::channel(false);
        self.stop_tx = Some(tx.clone());

        let cfg = self.config.clone();
        let manifest = self.manifest.clone();
        let network = self.network.clone();
        let client = self.http.clone();
        let manifest_spec = self.manifest_spec.clone();

        let handle = tokio::spawn(async move {
            let interval_duration = if cfg.health_probe_interval < cfg.replica_check_interval {
                cfg.health_probe_interval
            } else {
                cfg.replica_check_interval
            };
            let mut ticker = interval(interval_duration);
            // immediate first tick
            ticker.tick().await;
            loop {
                select! {
                    _ = ticker.tick() => {
                        if let Err(err) = check_and_heal(
                            &network,
                            &cfg,
                            &client,
                            &manifest,
                            &manifest_spec,
                        ).await {
                            warn!(error = %err, "self-heal iteration failed");
                        }
                    }
                    changed = rx.changed() => {
                        if changed.is_ok() && *rx.borrow() {
                            break;
                        }
                    }
                }
            }
            debug!("self-healer stopped");
        });
        self.handle = Some(handle);
    }

    pub async fn stop(&mut self) {
        if let Some(tx) = &self.stop_tx {
            let _ = tx.send(true);
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.await;
        }
        self.stop_tx = None;
    }
}

async fn check_and_heal(
    network: &Network,
    cfg: &Config,
    client: &Client,
    manifest: &Arc<Vec<u8>>,
    manifest_spec: &Arc<WorkloadManifest>,
) -> Result<()> {
    let status = perform_local_health_check(client, manifest_spec).await;
    let ready = status.liveness_ok && status.readiness_ok;
    if let Err(err) = network.update_local_status(status.liveness_ok, ready) {
        warn!(error = %err, "failed to update local WDHT status");
    }

    let desired_replicas = if manifest_spec.replicas == 0 {
        cfg.replicas
    } else {
        manifest_spec.replicas
    };
    let observed_records = network.find_service_peers();
    let healthy_records: Vec<ServiceRecord> = observed_records
        .into_iter()
        .filter(|record| record.ready && record.healthy)
        .collect();

    if healthy_records.len() >= desired_replicas {
        if healthy_records.len() > desired_replicas {
            warn!(
                observed = healthy_records.len(),
                desired = desired_replicas,
                "observed replicas exceed desired replicas"
            );
        }
        return Ok(());
    }

    let deficit = desired_replicas - healthy_records.len();

    if manifest_spec.kind.is_stateful() {
        let missing_ordinals = missing_stateful_ordinals(desired_replicas, &healthy_records);
        for ordinal in missing_ordinals.into_iter().take(deficit) {
            publish_clone_task(cfg, client, manifest, manifest_spec, Some(ordinal)).await?;
        }
    } else {
        for _ in 0..deficit {
            publish_clone_task(cfg, client, manifest, manifest_spec, None).await?;
        }
    }
    Ok(())
}

async fn publish_clone_task(
    cfg: &Config,
    client: &Client,
    manifest: &Arc<Vec<u8>>,
    manifest_spec: &WorkloadManifest,
    ordinal: Option<u32>,
) -> Result<()> {
    let task = Task {
        task_id: Uuid::new_v4().to_string(),
        kind: manifest_spec.kind.task_kind().to_string(),
        name: manifest_spec.name.clone(),
        manifest: manifest.as_ref().clone(),
        destination: "scheduler".to_string(),
        clone_request: true,
        replicas: 1,
        namespace: cfg.namespace.clone(),
        spec: ordinal.map(|ord| json!({ "ordinal": ord })),
    };

    let url = format!("{}/v1/publish_task", cfg.beemesh_api.trim_end_matches('/'));
    let resp = client
        .post(url)
        .json(&task)
        .send()
        .await
        .context("send clone task")?;
    if !resp.status().is_success() {
        return Err(anyhow!(
            "publish clone task failed with status {}",
            resp.status()
        ));
    }
    Ok(())
}

struct HealthStatus {
    liveness_ok: bool,
    readiness_ok: bool,
}

async fn perform_local_health_check(
    client: &Client,
    manifest_spec: &WorkloadManifest,
) -> HealthStatus {
    let mut liveness_ok = manifest_spec.liveness_http.is_empty();
    for probe in &manifest_spec.liveness_http {
        if probe_http(client, probe).await {
            liveness_ok = true;
        } else {
            liveness_ok = false;
            break;
        }
    }

    let mut readiness_ok = manifest_spec.readiness_http.is_empty();
    for probe in &manifest_spec.readiness_http {
        if probe_http(client, probe).await {
            readiness_ok = true;
        } else {
            readiness_ok = false;
            break;
        }
    }

    HealthStatus {
        liveness_ok,
        readiness_ok,
    }
}

async fn probe_http(client: &Client, probe: &crate::manifest::HttpProbe) -> bool {
    let host = probe
        .host
        .clone()
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let path = if probe.path.starts_with('/') {
        probe.path.clone()
    } else {
        format!("/{}", probe.path)
    };
    let scheme = if probe.scheme.is_empty() {
        "http".to_string()
    } else {
        probe.scheme.to_lowercase()
    };
    let url = format!("{scheme}://{host}:{}{}", probe.port, path);

    match client.get(url).send().await {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

fn missing_stateful_ordinals(desired_replicas: usize, records: &[ServiceRecord]) -> Vec<u32> {
    let mut present = std::collections::HashSet::new();
    for record in records {
        if let Some(ord) = record.ordinal {
            present.insert(ord);
        }
    }
    (0..desired_replicas as u32)
        .filter(|ord| !present.contains(ord))
        .collect()
}

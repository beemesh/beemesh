use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use reqwest::Client;
use serde::Serialize;
use serde_json::{Map, json};
use tokio::task::JoinHandle;
use tokio::time::{interval, sleep};
use tokio::{select, sync::watch};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::config::Config;
use crate::discovery;
use crate::discovery::ServiceRecord;
use crate::manifest::WorkloadManifest;
use crate::network::Network;
use crate::streams::{RPCRequest, send_request};
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
            let mut last_health_probe = Instant::now() - cfg.health_probe_interval;
            let mut last_replica_check = Instant::now() - cfg.replica_check_interval;
            loop {
                select! {
                    _ = ticker.tick() => {
                        let now = Instant::now();
                        if now.duration_since(last_health_probe) >= cfg.health_probe_interval {
                            if let Err(err) = update_local_health(&network, &client, &manifest_spec).await {
                                warn!(error = %err, "health probe failed");
                            }
                            last_health_probe = now;
                        }

                        if now.duration_since(last_replica_check) >= cfg.replica_check_interval {
                            if let Err(err) = reconcile_replicas(
                                &network,
                                &cfg,
                                &client,
                                &manifest,
                                &manifest_spec,
                            ).await {
                                warn!(error = %err, "replica reconciliation failed");
                            }
                            last_replica_check = now;
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

async fn update_local_health(
    network: &Network,
    client: &Client,
    manifest_spec: &WorkloadManifest,
) -> Result<()> {
    let status = perform_local_health_check(client, manifest_spec).await;
    let ready = status.liveness_ok && status.readiness_ok;
    network
        .update_local_status(status.liveness_ok, ready)
        .context("update WDHT with local health")
}

async fn reconcile_replicas(
    network: &Network,
    cfg: &Config,
    client: &Client,
    manifest: &Arc<Vec<u8>>,
    manifest_spec: &Arc<WorkloadManifest>,
) -> Result<()> {
    if !policy_allows_workload(cfg, &cfg.namespace, &cfg.workload_name) {
        debug!(
            namespace = %cfg.namespace,
            workload = %cfg.workload_name,
            "skipping reconciliation due to workload policy",
        );
        return Ok(());
    }
    let desired_replicas = desired_replica_count(cfg, manifest_spec);
    let relevant_records = current_workload_records(network, cfg);
    let evaluated_records = relevant_records
        .into_iter()
        .map(|mut record| {
            if record.ready && record.healthy && !wdht_healthtest(&record) {
                record.ready = false;
                record.healthy = false;
            }
            record
        })
        .collect::<Vec<_>>();

    let (mut healthy, mut unhealthy): (Vec<ServiceRecord>, Vec<ServiceRecord>) = evaluated_records
        .into_iter()
        .partition(|record| record.ready && record.healthy);

    healthy.sort_by(|a, b| rank_records(a, b));
    unhealthy.sort_by(|a, b| rank_records(a, b));

    // For stateful workloads, ensure we only keep one healthy replica per ordinal.
    let mut duplicates = Vec::new();
    if manifest_spec.kind.is_stateful() {
        let mut seen = HashSet::new();
        let mut deduped = Vec::new();
        for record in healthy.into_iter() {
            if let Some(ord) = record.ordinal {
                if !seen.insert(ord) {
                    duplicates.push(record);
                    continue;
                }
            }
            deduped.push(record);
        }
        healthy = deduped;
    }

    // Determine deficit by only counting healthy replicas.
    if healthy.len() < desired_replicas {
        let deficit = desired_replicas - healthy.len();
        if manifest_spec.kind.is_stateful() {
            let missing_ordinals = missing_stateful_ordinals(desired_replicas, &healthy);
            for ordinal in missing_ordinals.into_iter().take(deficit) {
                request_new_replica(cfg, client, manifest, manifest_spec.as_ref(), Some(ordinal))
                    .await?;
            }
        } else {
            for _ in 0..deficit {
                request_new_replica(cfg, client, manifest, manifest_spec.as_ref(), None).await?;
            }
        }
    }

    // Build removal list prioritizing duplicates and unhealthy replicas first.
    let mut removal_candidates = Vec::new();
    removal_candidates.extend(duplicates);
    removal_candidates.extend(unhealthy);

    let mut excess = healthy.len().saturating_sub(desired_replicas);
    if excess > 0 {
        let mut iter = healthy.into_iter().rev();
        while excess > 0 {
            if let Some(record) = iter.next() {
                removal_candidates.push(record);
            } else {
                break;
            }
            excess -= 1;
        }
    }

    for record in removal_candidates {
        if let Err(err) = remove_replica(cfg, client, &record).await {
            warn!(
                error = %err,
                peer = %record.peer_id,
                namespace = %record.namespace,
                workload = %record.workload_name,
                "failed to remove replica"
            );
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

async fn request_new_replica(
    cfg: &Config,
    client: &Client,
    manifest: &Arc<Vec<u8>>,
    manifest_spec: &WorkloadManifest,
    ordinal: Option<u32>,
) -> Result<()> {
    let action =
        || async { publish_clone_task(cfg, client, manifest, manifest_spec, ordinal).await };
    retry_with_backoff(action, cfg.replica_check_interval).await
}

#[derive(Serialize)]
struct ReplicaRemovalRequest {
    namespace: String,
    workload: String,
    peer_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pod_name: Option<String>,
}

async fn remove_replica(cfg: &Config, client: &Client, record: &ServiceRecord) -> Result<()> {
    let base = cfg.beemesh_api.trim_end_matches('/');
    let url = format!("{base}/v1/remove_replica");
    let request = ReplicaRemovalRequest {
        namespace: record.namespace.clone(),
        workload: record.workload_name.clone(),
        peer_id: record.peer_id.clone(),
        pod_name: record.pod_name.clone(),
    };
    let action = || async {
        let resp = client
            .post(url.clone())
            .json(&request)
            .send()
            .await
            .context("send remove replica request")?;
        if resp.status().is_success() {
            discovery::remove(&record.workload_id, &record.peer_id);
            Ok(())
        } else {
            Err(anyhow!(
                "remove replica failed with status {}",
                resp.status()
            ))
        }
    };
    retry_with_backoff(action, cfg.replica_check_interval).await
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

fn desired_replica_count(cfg: &Config, manifest_spec: &WorkloadManifest) -> usize {
    if manifest_spec.replicas == 0 {
        cfg.replicas.max(1)
    } else {
        manifest_spec.replicas
    }
}

fn current_workload_records(network: &Network, cfg: &Config) -> Vec<ServiceRecord> {
    let workload_id = cfg.workload_id();
    network
        .find_service_peers()
        .into_iter()
        .filter(|record| record.workload_id == workload_id)
        .filter(|record| policy_allows_workload(cfg, &record.namespace, &record.workload_name))
        .collect()
}

fn policy_allows_workload(cfg: &Config, namespace: &str, workload_name: &str) -> bool {
    let workload_id = format!("{namespace}/{workload_name}");
    if cfg
        .denied_workloads
        .iter()
        .any(|denied| denied == &workload_id)
    {
        return false;
    }
    if !cfg.allow_cross_namespace && namespace != cfg.namespace {
        return false;
    }
    if cfg.allowed_workloads.is_empty() {
        return true;
    }
    cfg.allowed_workloads
        .iter()
        .any(|allowed| allowed == &workload_id)
}

fn rank_records(a: &ServiceRecord, b: &ServiceRecord) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let key = |record: &ServiceRecord| {
        (
            record.ready && record.healthy,
            record.ready,
            record.healthy,
            record.ts,
            record.version,
            record.peer_id.clone(),
        )
    };
    match key(b).cmp(&key(a)) {
        Ordering::Equal => b.peer_id.cmp(&a.peer_id),
        other => other,
    }
}

fn wdht_healthtest(record: &ServiceRecord) -> bool {
    let request = RPCRequest {
        method: "healthz".to_string(),
        body: Map::new(),
    };
    match send_request(record, request) {
        Ok(response) => response.ok,
        Err(_) => false,
    }
}

async fn retry_with_backoff<F, Fut>(mut action: F, max_interval: Duration) -> Result<()>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    let mut delay = Duration::from_millis(200);
    let max_delay = std::cmp::min(Duration::from_secs(5), max_interval);
    let max_attempts = 5;
    for attempt in 1..=max_attempts {
        match action().await {
            Ok(()) => return Ok(()),
            Err(err) if attempt == max_attempts => return Err(err),
            Err(err) => {
                warn!(attempt, error = %err, "retrying HTTP operation");
                sleep(delay).await;
                delay = std::cmp::min(delay.saturating_mul(2), max_delay);
            }
        }
    }
    Ok(())
}

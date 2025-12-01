//! # Self-Healing – Replica Count Management & Health Monitoring
//!
//! This module implements the self-healing subsystem responsible for maintaining the
//! desired replica count for a workload. It monitors replica health via WDHT records
//! and HTTP probes, then triggers scale-up or scale-down via the machineplane API.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                          SelfHealer                                     │
//! │                                                                          │
//! │  ┌────────────────────┐        ┌────────────────────┐                   │
//! │  │  Health Probe Loop │        │  Replica Reconcile │                   │
//! │  │  (10s interval)    │        │  (30s interval)    │                   │
//! │  └─────────┬──────────┘        └─────────┬──────────┘                   │
//! │            │                             │                               │
//! │            ▼                             ▼                               │
//! │  ┌─────────────────────┐      ┌─────────────────────┐                   │
//! │  │ update_local_health │      │ reconcile_replicas  │                   │
//! │  │ (WDHT ready/healthy)│      │ (scale up/down)     │                   │
//! │  └─────────────────────┘      └──────────┬──────────┘                   │
//! │                                          │                               │
//! │                                          ▼                               │
//! │                               ┌──────────────────────┐                   │
//! │                               │  Machineplane API    │                   │
//! │                               │  POST /v1/publish_tender │               │
//! │                               │  POST /v1/remove_replica │               │
//! │                               └──────────────────────┘                   │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Health Probing
//!
//! Local health status is determined by HTTP probes configured via environment:
//! - **Liveness**: `BEE_LIVENESS_URL` (e.g., `http://127.0.0.1:8080/health`)
//! - **Readiness**: `BEE_READINESS_URL` (e.g., `http://127.0.0.1:8080/ready`)
//!
//! Results are published to the WDHT via [`Network::update_local_status`].
//!
//! ## Replica Reconciliation
//!
//! The reconciliation loop compares desired vs actual replica count:
//!
//! 1. Query WDHT for all workload replicas
//! 2. Verify health via RPC `healthz` calls
//! 3. Check disposal status via machineplane API
//! 4. If `healthy_count < desired`:
//!    - Publish clone tenders to machineplane
//! 5. If `healthy_count > desired`:
//!    - Request removal of excess replicas (prioritizing unhealthy ones)
//!
//! ## Disposal Handling
//!
//! Before scaling up, the self-healer checks `GET /disposal/{ns}/{kind}/{name}` on the
//! machineplane API. If the workload is marked for disposal:
//!
//! - The self-healer self-terminates with SIGTERM (exit code 143)
//! - This propagates disposal across the fabric even if gossipsub broadcast was missed
//! - Ensures complete workload teardown without race conditions
//!
//! ## Stateful Workload Handling
//!
//! For StatefulSets, reconciliation ensures:
//! - Only one replica per ordinal (duplicates are removed)
//! - Missing ordinals are requested with specific `spec.ordinal` values
//!
//! ## Policy Integration
//!
//! The `policy_allows_workload` function filters out workloads that are:
//! - On the denylist
//! - In different namespaces (unless `allow_cross_namespace` is true)
//! - Not on the allowlist (if allowlist is non-empty)

use std::collections::HashSet;
use std::process;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use reqwest::Client;
use serde::Serialize;
use serde_json::json;
use tokio::task::JoinHandle;
use tokio::time::{interval, sleep};
use tokio::{select, sync::watch};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::config::Config;
use crate::discovery;
use crate::discovery::ServiceRecord;
use crate::network::Network;
use crate::rpc::RPCRequest;

/// Tender for scheduling a new replica via the machineplane API.
///
/// Sent to `POST /v1/publish_tender` to request a new workload instance.
/// The machineplane already has the manifest stored; we only send the
/// workload metadata needed to identify which manifest to use.
#[derive(Debug, Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Tender {
    /// Unique identifier for this tender (UUID).
    pub tender_id: String,

    /// Workload kind: "StatelessWorkload" or "StatefulWorkload".
    pub kind: String,

    /// Workload name (e.g., "nginx").
    pub name: String,

    /// Routing destination (always "scheduler").
    pub destination: String,

    /// Whether this is a clone/scale-up request.
    pub clone_request: bool,

    /// Number of replicas requested (always 1 for scale-up).
    pub replicas: usize,

    /// Target namespace.
    pub namespace: String,

    /// Optional spec for stateful workloads (contains ordinal).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<serde_json::Value>,
}

/// Self-healer for maintaining replica count and health status.
///
/// Create via [`SelfHealer::new`], then call [`SelfHealer::start`] to begin
/// the background monitoring loop.
pub struct SelfHealer {
    /// Network for WDHT queries and health checks.
    network: Network,
    /// Agent configuration (includes health probe URLs and workload kind).
    config: Config,
    /// HTTP client for probes and API calls.
    http: Client,
    /// Channel to signal shutdown.
    stop_tx: Option<watch::Sender<bool>>,
    /// Background task handle.
    handle: Option<JoinHandle<()>>,
}

impl SelfHealer {
    /// Create a new self-healer instance.
    ///
    /// # Arguments
    ///
    /// * `network` - Network for WDHT and RPC
    /// * `config` - Agent configuration (includes liveness_url, readiness_url, workload_kind)
    pub fn new(network: Network, config: Config) -> Self {
        let timeout = config.health_probe_timeout;
        Self {
            network,
            config,
            http: Client::builder()
                .timeout(timeout)
                .build()
                .expect("reqwest client"),
            stop_tx: None,
            handle: None,
        }
    }

    /// Start the self-healing background loop.
    ///
    /// Spawns a task that runs two interleaved loops:
    /// - Health probing (every `health_probe_interval`)
    /// - Replica reconciliation (every `replica_check_interval`)
    ///
    /// Idempotent: calling start() multiple times has no effect.
    pub fn start(&mut self) {
        if self.handle.is_some() {
            return; // Already running
        }
        let (tx, mut rx) = watch::channel(false);
        self.stop_tx = Some(tx.clone());

        let cfg = self.config.clone();
        let network = self.network.clone();
        let client = self.http.clone();

        let handle = tokio::spawn(async move {
            // Use the smaller of the two intervals for the ticker
            let interval_duration = if cfg.health_probe_interval < cfg.replica_check_interval {
                cfg.health_probe_interval
            } else {
                cfg.replica_check_interval
            };
            let mut ticker = interval(interval_duration);
            ticker.tick().await; // Immediate first tick

            let mut last_health_probe = Instant::now() - cfg.health_probe_interval;
            let mut last_replica_check = Instant::now() - cfg.replica_check_interval;

            loop {
                select! {
                    _ = ticker.tick() => {
                        let now = Instant::now();

                        // Health probe check
                        if now.duration_since(last_health_probe) >= cfg.health_probe_interval {
                            if let Err(err) = update_local_health(&network, &client, &cfg).await {
                                warn!(error = %err, "health probe failed");
                            }
                            last_health_probe = now;
                        }

                        // Replica reconciliation check
                        if now.duration_since(last_replica_check) >= cfg.replica_check_interval {
                            match reconcile_replicas(&network, &cfg, &client).await {
                                Ok(()) => {
                                    crate::gauge!("workplane.reconciliation.failures", 0.0);
                                }
                                Err(err) => {
                                    crate::increment_counter!(
                                        "workplane.reconciliation.failures"
                                    );
                                    crate::gauge!("workplane.reconciliation.failures", 1.0);
                                    warn!(error = %err, "replica reconciliation failed");
                                }
                            }
                            last_replica_check = now;
                        }
                    }
                    // Shutdown signal
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

    /// Stop the self-healing loop and wait for cleanup.
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

// ============================================================================
// Health Probing
// ============================================================================

/// Update local health status based on HTTP probes and publish to WDHT.
async fn update_local_health(network: &Network, client: &Client, cfg: &Config) -> Result<()> {
    let status = perform_local_health_check(client, cfg).await;
    let ready = status.liveness_ok && status.readiness_ok;
    network
        .update_local_status(status.liveness_ok, ready)
        .context("update WDHT with local health")
}

// ============================================================================
// Replica Reconciliation
// ============================================================================

/// Main reconciliation loop: compare desired vs actual replicas, scale as needed.
///
/// This function:
/// 1. Queries WDHT for all replicas of this workload
/// 2. Verifies health via RPC healthz calls
/// 3. Scales up if below desired count
/// 4. Scales down if above desired count (prioritizing unhealthy replicas)
async fn reconcile_replicas(network: &Network, cfg: &Config, client: &Client) -> Result<()> {
    // Reset metrics at start of reconciliation
    crate::gauge!("workplane.reconciliation.scale_up", 0.0);
    crate::gauge!("workplane.reconciliation.scale_down", 0.0);

    // Policy check: skip reconciliation for denied workloads
    if !policy_allows_workload(cfg, &cfg.namespace, &cfg.workload_name) {
        debug!(
            namespace = %cfg.namespace,
            workload = %cfg.workload_name,
            "skipping reconciliation due to workload policy",
        );
        crate::gauge!("workplane.reconciliation.skipped_due_to_policy", 1.0);
        crate::increment_counter!("workplane.reconciliation.skipped_due_to_policy");
        return Ok(());
    }
    crate::gauge!("workplane.reconciliation.skipped_due_to_policy", 0.0);

    let desired_replicas = cfg.replicas.max(1);
    let relevant_records = current_workload_records(network, cfg);

    // ========================================================================
    // Phase 1: Verify health via RPC
    // ========================================================================
    // The WDHT records may be stale, so we verify each replica's health
    // by sending an RPC healthz request.

    let mut evaluated_records = Vec::new();
    for mut record in relevant_records {
        if record.ready && record.healthy {
            // Verify health via RPC
            if !wdht_healthtest(network, &record).await {
                record.ready = false;
                record.healthy = false;
            }
        }
        evaluated_records.push(record);
    }

    // Partition into healthy and unhealthy
    let (mut healthy, mut unhealthy): (Vec<ServiceRecord>, Vec<ServiceRecord>) = evaluated_records
        .into_iter()
        .partition(|record| record.ready && record.healthy);

    // Sort by rank for deterministic ordering
    healthy.sort_by(rank_records);
    unhealthy.sort_by(rank_records);

    info!(
        healthy = healthy.len(),
        unhealthy = unhealthy.len(),
        desired = desired_replicas,
        namespace = %cfg.namespace,
        workload = %cfg.workload_name,
        "replica health evaluated"
    );
    crate::gauge!("workplane.replicas.healthy", healthy.len() as f64);
    crate::gauge!("workplane.replicas.unhealthy", unhealthy.len() as f64);

    // ========================================================================
    // Phase 2: Handle stateful workload duplicates
    // ========================================================================
    // For StatefulSets, ensure only one healthy replica per ordinal.
    // Duplicate ordinals indicate split-brain or stale records.

    let mut duplicates = Vec::new();
    if cfg.is_stateful() {
        let mut seen = HashSet::new();
        let mut deduped = Vec::new();
        for record in healthy.into_iter() {
            if let Some(ord) = record.ordinal
                && !seen.insert(ord) {
                    duplicates.push(record);
                    continue;
                }
            deduped.push(record);
        }
        healthy = deduped;
    }

    // ========================================================================
    // Phase 3: Check disposal status before scale-up
    // ========================================================================
    // If the workload is marked for disposal, self-terminate instead of scaling up.
    // This propagates disposal across the fabric even if gossipsub was missed.
    if check_disposal_status(client, cfg).await.unwrap_or(false) {
        self_terminate(&cfg.workload_id());
    }

    // ========================================================================
    // Phase 4: Scale up if needed
    // ========================================================================
    if healthy.len() < desired_replicas {
        let deficit = desired_replicas - healthy.len();
        crate::gauge!("workplane.reconciliation.scale_up", deficit as f64);
        crate::increment_counter!("workplane.reconciliation.scale_up", deficit);

        if cfg.is_stateful() {
            // Request specific missing ordinals for stateful workloads
            let missing_ordinals = missing_stateful_ordinals(desired_replicas, &healthy);
            for ordinal in missing_ordinals.into_iter().take(deficit) {
                request_new_replica(cfg, client, Some(ordinal)).await?;
            }
        } else {
            // Request generic replicas for stateless workloads
            for _ in 0..deficit {
                request_new_replica(cfg, client, None).await?;
            }
        }
    }

    // ========================================================================
    // Phase 5: Scale down if needed
    // ========================================================================
    // Build removal list prioritizing: duplicates > unhealthy > excess healthy

    let mut removal_candidates = Vec::new();
    removal_candidates.extend(duplicates);
    removal_candidates.extend(unhealthy);

    // Add excess healthy replicas (from the end, lowest rank first)
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

    let removal_total = removal_candidates.len();
    crate::gauge!("workplane.reconciliation.scale_down", removal_total as f64);
    if removal_total > 0 {
        crate::increment_counter!("workplane.reconciliation.scale_down", removal_total);
    }

    // Request removal for each candidate
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

// ============================================================================
// Machineplane API Integration
// ============================================================================

/// Publish a clone request to the machineplane scheduler.
///
/// The machineplane exposes `POST /v1/publish_tender` for workload scheduling. Each request must
/// include a unique `tender_id`, the workload `kind` (StatelessWorkload or StatefulWorkload),
/// and set `clone_request=true` so the scheduler knows this task is an idempotent "add replica"
/// instruction. The machineplane already stores the manifest; we only send metadata to identify
/// the workload. The API is duplicate-tolerant as long as new tender IDs are used for each
/// scale-up and it responds with any `2xx` on acceptance. Non-successful status codes are treated
/// as errors so the caller can retry until the request succeeds or the backoff policy is exhausted.
async fn publish_clone_tender(
    cfg: &Config,
    client: &Client,
    ordinal: Option<u32>,
) -> Result<()> {
    let tender = Tender {
        tender_id: Uuid::new_v4().to_string(),
        kind: cfg.task_kind().to_string(),
        name: cfg.workload_name.clone(),
        destination: "scheduler".to_string(),
        clone_request: true,
        replicas: 1,
        namespace: cfg.namespace.clone(),
        spec: ordinal.map(|ord| json!({ "ordinal": ord })),
    };

    let url = format!(
        "{}/v1/publish_tender",
        cfg.beemesh_api.trim_end_matches('/')
    );
    let resp = client
        .post(url)
        .json(&tender)
        .send()
        .await
        .context("send clone tender")?;
    if !resp.status().is_success() {
        return Err(anyhow!(
            "publish clone tender failed with status {}",
            resp.status()
        ));
    }
    Ok(())
}

/// Request a new replica with retry backoff.
async fn request_new_replica(cfg: &Config, client: &Client, ordinal: Option<u32>) -> Result<()> {
    let action = || async { publish_clone_tender(cfg, client, ordinal).await };
    retry_with_backoff(action, cfg.replica_check_interval).await
}

/// Request body for replica removal via machineplane API.
#[derive(Serialize)]
struct ReplicaRemovalRequest {
    namespace: String,
    workload: String,
    peer_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pod_name: Option<String>,
}

/// Request a replica removal from the machineplane and eagerly purge the WDHT entry on success.
///
/// The machineplane exposes `POST /v1/remove_replica` which accepts a [`ReplicaRemovalRequest`]
/// describing the namespace, workload, peer, and optional pod that should be terminated. A `2xx`
/// response means the machineplane accepted the removal and we immediately delete the WDHT record so
/// the service discovery surface reflects the pending removal. Any non-success response is treated
/// as a hard error so callers can retry until the removal is acknowledged.
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
            // Eagerly purge from local WDHT cache
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

// ============================================================================
// Health Check Helpers
// ============================================================================

/// Check if the workload is marked for disposal on the local machineplane.
///
/// Queries `GET /disposal/{ns}/{kind}/{name}` on the machineplane API.
/// If disposing, the self-healer should terminate this replica.
async fn check_disposal_status(client: &Client, cfg: &Config) -> Result<bool> {
    let workload_id = cfg.workload_id();
    let url = format!(
        "{}/disposal/{}",
        cfg.beemesh_api.trim_end_matches('/'),
        workload_id
    );

    let resp = client
        .get(&url)
        .send()
        .await
        .context("check disposal status")?;

    if !resp.status().is_success() {
        // If endpoint not found or error, assume not disposing
        return Ok(false);
    }

    let body: serde_json::Value = resp.json().await.context("parse disposal response")?;
    Ok(body.get("disposing").and_then(|v| v.as_bool()).unwrap_or(false))
}

/// Self-terminate the workload process.
///
/// Called when disposal is detected. Uses SIGTERM semantics for graceful termination.
fn self_terminate(workload_id: &str) -> ! {
    error!(
        workload_id = %workload_id,
        "Disposal detected - self-terminating workload (SIGTERM)"
    );
    // Exit with code 143 (128 + 15 = SIGTERM equivalent) to indicate intentional termination
    process::exit(143);
}

/// Result of local health probes.
struct HealthStatus {
    liveness_ok: bool,
    readiness_ok: bool,
}

/// Perform local HTTP health probes using URLs from config.
///
/// If a URL is empty, that probe is considered passing (no probe configured).
async fn perform_local_health_check(client: &Client, cfg: &Config) -> HealthStatus {
    // Liveness: probe URL if configured, otherwise pass
    let liveness_ok = if cfg.liveness_url.is_empty() {
        true
    } else {
        probe_url(client, &cfg.liveness_url).await
    };

    // Readiness: probe URL if configured, otherwise pass
    let readiness_ok = if cfg.readiness_url.is_empty() {
        true
    } else {
        probe_url(client, &cfg.readiness_url).await
    };

    HealthStatus {
        liveness_ok,
        readiness_ok,
    }
}

/// Execute a single HTTP probe and return success/failure.
async fn probe_url(client: &Client, url: &str) -> bool {
    match client.get(url).send().await {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Calculate which ordinals are missing for a stateful workload.
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

/// Get all WDHT records for the current workload.
fn current_workload_records(network: &Network, cfg: &Config) -> Vec<ServiceRecord> {
    let workload_id = cfg.workload_id();
    network
        .find_service_peers()
        .into_iter()
        .filter(|record| record.workload_id == workload_id)
        .filter(|record| policy_allows_workload(cfg, &record.namespace, &record.workload_name))
        .collect()
}

/// Check if a workload is allowed by policy (allowlist/denylist).
fn policy_allows_workload(cfg: &Config, namespace: &str, workload_name: &str) -> bool {
    let workload_id = format!("{namespace}/{workload_name}");

    // Denylist takes precedence
    if cfg
        .denied_workloads
        .iter()
        .any(|denied| denied == &workload_id)
    {
        return false;
    }

    // Cross-namespace check
    if !cfg.allow_cross_namespace && namespace != cfg.namespace {
        return false;
    }

    // Empty allowlist = allow all
    if cfg.allowed_workloads.is_empty() {
        return true;
    }

    // Check allowlist
    cfg.allowed_workloads
        .iter()
        .any(|allowed| allowed == &workload_id)
}

/// Rank records for deterministic ordering during scale-down.
///
/// Higher-ranked records are kept, lower-ranked are removed first.
/// Ranking: (ready && healthy) > ready > healthy > timestamp > version > peer_id
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

/// Verify a replica's health via RPC healthz request.
async fn wdht_healthtest(network: &Network, record: &ServiceRecord) -> bool {
    let request = RPCRequest {
        method: "healthz".to_string(),
        body: serde_json::Map::new(),
        leader_only: false,
    };
    match network.send_request(&record.peer_id, request).await {
        Ok(response) => response.ok,
        Err(_) => false,
    }
}

/// Retry an async operation with exponential backoff.
///
/// Starts at 200ms, doubles up to 5s or `max_interval`, max 5 attempts.
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

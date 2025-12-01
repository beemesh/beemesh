//! # Workplane Agent Configuration
//!
//! This module defines the [`Config`] struct that holds all runtime configuration for a
//! workplane agent. Configuration values are typically populated from environment variables
//! in `main.rs` and then normalized via [`Config::apply_defaults`].
//!
//! ## Environment Variables
//!
//! | Variable                  | Field                    | Default           |
//! |--------------------------|--------------------------|-------------------|
//! | `BEE_NAMESPACE`          | `namespace`              | `"default"`       |
//! | `BEE_WORKLOAD_NAME`      | `workload_name`          | (required)        |
//! | `BEE_POD_NAME`           | `pod_name`               | (empty)           |
//! | `BEE_REPLICAS`           | `replicas`               | `1`               |
//! | `BEE_WORKLOAD_KIND`      | `workload_kind`          | `"Deployment"`    |
//! | `BEE_LIVENESS_URL`       | `liveness_url`           | (empty)           |
//! | `BEE_READINESS_URL`      | `readiness_url`          | (empty)           |
//! | `BEE_BOOTSTRAP_PEERS`    | `bootstrap_peer_strings` | (empty)           |
//! | `BEE_BEEMESH_API`        | `beemesh_api`            | `http://localhost:8080` |
//! | `BEE_REPLICA_CHECK_INTERVAL` | `replica_check_interval` | `30s`        |
//! | `BEE_DHT_TTL`            | `dht_ttl`                | `15s`             |
//! | `BEE_HEARTBEAT_INTERVAL` | `heartbeat_interval`     | `5s`              |
//! | `BEE_HEALTH_PROBE_INTERVAL` | `health_probe_interval` | `10s`          |
//! | `BEE_HEALTH_PROBE_TIMEOUT`  | `health_probe_timeout`  | `5s`           |
//! | `BEE_ALLOW_CROSS_NAMESPACE` | `allow_cross_namespace` | `false`        |
//! | `BEE_ALLOWED_WORKLOADS`  | `allowed_workloads`      | (empty = all)     |
//! | `BEE_DENIED_WORKLOADS`   | `denied_workloads`       | (empty)           |
//! | `BEE_LISTEN_ADDRS`       | `listen_addrs`           | (empty)           |
//!
//! ## Workload Identity
//!
//! The workload is uniquely identified by `namespace/workload_name`. This is returned by
//! [`Config::workload_id`] and used as the key prefix for WDHT records.
//!
//! For stateful workloads, the ordinal (replica index) is extracted from the pod name
//! suffix (e.g., `nginx-2` → ordinal `2`) via [`Config::ordinal`].
//!
//! ## Workload Kind
//!
//! The `workload_kind` field determines stateful vs stateless behavior:
//! - `"Deployment"` or `"StatelessWorkload"`: Stateless (no Raft consensus)
//! - `"StatefulSet"` or `"StatefulWorkload"`: Stateful (Raft leader election)
//!
//! ## Health Probes
//!
//! Health probe URLs are provided directly by the machineplane:
//! - `liveness_url`: Full URL for liveness probe (e.g., `http://127.0.0.1:8080/health`)
//! - `readiness_url`: Full URL for readiness probe (e.g., `http://127.0.0.1:8080/ready`)

use std::time::Duration;

/// Runtime configuration for a workplane agent.
///
/// All fields can be populated from environment variables. Call [`Config::apply_defaults`]
/// after construction to fill in sensible defaults for any unset values.
#[derive(Clone, Debug)]
pub struct Config {
    /// Optional explicit peer ID string. If `None`, generated from `private_key`.
    pub peer_id_str: Option<String>,

    /// Ed25519 private key bytes for libp2p identity.
    pub private_key: Vec<u8>,

    /// Kubernetes-style namespace for workload isolation.
    /// Used in workload ID: `{namespace}/{kind}/{workload_name}`.
    pub namespace: String,

    /// Name of the workload (e.g., "nginx", "redis").
    pub workload_name: String,

    /// Pod name, used to extract ordinal for stateful workloads.
    /// Expected format: `{workload_name}-{ordinal}` (e.g., "redis-0").
    pub pod_name: String,

    /// Desired number of replicas for self-healing.
    pub replicas: usize,

    /// Workload kind: "Deployment", "StatefulSet", "DaemonSet", etc.
    /// Determines stateful vs stateless behavior:
    /// - "Deployment" or "StatelessWorkload": Stateless (no Raft)
    /// - "StatefulSet" or "StatefulWorkload": Stateful (Raft leader election)
    pub workload_kind: String,

    /// Full URL for liveness probe (e.g., "http://127.0.0.1:8080/health").
    /// Empty string means no liveness probe.
    pub liveness_url: String,

    /// Full URL for readiness probe (e.g., "http://127.0.0.1:8080/ready").
    /// Empty string means no readiness probe.
    pub readiness_url: String,

    /// Bootstrap peer multiaddrs for initial DHT discovery.
    /// Format: `/ip4/{ip}/udp/{port}/quic-v1/p2p/{peer_id}`
    pub bootstrap_peer_strings: Vec<String>,

    /// Machineplane API base URL for clone tenders and replica removal.
    pub beemesh_api: String,

    /// Interval between replica count reconciliation checks.
    pub replica_check_interval: Duration,

    /// TTL for WDHT service records. Records not refreshed within TTL expire.
    pub dht_ttl: Duration,

    /// Interval for publishing WDHT heartbeats (record refreshes).
    pub heartbeat_interval: Duration,

    /// Interval for local health probe checks (liveness/readiness).
    pub health_probe_interval: Duration,

    /// HTTP client timeout for health probe requests.
    pub health_probe_timeout: Duration,

    /// Whether to allow discovering workloads from other namespaces.
    pub allow_cross_namespace: bool,

    /// Allowlist of workload IDs (`namespace/name`). Empty = allow all.
    pub allowed_workloads: Vec<String>,

    /// Denylist of workload IDs (`namespace/name`). Takes precedence over allowlist.
    pub denied_workloads: Vec<String>,

    /// libp2p listen addresses. Empty = OS-assigned port.
    pub listen_addrs: Vec<String>,
}

impl Config {
    /// Check if this workload is stateful (requires Raft consensus).
    ///
    /// Returns `true` for "StatefulSet" or "StatefulWorkload" kinds.
    pub fn is_stateful(&self) -> bool {
        let kind = self.workload_kind.trim();
        kind.eq_ignore_ascii_case("StatefulSet") || kind.eq_ignore_ascii_case("StatefulWorkload")
    }

    /// Get the task kind string for machineplane API requests.
    ///
    /// Maps workload kinds to task types:
    /// - "Deployment" or "StatelessWorkload" → "StatelessWorkload"
    /// - "StatefulSet" or "StatefulWorkload" → "StatefulWorkload"
    /// - "DaemonSet" → "DaemonSet"
    /// - Other → "CustomWorkload"
    pub fn task_kind(&self) -> &str {
        let kind = self.workload_kind.trim();
        if kind.eq_ignore_ascii_case("Deployment") || kind.eq_ignore_ascii_case("StatelessWorkload")
        {
            "StatelessWorkload"
        } else if kind.eq_ignore_ascii_case("StatefulSet")
            || kind.eq_ignore_ascii_case("StatefulWorkload")
        {
            "StatefulWorkload"
        } else if kind.eq_ignore_ascii_case("DaemonSet") {
            "DaemonSet"
        } else {
            "CustomWorkload"
        }
    }

    /// Apply default values to any unset configuration fields.
    ///
    /// This method should be called after parsing environment variables to ensure
    /// all required fields have sensible defaults:
    ///
    /// - `namespace`: defaults to `"default"`
    /// - `replicas`: defaults to `1`
    /// - `workload_kind`: defaults to `"Deployment"`
    /// - `beemesh_api`: defaults to `"http://localhost:8080"`
    /// - `replica_check_interval`: defaults to `30s`
    /// - `dht_ttl`: defaults to `15s`
    /// - `heartbeat_interval`: defaults to `5s`
    /// - `health_probe_interval`: defaults to `10s`
    /// - `health_probe_timeout`: defaults to `5s`
    pub fn apply_defaults(&mut self) {
        if self.namespace.is_empty() {
            self.namespace = "default".to_string();
        }
        if self.replicas == 0 {
            self.replicas = 1;
        }
        if self.workload_kind.is_empty() {
            self.workload_kind = "Deployment".to_string();
        }
        if self.beemesh_api.is_empty() {
            self.beemesh_api = "http://localhost:8080".to_string();
        }

        if self.replica_check_interval == Duration::from_secs(0) {
            self.replica_check_interval = Duration::from_secs(30);
        }
        if self.dht_ttl == Duration::from_secs(0) {
            self.dht_ttl = Duration::from_secs(15);
        }
        if self.heartbeat_interval == Duration::from_secs(0) {
            self.heartbeat_interval = Duration::from_secs(5);
        }
        if self.health_probe_interval == Duration::from_secs(0) {
            self.health_probe_interval = Duration::from_secs(10);
        }
        if self.health_probe_timeout == Duration::from_secs(0) {
            self.health_probe_timeout = Duration::from_secs(5);
        }
        if self.listen_addrs.is_empty() {
            self.listen_addrs = Vec::new();
        }
    }

    /// Returns the unique workload identifier.
    ///
    /// Format: `{namespace}/{workload_kind}/{workload_name}`
    ///
    /// This ID is used as the WDHT key prefix for service records and must match
    /// across all replicas of the same workload. The format aligns with Kubernetes
    /// resource coordinates and the Machineplane's resource identification scheme.
    pub fn workload_id(&self) -> String {
        format!("{}/{}/{}", self.namespace, self.workload_kind, self.workload_name)
    }

    /// Extract the stateful workload ordinal from the pod name.
    ///
    /// Parses the last hyphen-separated segment of `pod_name` as a `u32`.
    /// Returns `None` if the pod name doesn't follow the `{name}-{ordinal}` pattern.
    ///
    /// # Examples
    ///
    /// - `"redis-0"` → `Some(0)`
    /// - `"redis-master-2"` → `Some(2)`
    /// - `"nginx"` → `None`
    /// - `"nginx-abc"` → `None`
    pub fn ordinal(&self) -> Option<u32> {
        let suffix = self.pod_name.split('-').next_back()?;
        suffix.parse().ok()
    }
}

//! # Workplane CLI Entry Point
//!
//! This binary is the main entry point for the workplane agent. It parses configuration
//! from command-line arguments and environment variables, initializes the workload,
//! and runs until interrupted with Ctrl+C.
//!
//! ## Usage
//!
//! ```bash
//! workplane \
//!   --namespace default \
//!   --workload nginx \
//!   --pod nginx-0 \
//!   --workload-kind Deployment \
//!   --liveness-url http://127.0.0.1:8080/health \
//!   --readiness-url http://127.0.0.1:8080/ready \
//!   --privkey $BEE_PRIVATE_KEY_B64
//! ```
//!
//! ## Environment Variables
//!
//! All command-line arguments can be set via environment variables:
//!
//! | Variable                       | Argument                  | Description                    |
//! |--------------------------------|---------------------------|--------------------------------|
//! | `BEE_NAMESPACE`                | `--namespace`             | Kubernetes-style namespace     |
//! | `BEE_WORKLOAD_NAME`            | `--workload`              | Workload name                  |
//! | `BEE_POD_NAME`                 | `--pod`                   | Pod name (for ordinal)         |
//! | `BEE_REPLICAS`                 | `--replicas`              | Desired replica count          |
//! | `BEE_WORKLOAD_KIND`            | `--workload-kind`         | Kind: Deployment, StatefulSet  |
//! | `BEE_LIVENESS_URL`             | `--liveness-url`          | HTTP liveness probe URL        |
//! | `BEE_READINESS_URL`            | `--readiness-url`         | HTTP readiness probe URL       |
//! | `BEE_MACHINE_API`              | `--machine-api`           | Machineplane API URL           |
//! | `BEE_PRIVATE_KEY_B64`          | `--privkey`               | Base64-encoded private key     |
//! | `BEE_BOOTSTRAP_PEERS`          | `--bootstrap-peers`       | Comma-separated multiaddrs     |
//! | `BEE_LISTEN_ADDRS`             | `--listen-addrs`          | Comma-separated listen addrs   |
//! | `BEE_ALLOW_WORKLOADS`          | `--allow-workloads`       | Allowlist (namespace/name)     |
//! | `BEE_DENY_WORKLOADS`           | `--deny-workloads`        | Denylist (namespace/name)      |
//! | `BEE_ALLOW_CROSS_NAMESPACE`    | `--allow-cross-namespace` | Allow cross-namespace discovery|
//! | `BEE_DHT_TTL_SECS`             | `--dht-ttl-secs`          | WDHT record TTL                |
//! | `BEE_DHT_HEARTBEAT_SECS`       | `--dht-heartbeat-secs`    | Heartbeat interval             |
//! | `BEE_HEALTH_INTERVAL_SECS`     | `--health-interval-secs`  | Health probe interval          |
//! | `BEE_HEALTH_TIMEOUT_SECS`      | `--health-timeout-secs`   | Health probe timeout           |
//! | `BEE_REPLICA_CHECK_INTERVAL_SECS` | `--replica-check-interval-secs` | Reconciliation interval |
//!
//! ## Signal Handling
//!
//! The agent listens for `SIGINT` (Ctrl+C) and performs a graceful shutdown:
//! 1. Stops the leader monitor (if stateful)
//! 2. Stops the self-healer
//! 3. Stops the network heartbeat
//! 4. Exits

use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as Base64;
use clap::Parser;
use tokio::signal;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use workplane::{Config, Agent};

/// Command-line arguments for the workplane agent.
///
/// All arguments can be set via environment variables with the `BEE_` prefix.
#[derive(Parser, Debug)]
#[command(name = "workplane", author, version, about = "Beemesh Workplane agent")]
struct Args {
    /// Kubernetes-style namespace for workload isolation.
    #[arg(long, env = "BEE_NAMESPACE")]
    namespace: Option<String>,

    /// Workload name (e.g., "nginx", "redis").
    #[arg(long = "workload", env = "BEE_WORKLOAD_NAME")]
    workload_name: Option<String>,

    /// Pod name, used to extract ordinal for stateful workloads.
    #[arg(long = "pod", env = "BEE_POD_NAME")]
    pod_name: Option<String>,

    /// Desired number of replicas for self-healing.
    #[arg(long, env = "BEE_REPLICAS", default_value_t = 1)]
    replicas: usize,

    /// Workload kind: "Deployment", "StatefulSet", "DaemonSet".
    /// Determines stateful vs stateless behavior.
    #[arg(long = "workload-kind", env = "BEE_WORKLOAD_KIND", default_value = "Deployment")]
    workload_kind: String,

    /// Full URL for liveness probe (e.g., "http://127.0.0.1:8080/health").
    /// Empty or omitted means no liveness probe.
    #[arg(long = "liveness-url", env = "BEE_LIVENESS_URL", default_value = "")]
    liveness_url: String,

    /// Full URL for readiness probe (e.g., "http://127.0.0.1:8080/ready").
    /// Empty or omitted means no readiness probe.
    #[arg(long = "readiness-url", env = "BEE_READINESS_URL", default_value = "")]
    readiness_url: String,

    /// Interval in seconds between replica count reconciliation checks.
    #[arg(long, env = "BEE_REPLICA_CHECK_INTERVAL_SECS", default_value_t = 30)]
    replica_check_interval_secs: u64,

    /// Machineplane API base URL for clone tenders and replica removal.
    #[arg(
        long = "machine-api",
        env = "BEE_MACHINE_API",
        default_value = "http://localhost:8080"
    )]
    machine_api: String,

    /// Base64-encoded Ed25519 private key for libp2p identity.
    #[arg(long = "privkey", env = "BEE_PRIVATE_KEY_B64")]
    private_key_b64: Option<String>,

    /// Comma-separated bootstrap peer multiaddrs for DHT discovery.
    #[arg(long, env = "BEE_BOOTSTRAP_PEERS")]
    bootstrap_peers: Option<String>,

    /// Comma-separated libp2p listen addresses.
    #[arg(long, env = "BEE_LISTEN_ADDRS")]
    listen_addrs: Option<String>,

    /// Comma-separated workload IDs (namespace/name) to allow.
    #[arg(long, env = "BEE_ALLOW_WORKLOADS")]
    allow_workloads: Option<String>,

    /// Comma-separated workload IDs (namespace/name) to deny.
    #[arg(long, env = "BEE_DENY_WORKLOADS")]
    deny_workloads: Option<String>,

    /// Allow discovering workloads from other namespaces.
    #[arg(long, env = "BEE_ALLOW_CROSS_NAMESPACE", default_value_t = false)]
    allow_cross_namespace: bool,

    /// TTL in seconds for WDHT service records.
    #[arg(long, env = "BEE_DHT_TTL_SECS", default_value_t = 15)]
    dht_ttl_secs: u64,

    /// Interval in seconds for WDHT heartbeat publications.
    #[arg(long, env = "BEE_DHT_HEARTBEAT_SECS", default_value_t = 5)]
    dht_heartbeat_secs: u64,

    /// Interval in seconds for local health probe checks.
    #[arg(long, env = "BEE_HEALTH_INTERVAL_SECS", default_value_t = 10)]
    health_interval_secs: u64,

    /// Timeout in seconds for health probe HTTP requests.
    #[arg(long, env = "BEE_HEALTH_TIMEOUT_SECS", default_value_t = 5)]
    health_timeout_secs: u64,
}

/// Main entry point.
#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        error!(error = %err, "workplane failed");
        std::process::exit(1);
    }
}

/// Initialize and run the workplane agent.
///
/// 1. Parse command-line arguments and environment variables
/// 2. Validate required configuration
/// 3. Initialize and start the workload
/// 4. Wait for shutdown signal (Ctrl+C)
/// 5. Gracefully shut down
async fn run() -> Result<()> {
    init_tracing();

    let args = Args::parse();

    // Validate required arguments
    let namespace = args
        .namespace
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("namespace is required"))?;
    let workload_name = args
        .workload_name
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("workload name is required"))?;
    let pod_name = args
        .pod_name
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("pod name is required"))?;
    let private_key_b64 = args
        .private_key_b64
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("private key is required"))?;

    // Decode private key from base64
    let private_key = Base64
        .decode(private_key_b64.trim())
        .context("decode private key")?;

    // Parse comma-separated lists
    let bootstrap_peers = split_csv(args.bootstrap_peers);

    // Build configuration
    let mut cfg = Config {
        peer_id_str: None,
        private_key,
        namespace,
        workload_name: workload_name.clone(),
        pod_name: pod_name.clone(),
        replicas: args.replicas,
        workload_kind: args.workload_kind,
        liveness_url: args.liveness_url,
        readiness_url: args.readiness_url,
        bootstrap_peer_strings: bootstrap_peers,
        beemesh_api: args.machine_api,
        replica_check_interval: std::time::Duration::from_secs(args.replica_check_interval_secs),
        dht_ttl: std::time::Duration::from_secs(args.dht_ttl_secs),
        heartbeat_interval: std::time::Duration::from_secs(args.dht_heartbeat_secs),
        health_probe_interval: std::time::Duration::from_secs(args.health_interval_secs),
        health_probe_timeout: std::time::Duration::from_secs(args.health_timeout_secs),
        allow_cross_namespace: args.allow_cross_namespace,
        allowed_workloads: split_csv(args.allow_workloads),
        denied_workloads: split_csv(args.deny_workloads),
        listen_addrs: split_csv(args.listen_addrs),
    };
    cfg.apply_defaults();

    // Initialize and start agent
    let mut agent = Agent::new(cfg)?;
    agent.start();

    info!(%workload_name, %pod_name, "workplane started");

    // Wait for shutdown signal
    signal::ctrl_c().await.context("wait for ctrl+c")?;

    // Graceful shutdown
    info!("shutting down agent");
    agent.close().await;
    Ok(())
}

/// Initialize tracing subscriber with environment filter.
fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(false)
        .try_init();
}

/// Split a comma-separated string into a vector of trimmed, non-empty strings.
fn split_csv(input: Option<String>) -> Vec<String> {
    input
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

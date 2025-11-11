use std::path::PathBuf;

use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as Base64;
use clap::Parser;
use tokio::fs;
use tokio::signal;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use workplane::{Config, Workload};

#[derive(Parser, Debug)]
#[command(name = "workplane", author, version, about = "Beemesh Workplane agent")]
struct Args {
    #[arg(long, env = "BEE_NAMESPACE")]
    namespace: Option<String>,
    #[arg(long = "workload", env = "BEE_WORKLOAD_NAME")]
    workload_name: Option<String>,
    #[arg(long = "pod", env = "BEE_POD_NAME")]
    pod_name: Option<String>,
    #[arg(long, env = "BEE_REPLICAS", default_value_t = 1)]
    replicas: usize,
    #[arg(long, env = "BEE_REPLICA_CHECK_INTERVAL_SECS", default_value_t = 30)]
    replica_check_interval_secs: u64,
    #[arg(long, env = "BEE_MANIFEST_PATH")]
    manifest: Option<PathBuf>,
    #[arg(
        long = "machine-api",
        env = "BEE_MACHINE_API",
        default_value = "http://localhost:8080"
    )]
    machine_api: String,
    #[arg(long = "privkey", env = "BEE_PRIVATE_KEY_B64")]
    private_key_b64: Option<String>,
    #[arg(long, env = "BEE_BOOTSTRAP_PEERS")]
    bootstrap_peers: Option<String>,
    #[arg(long, env = "BEE_LISTEN_ADDRS")]
    listen_addrs: Option<String>,
    #[arg(long, env = "BEE_ALLOW_WORKLOADS")]
    allow_workloads: Option<String>,
    #[arg(long, env = "BEE_DENY_WORKLOADS")]
    deny_workloads: Option<String>,
    #[arg(long, env = "BEE_ALLOW_CROSS_NAMESPACE", default_value_t = false)]
    allow_cross_namespace: bool,
    #[arg(long, env = "BEE_DHT_TTL_SECS", default_value_t = 15)]
    dht_ttl_secs: u64,
    #[arg(long, env = "BEE_DHT_HEARTBEAT_SECS", default_value_t = 5)]
    dht_heartbeat_secs: u64,
    #[arg(long, env = "BEE_HEALTH_INTERVAL_SECS", default_value_t = 10)]
    health_interval_secs: u64,
    #[arg(long, env = "BEE_HEALTH_TIMEOUT_SECS", default_value_t = 5)]
    health_timeout_secs: u64,
}

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        error!(error = %err, "workplane failed");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    init_tracing();

    let args = Args::parse();

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
    let manifest_path = args
        .manifest
        .ok_or_else(|| anyhow::anyhow!("manifest path is required"))?;
    let private_key_b64 = args
        .private_key_b64
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("private key is required"))?;

    let manifest_bytes = fs::read(&manifest_path)
        .await
        .with_context(|| format!("read manifest at {}", manifest_path.display()))?;

    let private_key = Base64
        .decode(private_key_b64.trim())
        .context("decode private key")?;

    let bootstrap_peers = split_csv(args.bootstrap_peers);

    let mut cfg = Config {
        peer_id_str: None,
        private_key,
        namespace,
        workload_name: workload_name.clone(),
        pod_name: pod_name.clone(),
        replicas: args.replicas,
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

    let mut workload = Workload::new(cfg, manifest_bytes)?;
    workload.start();

    info!("workplane started", %workload_name, %pod_name);

    signal::ctrl_c().await.context("wait for ctrl+c")?;

    info!("shutting down workload");
    workload.close().await;
    Ok(())
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(false)
        .try_init();
}

fn split_csv(input: Option<String>) -> Vec<String> {
    input
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

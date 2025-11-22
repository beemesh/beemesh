use std::{str::FromStr, time::Duration};

use libp2p::{Multiaddr, multiaddr::Protocol};

pub const DEFAULT_BOOTSTRAP_PEERS: [(&str, &str); 2] = [
    (
        "12D3KooWAbcDefGhijkLmNoPqRsTuVwXyZaBcDeFgHiJkLmNoP",
        "/ip4/203.0.113.10/tcp/4001",
    ),
    (
        "12D3KooWZyxWvuTsRqPoNmLkJiHgFeDcBaZyXwVuTsRqPoNmLk",
        "/ip4/203.0.113.11/tcp/4001",
    ),
];

const DEFAULT_BOOTSTRAP_ENV: &str = "BEE_DEFAULT_BOOTSTRAP_PEERS";

#[derive(Clone, Debug)]
pub struct Config {
    pub peer_id_str: Option<String>,
    pub private_key: Vec<u8>,
    pub namespace: String,
    pub workload_name: String,
    pub pod_name: String,
    pub replicas: usize,
    pub bootstrap_peer_strings: Vec<String>,
    pub beemesh_api: String,
    pub replica_check_interval: Duration,
    pub dht_ttl: Duration,
    pub heartbeat_interval: Duration,
    pub health_probe_interval: Duration,
    pub health_probe_timeout: Duration,
    pub allow_cross_namespace: bool,
    pub allowed_workloads: Vec<String>,
    pub denied_workloads: Vec<String>,
    pub listen_addrs: Vec<String>,
}

impl Config {
    pub fn apply_defaults(&mut self) {
        if self.namespace.is_empty() {
            self.namespace = "default".to_string();
        }
        if self.replicas == 0 {
            self.replicas = 1;
        }
        if self.beemesh_api.is_empty() {
            self.beemesh_api = "http://localhost:8080".to_string();
        }
        if self.bootstrap_peer_strings.is_empty() {
            let env_peers = parse_env_bootstrap_peers();
            if !env_peers.is_empty() {
                tracing::info!(
                    count = env_peers.len(),
                    "No bootstrap peers provided; using environment defaults"
                );
                self.bootstrap_peer_strings = env_peers;
            } else {
                let mut defaults = Vec::new();
                for (peer_id, addr) in DEFAULT_BOOTSTRAP_PEERS {
                    match libp2p::PeerId::from_str(peer_id) {
                        Ok(pid) => {
                            let candidate = format!("{}/p2p/{}", addr, pid);
                            if let Err(err) = validate_bootstrap_multiaddr(&candidate) {
                                tracing::warn!(
                                    peer_id,
                                    addr,
                                    error = %err,
                                    "Skipping invalid default bootstrap peer"
                                );
                            } else {
                                defaults.push(candidate);
                            }
                        }
                        Err(_) => {
                            tracing::warn!(
                                peer_id,
                                addr,
                                "Skipping invalid default bootstrap peer id"
                            )
                        }
                    }
                }

                self.bootstrap_peer_strings = defaults;
            }
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

    pub fn workload_id(&self) -> String {
        format!("{}/{}", self.namespace, self.workload_name)
    }

    pub fn ordinal(&self) -> Option<u32> {
        let suffix = self.pod_name.split('-').last()?;
        suffix.parse().ok()
    }
}

fn parse_env_bootstrap_peers() -> Vec<String> {
    let raw = match std::env::var(DEFAULT_BOOTSTRAP_ENV) {
        Ok(val) if !val.trim().is_empty() => val,
        _ => return Vec::new(),
    };

    raw.split(',')
        .map(str::trim)
        .filter(|addr| !addr.is_empty())
        .filter_map(|addr| match validate_bootstrap_multiaddr(addr) {
            Ok(_) => Some(addr.to_string()),
            Err(err) => {
                tracing::warn!(
                    env = DEFAULT_BOOTSTRAP_ENV,
                    addr,
                    error = %err,
                    "Skipping invalid bootstrap peer from environment"
                );
                None
            }
        })
        .collect()
}

fn validate_bootstrap_multiaddr(addr: &str) -> anyhow::Result<()> {
    let multiaddr: Multiaddr = addr.parse()?;
    if multiaddr
        .iter()
        .any(|protocol| matches!(protocol, Protocol::P2p(_)))
    {
        Ok(())
    } else {
        Err(anyhow::anyhow!("missing /p2p/ component"))
    }
}

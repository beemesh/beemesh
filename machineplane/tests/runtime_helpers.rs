#![allow(dead_code)]

use machineplane::{DaemonConfig, start_machineplane};
use reqwest::Client;
use serde_json::Value;
use tokio::task::JoinHandle;
use tokio::time::{Duration, Instant, sleep};

/// Build a daemon configuration suitable for integration tests.
pub fn make_test_daemon(
    rest_api_port: u16,
    bootstrap_peers: Vec<String>,
    libp2p_quic_port: u16,
) -> DaemonConfig {
    DaemonConfig {
        ephemeral: true,
        rest_api_host: "127.0.0.1".to_string(),
        rest_api_port,
        key_dir: String::from("/tmp/.beemesh_test_unused"),
        bootstrap_peer: bootstrap_peers,
        libp2p_quic_port,
        // Prefer explicit PODMAN_HOST if provided, otherwise let machineplane detect defaults.
        podman_socket: std::env::var("PODMAN_HOST")
            .ok()
            .filter(|value| !value.trim().is_empty()),
        signing_ephemeral: true,
        kem_ephemeral: true,
        ephemeral_keys: true,
        ..DaemonConfig::default()
    }
}

/// Start a list of beemesh machineplane given their configurations.
///
/// Returns `JoinHandle`s for spawned background tasks.
pub async fn start_nodes(
    daemons: Vec<DaemonConfig>,
    startup_delay: Duration,
) -> Vec<JoinHandle<()>> {
    let mut all_handles = Vec::new();
    for daemon in daemons {
        let rest_api_host = daemon.rest_api_host.clone();
        let rest_api_port = daemon.rest_api_port;
        let libp2p_host = daemon.libp2p_host.clone();
        let libp2p_quic_port = daemon.libp2p_quic_port;
        log::info!(
            "starting test daemon: REST http://{}:{}, libp2p host {} port {}",
            rest_api_host,
            rest_api_port,
            libp2p_host,
            libp2p_quic_port
        );
        match start_machineplane(daemon).await {
            Ok(mut handles) => {
                all_handles.append(&mut handles);
            }
            Err(e) => panic!("failed to start node: {e:?}"),
        }
        tokio::spawn(log_local_address(
            rest_api_host,
            rest_api_port,
            libp2p_host,
            libp2p_quic_port,
        ));
        sleep(startup_delay).await;
    }
    all_handles
}

/// Wait for a daemon to report its local peer multiaddr.
pub async fn wait_for_local_multiaddr(
    rest_api_host: &str,
    rest_api_port: u16,
    libp2p_host: &str,
    libp2p_quic_port: u16,
    timeout: Duration,
) -> Option<String> {
    let display_host = if libp2p_host == "0.0.0.0" {
        "127.0.0.1"
    } else {
        libp2p_host
    };

    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Some(peer_id) = fetch_local_peer_id(rest_api_host, rest_api_port).await {
            let address = format!(
                "/ip4/{}/udp/{}/quic-v1/p2p/{}",
                display_host, libp2p_quic_port, peer_id
            );
            log::info!("detected local multiaddr: {}", address);
            return Some(address);
        }

        sleep(Duration::from_millis(500)).await;
    }

    None
}

async fn fetch_local_peer_id(rest_api_host: &str, rest_api_port: u16) -> Option<String> {
    let base = format!("http://{}:{}", rest_api_host, rest_api_port);
    let client = Client::new();
    let request = client
        .get(format!("{}/debug/local_peer_id", base))
        .timeout(Duration::from_secs(5));

    match request.send().await {
        Ok(resp) => match resp.json::<Value>().await {
            Ok(Value::Object(map)) if map.get("ok").and_then(|v| v.as_bool()) == Some(true) => map
                .get("local_peer_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            Ok(_) => None,
            Err(_) => None,
        },
        Err(_) => None,
    }
}

async fn log_local_address(
    rest_api_host: String,
    rest_api_port: u16,
    libp2p_host: String,
    libp2p_quic_port: u16,
) {
    let display_host = if libp2p_host == "0.0.0.0" {
        "127.0.0.1".to_string()
    } else {
        libp2p_host.clone()
    };

    let base = format!("http://{}:{}", rest_api_host, rest_api_port);
    let client = Client::new();
    let request = client
        .get(format!("{}/debug/local_peer_id", base))
        .timeout(Duration::from_secs(5));

    match request.send().await {
        Ok(resp) => match resp.json::<Value>().await {
            Ok(Value::Object(map)) => {
                if map.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                    if let Some(peer_id) = map.get("local_peer_id").and_then(|v| v.as_str()) {
                        let libp2p_address = format!(
                            "/ip4/{}/udp/{}/quic-v1/p2p/{}",
                            display_host, libp2p_quic_port, peer_id
                        );
                        log::info!(
                            "machineplane node at {} reports libp2p address {}",
                            base,
                            libp2p_address
                        );
                        return;
                    }
                }
                log::warn!("machineplane node at {} did not return a peer id", base);
            }
            Ok(_) => {
                log::warn!("machineplane node at {} returned unexpected JSON", base);
            }
            Err(err) => {
                log::warn!(
                    "failed to parse local_peer_id response from {}: {}",
                    base,
                    err
                );
            }
        },
        Err(err) => {
            log::warn!("failed to fetch local_peer_id from {}: {}", base, err);
        }
    }
}

/// Abort all spawned node tasks.
pub async fn shutdown_nodes(handles: &mut Vec<JoinHandle<()>>) {
    for handle in handles.drain(..) {
        handle.abort();
    }
}

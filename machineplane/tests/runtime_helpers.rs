#![allow(dead_code)]

use machineplane::{Cli, start_machineplane};
use tokio::task::JoinHandle;
use tokio::time::{Duration, sleep};

/// Build a CLI configuration suitable for integration tests.
pub fn make_test_cli(
    rest_api_port: u16,
    bootstrap_peers: Vec<String>,
    libp2p_quic_port: u16,
) -> Cli {
    let podman_socket = std::env::var("PODMAN_HOST")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "/run/podman/podman.sock".to_string());

    Cli {
        ephemeral: true,
        rest_api_host: "127.0.0.1".to_string(),
        rest_api_port,
        node_name: None,
        key_dir: String::from("/tmp/.beemesh_test_unused"),
        bootstrap_peer: bootstrap_peers,
        libp2p_quic_port,
        libp2p_host: "0.0.0.0".to_string(),
        podman_socket: Some(podman_socket),
        signing_ephemeral: true,
        kem_ephemeral: true,
        ephemeral_keys: true,
    }
}

/// Start a list of nodes given their CLIs. Returns JoinHandles for spawned background tasks.
pub async fn start_nodes(clis: Vec<Cli>, startup_delay: Duration) -> Vec<JoinHandle<()>> {
    let mut all_handles = Vec::new();
    for cli in clis {
        match start_machineplane(cli).await {
            Ok(mut handles) => {
                all_handles.append(&mut handles);
            }
            Err(e) => panic!("failed to start node: {:?}", e),
        }
        sleep(startup_delay).await;
    }
    all_handles
}

/// Abort all spawned node tasks.
pub async fn shutdown_nodes(handles: &mut Vec<JoinHandle<()>>) {
    for handle in handles.drain(..) {
        handle.abort();
    }
}

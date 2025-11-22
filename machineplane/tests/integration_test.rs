//! Integration tests for the Machineplane.
//!
//! This module contains the basic integration test suite for the Machineplane.
//! It verifies the core functionality of the system, including:
//! - Node startup and mesh formation.
//! - Health check endpoints.
//! - Public key retrieval.
//! - Basic peer discovery.

use env_logger::Env;
use log::info;

use std::time::Duration;
use tokio::time::sleep;

#[path = "test_utils/mod.rs"]
mod test_utils;
use test_utils::{make_test_cli, setup_cleanup_hook, start_nodes};

// We will start machines directly in this process by calling `start_machineplane(cli).await`.

/// Tests the full host application flow.
///
/// This test starts three nodes:
/// 1. A primary node (port 3000) with REST API and machineplane enabled.
/// 2. Two secondary nodes (ports 3100, 3200) to form a mesh.
///
/// It verifies:
/// - The mesh forms correctly (at least 2 peers discovered).
/// - The health endpoint returns "ok".
/// - The public key endpoints return valid keys.
#[tokio::test]
async fn test_run_host_application() {
    // Setup cleanup hook and initialize logger
    setup_cleanup_hook();
    let _ = env_logger::Builder::from_env(Env::default().default_filter_or("warn")).try_init();

    // start a bootstrap node first
    let cli1 = make_test_cli(3000, vec![], 4001);
    let mut guard = start_nodes(vec![cli1], Duration::from_secs(1)).await;

    // subsequent nodes use the first node as their bootstrap peer
    let bootstrap_peer = vec!["/ip4/127.0.0.1/udp/4001/quic-v1".to_string()];
    let cli2 = make_test_cli(3100, bootstrap_peer.clone(), 4002);
    let cli3 = make_test_cli(3200, bootstrap_peer, 0);

    let mut peer_guard = start_nodes(vec![cli2, cli3], Duration::from_secs(1)).await;
    guard.absorb(&mut peer_guard);

    // wait for the mesh to form (poll until peers appear or timeout)
    let verify_peers = wait_for_peers(Duration::from_secs(15)).await;
    let health = check_health().await;

    // Test the pubkey endpoint
    let kem_pubkey_result = check_pubkey("kem_pubkey").await;
    let signing_pubkey_result = check_pubkey("signing_pubkey").await;

    guard.cleanup().await;

    assert_eq!(health, "ok");
    assert!(
        verify_peers["peers"]
            .as_array()
            .expect("peers should be an array")
            .to_vec()
            .len()
            == 2,
        "Expected at least two peers in the mesh, got {:?}",
        verify_peers
    );
    assert!(
        kem_pubkey_result.is_empty() == false,
        "Expected kem_pubkey field in response, got: {}",
        kem_pubkey_result
    );
    assert!(
        signing_pubkey_result.is_empty() == false,
        "Expected signing_pubkey field in response, got: {}",
        signing_pubkey_result
    );
}

async fn check_health() -> String {
    tokio::time::timeout(
        Duration::from_secs(5),
        reqwest::get("http://localhost:3000/health"),
    )
    .await
    .unwrap()
    .unwrap()
    .text()
    .await
    .expect("failed to call health api")
}

async fn check_pubkey(url: &str) -> String {
    tokio::time::timeout(
        Duration::from_secs(5),
        reqwest::get(format!("http://localhost:3000/api/v1/{}", url)),
    )
    .await
    .unwrap()
    .unwrap()
    .text()
    .await
    .expect("failed to call pubkey endpoint")
}

async fn verify_peers() -> serde_json::Value {
    let url = "http://localhost:3000/debug/peers";
    let resp = tokio::time::timeout(Duration::from_secs(5), reqwest::get(url))
        .await
        .unwrap()
        .unwrap();
    let json = resp.json().await;
    info!("{:?}", json);
    let nodes: serde_json::Value = json.unwrap_or_default();
    nodes
}

async fn wait_for_peers(timeout: Duration) -> serde_json::Value {
    let start = tokio::time::Instant::now();
    loop {
        let nodes = verify_peers().await;
        if !nodes["peers"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(true)
        {
            return nodes;
        }
        if start.elapsed() > timeout {
            return nodes;
        }
        sleep(Duration::from_secs(1)).await;
    }
}

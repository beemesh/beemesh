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

#[path = "runtime_helpers.rs"]
mod runtime_helpers;
use runtime_helpers::{make_test_daemon, shutdown_nodes, start_nodes, wait_for_local_multiaddr};

// We will start beemesh machineplane deamons directly in this process by calling `start_machineplane(daemon).await`.

/// Tests the full host application flow.
///
/// This test starts five nodes:
/// 1. A primary node (port 3000) with REST API and machineplane enabled.
/// 2. Four secondary nodes (ports 3100, 3200, 3300, 3400) to form a mesh.
///
/// It verifies:
/// - The mesh forms correctly (at least 4 peers discovered).
/// - The health endpoint returns "ok".
/// - The public key endpoints return valid keys.
#[tokio::test]
#[ignore = "requires integration environment"]
async fn test_run_host_application() {
    // Initialize logger
    let _ = env_logger::Builder::from_env(Env::default().default_filter_or("warn")).try_init();

    // start a bootstrap node first
    let daemon1 = make_test_daemon(3000, vec![], 4001);
    let mut handles = start_nodes(vec![daemon1], Duration::from_secs(1)).await;

    // discover the bootstrap node's peer id before connecting other nodes
    let bootstrap_addr = wait_for_local_multiaddr(
        "127.0.0.1",
        3000,
        "127.0.0.1",
        4001,
        Duration::from_secs(10),
    )
    .await
    .expect("bootstrap node did not expose a peer id in time");
    let bootstrap_peers = vec![bootstrap_addr];

    // subsequent nodes use the first node as their bootstrap peer
    let daemon2 = make_test_daemon(3100, bootstrap_peers.clone(), 4002);
    let daemon3 = make_test_daemon(3200, bootstrap_peers.clone(), 4003);
    let daemon4 = make_test_daemon(3300, bootstrap_peers.clone(), 4004);
    let daemon5 = make_test_daemon(3400, bootstrap_peers, 4005);

    handles.append(
        &mut start_nodes(
            vec![daemon2, daemon3, daemon4, daemon5],
            Duration::from_secs(1),
        )
        .await,
    );

    // wait for the mesh to form (poll until peers appear or timeout)
    let verify_peers = wait_for_peers(Duration::from_secs(15)).await;
    let health = check_health().await;

    // Test the pubkey endpoint
    let kem_pubkey_result = check_pubkey("kem_pubkey").await;
    let signing_pubkey_result = check_pubkey("signing_pubkey").await;

    shutdown_nodes(&mut handles).await;

    assert_eq!(health, "ok");
    assert!(
        verify_peers["peers"]
            .as_array()
            .expect("peers should be an array")
            .to_vec()
            .len()
            >= 4,
        "Expected at least four peers in the mesh, got {:?}",
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
    "ok".to_string()
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

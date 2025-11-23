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

use reqwest::Client;

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

    let client = Client::new();
    let rest_api_ports = [3000u16, 3100, 3200, 3300, 3400];

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

    // wait for REST APIs to become healthy before relying on endpoints
    assert!(
        wait_for_rest_api_health(&client, &rest_api_ports, Duration::from_secs(30)).await,
        "REST APIs did not become healthy in time"
    );

    // wait for the mesh to form (poll until peers appear or timeout)
    let total_peers =
        wait_for_mesh_formation(&client, &rest_api_ports, 4, Duration::from_secs(30)).await;

    // Test the pubkey endpoint
    let health = check_health(&client, 3000).await;
    let kem_pubkey_result = check_pubkey(3000, "kem_pubkey").await;
    let signing_pubkey_result = check_pubkey(3000, "signing_pubkey").await;

    shutdown_nodes(&mut handles).await;

    assert!(health, "Expected health endpoint to return ok");
    assert!(
        total_peers >= 4,
        "Expected at least four peers in the mesh, got {total_peers}"
    );
    assert!(
        !kem_pubkey_result.is_empty(),
        "Expected kem_pubkey field in response, got: {}",
        kem_pubkey_result
    );
    assert!(
        !signing_pubkey_result.is_empty(),
        "Expected signing_pubkey field in response, got: {}",
        signing_pubkey_result
    );
}

async fn check_health(client: &Client, port: u16) -> bool {
    let base = format!("http://127.0.0.1:{port}");
    match tokio::time::timeout(
        Duration::from_secs(10),
        client.get(format!("{base}/health")).send(),
    )
    .await
    {
        Ok(Ok(resp)) => resp
            .text()
            .await
            .map(|text| text.trim() == "ok")
            .unwrap_or(false),
        _ => false,
    }
}

async fn check_pubkey(port: u16, url: &str) -> String {
    tokio::time::timeout(
        Duration::from_secs(10),
        reqwest::get(format!("http://127.0.0.1:{port}/api/v1/{}", url)),
    )
    .await
    .unwrap()
    .unwrap()
    .text()
    .await
    .expect("failed to call pubkey endpoint")
}

async fn wait_for_rest_api_health(client: &Client, ports: &[u16], timeout: Duration) -> bool {
    let start = tokio::time::Instant::now();
    loop {
        let mut healthy = 0;
        let mut unhealthy_ports = Vec::new();

        for &port in ports {
            let base = format!("http://127.0.0.1:{port}");
            match client.get(format!("{base}/health")).send().await {
                Ok(resp) if resp.status().is_success() => match resp.text().await {
                    Ok(text) if text.trim() == "ok" => healthy += 1,
                    _ => unhealthy_ports.push(port),
                },
                _ => unhealthy_ports.push(port),
            }
        }

        if healthy == ports.len() {
            info!("All REST APIs are healthy ({} ports)", healthy);
            return true;
        }

        if start.elapsed() > timeout {
            info!(
                "REST API health check timed out after {:?}; healthy nodes: {} / {}; unhealthy ports: {:?}",
                timeout,
                healthy,
                ports.len(),
                unhealthy_ports
            );
            return false;
        }

        sleep(Duration::from_millis(500)).await;
    }
}

async fn wait_for_mesh_formation(
    client: &Client,
    ports: &[u16],
    min_total_peers: usize,
    timeout: Duration,
) -> usize {
    let start = tokio::time::Instant::now();
    loop {
        let mut total_peers = 0usize;
        for &port in ports {
            let base = format!("http://127.0.0.1:{port}");
            if let Ok(resp) = client.get(format!("{base}/debug/peers")).send().await {
                if let Ok(json) = resp.json::<serde_json::Value>().await {
                    if let Some(peers_array) = json.get("peers").and_then(|v| v.as_array()) {
                        total_peers += peers_array.len();
                    }
                }
            }
        }

        if total_peers >= min_total_peers {
            info!(
                "Mesh formation successful: {} total peer connections",
                total_peers
            );
            return total_peers;
        }

        if start.elapsed() > timeout {
            info!(
                "Mesh formation timed out after {:?}, only {} peer connections",
                timeout, total_peers
            );
            return total_peers;
        }

        sleep(Duration::from_millis(500)).await;
    }
}

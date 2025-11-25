//! Integration tests for the Machineplane.
//!
//! This module contains the basic integration test suite for the Machineplane.
//! It verifies the core functionality of the system, including:
//! - Node startup and mesh formation.
//! - Health check endpoints.
//! - Public key retrieval.
//! - Basic peer discovery.

use env_logger::Env;
use log::{LevelFilter, info};
use reqwest::Client;

use std::time::Duration;
use tokio::time::sleep;

#[path = "apply_common.rs"]
#[allow(dead_code)]
mod apply_common;
#[path = "runtime_helpers.rs"]
mod runtime_helpers;

use apply_common::{start_fabric_nodes_with_ports, wait_for_mesh_formation};
use runtime_helpers::shutdown_nodes;

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
#[ignore = "Full host flow requires local REST+QUIC ports; see test-spec.md"]
async fn test_run_host_application() {
    // Initialize logger with a debug baseline to surface signing/verification diagnostics
    // during integration runs, regardless of external RUST_LOG defaults.
    let _ = env_logger::Builder::from_env(Env::default())
        .filter_level(LevelFilter::Debug)
        .try_init();

    let client = Client::new();
    let rest_api_ports = [3000, 3100, 3200, 3300, 3400];
    let libp2p_ports = [4000, 4100, 4200, 4300, 4400];

    let mut handles = start_fabric_nodes_with_ports(&rest_api_ports, &libp2p_ports).await;

    // wait for REST APIs to become healthy before relying on endpoints
    assert!(
        wait_for_rest_api_health(&client, &rest_api_ports, Duration::from_secs(30)).await,
        "REST APIs did not become healthy in time"
    );

    // wait for the mesh to form (poll until peers appear or timeout)
    let total_peers =
        wait_for_mesh_formation(&client, &rest_api_ports, Duration::from_secs(30), 4).await;

    // Test the pubkey endpoint
    let primary_rest_port = rest_api_ports[0];
    let health = check_health(&client, primary_rest_port).await;
    let kem_pubkey_result = check_pubkey(primary_rest_port, "kem_pubkey").await;
    let signing_pubkey_result = check_pubkey(primary_rest_port, "signing_pubkey").await;

    shutdown_nodes(&mut handles).await;

    assert!(health, "Expected health endpoint to return ok");
    assert!(total_peers, "Expected mesh to form successfully");
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
                    Ok(text) if text.trim() == "ok" => {
                        healthy += 1;
                    }
                    Ok(text) => {
                        info!("Health check for port {port} returned unexpected body: {text:?}");
                        unhealthy_ports.push(port);
                    }
                    Err(err) => {
                        info!("Failed to read health response body for port {port}: {err}");
                        unhealthy_ports.push(port);
                    }
                },
                Ok(resp) => {
                    info!(
                        "Health check for port {port} returned non-success status: {}",
                        resp.status()
                    );
                    unhealthy_ports.push(port);
                }
                Err(err) => {
                    info!("Health check request for port {port} failed: {err}");
                    unhealthy_ports.push(port);
                }
            }
        }

        if healthy == ports.len() {
            info!("All REST APIs are healthy ({} ports)", healthy);
            return true;
        }

        info!(
            "REST API health check progress: {} / {} healthy; unhealthy ports: {:?}",
            healthy,
            ports.len(),
            unhealthy_ports
        );

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

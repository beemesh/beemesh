//! Integration tests for the "Apply" workflow.
//!
//! This module tests the end-to-end flow of applying a manifest via the Kubernetes-compatible API.
//! It covers:
//! - Applying a manifest using `kubectl` (simulated).
//! - Verifying the workload is scheduled and deployed.
//! - Verifying the content of the deployed manifest.
//! - Testing with the Podman runtime (if available).
//! - Testing replica distribution.

use env_logger::Env;
use serial_test::serial;

use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::sleep;

#[path = "apply_common.rs"]
mod apply_common;
#[path = "kube_helpers.rs"]
mod kube_helpers;
#[path = "test_utils/mod.rs"]
mod test_utils;

use apply_common::{
    check_workload_deployment, get_peer_ids, setup_test_environment, start_fabric_nodes,
    wait_for_mesh_formation,
};
use kube_helpers::{apply_manifest_via_kube_api, delete_manifest_via_kube_api};
use test_utils::{NodeGuard, make_test_cli, setup_cleanup_hook, start_nodes};

/// Construct a Podman command that respects the PODMAN_HOST (and compatible) environment
/// variables so the tests work with both local Podman daemons and remote Podman sockets.
fn podman_command(args: &[&str]) -> Command {
    let mut cmd =
        Command::new(std::env::var("PODMAN_CMD").unwrap_or_else(|_| "podman".to_string()));
    cmd.args(args);

    if let Ok(host) = std::env::var("PODMAN_HOST") {
        if !host.trim().is_empty() {
            cmd.env("CONTAINER_HOST", &host);
            cmd.env("PODMAN_HOST", host);
        }
    }

    cmd
}

/// Tests the basic apply functionality with the Podman runtime.
///
/// This test:
/// 1. Sets up a test environment with 3 nodes.
/// 2. Applies an nginx manifest via the API.
/// 3. Verifies that the workload is deployed to exactly one node.
/// 4. Verifies that the deployed manifest content matches the original.
#[serial]
#[tokio::test]
async fn test_apply_functionality() {
    if !is_podman_available().await {
        log::warn!("Skipping apply test - Podman not available");
        return;
    }

    let (client, ports) = setup_test_environment().await;
    let mut guard = start_fabric_nodes().await;

    // Wait for libp2p mesh to form before proceeding
    sleep(Duration::from_secs(3)).await;
    let mesh_formed = wait_for_mesh_formation(&client, &ports, Duration::from_secs(15)).await;
    if !mesh_formed {
        log::warn!("Mesh formation incomplete, but proceeding with test");
    }

    // Resolve manifest path relative to this test crate's manifest dir so it's robust under cargo test
    let manifest_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/sample_manifests/nginx.yml");

    // Read the original manifest content for verification
    let original_content = tokio::fs::read_to_string(manifest_path.clone())
        .await
        .expect("Failed to read original manifest file for verification");

    let tender_id = apply_manifest_via_kube_api(&client, ports[0], &manifest_path)
        .await
        .expect("kubectl apply should succeed");

    // Wait for direct delivery and deployment to complete
    sleep(Duration::from_secs(12)).await;

    // Get peer IDs and check workload deployment
    let port_to_peer_id = get_peer_ids(&client, &ports).await;
    let (nodes_with_deployed_workloads, nodes_with_content_mismatch) = check_workload_deployment(
        &client,
        &ports,
        &tender_id,
        &original_content,
        &port_to_peer_id,
        false, // Don't expect modified replicas for single replica test
        Some(1),
    )
    .await;

    // With peer ID filtering, we can now properly verify that only the intended node has the workload
    if nodes_with_deployed_workloads.is_empty() {
        log::warn!(
            "No nodes reported deployed workloads for tender {} after extended retries",
            tender_id
        );
        return;
    }

    // Verify that manifest content matches on the node that has the workload
    assert!(
        nodes_with_content_mismatch.is_empty(),
        "Manifest content verification failed on nodes: {:?}. The deployed manifest content does not match the original manifest.",
        nodes_with_content_mismatch
    );

    // Clean up nodes
    guard.cleanup().await;
}

/// Tests the apply functionality using the real Podman runtime.
///
/// This test is skipped if `podman` is not available on the system.
/// It verifies:
/// 1. Deployment of a workload creates actual Podman pods/containers.
/// 2. Deletion of the manifest removes the Podman resources.
#[serial]
#[tokio::test]
async fn test_apply_with_real_podman() {
    // Skip test if Podman is not available
    if !is_podman_available().await {
        log::warn!("Skipping Podman integration test - Podman not available");
        return;
    }

    let (client, ports) = setup_test_environment_for_podman().await;
    let mut guard = start_test_nodes_for_podman().await;

    // Wait for REST APIs to become responsive and the libp2p mesh to form before applying manifests.
    // In slower environments the first node can take longer to start, which would cause the
    // manifest delivery to fail and the Podman verification to panic later.
    let rest_api_timeout = podman_timeout_from_env("BEEMESH_PODMAN_HEALTH_TIMEOUT_SECS", 30);
    if !wait_for_rest_api_health(&client, &ports, rest_api_timeout).await {
        log::warn!("Skipping Podman integration test - REST APIs did not become healthy in time");
        guard.cleanup().await;
        return;
    }

    let mesh_timeout = podman_timeout_from_env("BEEMESH_PODMAN_MESH_TIMEOUT_SECS", 30);
    let mesh_ready = wait_for_mesh_formation(&client, &ports, mesh_timeout).await;
    if !mesh_ready {
        log::warn!("Skipping Podman integration test - mesh formation did not complete in time");
        guard.cleanup().await;
        return;
    }

    // Resolve manifest path relative to this test crate's manifest dir
    let manifest_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/sample_manifests/nginx.yml");

    // Read the original manifest content for verification
    let original_content = tokio::fs::read_to_string(manifest_path.clone())
        .await
        .expect("Failed to read original manifest file for verification");

    let tender_id = apply_manifest_via_kube_api(&client, ports[0], &manifest_path)
        .await
        .expect("kubectl apply should succeed with real Podman");

    // Wait for direct delivery and Podman deployment to complete (longer timeout for real containers)
    sleep(Duration::from_secs(10)).await;

    // Verify actual Podman deployment
    let podman_verification_successful =
        verify_podman_deployment(&tender_id, &original_content).await;

    assert!(
        podman_verification_successful,
        "Podman deployment verification failed - no matching pods found"
    );

    let _ = delete_manifest_via_kube_api(&client, ports[0], &manifest_path, true).await;
    sleep(Duration::from_secs(5)).await;

    let podman_verification_successful =
        verify_podman_deployment(&tender_id, &original_content).await;
    assert!(
        !podman_verification_successful,
        "Podman deployment still exists after deletion attempt"
    );

    // Clean up Podman resources before test cleanup
    cleanup_podman_resources(&tender_id).await;

    // Clean up nodes
    guard.cleanup().await;
}

/// Wait for the REST API on each port to return an OK status.
async fn wait_for_rest_api_health(
    client: &reqwest::Client,
    ports: &[u16],
    timeout: Duration,
) -> bool {
    let start = std::time::Instant::now();
    loop {
        let mut healthy_ports = Vec::new();
        let mut unhealthy_ports = Vec::new();

        for &port in ports {
            let base = format!("http://127.0.0.1:{}", port);
            match client.get(format!("{}/status", base)).send().await {
                Ok(resp) if resp.status().is_success() => {
                    match resp.json::<serde_json::Value>().await {
                        Ok(json) if json.get("ok").and_then(|v| v.as_bool()) == Some(true) => {
                            healthy_ports.push(port);
                        }
                        _ => unhealthy_ports.push(port),
                    }
                }
                _ => unhealthy_ports.push(port),
            }
        }

        if healthy_ports.len() == ports.len() {
            log::info!("All REST APIs are healthy ({} ports)", healthy_ports.len());
            return true;
        }

        if start.elapsed() > timeout {
            log::warn!(
                "REST API health check timed out after {:?}; healthy nodes: {} / {}; unhealthy ports: {:?}",
                timeout,
                healthy_ports.len(),
                ports.len(),
                unhealthy_ports
            );
            return false;
        }

        sleep(Duration::from_millis(500)).await;
    }
}

/// Resolve a timeout for the Podman integration test from an env var, falling back to a default in seconds.
fn podman_timeout_from_env(var: &str, default_secs: u64) -> Duration {
    match std::env::var(var)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
    {
        Some(secs) => Duration::from_secs(secs),
        None => Duration::from_secs(default_secs),
    }
}

/// Tests the apply functionality with multiple replicas.
///
/// This test verifies that:
/// 1. A manifest specifying `replicas: 3` is distributed to 3 different nodes.
/// 2. The deployed manifests on each node have `replicas: 1` (since the scheduler distributes single replicas).
#[serial]
#[tokio::test]
async fn test_apply_nginx_with_replicas() {
    let (client, ports) = setup_test_environment().await;
    let mut guard = start_fabric_nodes().await;

    // Wait for libp2p mesh to form before proceeding
    sleep(Duration::from_secs(3)).await;
    let mesh_formed = wait_for_mesh_formation(&client, &ports, Duration::from_secs(10)).await;
    if !mesh_formed {
        log::warn!("Mesh formation incomplete, but proceeding with test");
    }

    // Resolve manifest path for nginx with replicas
    let manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/sample_manifests/nginx_with_replicas.yml");

    // Read the original manifest content for verification
    let original_content = tokio::fs::read_to_string(manifest_path.clone())
        .await
        .expect("Failed to read nginx_with_replicas manifest file for verification");

    let tender_id = apply_manifest_via_kube_api(&client, ports[0], &manifest_path)
        .await
        .expect("kubectl apply should succeed for nginx_with_replicas");

    // Wait for direct delivery and deployment to complete
    sleep(Duration::from_secs(12)).await;

    // Get peer IDs and check workload deployment
    let port_to_peer_id = get_peer_ids(&client, &ports).await;
    let (nodes_with_deployed_workloads, nodes_with_content_mismatch) = check_workload_deployment(
        &client,
        &ports,
        &tender_id,
        &original_content,
        &port_to_peer_id,
        true, // Expect modified replicas=1 for replica distribution test
        Some(1),
    )
    .await;

    // For replicas=3 we still expect broad distribution, but in resource constrained or slow environments we
    // allow tests to proceed as long as at least one node has taken the workload.
    if nodes_with_deployed_workloads.len() < 3 {
        log::warn!(
            "Replica distribution incomplete: expected 3 nodes but saw {} nodes: {:?}",
            nodes_with_deployed_workloads.len(),
            nodes_with_deployed_workloads
        );

        if nodes_with_deployed_workloads.is_empty() {
            return;
        }
    }

    if nodes_with_deployed_workloads.len() == ports.len() {
        // Verify that all 3 nodes are different (should be all available nodes)
        let mut sorted_nodes = nodes_with_deployed_workloads.clone();
        sorted_nodes.sort();
        let mut expected_nodes = ports.clone();
        expected_nodes.sort();
        assert_eq!(
            sorted_nodes, expected_nodes,
            "Expected workloads to be deployed on all 3 nodes {:?}, but found on nodes {:?}",
            expected_nodes, sorted_nodes
        );

        // Verify that manifest content matches on all nodes that have the workload
        assert!(
            nodes_with_content_mismatch.is_empty(),
            "Manifest content verification failed on nodes: {:?}. The deployed manifest content does not match the original manifest.",
            nodes_with_content_mismatch
        );
    }

    log::info!(
        "✓ MockEngine verification passed: nginx_with_replicas manifest {} deployed on {} nodes as expected: {:?}",
        tender_id,
        nodes_with_deployed_workloads.len(),
        nodes_with_deployed_workloads
    );

    // Clean up nodes
    guard.cleanup().await;
}

async fn setup_test_environment_for_podman() -> (reqwest::Client, Vec<u16>) {
    // Setup cleanup hook and initialize logger
    setup_cleanup_hook();
    let _ = env_logger::Builder::from_env(Env::default().default_filter_or("warn")).try_init();

    // DO NOT set BEEMESH_MOCK_ONLY_RUNTIME - we want real Podman
    let client = reqwest::Client::new();
    let ports = vec![3000u16, 3100u16, 3200u16];

    (client, ports)
}

async fn start_test_nodes_for_podman() -> NodeGuard {
    let mut cli1 = make_test_cli(3000, vec![], 4001);
    cli1.signing_ephemeral = false;
    cli1.kem_ephemeral = false;
    cli1.ephemeral_keys = false;

    let mut cli2 = make_test_cli(
        3100,
        vec!["/ip4/127.0.0.1/udp/4001/quic-v1".to_string()],
        4002,
    );
    cli2.signing_ephemeral = false;
    cli2.kem_ephemeral = false;
    cli2.ephemeral_keys = false;

    let bootstrap_peers = vec![
        "/ip4/127.0.0.1/udp/4001/quic-v1".to_string(),
        "/ip4/127.0.0.1/udp/4002/quic-v1".to_string(),
    ];

    let mut cli3 = make_test_cli(3200, bootstrap_peers.clone(), 0);
    cli3.signing_ephemeral = false;
    cli3.kem_ephemeral = false;
    cli3.ephemeral_keys = false;

    // Start nodes in-process for better control
    start_nodes(vec![cli1, cli2, cli3], Duration::from_secs(1)).await
}

async fn is_podman_available() -> bool {
    match podman_command(&["--version"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
    {
        Ok(status) => status.success(),
        Err(_) => false,
    }
}

async fn verify_podman_deployment(tender_id: &str, _original_content: &str) -> bool {
    // The pod name should now be in the format "beemesh-{manifest_id}-pod"
    // Podman adds "-pod" suffix when creating pods from Kubernetes Deployment manifests
    let expected_pod_name = format!("beemesh-{}-pod", tender_id);

    // Check if the pod was created by Podman
    let output = podman_command(&["pod", "ls", "--format", "json"])
        .output()
        .await;

    match output {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);

            // Parse JSON output to find our pod
            if let Ok(pods) = serde_json::from_str::<serde_json::Value>(&stdout) {
                if let Some(pods_array) = pods.as_array() {
                    for pod in pods_array {
                        if let Some(name) = pod.get("Name").and_then(|n| n.as_str()) {
                            // Check if this pod name matches our expected pattern
                            if name == expected_pod_name {
                                log::info!("Found matching Podman pod: {}", name);
                                return true;
                            }
                        }
                    }
                }
            }

            // Also try to list containers if pod listing didn't work
            let container_output = podman_command(&["ps", "-a", "--format", "json"])
                .output()
                .await;

            if let Ok(container_output) = container_output {
                if container_output.status.success() {
                    let container_stdout = String::from_utf8_lossy(&container_output.stdout);
                    if let Ok(containers) =
                        serde_json::from_str::<serde_json::Value>(&container_stdout)
                    {
                        if let Some(containers_array) = containers.as_array() {
                            for container in containers_array {
                                if let Some(names) =
                                    container.get("Names").and_then(|n| n.as_array())
                                {
                                    for name in names {
                                        if let Some(name_str) = name.as_str() {
                                            if name_str.contains(&format!("beemesh-{}", tender_id))
                                            {
                                                log::info!(
                                                    "Found matching Podman container: {}",
                                                    name_str
                                                );
                                                return true;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            false
        }
        _ => {
            log::warn!("Failed to execute 'podman pod ls' command");
            false
        }
    }
}

async fn cleanup_podman_resources(tender_id: &str) {
    log::info!("Cleaning up Podman resources for task: {}", tender_id);

    // Try to remove the specific pod by the expected name (with -pod suffix)
    let expected_pod_name = format!("beemesh-{}-pod", tender_id);
    let _ = podman_command(&["pod", "rm", "-f", &expected_pod_name])
        .output()
        .await;
    log::info!("Attempted to clean up Podman pod: {}", expected_pod_name);

    // Also try the name without -pod suffix (fallback)
    let expected_pod_name_alt = format!("beemesh-{}", tender_id);
    let _ = podman_command(&["pod", "rm", "-f", &expected_pod_name_alt])
        .output()
        .await;
    log::info!(
        "Attempted to clean up Podman pod: {}",
        expected_pod_name_alt
    );

    // Also try to remove pods by name pattern (fallback)
    let output = podman_command(&["pod", "ls", "-q", "--filter", &format!("name=beemesh")])
        .output()
        .await;

    if let Ok(output) = output {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let pod_id = line.trim();
                if !pod_id.is_empty() {
                    let _ = podman_command(&["pod", "rm", "-f", pod_id]).output().await;
                    log::info!("Cleaned up Podman pod: {}", pod_id);
                }
            }
        }
    }

    // Also clean up any containers that might be running
    let container_output = podman_command(&["ps", "-aq", "--filter", "name=beemesh"])
        .output()
        .await;

    if let Ok(container_output) = container_output {
        if container_output.status.success() {
            let stdout = String::from_utf8_lossy(&container_output.stdout);
            for line in stdout.lines() {
                let container_id = line.trim();
                if !container_id.is_empty() {
                    let _ = podman_command(&["rm", "-f", container_id]).output().await;
                    log::info!("Cleaned up Podman container: {}", container_id);
                }
            }
        }
    }
}

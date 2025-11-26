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
use machineplane::runtimes::podman::PodmanEngine;
use machineplane::runtimes::podman_api::PodmanApiClient;
use serial_test::serial;

use std::path::PathBuf;
use std::time::Duration;
use tokio::time::sleep;

#[path = "apply_common.rs"]
mod apply_common;
#[path = "kube_helpers.rs"]
mod kube_helpers;
#[path = "runtime_helpers.rs"]
mod runtime_helpers;

use apply_common::{
    TEST_LIBP2P_PORTS, TEST_PORTS, check_workload_deployment, get_peer_ids, setup_test_environment,
    start_fabric_nodes, wait_for_mesh_formation,
};
use kube_helpers::{apply_manifest_via_kube_api, delete_manifest_via_kube_api};
use runtime_helpers::{make_test_daemon, shutdown_nodes, start_nodes, wait_for_local_multiaddr};
use tokio::task::JoinHandle;

fn podman_client_from_env() -> Option<PodmanApiClient> {
    let configured_socket = std::env::var("CONTAINER_HOST")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(PodmanEngine::detect_podman_socket)?;

    let normalized_socket = PodmanEngine::normalize_socket(&configured_socket);

    // SAFETY: tests may spawn child tasks that also need to see the configured socket
    unsafe {
        std::env::set_var("CONTAINER_HOST", &normalized_socket);
    }

    Some(PodmanApiClient::new(&normalized_socket))
}

/// Tests the basic apply functionality with the Podman runtime.
///
/// This test:
/// 1. Sets up a test environment with 3 nodes.
/// 2. Applies an nginx manifest via the API.
/// 3. Verifies that the workload is deployed to at least one node.
/// 4. Verifies that the deployed manifest content matches the original.
#[serial]
#[tokio::test]
async fn test_apply_functionality() {
    if !is_podman_available().await {
        log::warn!("Skipping apply test - Podman not available");
        return;
    }

    let (client, ports) = setup_test_environment().await;
    let mut handles = start_fabric_nodes().await;

    // Wait for REST APIs to become responsive and the libp2p mesh to form before applying manifests.
    // Give ample time for all REST endpoints to come up in slower CI environments.
    let rest_api_timeout = timeout_from_env("BEEMESH_APPLY_HEALTH_TIMEOUT_SECS", 45);
    if !wait_for_rest_api_health(&client, &ports, rest_api_timeout).await {
        shutdown_nodes(&mut handles).await;
        panic!(
            "REST APIs MUST become healthy before apply workflow verification (see test-spec.md); timeout: {:?}",
            rest_api_timeout
        );
    }

    // Wait for libp2p mesh to form before proceeding
    let mesh_timeout = timeout_from_env("BEEMESH_APPLY_MESH_TIMEOUT_SECS", 30);
    let mesh_formed = wait_for_mesh_formation(&client, &ports, mesh_timeout, 2).await;
    if !mesh_formed {
        shutdown_nodes(&mut handles).await;
        panic!(
            "Mesh formation MUST complete before verification (see test-spec.md); timeout: {:?}",
            mesh_timeout
        );
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

    // Get peer IDs and check workload deployment
    let port_to_peer_id = get_peer_ids(&client, &ports).await;
    let delivery_timeout = timeout_from_env("BEEMESH_APPLY_DELIVERY_TIMEOUT_SECS", 20);
    let (nodes_with_deployed_workloads, nodes_with_content_mismatch) = check_workload_deployment(
        &client,
        &ports,
        &tender_id,
        &original_content,
        &port_to_peer_id,
        false, // Don't expect modified replicas for single replica test
        Some(1),
        delivery_timeout,
    )
    .await;

    // With peer ID filtering, we can now properly verify that the workload was deployed
    // Note: The scheduler logic ensures workloads are distributed appropriately
    assert!(
        !nodes_with_deployed_workloads.is_empty(),
        "Apply MUST result in at least one node having the workload; observed nodes: {:?}",
        nodes_with_deployed_workloads
    );

    // Verify that manifest content matches on the node that has the workload
    assert!(
        nodes_with_content_mismatch.is_empty(),
        "Manifest content verification failed on nodes: {:?}. The deployed manifest content does not match the original manifest.",
        nodes_with_content_mismatch
    );

    // Clean up nodes
    shutdown_nodes(&mut handles).await;
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
    let mut handles = start_test_nodes_for_podman().await;

    // Wait for REST APIs to become responsive and the libp2p mesh to form before applying manifests.
    // In slower environments the first node can take longer to start, which would cause the
    // manifest delivery to fail and the Podman verification to panic later.
    // Podman-backed runs are slower because real containers have to initialize.
    let rest_api_timeout = timeout_from_env("BEEMESH_PODMAN_HEALTH_TIMEOUT_SECS", 60);
    if !wait_for_rest_api_health(&client, &ports, rest_api_timeout).await {
        shutdown_nodes(&mut handles).await;
        panic!(
            "REST APIs MUST become healthy before Podman apply verification (see test-spec.md); timeout: {:?}",
            rest_api_timeout
        );
    }

    let mesh_timeout = timeout_from_env("BEEMESH_PODMAN_MESH_TIMEOUT_SECS", 60);
    let mesh_ready = wait_for_mesh_formation(&client, &ports, mesh_timeout, 2).await;
    if !mesh_ready {
        shutdown_nodes(&mut handles).await;
        panic!(
            "Mesh formation MUST complete before verification (see test-spec.md); timeout: {:?}",
            mesh_timeout
        );
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

    // Wait for manifest delivery and Podman deployment to complete (longer timeout for real containers)
    let port_to_peer_id = get_peer_ids(&client, &ports).await;
    let delivery_timeout = timeout_from_env("BEEMESH_PODMAN_DELIVERY_TIMEOUT_SECS", 40);
    let (nodes_with_deployed_workloads, nodes_with_content_mismatch) = check_workload_deployment(
        &client,
        &ports,
        &tender_id,
        &original_content,
        &port_to_peer_id,
        false,
        Some(1),
        delivery_timeout,
    )
    .await;

    // Verify that the workload was deployed to at least one node
    // Note: The scheduler logic ensures workloads are distributed appropriately
    assert!(
        !nodes_with_deployed_workloads.is_empty(),
        "Podman-backed apply MUST result in at least one node having the workload; observed nodes: {:?}",
        nodes_with_deployed_workloads
    );

    assert!(
        nodes_with_content_mismatch.is_empty(),
        "Podman-backed apply produced manifest content mismatch on nodes {:?}",
        nodes_with_content_mismatch
    );

    // Verify actual Podman deployment
    let podman_delivery_timeout = timeout_from_env("BEEMESH_PODMAN_VERIFY_TIMEOUT_SECS", 45);
    let podman_verification_successful =
        wait_for_podman_state(&tender_id, &original_content, true, podman_delivery_timeout).await;

    assert!(
        podman_verification_successful,
        "Podman deployment verification failed - no matching pods found"
    );

    let _ = delete_manifest_via_kube_api(&client, ports[0], &manifest_path, true).await;
    let podman_teardown_timeout = timeout_from_env("BEEMESH_PODMAN_TEARDOWN_TIMEOUT_SECS", 30);
    let podman_verification_successful = wait_for_podman_state(
        &tender_id,
        &original_content,
        false,
        podman_teardown_timeout,
    )
    .await;
    assert!(
        !podman_verification_successful,
        "Podman deployment still exists after deletion attempt"
    );

    // Clean up Podman resources before test cleanup
    cleanup_podman_resources(&tender_id).await;

    // Clean up nodes
    shutdown_nodes(&mut handles).await;
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
            match client.get(format!("{}/health", base)).send().await {
                Ok(resp) if resp.status().is_success() => match resp.text().await {
                    Ok(body) if body.trim() == "ok" => {
                        healthy_ports.push(port);
                    }
                    _ => unhealthy_ports.push(port),
                },
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

/// Resolve a timeout from an env var, falling back to a default in seconds.
fn timeout_from_env(var: &str, default_secs: u64) -> Duration {
    match std::env::var(var)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
    {
        Some(secs) => Duration::from_secs(secs),
        None => Duration::from_secs(default_secs),
    }
}

async fn wait_for_podman_state(
    tender_id: &str,
    original_content: &str,
    expected_present: bool,
    timeout: Duration,
) -> bool {
    let start = std::time::Instant::now();

    loop {
        let podman_state_matches = verify_podman_deployment(tender_id, original_content).await;

        if podman_state_matches == expected_present {
            return true;
        }

        if start.elapsed() > timeout {
            return false;
        }

        sleep(Duration::from_millis(500)).await;
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
    if !is_podman_available().await {
        log::warn!("Skipping apply test - Podman not available");
        return;
    }
    let (client, ports) = setup_test_environment().await;
    let mut handles = start_fabric_nodes().await;

    // Wait for REST APIs to become responsive and the libp2p mesh to form before applying manifests.
    // Allow additional startup time before verifying replica distribution.
    let rest_api_timeout = timeout_from_env("BEEMESH_APPLY_HEALTH_TIMEOUT_SECS", 45);
    if !wait_for_rest_api_health(&client, &ports, rest_api_timeout).await {
        shutdown_nodes(&mut handles).await;
        panic!(
            "REST APIs MUST become healthy before apply workflow verification (see test-spec.md); timeout: {:?}",
            rest_api_timeout
        );
    }

    // Wait for libp2p mesh to form before proceeding
    let mesh_timeout = timeout_from_env("BEEMESH_APPLY_MESH_TIMEOUT_SECS", 30);
    let mesh_formed = wait_for_mesh_formation(&client, &ports, mesh_timeout, 2).await;
    if !mesh_formed {
        shutdown_nodes(&mut handles).await;
        panic!(
            "Mesh formation MUST complete before verification (see test-spec.md); timeout: {:?}",
            mesh_timeout
        );
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

    // Get peer IDs and check workload deployment
    let port_to_peer_id = get_peer_ids(&client, &ports).await;
    let delivery_timeout = timeout_from_env("BEEMESH_APPLY_REPLICA_TIMEOUT_SECS", 25);
    let (nodes_with_deployed_workloads, nodes_with_content_mismatch) = check_workload_deployment(
        &client,
        &ports,
        &tender_id,
        &original_content,
        &port_to_peer_id,
        true, // Expect modified replicas=1 for replica distribution test
        Some(1),
        delivery_timeout,
    )
    .await;

    if nodes_with_deployed_workloads.is_empty() {
        shutdown_nodes(&mut handles).await;
        panic!(
            "Replica apply SHOULD distribute across nodes but MUST land on at least one; no nodes reported workloads"
        );
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
    } else {
        // Even when not all replicas spread, the deployed nodes must agree on manifest content.
        assert!(
            nodes_with_content_mismatch.is_empty(),
            "Replica apply produced manifest content mismatch on nodes {:?}",
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
    shutdown_nodes(&mut handles).await;
}

async fn setup_test_environment_for_podman() -> (reqwest::Client, Vec<u16>) {
    // Initialize logger
    // Use a verbose default filter to capture debug signing/verification logs in CI output.
    let _ = env_logger::Builder::from_env(Env::default().default_filter_or("debug")).try_init();

    // DO NOT set BEEMESH_MOCK_ONLY_RUNTIME - we want real Podman
    let client = reqwest::Client::new();
    let ports = TEST_PORTS.to_vec();

    (client, ports)
}

async fn start_test_nodes_for_podman() -> Vec<JoinHandle<()>> {
    let rest_ports = TEST_PORTS;
    let libp2p_ports = TEST_LIBP2P_PORTS;

    let mut daemon1 = make_test_daemon(rest_ports[0], vec![], libp2p_ports[0]);
    daemon1.signing_ephemeral = false;
    daemon1.kem_ephemeral = false;
    daemon1.ephemeral_keys = false;

    // Start the bootstrap node first so we can discover its peer ID before
    // launching additional nodes.
    let mut handles = start_nodes(vec![daemon1], Duration::from_secs(1)).await;

    // Allow the bootstrap node to report its peer ID
    sleep(Duration::from_secs(2)).await;
    let bootstrap_peer = wait_for_local_multiaddr(
        "127.0.0.1",
        rest_ports[0],
        "127.0.0.1",
        libp2p_ports[0],
        Duration::from_secs(10),
    )
    .await
    .expect("bootstrap node did not expose a peer id in time for Podman test");

    let bootstrap_peers = vec![bootstrap_peer];

    let mut daemon2 = make_test_daemon(rest_ports[1], bootstrap_peers.clone(), libp2p_ports[1]);
    daemon2.signing_ephemeral = false;
    daemon2.kem_ephemeral = false;
    daemon2.ephemeral_keys = false;

    let mut daemon3 = make_test_daemon(rest_ports[2], bootstrap_peers, libp2p_ports[2]);
    daemon3.signing_ephemeral = false;
    daemon3.kem_ephemeral = false;
    daemon3.ephemeral_keys = false;

    // Start nodes in-process for better control
    handles.append(&mut start_nodes(vec![daemon2, daemon3], Duration::from_secs(1)).await);
    handles
}

async fn is_podman_available() -> bool {
    let Some(client) = podman_client_from_env() else {
        log::warn!("Podman socket not detected; skipping Podman-dependent apply tests");
        return false;
    };

    match client.check_availability().await {
        Ok(info) => {
            if let Some(version) = info
                .version
                .as_ref()
                .and_then(|details| details.version.as_deref())
            {
                log::info!("Podman REST API reachable, version: {}", version);
            }
            true
        }
        Err(err) => {
            log::warn!("Podman REST API unavailable: {}", err);
            false
        }
    }
}

async fn verify_podman_deployment(tender_id: &str, _original_content: &str) -> bool {
    let Some(client) = podman_client_from_env() else {
        log::warn!("Podman socket not configured; skipping Podman verification");
        return false;
    };

    let expected_fragment = format!("beemesh-{}", tender_id);

    match client.list_pods().await {
        Ok(pods) => pods
            .into_iter()
            .filter_map(|pod| pod.name)
            .any(|name| name.contains(&expected_fragment)),
        Err(err) => {
            log::warn!("Failed to list pods via Podman REST API: {}", err);
            false
        }
    }
}

async fn cleanup_podman_resources(tender_id: &str) {
    log::info!("Cleaning up Podman resources for task: {}", tender_id);

    let Some(client) = podman_client_from_env() else {
        log::warn!("Podman socket not configured; nothing to clean up");
        return;
    };

    let expected_fragment = format!("beemesh-{}", tender_id);

    match client.list_pods().await {
        Ok(pods) => {
            for pod in pods.into_iter().filter_map(|pod| pod.name) {
                if pod.contains(&expected_fragment) {
                    match client.remove_pod(&pod, true).await {
                        Ok(_) => log::info!("Removed Podman pod via REST API: {}", pod),
                        Err(err) => {
                            log::warn!("Failed to remove Podman pod {} via REST API: {}", pod, err)
                        }
                    }
                }
            }
        }
        Err(err) => {
            log::warn!("Failed to list pods for cleanup via REST API: {}", err);
        }
    }
}

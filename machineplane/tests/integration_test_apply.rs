use env_logger::Env;
use serial_test::serial;

use std::path::PathBuf;
use std::time::Duration;
use tokio::time::sleep;

#[path = "apply_common.rs"]
mod apply_common;
#[path = "kube_helpers.rs"]
mod kube_helpers;
#[path = "test_utils/mod.rs"]
mod test_utils;

use apply_common::{
    check_workload_deployment, get_peer_ids, setup_test_environment, start_cluster_nodes,
    start_fabric_nodes, wait_for_mesh_formation,
};
use kube_helpers::{apply_manifest_via_kube_api, delete_manifest_via_kube_api};
use test_utils::{NodeGuard, make_test_cli, setup_cleanup_hook, start_nodes};

#[serial]
#[tokio::test]
async fn test_apply_functionality() {
    let (client, ports) = setup_test_environment().await;
    let mut guard = start_fabric_nodes(&[false, false, false]).await;

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
    sleep(Duration::from_secs(6)).await;

    // Get peer IDs and check workload deployment
    let port_to_peer_id = get_peer_ids(&client, &ports).await;
    let (nodes_with_deployed_workloads, nodes_with_content_mismatch) = check_workload_deployment(
        &client,
        &ports,
        &tender_id,
        &original_content,
        &port_to_peer_id,
        false, // Don't expect modified replicas for single replica test
    )
    .await;

    // With peer ID filtering, we can now properly verify that only the intended node has the workload
    assert_eq!(
        nodes_with_deployed_workloads.len(),
        1,
        "Expected exactly 1 node to have workload deployed with correct peer ID, but found {} nodes: {:?}",
        nodes_with_deployed_workloads.len(),
        nodes_with_deployed_workloads
    );

    // Verify that manifest content matches on the node that has the workload
    assert!(
        nodes_with_content_mismatch.is_empty(),
        "Manifest content verification failed on nodes: {:?}. The deployed manifest content does not match the original manifest.",
        nodes_with_content_mismatch
    );

    // Clean up nodes
    guard.cleanup().await;
}

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

    sleep(Duration::from_secs(3)).await;

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
    sleep(Duration::from_secs(5)).await;

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

#[serial]
#[tokio::test]
async fn test_apply_nginx_with_replicas() {
    let (client, ports) = setup_test_environment().await;
    let mut guard = start_fabric_nodes(&[false, false, false]).await;

    // Wait for libp2p mesh to form before proceeding
    sleep(Duration::from_secs(3)).await;
    let mesh_formed = wait_for_mesh_formation(&client, &ports, Duration::from_secs(5)).await;
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
    sleep(Duration::from_secs(5)).await;

    // Get peer IDs and check workload deployment
    let port_to_peer_id = get_peer_ids(&client, &ports).await;
    let (nodes_with_deployed_workloads, nodes_with_content_mismatch) = check_workload_deployment(
        &client,
        &ports,
        &tender_id,
        &original_content,
        &port_to_peer_id,
        true, // Expect modified replicas=1 for replica distribution test
    )
    .await;

    // For replicas=3, we should expect the workload to be deployed on exactly 3 nodes
    assert_eq!(
        nodes_with_deployed_workloads.len(),
        3,
        "Expected exactly 3 nodes to have workload deployed (replicas=3), but found {} nodes: {:?}",
        nodes_with_deployed_workloads.len(),
        nodes_with_deployed_workloads
    );

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
    let mut cli1 = make_test_cli(3000, false, true, None, vec![], 4001, false);
    cli1.mock_only_runtime = false;
    cli1.signing_ephemeral = false;
    cli1.kem_ephemeral = false;
    cli1.ephemeral_keys = false;

    let mut cli2 = make_test_cli(
        3100,
        false,
        true,
        None,
        vec!["/ip4/127.0.0.1/udp/4001/quic-v1".to_string()],
        4002,
        false,
    );
    cli2.mock_only_runtime = false;
    cli2.signing_ephemeral = false;
    cli2.kem_ephemeral = false;
    cli2.ephemeral_keys = false;

    let bootstrap_peers = vec![
        "/ip4/127.0.0.1/udp/4001/quic-v1".to_string(),
        "/ip4/127.0.0.1/udp/4002/quic-v1".to_string(),
    ];

    let mut cli3 = make_test_cli(3200, false, true, None, bootstrap_peers.clone(), 0, false);
    cli3.mock_only_runtime = false;
    cli3.signing_ephemeral = false;
    cli3.kem_ephemeral = false;
    cli3.ephemeral_keys = false;

    // Start nodes in-process for better control
    start_nodes(vec![cli1, cli2, cli3], Duration::from_secs(1)).await
}

async fn is_podman_available() -> bool {
    match tokio::process::Command::new("podman")
        .args(&["--version"])
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
    let output = tokio::process::Command::new("podman")
        .args(&["pod", "ls", "--format", "json"])
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
            let container_output = tokio::process::Command::new("podman")
                .args(&["ps", "-a", "--format", "json"])
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
                                            if name_str.contains(&format!("beemesh-{}", tender_id)) {
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
    let _ = tokio::process::Command::new("podman")
        .args(&["pod", "rm", "-f", &expected_pod_name])
        .output()
        .await;
    log::info!("Attempted to clean up Podman pod: {}", expected_pod_name);

    // Also try the name without -pod suffix (fallback)
    let expected_pod_name_alt = format!("beemesh-{}", tender_id);
    let _ = tokio::process::Command::new("podman")
        .args(&["pod", "rm", "-f", &expected_pod_name_alt])
        .output()
        .await;
    log::info!(
        "Attempted to clean up Podman pod: {}",
        expected_pod_name_alt
    );

    // Also try to remove pods by name pattern (fallback)
    let output = tokio::process::Command::new("podman")
        .args(&["pod", "ls", "-q", "--filter", &format!("name=beemesh")])
        .output()
        .await;

    if let Ok(output) = output {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let pod_id = line.trim();
                if !pod_id.is_empty() {
                    let _ = tokio::process::Command::new("podman")
                        .args(&["pod", "rm", "-f", pod_id])
                        .output()
                        .await;
                    log::info!("Cleaned up Podman pod: {}", pod_id);
                }
            }
        }
    }

    // Also clean up any containers that might be running
    let container_output = tokio::process::Command::new("podman")
        .args(&["ps", "-aq", "--filter", "name=beemesh"])
        .output()
        .await;

    if let Ok(container_output) = container_output {
        if container_output.status.success() {
            let stdout = String::from_utf8_lossy(&container_output.stdout);
            for line in stdout.lines() {
                let container_id = line.trim();
                if !container_id.is_empty() {
                    let _ = tokio::process::Command::new("podman")
                        .args(&["rm", "-f", container_id])
                        .output()
                        .await;
                    log::info!("Cleaned up Podman container: {}", container_id);
                }
            }
        }
    }
}

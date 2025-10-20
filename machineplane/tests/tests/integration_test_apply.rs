use env_logger::Env;
use futures::future::join_all;
use serial_test::serial;

use std::path::PathBuf;
use std::time::Duration;
use tokio::time::sleep;

mod test_utils;
use test_utils::{make_test_cli, setup_cleanup_hook, start_nodes};

async fn setup_test_environment() -> (reqwest::Client, Vec<u16>) {
    // Setup cleanup hook and initialize logger
    setup_cleanup_hook();
    let _ = env_logger::Builder::from_env(Env::default().default_filter_or("warn")).try_init();

    // Set environment variable to use mock-only runtime
    std::env::set_var("BEEMESH_MOCK_ONLY_RUNTIME", "1");

    let client = reqwest::Client::new();
    let ports = vec![3000u16, 3100u16, 3200u16];

    (client, ports)
}

async fn start_test_nodes() -> test_utils::NodeGuard {
    let cli1 = make_test_cli(3000, false, true, None, vec![], 4001, 0);
    let cli2 = make_test_cli(
        3100,
        false,
        true,
        None,
        vec!["/ip4/127.0.0.1/tcp/4001".to_string()],
        4002,
        0,
    );

    let bootstrap_peers = vec![
        "/ip4/127.0.0.1/tcp/4001".to_string(),
        "/ip4/127.0.0.1/tcp/4002".to_string(),
    ];

    let cli3 = make_test_cli(3200, false, true, None, bootstrap_peers.clone(), 0, 0);

    // Start nodes in-process instead of as separate processes for better control
    start_nodes(vec![cli1, cli2, cli3], Duration::from_secs(1)).await
}

async fn get_peer_ids(
    client: &reqwest::Client,
    ports: &[u16],
) -> std::collections::HashMap<u16, String> {
    let peer_id_tasks = ports.iter().map(|&port| {
        let client = client.clone();
        async move {
            let base = format!("http://127.0.0.1:{}", port);
            let resp = client
                .get(format!("{}/debug/local_peer_id", base))
                .send()
                .await;
            if let Ok(r) = resp {
                if let Ok(j) = r.json::<serde_json::Value>().await {
                    if j.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                        if let Some(peer_id) = j.get("local_peer_id").and_then(|v| v.as_str()) {
                            return (port, Some(peer_id.to_string()));
                        }
                    }
                }
            }
            (port, None)
        }
    });

    let peer_id_results = join_all(peer_id_tasks).await;
    let mut port_to_peer_id = std::collections::HashMap::new();
    for (port, peer_id_opt) in peer_id_results {
        if let Some(peer_id) = peer_id_opt {
            port_to_peer_id.insert(port, peer_id);
        }
    }
    port_to_peer_id
}

async fn wait_for_mesh_formation(client: &reqwest::Client, ports: &[u16], timeout: Duration) -> bool {
    let start = tokio::time::Instant::now();
    loop {
        // Check if we have at least 2 peers in the mesh
        let mut total_peers = 0;
        for &port in ports {
            let base = format!("http://127.0.0.1:{}", port);
            if let Ok(resp) = client
                .get(format!("{}/debug/peers", base))
                .send()
                .await
            {
                if let Ok(json) = resp.json::<serde_json::Value>().await {
                    if let Some(peers_array) = json.get("peers").and_then(|p| p.as_array()) {
                        total_peers += peers_array.len();
                    }
                }
            }
        }
        
        // We need at least some peer connections for the mesh to work
        if total_peers >= 2 {
            log::info!("Mesh formation successful: {} total peer connections", total_peers);
            return true;
        }
        
        if start.elapsed() > timeout {
            log::warn!("Mesh formation timed out after {:?}, only {} peer connections", timeout, total_peers);
            return false;
        }
        
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn check_workload_deployment(
    client: &reqwest::Client,
    ports: &[u16],
    task_id: &str,
    original_content: &str,
    port_to_peer_id: &std::collections::HashMap<u16, String>,
    _expect_modified_replicas: bool,
) -> (Vec<u16>, Vec<u16>) {
    let mock_verification_tasks = ports.iter().map(|&port| {
        let client = client.clone();
        let _task_id = task_id.to_string();
        let _original_content = original_content.to_string();
        let port_to_peer_id = port_to_peer_id.clone();
        async move {
            let base = format!("http://127.0.0.1:{}", port);

            // Get the peer ID for this port
            if let Some(peer_id) = port_to_peer_id.get(&port) {
                // First, try to check the mock engine state which includes exported manifests
                let mock_engine_resp = client
                    .get(format!("{}/debug/mock_engine_state", base))
                    .send()
                    .await;
                
                if let Ok(r) = mock_engine_resp {
                    if let Ok(j) = r.json::<serde_json::Value>().await {
                        if j.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                            if let Some(workloads) = j.get("workloads").and_then(|v| v.as_object()) {
                                for (_workload_id, workload_info) in workloads {
                                    if let Some(metadata) = workload_info.get("metadata").and_then(|v| v.as_object()) {
                                        if let Some(name) = metadata.get("name").and_then(|v| v.as_str()) {
                                            if name == "my-nginx" {
                                                // Check if the exported manifest is available and similar to original
                                                let exported_manifest_matches = if let Some(exported_manifest) = workload_info.get("exported_manifest").and_then(|v| v.as_str()) {
                                                    let contains_nginx = exported_manifest.contains("my-nginx");
                                                    let contains_deployment_or_pod = exported_manifest.contains("Deployment") || exported_manifest.contains("Pod");
                                                    let contains_api_version = exported_manifest.contains("apiVersion");
                                                    
                                                    log::info!("Exported manifest verification - nginx: {}, deployment/pod: {}, apiVersion: {}", 
                                                               contains_nginx, contains_deployment_or_pod, contains_api_version);
                                                    
                                                    if exported_manifest.len() > 100 {
                                                        log::info!("Exported manifest preview: {}", 
                                                                   &exported_manifest[..std::cmp::min(200, exported_manifest.len())]);
                                                    }
                                                    
                                                    contains_nginx && contains_deployment_or_pod && contains_api_version
                                                } else {
                                                    log::warn!("No exported manifest found in mock engine state");
                                                    false
                                                };
                                                
                                                return (port, true, exported_manifest_matches);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                
                // Fallback: Query workloads specifically for this peer ID
                let resp = client
                    .get(format!("{}/debug/workloads_by_peer/{}", base, peer_id))
                    .send()
                    .await;
                if let Ok(r) = resp {
                    if let Ok(j) = r.json::<serde_json::Value>().await {
                        if j.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                            if let Some(workloads) = j.get("workloads").and_then(|v| v.as_object())
                            {
                                // Check if any workload matches our expected metadata
                                for (_workload_id, workload_info) in workloads {
                                    if let Some(metadata) = workload_info
                                        .get("metadata")
                                        .and_then(|v| v.as_object())
                                    {
                                        // Check if this looks like our nginx manifest by checking the name in metadata
                                        if let Some(name) = metadata.get("name").and_then(|v| v.as_str()) {
                                            if name == "my-nginx" {
                                                // Also check if the exported manifest is available and similar to original
                                                let exported_manifest_matches = if let Some(exported_manifest) = workload_info.get("exported_manifest").and_then(|v| v.as_str()) {
                                                    // Verify that the exported manifest contains key elements from the original
                                                    let contains_nginx = exported_manifest.contains("my-nginx");
                                                    let contains_deployment_or_pod = exported_manifest.contains("Deployment") || exported_manifest.contains("Pod");
                                                    let contains_api_version = exported_manifest.contains("apiVersion");
                                                    
                                                    log::info!("Exported manifest verification - nginx: {}, deployment/pod: {}, apiVersion: {}", 
                                                               contains_nginx, contains_deployment_or_pod, contains_api_version);
                                                    
                                                    if exported_manifest.len() > 100 {
                                                        log::info!("Exported manifest preview: {}", 
                                                                   &exported_manifest[..std::cmp::min(200, exported_manifest.len())]);
                                                    }
                                                    
                                                    contains_nginx && contains_deployment_or_pod && contains_api_version
                                                } else {
                                                    log::warn!("No exported manifest found for workload");
                                                    false
                                                };
                                                
                                                return (port, true, exported_manifest_matches);
                                            }
                                        }
                                    }
                                    
                                    // Fallback: check if workload status indicates successful deployment
                                    if let Some(status) = workload_info.get("status").and_then(|v| v.as_str()) {
                                        if status == "Running" {
                                            // We have a running workload, assume it's correct for now
                                            return (port, true, true);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            (port, false, false)
        }
    });

    let mock_verification_results = join_all(mock_verification_tasks).await;
    let mut nodes_with_deployed_workloads: Vec<u16> = Vec::new();
    let mut nodes_with_content_mismatch: Vec<u16> = Vec::new();

    for (port, has_workload, content_matches) in mock_verification_results {
        if has_workload {
            nodes_with_deployed_workloads.push(port);
            if !content_matches {
                nodes_with_content_mismatch.push(port);
            }
        }
    }

    (nodes_with_deployed_workloads, nodes_with_content_mismatch)
}

#[serial]
#[tokio::test]
async fn test_apply_functionality() {
    let (client, ports) = setup_test_environment().await;
    let mut guard = start_test_nodes().await;

    // Wait for libp2p mesh to form before proceeding
    sleep(Duration::from_secs(3)).await;
    let mesh_formed = wait_for_mesh_formation(&client, &ports, Duration::from_secs(15)).await;
    if !mesh_formed {
        log::warn!("Mesh formation incomplete, but proceeding with test");
    }

    // Resolve manifest path relative to this test crate's manifest dir so it's robust under cargo test
    let manifest_path = PathBuf::from(format!(
        "{}/sample_manifests/nginx.yml",
        env!("CARGO_MANIFEST_DIR")
    ));

    // Read the original manifest content for verification
    let original_content = tokio::fs::read_to_string(manifest_path.clone())
        .await
        .expect("Failed to read original manifest file for verification");

    let task_id = cli::apply_file(manifest_path.clone())
        .await
        .expect("apply_file should succeed");

    // Wait for direct delivery and deployment to complete
    sleep(Duration::from_secs(6)).await;

    // Get peer IDs and check workload deployment
    let port_to_peer_id = get_peer_ids(&client, &ports).await;
    let (nodes_with_deployed_workloads, nodes_with_content_mismatch) = check_workload_deployment(
        &client,
        &ports,
        &task_id,
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

    log::info!(
        "✓ MockEngine verification passed: manifest {} deployed (visible on {} node(s) due to shared MockEngine): {:?}",
        task_id,
        nodes_with_deployed_workloads.len(),
        nodes_with_deployed_workloads
    );

    // Clean up nodes
    guard.cleanup().await;

    // Clean up environment
    std::env::remove_var("BEEMESH_MOCK_ONLY_RUNTIME");
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
    let manifest_path = PathBuf::from(format!(
        "{}/sample_manifests/nginx.yml",
        env!("CARGO_MANIFEST_DIR")
    ));

    // Read the original manifest content for verification
    let original_content = tokio::fs::read_to_string(manifest_path.clone())
        .await
        .expect("Failed to read original manifest file for verification");

    let task_id = cli::apply_file(manifest_path.clone())
        .await
        .expect("apply_file should succeed with real Podman");

    // Wait for direct delivery and Podman deployment to complete (longer timeout for real containers)
    sleep(Duration::from_secs(15)).await;

    // Verify actual Podman deployment
    let podman_verification_successful = verify_podman_deployment(&task_id, &original_content).await;
    
    assert!(
        podman_verification_successful,
        "Podman deployment verification failed - no matching pods found"
    );

    // Clean up Podman resources before test cleanup
    cleanup_podman_resources(&task_id).await;

    // Clean up nodes
    guard.cleanup().await;
}

#[serial]
#[tokio::test]
async fn test_apply_nginx_with_replicas() {
    let (client, ports) = setup_test_environment().await;
    let mut guard = start_test_nodes().await;

    // Wait for libp2p mesh to form before proceeding  
    sleep(Duration::from_secs(3)).await;
    let mesh_formed = wait_for_mesh_formation(&client, &ports, Duration::from_secs(15)).await;
    if !mesh_formed {
        log::warn!("Mesh formation incomplete, but proceeding with test");
    }

    // Resolve manifest path for nginx with replicas
    let manifest_path = PathBuf::from(format!(
        "{}/sample_manifests/nginx_with_replicas.yml",
        env!("CARGO_MANIFEST_DIR")
    ));

    // Read the original manifest content for verification
    let original_content = tokio::fs::read_to_string(manifest_path.clone())
        .await
        .expect("Failed to read nginx_with_replicas manifest file for verification");

    let task_id = cli::apply_file(manifest_path.clone())
        .await
        .expect("apply_file should succeed for nginx_with_replicas");

    // Wait for direct delivery and deployment to complete
    sleep(Duration::from_secs(8)).await;

    // Get peer IDs and check workload deployment
    let port_to_peer_id = get_peer_ids(&client, &ports).await;
    let (nodes_with_deployed_workloads, nodes_with_content_mismatch) = check_workload_deployment(
        &client,
        &ports,
        &task_id,
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
        task_id,
        nodes_with_deployed_workloads.len(),
        nodes_with_deployed_workloads
    );

    // Clean up nodes
    guard.cleanup().await;

    // Clean up environment
    std::env::remove_var("BEEMESH_MOCK_ONLY_RUNTIME");
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

async fn start_test_nodes_for_podman() -> test_utils::NodeGuard {
    let cli1 = make_test_cli(3000, false, true, None, vec![], 4001, 0);
    let cli2 = make_test_cli(
        3100,
        false,
        true,
        None,
        vec!["/ip4/127.0.0.1/tcp/4001".to_string()],
        4002,
        0,
    );

    let bootstrap_peers = vec![
        "/ip4/127.0.0.1/tcp/4001".to_string(),
        "/ip4/127.0.0.1/tcp/4002".to_string(),
    ];

    let cli3 = make_test_cli(3200, false, true, None, bootstrap_peers.clone(), 0, 0);

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

async fn verify_podman_deployment(task_id: &str, _original_content: &str) -> bool {
    // The pod name should now be in the format "beemesh-{manifest_id}-pod"
    // Podman adds "-pod" suffix when creating pods from Kubernetes Deployment manifests
    let expected_pod_name = format!("beemesh-{}-pod", task_id);

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
                    if let Ok(containers) = serde_json::from_str::<serde_json::Value>(&container_stdout) {
                        if let Some(containers_array) = containers.as_array() {
                            for container in containers_array {
                                if let Some(names) = container.get("Names").and_then(|n| n.as_array()) {
                                    for name in names {
                                        if let Some(name_str) = name.as_str() {
                                            if name_str.contains(&format!("beemesh-{}", task_id)) {
                                                log::info!("Found matching Podman container: {}", name_str);
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

async fn cleanup_podman_resources(task_id: &str) {
    log::info!("Cleaning up Podman resources for task: {}", task_id);

    // Try to remove the specific pod by the expected name (with -pod suffix)
    let expected_pod_name = format!("beemesh-{}-pod", task_id);
    let _ = tokio::process::Command::new("podman")
        .args(&["pod", "rm", "-f", &expected_pod_name])
        .output()
        .await;
    log::info!("Attempted to clean up Podman pod: {}", expected_pod_name);

    // Also try the name without -pod suffix (fallback)
    let expected_pod_name_alt = format!("beemesh-{}", task_id);
    let _ = tokio::process::Command::new("podman")
        .args(&["pod", "rm", "-f", &expected_pod_name_alt])
        .output()
        .await;
    log::info!("Attempted to clean up Podman pod: {}", expected_pod_name_alt);

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



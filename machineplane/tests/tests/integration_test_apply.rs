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

async fn check_workload_deployment(
    client: &reqwest::Client,
    ports: &[u16],
    task_id: &str,
    original_content: &str,
    port_to_peer_id: &std::collections::HashMap<u16, String>,
    expect_modified_replicas: bool,
) -> (Vec<u16>, Vec<u16>) {
    let mock_verification_tasks = ports.iter().map(|&port| {
        let client = client.clone();
        let _task_id = task_id.to_string();
        let original_content = original_content.to_string();
        let port_to_peer_id = port_to_peer_id.clone();
        async move {
            let base = format!("http://127.0.0.1:{}", port);

            // Get the peer ID for this port
            if let Some(peer_id) = port_to_peer_id.get(&port) {
                // Query workloads specifically for this peer ID
                let resp = client
                    .get(format!("{}/debug/workloads_by_peer/{}", base, peer_id))
                    .send()
                    .await;
                if let Ok(r) = resp {
                    if let Ok(j) = r.json::<serde_json::Value>().await {
                        if j.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                            if let Some(workloads) = j.get("workloads").and_then(|v| v.as_object())
                            {
                                // Check if any workload matches our expected content
                                for (_workload_id, workload_info) in workloads {
                                    if let Some(manifest_content) = workload_info
                                        .get("manifest_content")
                                        .and_then(|v| v.as_str())
                                    {
                                        if let Ok(actual_json) = serde_json::from_str::<serde_json::Value>(manifest_content) {
                                            // For replica distribution, check if this looks like our nginx manifest
                                            if let Some(metadata) = actual_json.get("metadata") {
                                                if let Some(name) = metadata.get("name").and_then(|v| v.as_str()) {
                                                    if name == "my-nginx" {
                                                        // Verify it has the expected replicas count
                                                        if expect_modified_replicas {
                                                            // Should have replicas=1 for distributed scenarios
                                                            if let Some(spec) = actual_json.get("spec") {
                                                                if let Some(replicas) = spec.get("replicas").and_then(|v| v.as_u64()) {
                                                                    if replicas == 1 {
                                                                        return (port, true, true);
                                                                    } else {
                                                                        return (port, true, false); // wrong replica count
                                                                    }
                                                                }
                                                            }
                                                        } else {
                                                            // For single replica test, match exactly with original
                                                            let expected_json: serde_json::Value =
                                                                serde_yaml::from_str(&original_content)
                                                                .unwrap_or_else(|_| serde_json::json!({"raw": original_content}));
                                                            if expected_json == actual_json {
                                                                return (port, true, true);
                                                            } else {
                                                                return (port, true, false);
                                                            }
                                                        }
                                                    }
                                                }
                                            }
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

    sleep(Duration::from_secs(3)).await;

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

    sleep(Duration::from_secs(3)).await;

    // Check which nodes have the assigned task using debug endpoints first
    let task_assignment_tasks = ports.iter().map(|&port| {
        let client = client.clone();
        let task_id = task_id.clone();
        async move {
            let base = format!("http://127.0.0.1:{}", port);
            let mut has_task = false;

            // Check if this node has the task assigned
            let resp = client.get(format!("{}/debug/tasks", base)).send().await;
            if let Ok(r) = resp {
                if let Ok(j) = r.json::<serde_json::Value>().await {
                    if j.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                        if let Some(tasks) = j.get("tasks").and_then(|v| v.as_object()) {
                            if tasks.contains_key(&task_id) {
                                has_task = true;
                            }
                        }
                    }
                }
            }

            (port, has_task)
        }
    });

    // Execute all task assignment checks in parallel
    let assignment_results = join_all(task_assignment_tasks).await;
    let mut nodes_with_assigned_tasks: Vec<u16> = Vec::new();

    for (port, has_task) in assignment_results {
        if has_task {
            nodes_with_assigned_tasks.push(port);
        }
    }

    // Verify at least one node got the task assignment
    assert!(
        !nodes_with_assigned_tasks.is_empty(),
        "No nodes have the assigned task - direct delivery failed"
    );

    // Wait a bit longer for deployment to complete
    sleep(Duration::from_secs(3)).await;

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
async fn test_apply_nginx_with_replicas() {
    let (client, ports) = setup_test_environment().await;
    let mut guard = start_test_nodes().await;

    sleep(Duration::from_secs(3)).await;

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

    sleep(Duration::from_secs(5)).await;

    // Check which nodes have the assigned task using debug endpoints first
    let task_assignment_tasks = ports.iter().map(|&port| {
        let client = client.clone();
        let task_id = task_id.clone();
        async move {
            let base = format!("http://127.0.0.1:{}", port);
            let mut has_task = false;

            // Check if this node has the task assigned
            let resp = client.get(format!("{}/debug/tasks", base)).send().await;
            if let Ok(r) = resp {
                if let Ok(j) = r.json::<serde_json::Value>().await {
                    if j.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                        if let Some(tasks) = j.get("tasks").and_then(|v| v.as_object()) {
                            if tasks.contains_key(&task_id) {
                                has_task = true;
                            }
                        }
                    }
                }
            }

            (port, has_task)
        }
    });

    // Execute all task assignment checks in parallel
    let assignment_results = join_all(task_assignment_tasks).await;
    let mut nodes_with_assigned_tasks: Vec<u16> = Vec::new();

    for (port, has_task) in assignment_results {
        if has_task {
            nodes_with_assigned_tasks.push(port);
        }
    }

    // Verify at least one node got the task assignment
    assert!(
        !nodes_with_assigned_tasks.is_empty(),
        "No nodes have the assigned task - direct delivery failed"
    );

    // Wait a bit longer for deployment to complete
    sleep(Duration::from_secs(3)).await;

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

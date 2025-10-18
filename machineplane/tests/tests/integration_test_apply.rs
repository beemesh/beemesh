use env_logger::Env;
use futures::future::join_all;
use serial_test::serial;

use std::path::PathBuf;
use std::time::Duration;
use tokio::time::sleep;

mod test_utils;
use test_utils::{make_test_cli, setup_cleanup_hook, start_nodes};

#[serial]
#[tokio::test]
async fn test_apply_functionality() {
    // Setup cleanup hook and initialize logger
    setup_cleanup_hook();
    let _ = env_logger::Builder::from_env(Env::default().default_filter_or("warn")).try_init();

    // Set environment variable to use mock-only runtime
    std::env::set_var("BEEMESH_MOCK_ONLY_RUNTIME", "1");

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
    let mut guard = start_nodes(vec![cli1, cli2, cli3], Duration::from_secs(1)).await;

    sleep(Duration::from_secs(5)).await;

    let client = reqwest::Client::new();
    let ports = vec![3000u16, 3100u16, 3200u16];

    // Resolve manifest path relative to this test crate's manifest dir so it's robust under cargo test
    let manifest_path = PathBuf::from(format!(
        "{}/sample_manifests/nginx",
        env!("CARGO_MANIFEST_DIR")
    ));

    // Read the original manifest content for verification
    let original_content = tokio::fs::read_to_string(manifest_path.clone())
        .await
        .expect("Failed to read original manifest file for verification");

    let _original_manifest_json: serde_json::Value = match serde_yaml::from_str(&original_content) {
        Ok(v) => v,
        Err(_) => serde_json::json!({ "raw": original_content }),
    };

    let task_id = cli::apply_file(manifest_path.clone())
        .await
        .expect("apply_file should succeed");

    sleep(Duration::from_secs(5)).await;

    // Check which nodes have the assigned task using debug endpoints first
    // (we still need this to verify the system is working before checking MockEngine)
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
    sleep(Duration::from_secs(5)).await;

    // First, get the local peer ID for each node
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

    // Now check workloads using peer ID filtering
    let mock_verification_tasks = ports.iter().map(|&port| {
        let client = client.clone();
        let task_id = task_id.clone();
        let original_content = original_content.clone();
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
                            if let Some(workloads) = j.get("workloads").and_then(|v| v.as_object()) {
                                // Check if any workload has our task_id as manifest_id
                                for (_workload_id, workload_info) in workloads {
                                    if let Some(manifest_id) =
                                        workload_info.get("manifest_id").and_then(|v| v.as_str())
                                    {
                                        if manifest_id == task_id {
                                            // Verify the manifest content if available
                                            if let Some(manifest_content) = workload_info
                                                .get("manifest_content")
                                                .and_then(|v| v.as_str())
                                            {
                                                // Parse both to JSON for comparison
                                                let expected_json: serde_json::Value =
                                                    serde_yaml::from_str(&original_content)
                                                    .unwrap_or_else(|_| serde_json::json!({"raw": original_content}));
                                                let actual_json: serde_json::Value =
                                                    serde_json::from_str(manifest_content)
                                                    .unwrap_or_else(|_| serde_json::json!({"raw": manifest_content}));
                                                let content_matches = expected_json == actual_json;
                                                return (port, true, content_matches);
                                            } else {
                                                return (port, true, false); // has workload but no content to verify
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

    // With peer ID filtering, we can now properly verify that only the intended node has the workload
    // Even though the runtime registry is global, the peer ID metadata allows us to distinguish
    // which specific node actually processed the deployment
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

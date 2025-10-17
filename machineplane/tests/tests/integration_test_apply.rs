use env_logger::Env;
use futures::future::join_all;
use serial_test::serial;

use std::path::PathBuf;
use std::time::Duration;
use tokio::time::sleep;

mod test_utils;
use test_utils::{make_test_cli, setup_cleanup_hook, start_nodes_as_processes};

#[serial]
#[tokio::test]
async fn test_apply_functionality() {
    // Setup cleanup hook and initialize logger
    setup_cleanup_hook();
    let _ = env_logger::Builder::from_env(Env::default().default_filter_or("warn")).try_init();

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

    let mut guard = start_nodes_as_processes(vec![cli1, cli2, cli3], Duration::from_secs(1)).await;

    println!("Waiting for node discovery...");
    sleep(Duration::from_secs(10)).await;

    let client = reqwest::Client::new();
    let ports = vec![3000u16, 3100u16, 3200u16];
    let mut peer_counts = Vec::new();
    for port in &ports {
        let resp = client
            .get(&format!("http://127.0.0.1:{}/debug/peers", port))
            .send()
            .await;
        if let Ok(r) = resp {
            if let Ok(text) = r.text().await {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                    let peer_count = json
                        .get("peers")
                        .and_then(|p| p.as_array())
                        .map(|a| a.len())
                        .unwrap_or(0);
                    peer_counts.push(peer_count);
                } else {
                    peer_counts.push(0);
                }
            }
        }
    }
    println!(
        "Peer discovery: {} peers each for nodes {}",
        peer_counts
            .iter()
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join("/"),
        ports
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join("/")
    );

    // Call the apply function
    // Resolve manifest path relative to this test crate's manifest dir so it's robust under cargo test
    let manifest_path = PathBuf::from(format!(
        "{}/sample_manifests/nginx",
        env!("CARGO_MANIFEST_DIR")
    ));
    println!(
        "About to call cli::apply_file with path: {:?}",
        manifest_path
    );
    let task_id = match cli::apply_file(manifest_path.clone()).await {
        Ok(id) => {
            println!("apply_file succeeded with task_id: {}", id);
            id
        }
        Err(e) => {
            println!("apply_file failed with error: {:?}", e);
            panic!("apply failed: {}", e);
        }
    };

    // Wait longer for keyshare distribution and DHT activity in local test environment
    sleep(Duration::from_secs(15)).await;

    // Check for successful task assignment (simplified approach)
    let client = reqwest::Client::new();

    println!("Checking nodes for successful task assignment...");

    // Wait for task assignment to propagate
    sleep(Duration::from_secs(5)).await;

    // Check which nodes have the assigned task
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
                                println!("  Node {} has assigned task {}", port, task_id);
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

    println!(
        "  Nodes with assigned tasks: {:?}",
        nodes_with_assigned_tasks
    );

    // Verify at least one node got the task assignment
    assert!(
        !nodes_with_assigned_tasks.is_empty(),
        "No nodes have the assigned task - direct delivery failed"
    );

    println!("✓ Task assignment verification passed:");
    println!(
        "  - Task {} successfully assigned to {} node(s): {:?}",
        task_id,
        nodes_with_assigned_tasks.len(),
        nodes_with_assigned_tasks
    );

    // Verify successful task execution (simplified for direct delivery)
    println!("Checking for successful task execution...");

    // For direct delivery, we just need to confirm the winning node received and processed the manifest
    if !nodes_with_assigned_tasks.is_empty() {
        let winning_node_port = nodes_with_assigned_tasks[0];
        println!(
            "✓ Direct manifest delivery successful to winning node on port {}",
            winning_node_port
        );

        // 1) Check the node's tasks endpoint to ensure the task exists (keeps previous behavior)
        let resp_tasks = client
            .get(&format!(
                "http://127.0.0.1:{}/debug/tasks",
                winning_node_port
            ))
            .send()
            .await;

        if let Ok(r) = resp_tasks {
            if let Ok(j) = r.json::<serde_json::Value>().await {
                if j.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                    if let Some(tasks) = j.get("tasks").and_then(|v| v.as_object()) {
                        if tasks.contains_key(&task_id) {
                            println!("✓ Winning node successfully received and processed the task");
                        }
                    }
                }
            }
        }

        // 2) Verify the decrypted manifest is stored by the node and matches the original input manifest.
        // Read original manifest file contents and parse similarly to the CLI (YAML -> JSON or raw wrapper).
        let original_content = tokio::fs::read_to_string(manifest_path.clone())
            .await
            .expect("Failed to read original manifest file for verification");

        let original_manifest_json: serde_json::Value =
            match serde_yaml::from_str(&original_content) {
                Ok(v) => v,
                Err(_) => serde_json::json!({ "raw": original_content }),
            };

        // Query the debug endpoint that exposes decrypted manifests collected by the node
        let resp = client
            .get(&format!(
                "http://127.0.0.1:{}/debug/decrypted_manifests",
                winning_node_port
            ))
            .send()
            .await
            .expect("Failed to query decrypted_manifests endpoint");

        let body_json = resp
            .json::<serde_json::Value>()
            .await
            .expect("Failed to parse decrypted_manifests response");

        // Basic response validation
        assert_eq!(
            body_json.get("ok").and_then(|v| v.as_bool()),
            Some(true),
            "decrypted_manifests endpoint returned error"
        );

        // Extract decrypted manifests map
        let decrypted_map = body_json
            .get("decrypted_manifests")
            .and_then(|v| v.as_object())
            .expect("decrypted_manifests missing or not an object");

        // Verify that the manifest_id returned by apply_file is present in the decrypted map
        if let Some(decrypted_value) = decrypted_map.get(&task_id) {
            assert_eq!(
                decrypted_value, &original_manifest_json,
                "Decrypted manifest on node does not match the original manifest content"
            );
            println!("✓ Decrypted manifest on winning node matches the original manifest");
        } else {
            panic!(
                "No decrypted manifest stored for manifest_id {} on winning node {}",
                task_id, winning_node_port
            );
        }
    }

    println!("✅ Direct manifest delivery test completed successfully!");
    println!("Summary:");
    println!(
        "  - Task {} was assigned to {} node(s)",
        task_id,
        nodes_with_assigned_tasks.len()
    );
    println!("  - Decrypted manifest verified on winning node");

    // Clean up nodes
    guard.cleanup().await;
}

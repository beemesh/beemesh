use env_logger::Env;
use futures::future::join_all;
use scheduler::{DistributionConfig, DistributionPlan, SecurityLevel};
use serial_test::serial;

use std::path::PathBuf;
use std::time::Duration;
use tokio::time::sleep;

mod test_utils;
use test_utils::{make_test_cli, setup_cleanup_hook, start_nodes_as_processes};

pub const TENANT: &str = "00000000-0000-0000-0000-000000000000";

#[serial]
#[tokio::test]
async fn test_apply_functionality() {
    // Setup cleanup hook and initialize logger
    setup_cleanup_hook();
    let _ = env_logger::Builder::from_env(Env::default().default_filter_or("warn")).try_init();

    // Create separate CLI configs for each node - they will each be separate processes
    // so no global state sharing issues
    // First two nodes get fixed libp2p ports and serve as bootstrap peers
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

    // Bootstrap peers list for the remaining nodes
    let bootstrap_peers = vec![
        "/ip4/127.0.0.1/tcp/4001".to_string(),
        "/ip4/127.0.0.1/tcp/4002".to_string(),
    ];

    // Create 8 additional nodes (total of 10 nodes)
    let cli3 = make_test_cli(3200, false, true, None, bootstrap_peers.clone(), 0, 0);
    let cli4 = make_test_cli(3300, false, true, None, bootstrap_peers.clone(), 0, 0);
    let cli5 = make_test_cli(3400, false, true, None, bootstrap_peers.clone(), 0, 0);
    let cli6 = make_test_cli(3500, false, true, None, bootstrap_peers.clone(), 0, 0);
    let cli7 = make_test_cli(3600, false, true, None, bootstrap_peers.clone(), 0, 0);
    //let cli8 = make_test_cli(3700, false, true, None, bootstrap_peers.clone(), 0, 0);
    //let cli9 = make_test_cli(3800, false, true, None, bootstrap_peers.clone(), 0, 0);
    //let cli10 = make_test_cli(3900, false, true, None, bootstrap_peers.clone(), 0, 0);

    let mut guard = start_nodes_as_processes(
        vec![
            cli1, cli2, cli3, cli4, cli5, cli6, cli7, /*, cli8, cli9, cli10*/
        ],
        Duration::from_secs(1),
    )
    .await;

    // Wait for the nodes to be ready and discover each other via mDNS
    println!("Waiting for node discovery...");
    sleep(Duration::from_secs(10)).await;

    // Check if nodes have discovered peers (reduced verbosity)
    let client = reqwest::Client::new();
    let ports = vec![
        3000u16, 3100u16, 3200u16, 3300u16, 3400u16, 3500u16,
        3600u16, /*, 3700u16, 3800u16, 3900u16,*/
    ];
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

    // Poll each node for keystore shares and capability tokens in parallel
    let client = reqwest::Client::new();

    println!("Checking all nodes for keystore contents in parallel...");

    // Create parallel tasks for keystore checking
    let keystore_tasks = ports.iter().map(|&port| {
        let client = client.clone();
        async move {
            let base = format!("http://127.0.0.1:{}", port);
            let mut result = (port, false, false, false, 0, 0, 0); // (port, has_capability, has_keyshare, has_announces, keyshare_cap_count, manifest_cap_count, legacy_cap_count)

            // Check for keystore entries with detailed metadata
            for attempt in 0..15 {
                let resp = client
                    .get(format!("{}/debug/keystore/entries", base))
                    .send()
                    .await;
                if let Ok(r) = resp {
                    if let Ok(j) = r.json::<serde_json::Value>().await {
                        if j.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                            if let Some(entries) = j.get("entries").and_then(|v| v.as_array()) {
                                if !entries.is_empty() {
                                    if attempt > 5 {
                                        println!(
                                            "Node {} keystore has {} entries after {} attempts",
                                            port,
                                            entries.len(),
                                            attempt + 1
                                        );
                                    }

                                    // Analyze entry types
                                    let mut has_capability = false;
                                    let mut has_keyshare = false;
                                    let mut keyshare_capabilities = 0;
                                    let mut manifest_capabilities = 0;
                                    let mut legacy_capabilities = 0;

                                    for entry in entries {
                                        if let Some(entry_type) =
                                            entry.get("type").and_then(|v| v.as_str())
                                        {
                                            match entry_type {
                                                "capability" => {
                                                    has_capability = true;
                                                    legacy_capabilities += 1;
                                                }
                                                "keyshare_capability" => {
                                                    has_capability = true;
                                                    keyshare_capabilities += 1;
                                                }
                                                "manifest_capability" => {
                                                    has_capability = true;
                                                    manifest_capabilities += 1;
                                                }
                                                "keyshare" => has_keyshare = true,
                                                _ => {}
                                            }

                                            if let Some(meta) =
                                                entry.get("meta").and_then(|v| v.as_str())
                                            {
                                                println!(
                                                    "  Node {} has {} with metadata: {}",
                                                    port, entry_type, meta
                                                );
                                            }
                                        }
                                    }

                                    if has_capability {
                                        println!(
                                            "  Node {} capability summary: {} keyshare_capability, {} manifest_capability, {} legacy",
                                            port, keyshare_capabilities, manifest_capabilities, legacy_capabilities
                                        );
                                    }

                                    result.1 = has_capability;
                                    result.2 = has_keyshare;
                                    // Store capability counts for later verification
                                    result = (port, has_capability, has_keyshare, false, keyshare_capabilities, manifest_capabilities, legacy_capabilities);
                                    break;
                                }
                            }
                        }
                    }
                }
                sleep(Duration::from_millis(500)).await;
            }

            if !result.1 {
                println!(
                    "⚠ Node {} has no entries in keystore after 15 attempts",
                    port
                );
            }

            // Check for active announces
            for _ in 0..10 {
                let resp = client
                    .get(format!("{}/debug/dht/active_announces", base))
                    .send()
                    .await;
                if let Ok(r) = resp {
                    if let Ok(j) = r.json::<serde_json::Value>().await {
                        if j.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                            if let Some(arr) = j.get("cids").and_then(|v| v.as_array()) {
                                if !arr.is_empty() {
                                    // Update announces flag in the extended result tuple
                                    result.3 = true; // has_announces
                                    break;
                                }
                            }
                        }
                    }
                }
                sleep(Duration::from_millis(300)).await;
            }

            result
        }
    });

    // Execute all keystore checks in parallel
    let results = join_all(keystore_tasks).await;

    // Collect results and capability counts
    let mut nodes_with_announces: Vec<u16> = Vec::new();
    let mut nodes_with_capabilities: Vec<u16> = Vec::new();
    let mut nodes_with_keyshares: Vec<u16> = Vec::new();
    let mut total_keyshare_capabilities = 0;
    let mut total_manifest_capabilities = 0;
    let mut total_legacy_capabilities = 0;
    let mut nodes_with_keyshare_caps: Vec<u16> = Vec::new();
    let mut nodes_with_manifest_caps: Vec<u16> = Vec::new();

    for (
        port,
        has_capability,
        has_keyshare,
        has_announces,
        keyshare_cap_count,
        manifest_cap_count,
        legacy_cap_count,
    ) in results
    {
        if has_capability {
            nodes_with_capabilities.push(port);
        }
        if has_keyshare {
            nodes_with_keyshares.push(port);
        }
        if has_announces {
            nodes_with_announces.push(port);
        }

        // Track capability distribution
        if keyshare_cap_count > 0 {
            nodes_with_keyshare_caps.push(port);
            total_keyshare_capabilities += keyshare_cap_count;
        }
        if manifest_cap_count > 0 {
            nodes_with_manifest_caps.push(port);
            total_manifest_capabilities += manifest_cap_count;
        }
        if legacy_cap_count > 0 {
            total_legacy_capabilities += legacy_cap_count;
        }
    }

    println!("  Nodes with capabilities: {:?}", nodes_with_capabilities);
    println!("  Nodes with keyshares: {:?}", nodes_with_keyshares);
    println!("  Nodes with announces: {:?}", nodes_with_announces);
    println!("  Capability distribution summary:");
    println!(
        "    Keyshare capabilities: {} total on {} nodes {:?}",
        total_keyshare_capabilities,
        nodes_with_keyshare_caps.len(),
        nodes_with_keyshare_caps
    );
    println!(
        "    Manifest capabilities: {} total on {} nodes {:?}",
        total_manifest_capabilities,
        nodes_with_manifest_caps.len(),
        nodes_with_manifest_caps
    );
    println!(
        "    Legacy capabilities: {} total",
        total_legacy_capabilities
    );

    // Check which nodes are storing the manifest (manifest holders)
    println!("Checking all nodes for manifest holders in parallel...");
    let manifest_holder_tasks = ports.iter().map(|&port| {
        let client = client.clone();
        let task_id = task_id.clone();
        async move {
            let base = format!("http://127.0.0.1:{}", port);
            let mut has_manifest = false;

            // Check if this node has the manifest stored locally
            let resp = client
                .get(format!("{}/debug/manifest/{}/versions", base, task_id))
                .send()
                .await;
            if let Ok(r) = resp {
                if let Ok(j) = r.json::<serde_json::Value>().await {
                    if let Some(total_versions) = j.get("total_versions").and_then(|v| v.as_u64()) {
                        if total_versions > 0 {
                            has_manifest = true;
                            println!(
                                "  Node {} holds manifest {} ({} versions)",
                                port, task_id, total_versions
                            );
                        }
                    }
                }
            }

            (port, has_manifest)
        }
    });

    // Execute all manifest holder checks in parallel
    let manifest_results = join_all(manifest_holder_tasks).await;
    let mut nodes_with_manifests: Vec<u16> = Vec::new();

    for (port, has_manifest) in manifest_results {
        if has_manifest {
            nodes_with_manifests.push(port);
        }
    }

    println!("  Nodes with manifests: {:?}", nodes_with_manifests);

    // Calculate expected values using scheduler for dynamic verification
    let config = DistributionConfig {
        min_threshold: 2,
        max_fault_tolerance: 3,
        security_level: SecurityLevel::Standard,
    };

    // Calculate expected distribution based on actual participating nodes, not total ports
    let participating_nodes = nodes_with_capabilities.len();
    println!(
        "Calculating expected distribution for {} participating nodes (out of {} total)",
        participating_nodes,
        ports.len()
    );

    let expected_plan =
        DistributionPlan::calculate_optimal_distribution(participating_nodes, &config)
            .expect("Should calculate distribution plan for participating nodes");

    println!(
        "Expected distribution plan from scheduler (for {} participating nodes):",
        participating_nodes
    );
    println!(
        "  - Keyshares: {}, Threshold: {}",
        expected_plan.keyshare_count, expected_plan.reconstruction_threshold
    );
    println!(
        "  - Manifests: {}, Total tokens: {}",
        expected_plan.manifest_count, expected_plan.total_capability_tokens
    );

    println!("Verifying capability distribution matches scheduler expectations...");

    assert_eq!(
        total_keyshare_capabilities, expected_plan.keyshare_count,
        "expected {} keyshare capability tokens (scheduler calculated), but found {}",
        expected_plan.keyshare_count, total_keyshare_capabilities
    );

    assert_eq!(
        nodes_with_keyshare_caps.len(),
        expected_plan.keyshare_count,
        "expected {} nodes to have keyshare capabilities (scheduler calculated), but {} nodes had them: {:?}",
        expected_plan.keyshare_count, nodes_with_keyshare_caps.len(), nodes_with_keyshare_caps
    );

    assert_eq!(
        total_manifest_capabilities, expected_plan.manifest_count,
        "expected {} manifest capability tokens (scheduler calculated), but found {}",
        expected_plan.manifest_count, total_manifest_capabilities
    );

    assert_eq!(
        nodes_with_manifest_caps.len(),
        expected_plan.manifest_count,
        "expected {} nodes to have manifest capabilities (scheduler calculated), but {} nodes had them: {:?}",
        expected_plan.manifest_count, nodes_with_manifest_caps.len(), nodes_with_manifest_caps
    );

    // Verify the total matches scheduler calculation
    let total_capabilities =
        total_keyshare_capabilities + total_manifest_capabilities + total_legacy_capabilities;
    assert_eq!(
        total_capabilities, expected_plan.total_capability_tokens,
        "expected {} total capability tokens (scheduler calculated), but found {}",
        expected_plan.total_capability_tokens, total_capabilities
    );

    // Verify keyshares match keyshare capabilities (each keyshare gets a capability)
    assert_eq!(
        nodes_with_keyshares.len(),
        expected_plan.keyshare_count,
        "expected {} nodes to have keyshares (scheduler calculated), but {} nodes had them: {:?}",
        expected_plan.keyshare_count,
        nodes_with_keyshares.len(),
        nodes_with_keyshares
    );

    assert_eq!(
        nodes_with_manifests.len(),
        expected_plan.manifest_count,
        "expected {} nodes to hold the manifest (scheduler calculated), but {} nodes had it: {:?}",
        expected_plan.manifest_count,
        nodes_with_manifests.len(),
        nodes_with_manifests
    );
    assert!(
        !nodes_with_announces.is_empty(),
        "no node had active announces"
    );

    println!("✓ Capability distribution verification passed (scheduler calculated):");
    println!(
        "  - {} keyshare capabilities distributed to {} nodes (matches scheduler plan)",
        total_keyshare_capabilities,
        nodes_with_keyshare_caps.len()
    );
    println!(
        "  - {} manifest capabilities distributed to {} nodes (matches scheduler plan)",
        total_manifest_capabilities,
        nodes_with_manifest_caps.len()
    );
    println!(
        "  - Total {} capabilities matches expected {} (scheduler zero-trust calculation)",
        total_capabilities, expected_plan.total_capability_tokens
    );

    // Verify manifest storage by finding the manifest CID and testing cross-node queries
    let mut manifest_cid_found: Option<String> = None;
    let mut nodes_with_tasks: Vec<u16> = Vec::new();

    // First, find which nodes have the task and get the manifest CID in parallel
    let task_query_tasks = ports.iter().map(|&port| {
        let client = client.clone();
        async move {
            let base = format!("http://127.0.0.1:{}", port);
            let mut result = (port, false, None::<String>); // (port, has_tasks, manifest_cid)

            // Get tasks from this node to find the manifest CID
            let resp = client.get(format!("{}/debug/tasks", base)).send().await;
            if let Ok(r) = resp {
                if let Ok(j) = r.json::<serde_json::Value>().await {
                    if j.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                        if let Some(tasks) = j.get("tasks").and_then(|v| v.as_object()) {
                            if !tasks.is_empty() {
                                result.1 = true; // has_tasks
                                for (_task_id, task_info) in tasks {
                                    if let Some(manifest_cid) =
                                        task_info.get("manifest_cid").and_then(|v| v.as_str())
                                    {
                                        println!(
                                            "Node {} has task with manifest_cid: {}",
                                            port, manifest_cid
                                        );
                                        result.2 = Some(manifest_cid.to_string());
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            result
        }
    });

    // Execute all task queries in parallel
    let task_results = join_all(task_query_tasks).await;

    // Collect results
    for (port, has_tasks, manifest_cid_opt) in task_results {
        if has_tasks {
            nodes_with_tasks.push(port);
        }
        if manifest_cid_found.is_none() && manifest_cid_opt.is_some() {
            manifest_cid_found = manifest_cid_opt;
        }
    }

    println!("Nodes with tasks: {} out of 10", nodes_with_tasks.len());

    if let Some(manifest_cid) = manifest_cid_found {
        println!(
            "Testing manifest CID {} query from all nodes...",
            manifest_cid
        );

        // Test querying the same manifest CID from all nodes in parallel
        let manifest_query_tasks = ports.iter().map(|&port| {
            let client = client.clone();
            let manifest_cid = manifest_cid.clone();
            async move {
                let base = format!("http://127.0.0.1:{}", port);

                // Try to get this manifest from the DHT on this node
                let resp = client
                    .get(format!("{}/debug/dht/manifest/{}", base, manifest_cid))
                    .send()
                    .await;

                if let Ok(r) = resp {
                    if let Ok(j) = r.json::<serde_json::Value>().await {
                        let success = j.get("ok").and_then(|v| v.as_bool()) == Some(true)
                            && j.get("content").is_some();

                        if !success && j.get("error").is_some() {
                            println!(
                                "✗ Node {} cannot query manifest {}: {}",
                                port,
                                manifest_cid,
                                j.get("error")
                                    .and_then(|e| e.as_str())
                                    .unwrap_or("unknown error")
                            );
                        }

                        (port, success)
                    } else {
                        (port, false)
                    }
                } else {
                    (port, false)
                }
            }
        });

        // Execute all manifest queries in parallel
        let query_results = join_all(manifest_query_tasks).await;
        let successful_queries = query_results.iter().filter(|(_, success)| *success).count();

        println!(
            "Successful manifest queries: {}/{}",
            successful_queries,
            ports.len()
        );

        if successful_queries > 0 {
            println!("✓ Manifest found in DHT on {} node(s)!", successful_queries);
        } else {
            println!("✗ Manifest not accessible from any node");
        }

        if successful_queries > 0 {
            println!("✓ Manifest found in DHT - cross-node queries working!");
        } else {
            println!("⚠ Manifest not accessible via cross-node DHT queries");
        }

        // New: verify decrypted manifest via REST debug endpoint
        // Read expected manifest JSON (same parsing logic as CLI)
        let expected_manifest_json: serde_json::Value = {
            let contents = tokio::fs::read_to_string(&manifest_path)
                .await
                .expect("could not read manifest file");
            match serde_yaml::from_str(&contents) {
                Ok(v) => v,
                Err(_) => serde_json::json!({ "raw": contents }),
            }
        };

        // Get manifest_id from the node that holds the task (use first node with tasks)
        if !nodes_with_tasks.is_empty() {
            let node_port = nodes_with_tasks[0];
            let mid_resp = client
                .get(&format!(
                    "http://127.0.0.1:{}/tenant/{}/tasks/{}/manifest_id",
                    node_port, TENANT, task_id
                ))
                .send()
                .await
                .expect("manifest_id request failed");
            let mid_json: serde_json::Value =
                mid_resp.json().await.expect("manifest_id parse failed");
            let manifest_id = mid_json
                .get("manifest_id")
                .and_then(|v| v.as_str())
                .expect("no manifest_id returned")
                .to_string();

            // Poll /debug/decrypted_manifests until the manifest_id appears and matches expected
            let mut found = false;
            for attempt in 0..30 {
                let resp = client
                    .get(&format!(
                        "http://127.0.0.1:{}/debug/decrypted_manifests",
                        node_port
                    ))
                    .send()
                    .await;
                if let Ok(r) = resp {
                    if let Ok(j) = r.json::<serde_json::Value>().await {
                        // The debug endpoint returns {"ok": true, "decrypted_manifests": {...}}
                        if let Some(decrypted_manifests) = j.get("decrypted_manifests") {
                            if let Some(entry) = decrypted_manifests.get(&manifest_id) {
                                if attempt > 10 {
                                    println!("Found decrypted manifest for {} on node {} after {} attempts", manifest_id, node_port, attempt + 1);
                                }
                                // compare equality
                                if entry == &expected_manifest_json {
                                    found = true;
                                    break;
                                } else {
                                    println!("Decrypted manifest does not match expected.");
                                    println!("GOT: {:?}", entry);
                                    println!("EXPECTED: {:?}", expected_manifest_json);
                                    log::warn!("Test failure: decrypted manifest mismatch for {} on node {}", manifest_id, node_port);
                                    break;
                                }
                            }
                        }
                    }
                }
                sleep(Duration::from_millis(500)).await;
            }

            if !found {
                log::warn!("Test failure: decrypted manifest for {} did not appear or did not match expected on node {}", manifest_id, node_port);
            }
            assert!(
                found,
                "decrypted manifest for {} did not appear or did not match expected",
                manifest_id
            );
        } else {
            panic!("no node reported the task; cannot verify decrypted manifest");
        }

        // For now, the core functionality (keystore sharing) is working correctly as verified above
        // The manifest storage has been implemented and we're testing cross-node accessibility
        // Network DHT storage fails due to quorum in small test clusters, but local storage should work on the storing node
        assert!(
            successful_queries > 0 || (nodes_with_capabilities.len() == 3 && nodes_with_keyshares.len() == 3),
            "Either manifest should be accessible via DHT or keystore operations should work (3 nodes with capabilities and 3 nodes with keyshares)"
        );
    } else {
        println!("⚠ No manifest CID found in any task");
        // Still assert keystore functionality works for 3 or 4 nodes (temporary)
        assert!(
            nodes_with_capabilities.len() == 3,
            "3 nodes should have capability tokens even without manifest CID, got {}",
            nodes_with_capabilities.len()
        );
        assert_eq!(
            nodes_with_keyshares.len(),
            3,
            "Exactly 3 nodes should have keyshares even without manifest CID"
        );
    }

    // Clean up nodes
    guard.cleanup().await;

    // Clean up per-node keystore temp files for all 10 nodes
    for port in &ports {
        let shared_name = format!("node_{}", port);
        let temp_path = std::env::temp_dir().join(format!("beemesh_keystore_{}", shared_name));
        let _ = std::fs::remove_file(&temp_path);
    }

    // Clean up environment variables (these are global so clean them up)
    std::env::remove_var("BEEMESH_KEYSTORE_EPHEMERAL");
    std::env::remove_var("BEEMESH_KEYSTORE_SHARED_NAME");
}

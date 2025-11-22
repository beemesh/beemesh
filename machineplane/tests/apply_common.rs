//! Common helper functions for "Apply" workflow tests.
//!
//! This module provides shared utilities for setting up test environments,
//! starting fabric nodes, and verifying workload deployments.

use env_logger::Env;
use futures::future::join_all;
use std::collections::HashMap as StdHashMap;
use std::time::Duration;
use tokio::time::{Instant, sleep};

#[path = "runtime_helpers.rs"]
mod runtime_helpers;
use runtime_helpers::{make_test_daemon, start_nodes};
use tokio::task::JoinHandle;

pub const TEST_PORTS: [u16; 3] = [3000u16, 3100u16, 3200u16];

/// Prepare logging and environment for runtime tests.
pub async fn setup_test_environment() -> (reqwest::Client, Vec<u16>) {
    let _ = env_logger::Builder::from_env(Env::default().default_filter_or("warn")).try_init();

    let client = reqwest::Client::new();
    (client, TEST_PORTS.to_vec())
}

/// Build standard three-node fabric configuration.
/// Returns JoinHandles for the spawned tasks so tests can abort them when finished.
pub async fn start_fabric_nodes() -> Vec<JoinHandle<()>> {
    // Start a single bootstrap node first to give it time to initialize.
    let bootstrap_daemon = make_test_daemon(3000, vec![], 4001);
    let mut handles = start_nodes(vec![bootstrap_daemon], Duration::from_secs(1)).await;

    // Allow the bootstrap node to settle before connecting peers.
    sleep(Duration::from_secs(2)).await;

    // Start additional nodes that exclusively use the first node as bootstrap.
    let bootstrap_peers = vec!["/ip4/127.0.0.1/udp/4001/quic-v1".to_string()];
    let daemon2 = make_test_daemon(3100, bootstrap_peers.clone(), 4002);
    let daemon3 = make_test_daemon(3200, bootstrap_peers, 0);

    handles.append(&mut start_nodes(vec![daemon2, daemon3], Duration::from_secs(1)).await);
    handles
}

/// Fetch peer ids for provided REST API ports.
pub async fn get_peer_ids(client: &reqwest::Client, ports: &[u16]) -> StdHashMap<u16, String> {
    let peer_id_tasks = ports.iter().copied().map(|port| {
        let client = client.clone();
        async move {
            let base = format!("http://127.0.0.1:{}", port);
            match client
                .get(format!("{}/debug/local_peer_id", base))
                .send()
                .await
            {
                Ok(resp) => match resp.json::<serde_json::Value>().await {
                    Ok(json) if json.get("ok").and_then(|v| v.as_bool()) == Some(true) => {
                        if let Some(peer_id) = json.get("local_peer_id").and_then(|v| v.as_str()) {
                            return (port, Some(peer_id.to_string()));
                        }
                    }
                    _ => {}
                },
                Err(_) => {}
            }
            (port, None)
        }
    });

    let peer_id_results = join_all(peer_id_tasks).await;
    let mut port_to_peer_id = StdHashMap::new();
    for (port, maybe_id) in peer_id_results {
        if let Some(id) = maybe_id {
            port_to_peer_id.insert(port, id);
        }
    }
    port_to_peer_id
}

/// Wait until the libp2p mesh has at least two peer connections across provided ports.
pub async fn wait_for_mesh_formation(
    client: &reqwest::Client,
    ports: &[u16],
    timeout: Duration,
) -> bool {
    let start = Instant::now();
    loop {
        let mut total_peers = 0usize;
        for &port in ports {
            let base = format!("http://127.0.0.1:{}", port);
            if let Ok(resp) = client.get(format!("{}/debug/peers", base)).send().await {
                if let Ok(json) = resp.json::<serde_json::Value>().await {
                    if let Some(peers_array) = json.get("peers").and_then(|v| v.as_array()) {
                        total_peers += peers_array.len();
                    }
                }
            }
        }

        if total_peers >= 2 {
            log::info!(
                "Mesh formation successful: {} total peer connections",
                total_peers
            );
            return true;
        }

        if start.elapsed() > timeout {
            log::warn!(
                "Mesh formation timed out after {:?}, only {} peer connections",
                timeout,
                total_peers
            );
            return false;
        }

        sleep(Duration::from_millis(500)).await;
    }
}

/// Inspect node debug endpoints to determine workload placement.

pub async fn check_workload_deployment(
    client: &reqwest::Client,
    ports: &[u16],
    task_id: &str,
    original_content: &str,
    port_to_peer_id: &StdHashMap<u16, String>,
    _expect_modified_replicas: bool,
    expected_nodes: Option<usize>,
) -> (Vec<u16>, Vec<u16>) {
    let max_attempts = 60usize;
    let mut attempt = 0usize;

    loop {
        let verification_tasks = ports.iter().copied().map(|port| {
            let client = client.clone();
            let _task_id = task_id.to_string();
            let _original_content = original_content.to_string();
            let port_to_peer_id = port_to_peer_id.clone();
            async move {
                let base = format!("http://127.0.0.1:{}", port);
                if let Some(peer_id) = port_to_peer_id.get(&port) {
                    let peer_resp = client
                        .get(format!("{}/debug/workloads_by_peer/{}", base, peer_id))
                        .send()
                        .await;

                    if let Ok(resp) = peer_resp {
                        if let Ok(json) = resp.json::<serde_json::Value>().await {
                            log::info!(
                                "Peer-specific endpoint response for peer {} on port {}: {}",
                                peer_id,
                                port,
                                json
                            );
                            if json.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                                let workload_count = json
                                    .get("workload_count")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0);

                                if workload_count == 0 {
                                    log::info!(
                                        "No workloads for peer {} on port {} - returning early",
                                        peer_id,
                                        port
                                    );
                                    return (port, false, false);
                                }

                                if let Some(workloads) =
                                    json.get("workloads").and_then(|v| v.as_object())
                                {
                                    for workload_info in workloads.values() {
                                        if let Some(metadata) = workload_info
                                            .get("metadata")
                                            .and_then(|v| v.as_object())
                                        {
                                            if let Some(name) =
                                                metadata.get("name").and_then(|v| v.as_str())
                                            {
                                                if name == "my-nginx" {
                                                    let exported_manifest_matches = workload_info
                                                        .get("exported_manifest")
                                                        .and_then(|v| v.as_str())
                                                        .map(|manifest| {
                                                            let contains_nginx =
                                                                manifest.contains("my-nginx");
                                                            let contains_kind = manifest
                                                                .contains("Deployment")
                                                                || manifest.contains("Pod");
                                                            let contains_api =
                                                                manifest.contains("apiVersion");

                                                            if manifest.len() > 100 {
                                                                log::info!(
                                                                    "Exported manifest preview: {}",
                                                                    &manifest[..manifest.len().min(200)]
                                                                );
                                                            }
                                                            contains_nginx && contains_kind && contains_api
                                                        })
                                                        .unwrap_or(false);

                                                    return (port, true, exported_manifest_matches);
                                                }
                                            }
                                        }

                                        if let Some(status) =
                                            workload_info.get("status").and_then(|v| v.as_str())
                                        {
                                            if status == "Running" {
                                                return (port, true, true);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Fall back to inspecting the debug workloads endpoint
                match client
                    .get(format!("{}/debug/workloads", base))
                    .send()
                    .await
                {
                    Ok(resp) => match resp.json::<serde_json::Value>().await {
                        Ok(json) if json.get("ok").and_then(|v| v.as_bool()) == Some(true) => {
                            if let Some(workloads) =
                                json.get("workloads").and_then(|v| v.as_array())
                            {
                                for workload in workloads {
                                    if let Some(exported_manifest) = workload
                                        .get("exported_manifest")
                                        .and_then(|v| v.as_str())
                                    {
                                        let exported_manifest_matches = exported_manifest
                                            .contains(&format!("tender_id: \\\"{}\\\"", _task_id))
                                            && exported_manifest.contains("my-nginx")
                                            && exported_manifest.contains("Deployment")
                                            && exported_manifest.contains("apiVersion");

                                        if exported_manifest_matches {
                                            return (port, true, true);
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    },
                    Err(err) => {
                        log::warn!(
                            "Failed to fetch workloads for port {}: {}. Continuing without immediate failure to allow retries.",
                            port, err
                        );
                    }
                }

                (port, false, false)
            }
        });

        let verification_results = join_all(verification_tasks).await;
        let nodes_with_deployed_workloads: Vec<u16> = verification_results
            .iter()
            .filter_map(
                |(port, has_workload, _)| {
                    if *has_workload { Some(*port) } else { None }
                },
            )
            .collect();

        let nodes_with_content_mismatch: Vec<u16> = verification_results
            .iter()
            .filter_map(|(port, has_workload, manifest_matches)| {
                if *has_workload && !manifest_matches {
                    Some(*port)
                } else {
                    None
                }
            })
            .collect();

        if let Some(expected_nodes) = expected_nodes {
            if nodes_with_deployed_workloads.len() >= expected_nodes
                && nodes_with_content_mismatch.is_empty()
            {
                return (nodes_with_deployed_workloads, nodes_with_content_mismatch);
            }
        } else if !nodes_with_deployed_workloads.is_empty()
            && nodes_with_content_mismatch.is_empty()
        {
            return (nodes_with_deployed_workloads, nodes_with_content_mismatch);
        }

        attempt += 1;
        if attempt >= max_attempts {
            return (nodes_with_deployed_workloads, nodes_with_content_mismatch);
        }

        sleep(Duration::from_millis(500)).await;
    }
}

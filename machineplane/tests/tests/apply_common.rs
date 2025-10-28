use env_logger::Env;
use futures::future::join_all;
use std::collections::HashMap as StdHashMap;
use std::time::Duration;
use tokio::time::{sleep, Instant};

mod test_utils;
use machine::Cli;
use test_utils::{make_test_cli, setup_cleanup_hook, start_nodes, NodeGuard};

pub const TEST_PORTS: [u16; 3] = [3000u16, 3100u16, 3200u16];

/// Prepare logging and environment for mock runtime tests.
pub async fn setup_test_environment() -> (reqwest::Client, Vec<u16>) {
    setup_cleanup_hook();
    let _ = env_logger::Builder::from_env(Env::default().default_filter_or("warn")).try_init();

    std::env::set_var("BEEMESH_MOCK_ONLY_RUNTIME", "1");

    let client = reqwest::Client::new();
    (client, TEST_PORTS.to_vec())
}

/// Build standard three-node cluster configuration with optional scheduling disable flags.
fn make_standard_node_clis(disable_flags: &[bool]) -> Vec<Cli> {
    assert!(disable_flags.len() == TEST_PORTS.len());

    let cli1 = make_test_cli(3000, false, true, None, vec![], 4001, 0, disable_flags[0]);
    let cli2 = make_test_cli(
        3100,
        false,
        true,
        None,
        vec!["/ip4/127.0.0.1/tcp/4001".to_string()],
        4002,
        0,
        disable_flags[1],
    );

    let bootstrap_peers = vec![
        "/ip4/127.0.0.1/tcp/4001".to_string(),
        "/ip4/127.0.0.1/tcp/4002".to_string(),
    ];
    let cli3 = make_test_cli(
        3200,
        false,
        true,
        None,
        bootstrap_peers,
        0,
        0,
        disable_flags[2],
    );

    vec![cli1, cli2, cli3]
}

/// Start the standard set of nodes with the given scheduling disable flags.
pub async fn start_cluster_nodes(disable_flags: &[bool]) -> NodeGuard {
    let clis = make_standard_node_clis(disable_flags);
    start_nodes(clis, Duration::from_secs(1)).await
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
) -> (Vec<u16>, Vec<u16>) {
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
                                    if let Some(metadata) =
                                        workload_info.get("metadata").and_then(|v| v.as_object())
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
                                                                &manifest
                                                                    [..manifest.len().min(200)]
                                                            );
                                                        }
                                                        contains_nginx
                                                            && contains_kind
                                                            && contains_api
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
            (port, false, false)
        }
    });

    let results = join_all(verification_tasks).await;
    let mut deployed = Vec::new();
    let mut mismatched = Vec::new();

    for (port, has_workload, content_matches) in results {
        if has_workload {
            deployed.push(port);
            if !content_matches {
                mismatched.push(port);
            }
        }
    }

    (deployed, mismatched)
}

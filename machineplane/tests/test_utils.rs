//! Consolidated Test Utilities for Machineplane Integration Tests
//!
//! This module provides shared utilities for all integration tests:
//! - Daemon configuration and startup
//! - Fabric mesh formation
//! - Pod deployment verification
//! - Kubernetes API helpers
//!
//! # Usage
//!
//! Import this module in test files:
//! ```ignore
//! #[path = "test_utils.rs"]
//! mod test_utils;
//! use test_utils::*;
//! ```

#![allow(dead_code)]

use anyhow::{Context, Result, anyhow};
use env_logger::Env;
use futures::future::join_all;
use machineplane::{DaemonConfig, start_machineplane as spawn_machineplane};
use reqwest::{Client, StatusCode};
use serde_json::Value;
use std::collections::HashMap as StdHashMap;
use std::path::Path;
use std::time::Duration;
use tokio::fs;
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep};

// ============================================================================
// Constants
// ============================================================================

/// Default REST API ports for 3-node fabric tests.
pub const TEST_PORTS: [u16; 3] = [3000u16, 3100u16, 3200u16];

/// Default libp2p QUIC ports for 3-node fabric tests.
pub const TEST_LIBP2P_PORTS: [u16; 3] = [4000u16, 4100u16, 4200u16];

/// Default REST API ports for 5-node fabric tests.
pub const TEST_PORTS_5: [u16; 5] = [3000u16, 3100u16, 3200u16, 3300u16, 3400u16];

/// Default libp2p QUIC ports for 5-node fabric tests.
pub const TEST_LIBP2P_PORTS_5: [u16; 5] = [4000u16, 4100u16, 4200u16, 4300u16, 4400u16];

// ============================================================================
// Daemon Configuration
// ============================================================================

/// Build a daemon configuration suitable for integration tests.
///
/// # Arguments
///
/// * `rest_api_port` - HTTP API port for this node
/// * `bootstrap_peers` - List of multiaddr strings for bootstrap nodes
/// * `libp2p_quic_port` - libp2p QUIC transport port
pub fn make_test_daemon(
    rest_api_port: u16,
    bootstrap_peers: Vec<String>,
    libp2p_quic_port: u16,
) -> DaemonConfig {
    DaemonConfig {
        rest_api_host: "127.0.0.1".to_string(),
        rest_api_port,
        bootstrap_peer: bootstrap_peers,
        libp2p_quic_port,
        // Prefer explicit CONTAINER_HOST if provided, otherwise let machineplane detect defaults.
        podman_socket: std::env::var("CONTAINER_HOST")
            .ok()
            .filter(|value| !value.trim().is_empty()),
        signing_ephemeral: true,
        kem_ephemeral: true,
        ephemeral_keys: true,
        ..DaemonConfig::default()
    }
}

// ============================================================================
// Node Lifecycle
// ============================================================================

/// Start a list of beemesh machineplane daemons given their configurations.
///
/// Returns `JoinHandle`s for spawned background tasks.
pub async fn start_nodes(
    daemons: Vec<DaemonConfig>,
    startup_delay: Duration,
) -> Vec<JoinHandle<()>> {
    let mut all_handles = Vec::new();
    for daemon in daemons {
        let rest_api_host = daemon.rest_api_host.clone();
        let rest_api_port = daemon.rest_api_port;
        let libp2p_host = daemon.libp2p_host.clone();
        let libp2p_quic_port = daemon.libp2p_quic_port;
        log::info!(
            "starting test daemon: REST http://{}:{}, libp2p host {} port {}",
            rest_api_host,
            rest_api_port,
            libp2p_host,
            libp2p_quic_port
        );
        match spawn_machineplane(daemon).await {
            Ok(mut handles) => {
                all_handles.append(&mut handles);
            }
            Err(e) => panic!("failed to start node: {e:?}"),
        }
        tokio::spawn(log_local_address(
            rest_api_host,
            rest_api_port,
            libp2p_host,
            libp2p_quic_port,
        ));
        sleep(startup_delay).await;
    }
    all_handles
}

/// Abort all spawned node tasks.
pub async fn shutdown_nodes(handles: &mut Vec<JoinHandle<()>>) {
    for handle in handles.drain(..) {
        handle.abort();
    }
}

/// Wait for a daemon to report its local peer multiaddr.
pub async fn wait_for_local_multiaddr(
    rest_api_host: &str,
    rest_api_port: u16,
    libp2p_host: &str,
    libp2p_quic_port: u16,
    timeout: Duration,
) -> Option<String> {
    let display_host = if libp2p_host == "0.0.0.0" {
        "127.0.0.1"
    } else {
        libp2p_host
    };

    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Some(peer_id) = fetch_local_peer_id(rest_api_host, rest_api_port).await {
            let address = format!(
                "/ip4/{}/udp/{}/quic-v1/p2p/{}",
                display_host, libp2p_quic_port, peer_id
            );
            log::info!("detected local multiaddr: {}", address);
            return Some(address);
        }

        sleep(Duration::from_millis(500)).await;
    }

    None
}

async fn fetch_local_peer_id(rest_api_host: &str, rest_api_port: u16) -> Option<String> {
    let base = format!("http://{}:{}", rest_api_host, rest_api_port);
    let client = Client::new();
    let request = client
        .get(format!("{}/debug/local_peer_id", base))
        .timeout(Duration::from_secs(5));

    match request.send().await {
        Ok(resp) => match resp.json::<Value>().await {
            Ok(Value::Object(map)) if map.get("ok").and_then(|v| v.as_bool()) == Some(true) => map
                .get("local_peer_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            Ok(_) => None,
            Err(_) => None,
        },
        Err(_) => None,
    }
}

async fn log_local_address(
    rest_api_host: String,
    rest_api_port: u16,
    libp2p_host: String,
    libp2p_quic_port: u16,
) {
    let display_host = if libp2p_host == "0.0.0.0" {
        "127.0.0.1".to_string()
    } else {
        libp2p_host.clone()
    };

    let base = format!("http://{}:{}", rest_api_host, rest_api_port);
    let client = Client::new();
    let request = client
        .get(format!("{}/debug/local_peer_id", base))
        .timeout(Duration::from_secs(5));

    match request.send().await {
        Ok(resp) => match resp.json::<Value>().await {
            Ok(Value::Object(map)) => {
                if map.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                    if let Some(peer_id) = map.get("local_peer_id").and_then(|v| v.as_str()) {
                        let libp2p_address = format!(
                            "/ip4/{}/udp/{}/quic-v1/p2p/{}",
                            display_host, libp2p_quic_port, peer_id
                        );
                        log::info!(
                            "machineplane node at {} reports libp2p address {}",
                            base,
                            libp2p_address
                        );
                        return;
                    }
                }
                log::warn!("machineplane node at {} did not return a peer id", base);
            }
            Ok(_) => {
                log::warn!("machineplane node at {} returned unexpected JSON", base);
            }
            Err(err) => {
                log::warn!(
                    "failed to parse local_peer_id response from {}: {}",
                    base,
                    err
                );
            }
        },
        Err(err) => {
            log::warn!("failed to fetch local_peer_id from {}: {}", base, err);
        }
    }
}

// ============================================================================
// Environment Setup
// ============================================================================

/// Prepare logging and environment for runtime tests.
pub async fn setup_test_environment() -> (Client, Vec<u16>) {
    // Use a verbose default filter to surface signing/verification debug logs in CI output.
    let _ = env_logger::Builder::from_env(Env::default().default_filter_or("info")).try_init();

    let client = Client::new();
    (client, TEST_PORTS.to_vec())
}

// ============================================================================
// Fabric Formation
// ============================================================================

/// Build standard three-node fabric configuration.
/// Returns JoinHandles for the spawned tasks so tests can abort them when finished.
pub async fn start_fabric_nodes() -> Vec<JoinHandle<()>> {
    start_fabric_nodes_with_ports(&TEST_PORTS, &TEST_LIBP2P_PORTS).await
}

/// Start a fabric mesh using the provided REST and libp2p port lists.
///
/// The first entry in `rest_ports`/`libp2p_ports` is treated as the bootstrap node;
/// all subsequent nodes will join using its advertised peer address. All nodes are
/// configured with persistent key material to make debugging easier.
pub async fn start_fabric_nodes_with_ports(
    rest_ports: &[u16],
    libp2p_ports: &[u16],
) -> Vec<JoinHandle<()>> {
    assert!(
        !rest_ports.is_empty() && rest_ports.len() == libp2p_ports.len(),
        "REST and libp2p port lists must be non-empty and the same length"
    );

    // Start a single bootstrap node first to give it time to initialize.
    let mut bootstrap_daemon = make_test_daemon(rest_ports[0], vec![], libp2p_ports[0]);
    bootstrap_daemon.signing_ephemeral = true;
    bootstrap_daemon.kem_ephemeral = true;
    bootstrap_daemon.ephemeral_keys = true;
    let mut handles = start_nodes(vec![bootstrap_daemon], Duration::from_secs(1)).await;

    // Allow the bootstrap node to settle before connecting peers.
    sleep(Duration::from_secs(2)).await;

    // Discover the bootstrap node's advertised libp2p address (with peer ID)
    // so subsequent nodes can join the mesh successfully.
    let bootstrap_peer = wait_for_local_multiaddr(
        "127.0.0.1",
        rest_ports[0],
        "127.0.0.1",
        libp2p_ports[0],
        Duration::from_secs(10),
    )
    .await
    .expect("bootstrap node did not expose a peer id in time");

    // Start additional nodes that exclusively use the first node as bootstrap.
    let bootstrap_peers = vec![bootstrap_peer];
    let mut peer_daemons = Vec::new();
    for (&rest_port, &libp2p_port) in rest_ports.iter().zip(libp2p_ports.iter()).skip(1) {
        let mut daemon = make_test_daemon(rest_port, bootstrap_peers.clone(), libp2p_port);
        daemon.signing_ephemeral = true;
        daemon.kem_ephemeral = true;
        daemon.ephemeral_keys = true;
        peer_daemons.push(daemon);
    }

    handles.append(&mut start_nodes(peer_daemons, Duration::from_secs(1)).await);
    handles
}

/// Fetch peer ids for provided REST API ports.
pub async fn get_peer_ids(client: &Client, ports: &[u16]) -> StdHashMap<u16, String> {
    let peer_id_tasks = ports.iter().copied().map(|port| {
        let client = client.clone();
        async move {
            let base = format!("http://127.0.0.1:{}", port);
            match client
                .get(format!("{}/debug/local_peer_id", base))
                .send()
                .await
            {
                Ok(resp) => match resp.json::<Value>().await {
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

/// Wait until the libp2p mesh has at least the specified peer connections across provided ports.
pub async fn wait_for_mesh_formation(
    client: &Client,
    ports: &[u16],
    timeout: Duration,
    min_total_peers: usize,
) -> bool {
    let start = Instant::now();
    loop {
        let mut total_peers = 0usize;
        for &port in ports {
            let base = format!("http://127.0.0.1:{}", port);
            if let Ok(resp) = client.get(format!("{}/debug/peers", base)).send().await {
                if let Ok(json) = resp.json::<Value>().await {
                    if let Some(peers_array) = json.get("peers").and_then(|v| v.as_array()) {
                        total_peers += peers_array.len();
                    }
                }
            }
        }

        if total_peers >= min_total_peers {
            log::info!(
                "Mesh formation successful: {} total peer connections (target: {})",
                total_peers,
                min_total_peers
            );
            return true;
        }

        if start.elapsed() > timeout {
            log::warn!(
                "Mesh formation timed out after {:?}, only {} peer connections (target: {})",
                timeout,
                total_peers,
                min_total_peers
            );
            return false;
        }

        sleep(Duration::from_millis(500)).await;
    }
}

// ============================================================================
// Kubernetes API Helpers
// ============================================================================

/// Extracts metadata (namespace, name, kind) from a Kubernetes manifest.
fn manifest_metadata(manifest: &Value) -> Result<(String, String, String)> {
    let metadata = manifest
        .get("metadata")
        .and_then(|v| v.as_object())
        .ok_or_else(|| anyhow!("manifest missing metadata"))?;

    let name = metadata
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("manifest missing metadata.name"))?
        .to_string();

    let namespace = metadata
        .get("namespace")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "default".to_string());

    let kind = manifest
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or("Deployment")
        .to_string();

    Ok((namespace, name, kind))
}

/// Computes a deterministic resource key based on namespace, name, and kind.
/// Used for tracking workloads during test verification.
fn compute_resource_key(namespace: &str, name: &str, kind: &str) -> String {
    format!("{}/{}/{}", namespace, kind, name)
}

/// Loads and parses a YAML manifest from a file.
pub async fn load_manifest(path: &Path) -> Result<Value> {
    let contents = fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read manifest at {}", path.display()))?;
    let manifest: Value = serde_yaml::from_str(&contents)
        .with_context(|| format!("failed to parse manifest at {}", path.display()))?;
    Ok(manifest)
}

/// Simulates `kubectl apply` by sending a POST request to the API.
///
/// Returns the `tender_id` on success.
pub async fn apply_manifest_via_kube_api(
    client: &Client,
    port: u16,
    manifest_path: &Path,
) -> Result<String> {
    let manifest = load_manifest(manifest_path).await?;
    let (namespace, _name, _kind) = manifest_metadata(&manifest)?;

    let url = format!(
        "http://127.0.0.1:{}/apis/apps/v1/namespaces/{}/deployments",
        port, namespace
    );

    let response = client
        .post(url)
        .json(&manifest)
        .send()
        .await
        .context("failed to send apply request")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "<unavailable>".to_string());
        return Err(anyhow!(
            "kubectl-compatible apply failed with HTTP {}: {}",
            status,
            body
        ));
    }

    let response_json: Value = response
        .json()
        .await
        .context("failed to decode apply response body")?;

    response_json
        .get("tender_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("apply response missing tender_id"))
}

/// Simulates `kubectl delete` by sending a DELETE request to the API.
///
/// If `force` is true, it adds query parameters to force immediate deletion.
/// Returns the resource key (namespace/kind/name) on success.
pub async fn delete_manifest_via_kube_api(
    client: &Client,
    port: u16,
    manifest_path: &Path,
    force: bool,
) -> Result<String> {
    let manifest = load_manifest(manifest_path).await?;
    let (namespace, name, kind) = manifest_metadata(&manifest)?;
    let resource_key = compute_resource_key(&namespace, &name, &kind);

    let mut request = client.delete(format!(
        "http://127.0.0.1:{}/apis/apps/v1/namespaces/{}/deployments/{}",
        port, namespace, name
    ));

    if force {
        request = request.query(&[
            ("gracePeriodSeconds", "0"),
            ("propagationPolicy", "Foreground"),
        ]);
    }

    let response = request
        .send()
        .await
        .context("failed to send delete request")?;

    match response.status() {
        status if status.is_success() => Ok(resource_key),
        StatusCode::NOT_FOUND => Err(anyhow!(
            "delete failed: deployment {} in namespace {} not found",
            name,
            namespace
        )),
        status => {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<unavailable>".to_string());
            Err(anyhow!(
                "kubectl-compatible delete failed with HTTP {}: {}",
                status,
                body
            ))
        }
    }
}

// ============================================================================
// Pod Verification
// ============================================================================

/// Inspect node debug endpoints to determine pod placement.
pub async fn check_instance_deployment(
    client: &Client,
    ports: &[u16],
    task_id: &str,
    original_content: &str,
    port_to_peer_id: &StdHashMap<u16, String>,
    _expect_modified_replicas: bool,
    expected_nodes: Option<usize>,
    timeout: Duration,
) -> (Vec<u16>, Vec<u16>) {
    let start = Instant::now();

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
                        .get(format!("{}/debug/instances_by_peer/{}", base, peer_id))
                        .send()
                        .await;

                    if let Ok(resp) = peer_resp {
                        if let Ok(json) = resp.json::<Value>().await {
                            log::info!(
                                "Peer-specific endpoint response for peer {} on port {}: {}",
                                peer_id,
                                port,
                                json
                            );
                            if json.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                                let instance_count = json
                                    .get("instance_count")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0);

                                if instance_count == 0 {
                                    log::info!(
                                        "No pods for peer {} on port {} - returning early",
                                        peer_id,
                                        port
                                    );
                                    return (port, false, false);
                                }

                                if let Some(pods) =
                                    json.get("instances").and_then(|v| v.as_object())
                                {
                                    for pod_info in pods.values() {
                                        if let Some(metadata) = pod_info
                                            .get("metadata")
                                            .and_then(|v| v.as_object())
                                        {
                                            // Check both "name" key and K8s app name label
                                            let app_name = metadata
                                                .get("name")
                                                .or_else(|| metadata.get("app.kubernetes.io/name"))
                                                .and_then(|v| v.as_str());
                                            
                                            if app_name == Some("my-nginx") {
                                                // Check pod status first - only verify manifest for Running pods
                                                let is_running = pod_info
                                                    .get("status")
                                                    .and_then(|v| v.as_str())
                                                    .map(|s| s == "Running")
                                                    .unwrap_or(false);
                                                
                                                // If not running, treat as "not yet deployed" to keep waiting
                                                if !is_running {
                                                    log::info!(
                                                        "Found pod {} but status is not Running yet, continuing to wait",
                                                        app_name.unwrap_or("unknown")
                                                    );
                                                    continue;
                                                }
                                                
                                                // Found pod by app name - verify exported manifest
                                                // is valid K8s YAML (has apiVersion and kind)
                                                // Note: Pod name may differ from app name due to pod_id
                                                let exported_manifest_matches = pod_info
                                                    .get("exported_manifest")
                                                    .and_then(|v| v.as_str())
                                                    .map(|manifest| {
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
                                                        contains_kind && contains_api
                                                    })
                                                    .unwrap_or_else(|| {
                                                        // exported_manifest is null - may happen even for Running pods
                                                        // if kube generation fails. Log and continue waiting.
                                                        log::info!(
                                                            "Pod is Running but exported_manifest is null, treating as not ready"
                                                        );
                                                        false
                                                    });

                                                // Only return success if manifest matches
                                                if exported_manifest_matches {
                                                    return (port, true, true);
                                                } else {
                                                    // Running but manifest not ready - continue waiting
                                                    log::info!(
                                                        "Pod is Running but manifest validation failed, continuing to wait"
                                                    );
                                                    continue;
                                                }
                                            }
                                        }

                                        // Fallback: check for any running pod
                                        if let Some(status) =
                                            pod_info.get("status").and_then(|v| v.as_str())
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

                // Fall back to inspecting the debug pods endpoint
                match client
                    .get(format!("{}/debug/pods", base))
                    .send()
                    .await
                {
                    Ok(resp) => match resp.json::<Value>().await {
                        Ok(json) if json.get("ok").and_then(|v| v.as_bool()) == Some(true) => {
                            if let Some(pods) =
                                json.get("instances").and_then(|v| v.as_array())
                            {
                                for pod in pods {
                                    if let Some(exported_manifest) = pod
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
                            "Failed to fetch pods for port {}: {}. Continuing without immediate failure to allow retries.",
                            port, err
                        );
                    }
                }

                (port, false, false)
            }
        });

        let verification_results = join_all(verification_tasks).await;
        let nodes_with_deployed_instances: Vec<u16> = verification_results
            .iter()
            .filter_map(
                |(port, has_instance, _)| {
                    if *has_instance { Some(*port) } else { None }
                },
            )
            .collect();

        let nodes_with_content_mismatch: Vec<u16> = verification_results
            .iter()
            .filter_map(|(port, has_instance, manifest_matches)| {
                if *has_instance && !manifest_matches {
                    Some(*port)
                } else {
                    None
                }
            })
            .collect();

        if let Some(expected_nodes) = expected_nodes {
            if nodes_with_deployed_instances.len() >= expected_nodes
                && nodes_with_content_mismatch.is_empty()
            {
                return (nodes_with_deployed_instances, nodes_with_content_mismatch);
            }
        } else if !nodes_with_deployed_instances.is_empty()
            && nodes_with_content_mismatch.is_empty()
        {
            return (nodes_with_deployed_instances, nodes_with_content_mismatch);
        }

        if start.elapsed() >= timeout {
            return (nodes_with_deployed_instances, nodes_with_content_mismatch);
        }

        sleep(Duration::from_millis(500)).await;
    }
}

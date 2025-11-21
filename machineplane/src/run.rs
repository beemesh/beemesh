//! Workload Integration Module
//!
//! This module provides integration between the existing libp2p apply message handling
//! and the new workload manager system. It updates the apply message handler to use
//! the runtime engines and provider announcement system.

use crate::network::behaviour::MyBehaviour;
use crate::messages::machine;
use crate::placement::{PlacementConfig, PlacementManager};
use crate::capacity::CapacityVerifier;
use crate::runtimes::{DeploymentConfig, RuntimeRegistry, create_default_registry};
use libp2p::Swarm;
use libp2p::request_response;
use log::{debug, error, info, warn};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use crate::messages::types::ApplyRequest;

/// Global runtime registry for all available engines
static RUNTIME_REGISTRY: Lazy<Arc<RwLock<Option<RuntimeRegistry>>>> =
    Lazy::new(|| Arc::new(RwLock::new(None)));

/// Global placement manager for announcements
static PLACEMENT_MANAGER: Lazy<Arc<RwLock<Option<PlacementManager>>>> =
    Lazy::new(|| Arc::new(RwLock::new(None)));

/// Global capacity verifier for resource checks
static CAPACITY_VERIFIER: Lazy<Arc<CapacityVerifier>> =
    Lazy::new(|| Arc::new(CapacityVerifier::new()));

/// Node-local cache mapping manifest IDs to owner public keys.
static MANIFEST_OWNER_MAP: Lazy<RwLock<HashMap<String, Vec<u8>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

/// Initialize the runtime registry and provider manager
pub async fn initialize_podman_manager(
    force_mock_runtime: bool,
    mock_only_runtime: bool,
    scheduling_enabled: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if !scheduling_enabled {
        info!("Scheduling disabled; skipping runtime registry and provider manager initialization");
        return Ok(());
    }

    info!("Initializing runtime registry and placement manager for manifest deployment");

    // Create runtime registry - use mock-only for tests if environment variable is set
    let use_mock_registry = force_mock_runtime || mock_only_runtime;

    #[cfg(debug_assertions)]
    let registry = if use_mock_registry {
        info!("Using mock-only runtime registry for testing");
        crate::runtimes::create_mock_only_registry().await
    } else {
        create_default_registry().await
    };

    #[cfg(not(debug_assertions))]
    let registry = {
        if use_mock_registry {
            warn!(
                "mock runtime requested but not compiled in release build; falling back to default registry"
            );
        }
        create_default_registry().await
    };
    let available_engines = registry.check_available_engines().await;

    info!("Available runtime engines: {:?}", available_engines);

    // Store the registry globally
    {
        let mut global_registry = RUNTIME_REGISTRY.write().await;
        *global_registry = Some(registry);
    }

    // Create placement manager
    let placement_config = PlacementConfig {
        default_ttl_seconds: 3600, // 1 hour
        ..Default::default()
    };
    let placement_manager = PlacementManager::new(placement_config);

    {
        let mut global_placement_manager = PLACEMENT_MANAGER.write().await;
        *global_placement_manager = Some(placement_manager);
    }

    // Initialize capacity verifier with system resources
    info!("Initializing capacity verifier");
    let verifier = get_global_capacity_verifier();
    if let Err(e) = verifier.update_system_resources().await {
        warn!("Failed to update system resources: {}", e);
    }

    info!("Runtime registry and placement manager initialized successfully");
    Ok(())
}

/// Record the owner public key for a manifest on this node.
pub async fn record_manifest_owner(manifest_id: &str, owner_pubkey: &[u8]) {
    let mut map = MANIFEST_OWNER_MAP.write().await;
    map.insert(manifest_id.to_string(), owner_pubkey.to_vec());
    info!(
        "record_manifest_owner: stored owner_pubkey len={} for manifest_id={}",
        owner_pubkey.len(),
        manifest_id
    );
}

/// Retrieve the owner public key for a manifest if known.
pub async fn get_manifest_owner(manifest_id: &str) -> Option<Vec<u8>> {
    let map = MANIFEST_OWNER_MAP.read().await;
    map.get(manifest_id).cloned()
}

/// Remove the owner mapping for a manifest.
pub async fn remove_manifest_owner(manifest_id: &str) -> Option<Vec<u8>> {
    let mut map = MANIFEST_OWNER_MAP.write().await;
    map.remove(manifest_id)
}

/// Get access to the global runtime registry (for testing and debug endpoints)
pub async fn get_global_runtime_registry()
-> Option<tokio::sync::RwLockReadGuard<'static, Option<RuntimeRegistry>>> {
    let registry_guard = RUNTIME_REGISTRY.read().await;
    if registry_guard.is_some() {
        Some(registry_guard)
    } else {
        None
    }
}

/// Get access to the global capacity verifier
pub fn get_global_capacity_verifier() -> Arc<CapacityVerifier> {
    Arc::clone(&CAPACITY_VERIFIER)
}

/// Enhanced apply message handler that uses the workload manager
pub async fn handle_apply_message_with_podman_manager(
    message: request_response::Message<Vec<u8>, Vec<u8>>,
    peer: libp2p::PeerId,
    swarm: &mut Swarm<MyBehaviour>,
    _local_peer: libp2p::PeerId,
) {
    match message {
        request_response::Message::Request {
            request, channel, ..
        } => {
            info!("Received apply request from peer={}", peer);

            // Parse the apply request
            match machine::root_as_apply_request(&request) {
                Ok(apply_req) => {
                    info!(
                        "Apply request - operation_id={:?} replicas={}",
                        apply_req.operation_id,
                        apply_req.replicas
                    );

                    let owner_pubkey = Vec::new();

                    // Extract and validate manifest
                    if !apply_req.manifest_json.is_empty() {
                        let manifest_id = apply_req.manifest_id.as_str();
                        if manifest_id.is_empty() {
                            warn!(
                                "Apply request missing manifest_id; rejecting from peer={}",
                                peer
                            );
                            let error_response = machine::build_apply_response(
                                false,
                                "unknown",
                                "missing manifest id",
                            );
                            let _ = swarm
                                .behaviour_mut()
                                .apply_rr
                                .send_response(channel, error_response);
                            return;
                        }

                        let reservation_ok = get_global_capacity_verifier()
                            .has_active_reservation_for_manifest(manifest_id)
                            .await;
                        if !reservation_ok {
                            warn!(
                                "Apply request for manifest_id={} from peer={} without prior reservation",
                                manifest_id, peer
                            );
                            let error_response = machine::build_apply_response(
                                false,
                                manifest_id,
                                "no active capacity reservation",
                            );
                            let _ = swarm
                                .behaviour_mut()
                                .apply_rr
                                .send_response(channel, error_response);
                            return;
                        }

                match process_manifest_deployment(
                    swarm,
                    &apply_req,
                    &apply_req.manifest_json,
                    &owner_pubkey,
                )
                        .await
                        {
                            Ok(workload_id) => {
                                info!(
                                    "Successfully deployed workload {} for apply request",
                                    workload_id
                                );

                                // Send success response
                                let success_response = machine::build_apply_response(
                                    true,
                                    &workload_id,
                                    "workload deployed successfully",
                                );
                                let _ = swarm
                                    .behaviour_mut()
                                    .apply_rr
                                    .send_response(channel, success_response);
                            }
                            Err(e) => {
                                error!("Failed to deploy workload for apply request: {}", e);

                                // Send error response
                                let error_message = format!("deployment failed: {}", e);
                                let error_response =
                                    machine::build_apply_response(false, "unknown", &error_message);
                                let _ = swarm
                                    .behaviour_mut()
                                    .apply_rr
                                    .send_response(channel, error_response);
                            }
                        }
                    } else {
                        warn!("Apply request missing manifest JSON");
                        let error_response = machine::build_apply_response(
                            false,
                            "unknown",
                            "missing manifest JSON",
                        );
                        let _ = swarm
                            .behaviour_mut()
                            .apply_rr
                            .send_response(channel, error_response);
                    }
                }
                Err(e) => {
                    error!("Failed to parse apply request: {}", e);
                    let error_response = machine::build_apply_response(
                        false,
                        "unknown",
                        "invalid apply request format",
                    );
                    let _ = swarm
                        .behaviour_mut()
                        .apply_rr
                        .send_response(channel, error_response);
                }
            }
        }
        request_response::Message::Response { .. } => {
            debug!("Received apply response from peer={}", peer);
            // Handle response if needed
        }
    }
}

/// Process manifest deployment using the workload manager
async fn process_manifest_deployment(
    swarm: &mut Swarm<MyBehaviour>,
    apply_req: &ApplyRequest,
    manifest_json: &str,
    owner_pubkey: &[u8],
) -> Result<String, Box<dyn std::error::Error>> {
    info!("Processing manifest deployment");

    // Parse the manifest content (always cleartext)
    let manifest_content = manifest_json.as_bytes().to_vec();

    // Use the manifest_id from the apply request for placement announcements
    // This ensures consistency between apply and delete operations
    let manifest_id = if apply_req.manifest_id.is_empty() {
        "unknown".to_string()
    } else {
        apply_req.manifest_id.clone()
    };

    info!(
        "Processing manifest deployment for manifest_id: {}",
        manifest_id
    );

    if owner_pubkey.is_empty() {
        warn!(
            "process_manifest_deployment: missing owner pubkey for manifest_id={}",
            manifest_id
        );
    } else {
        record_manifest_owner(&manifest_id, owner_pubkey).await;
    }

    // Modify manifest to set replicas=1 for this node deployment
    // The original manifest is stored in DHT, but each node deploys with replicas=1
    let modified_manifest_content = modify_manifest_replicas(&manifest_content)?;

    // Create deployment configuration
    let deployment_config = create_deployment_config(apply_req);

    // Select appropriate runtime engine based on manifest type
    let engine_name = select_runtime_engine(&modified_manifest_content).await?;
    info!(
        "Selected runtime engine '{}' for manifest_id: {}",
        engine_name, manifest_id
    );

    // Get runtime registry
    let registry_guard = RUNTIME_REGISTRY.read().await;
    let registry = registry_guard
        .as_ref()
        .ok_or("Runtime registry not initialized")?;

    // Get the selected engine
    let engine = registry
        .get_engine(&engine_name)
        .ok_or(format!("Runtime engine '{}' not available", engine_name))?;

    // Deploy the workload with modified manifest (replicas=1)
    // use peer-aware deployment for mock engine when compiled in debug builds
    #[cfg(debug_assertions)]
    let workload_info = {
        if engine_name == "mock" {
            if let Some(mock_engine) = engine
                .as_any()
                .downcast_ref::<crate::runtimes::mock::MockEngine>()
            {
                debug!("Using peer-aware deployment for mock engine");
                mock_engine
                    .deploy_workload_with_peer(
                        &manifest_id,
                        &modified_manifest_content,
                        &deployment_config,
                        *swarm.local_peer_id(),
                    )
                    .await?
            } else {
                engine
                    .deploy_workload(&manifest_id, &modified_manifest_content, &deployment_config)
                    .await?
            }
        } else {
            engine
                .deploy_workload(&manifest_id, &modified_manifest_content, &deployment_config)
                .await?
        }
    };

    #[cfg(not(debug_assertions))]
    let workload_info = {
        if engine_name == "mock" {
            warn!(
                "mock runtime selected but not included in release build; proceeding with default deployment path"
            );
        }
        engine
            .deploy_workload(&manifest_id, &modified_manifest_content, &deployment_config)
            .await?
    };

    info!(
        "Workload deployed successfully: {} using engine '{}', status: {:?}",
        workload_info.id, engine_name, workload_info.status
    );

    // Announce placement if deployment successful
    if let Some(placement_manager) = PLACEMENT_MANAGER.read().await.as_ref() {
        let mut metadata = HashMap::new();
        metadata.insert("runtime_engine".to_string(), engine_name.clone());
        metadata.insert("workload_id".to_string(), workload_info.id.clone());
        metadata.insert("node_type".to_string(), "beemesh-machine".to_string());

        if let Err(e) = placement_manager.announce_placement(swarm, &manifest_id, metadata) {
            warn!(
                "Failed to announce placement for manifest {}: {}",
                manifest_id, e
            );
        } else {
            info!(
                "Announced placement for manifest_id: {} using engine '{}'",
                manifest_id, engine_name
            );
        }
    }

    Ok(workload_info.id)
}

/// Modify the manifest to set replicas=1 for single-node deployment
/// Each node in the fabric will deploy one replica of the workload
fn modify_manifest_replicas(
    manifest_content: &[u8],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let manifest_str = String::from_utf8_lossy(manifest_content);

    // Try to parse as YAML/JSON and modify replicas field
    if let Ok(mut doc) = serde_yaml::from_str::<serde_yaml::Value>(&manifest_str) {
        // Check for spec.replicas field (Kubernetes-style)
        if let Some(spec) = doc.get_mut("spec") {
            if let Some(spec_map) = spec.as_mapping_mut() {
                spec_map.insert(
                    serde_yaml::Value::String("replicas".to_string()),
                    serde_yaml::Value::Number(serde_yaml::Number::from(1)),
                );
            }
        }
        // Check for top-level replicas field
        else if doc.get("replicas").is_some() {
            if let Some(doc_map) = doc.as_mapping_mut() {
                doc_map.insert(
                    serde_yaml::Value::String("replicas".to_string()),
                    serde_yaml::Value::Number(serde_yaml::Number::from(1)),
                );
            }
        }

        // Convert back to YAML bytes
        let modified_yaml = serde_yaml::to_string(&doc)?;
        info!("Modified manifest to set replicas=1 for single-node deployment");
        Ok(modified_yaml.into_bytes())
    } else {
        // If parsing fails, return original content unchanged
        warn!("Failed to parse manifest for replica modification, using original");
        Ok(manifest_content.to_vec())
    }
}

/// Select the appropriate runtime engine based on manifest content and annotations
async fn select_runtime_engine(
    manifest_content: &[u8],
) -> Result<String, Box<dyn std::error::Error>> {
    let manifest_str = String::from_utf8_lossy(manifest_content);

    // Try to parse as YAML and look for annotations
    if let Ok(doc) = serde_yaml::from_str::<serde_yaml::Value>(&manifest_str) {
        // Check for runtime engine annotation
        if let Some(metadata) = doc.get("metadata") {
            if let Some(annotations) = metadata.get("annotations") {
                if let Some(engine) = annotations
                    .get("beemesh.io/runtime-engine")
                    .and_then(|v| v.as_str())
                {
                    info!("Found runtime engine annotation: {}", engine);
                    return Ok(engine.to_string());
                }
            }
        }

        // Check manifest type - note preferences, but don't hardcode
        if let Some(kind) = doc.get("kind").and_then(|k| k.as_str()) {
            match kind {
                "Pod" | "Deployment" | "Service" | "ConfigMap" | "Secret" => {
                    // Kubernetes resources - will prefer Podman below if available
                    debug!("Detected Kubernetes manifest kind: {}", kind);
                }
                _ => {}
            }
        }

        // Check for Docker Compose format - note preference, but don't hardcode
        if doc.get("services").is_some() && doc.get("version").is_some() {
            debug!("Detected Docker Compose manifest");
            // Will prefer Docker below if available
        }
    }

    // Get available engines and select the best one
    if let Some(registry) = RUNTIME_REGISTRY.read().await.as_ref() {
        let available = registry.check_available_engines().await;

        // Prefer Podman, then Docker, then mock for testing
        if *available.get("podman").unwrap_or(&false) {
            return Ok("podman".to_string());
        } else if *available.get("docker").unwrap_or(&false) {
            return Ok("docker".to_string());
        } else if *available.get("mock").unwrap_or(&false) {
            return Ok("mock".to_string());
        }
    }

    Err("No suitable runtime engine available".into())
}



/// Create deployment configuration from apply request
fn create_deployment_config(apply_req: &ApplyRequest) -> DeploymentConfig {
    let mut config = DeploymentConfig::default();

    // Set replicas
    config.replicas = apply_req.replicas;

    // Metadata from apply request can be added here if needed
    if !apply_req.operation_id.is_empty() {
        config
            .env
            .insert("BEEMESH_OPERATION_ID".to_string(), apply_req.operation_id.clone());
    }

    config
}



/// Enhanced self-apply processing with workload manager
pub async fn process_enhanced_self_apply_request(manifest: &[u8], swarm: &mut Swarm<MyBehaviour>) {
    debug!(
        "Processing enhanced self-apply request (manifest len={})",
        manifest.len()
    );

    match machine::root_as_apply_request(manifest) {
        Ok(apply_req) => {
            debug!(
                "Enhanced self-apply request - operation_id={:?} replicas={}",
                apply_req.operation_id,
                apply_req.replicas
            );

            if !apply_req.manifest_json.is_empty() {
                let manifest_json = apply_req.manifest_json.clone();
                let owner_pubkey = Vec::new();

                match process_manifest_deployment(
                    swarm,
                    &apply_req,
                    &manifest_json,
                    &owner_pubkey,
                )
                    .await
                {
                    Ok(workload_id) => {
                        info!(
                            "Successfully deployed self-applied workload: {}",
                            workload_id
                        );
                    }
                    Err(e) => {
                        error!("Failed to deploy self-applied workload: {}", e);
                    }
                }
            } else {
                warn!("Self-apply request missing manifest JSON");
            }
        }
        Err(e) => {
            error!("Failed to parse self-apply request: {}", e);
        }
    }
}

/// Get runtime registry statistics
pub async fn get_runtime_registry_stats() -> HashMap<String, bool> {
    if let Some(registry) = RUNTIME_REGISTRY.read().await.as_ref() {
        registry.check_available_engines().await
    } else {
        HashMap::new()
    }
}

/// List all available runtime engines
pub async fn list_available_engines() -> Vec<String> {
    if let Some(registry) = RUNTIME_REGISTRY.read().await.as_ref() {
        registry
            .list_engines()
            .iter()
            .map(|s| s.to_string())
            .collect()
    } else {
        Vec::new()
    }
}

/// Remove a workload by ID (requires engine name)
pub async fn remove_workload_by_id(
    workload_id: &str,
    engine_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let registry_guard = RUNTIME_REGISTRY.read().await;
    let registry = registry_guard
        .as_ref()
        .ok_or("Runtime registry not initialized")?;

    let engine = registry
        .get_engine(engine_name)
        .ok_or(format!("Runtime engine '{}' not available", engine_name))?;

    engine.remove_workload(workload_id).await?;
    info!(
        "Successfully removed workload: {} from engine: {}",
        workload_id, engine_name
    );
    Ok(())
}

/// Remove workloads by manifest ID - searches through all engines and removes matching workloads
pub async fn remove_workloads_by_manifest_id(
    manifest_id: &str,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    info!(
        "remove_workloads_by_manifest_id: manifest_id={}",
        manifest_id
    );

    let registry_guard = RUNTIME_REGISTRY.read().await;
    let registry = registry_guard
        .as_ref()
        .ok_or("Runtime registry not initialized")?;

    let mut removed_workloads = Vec::new();
    let mut errors = Vec::new();
    let engine_names = registry.list_engines();

    for engine_name in &engine_names {
        if let Some(engine) = registry.get_engine(engine_name) {
            info!(
                "Checking engine '{}' for workloads with manifest_id '{}'",
                engine_name, manifest_id
            );

            // List workloads from this engine
            match engine.list_workloads().await {
                Ok(workloads) => {
                    // Find workloads that match the manifest_id
                    for workload in workloads {
                        if workload.manifest_id == manifest_id {
                            info!(
                                "Found matching workload: {} in engine '{}'",
                                workload.id, engine_name
                            );

                            // Remove the workload
                            match engine.remove_workload(&workload.id).await {
                                Ok(()) => {
                                    info!(
                                        "Successfully removed workload: {} from engine '{}'",
                                        workload.id, engine_name
                                    );
                                    removed_workloads.push(workload.id);
                                }
                                Err(e) => {
                                    error!(
                                        "Failed to remove workload {} from engine '{}': {}",
                                        workload.id, engine_name, e
                                    );
                                    errors.push(format!("{}:{}", engine_name, workload.id));
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        "Failed to list workloads from engine '{}': {}",
                        engine_name, e
                    );
                    errors.push(format!("list:{}", engine_name));
                }
            }
        }
    }

    // Also withdraw placement announcement if we were providing this manifest
    if let Some(placement_manager) = PLACEMENT_MANAGER.read().await.as_ref() {
        if let Err(e) = placement_manager.stop_providing(manifest_id) {
            warn!(
                "Failed to stop providing manifest_id {}: {}",
                manifest_id, e
            );
        } else {
            info!("Stopped providing manifest_id: {}", manifest_id);
        }
    }

    if errors.is_empty() {
        // Drop the cached owner once the manifest workloads are removed successfully.
        if remove_manifest_owner(manifest_id).await.is_some() {
            info!(
                "remove_workloads_by_manifest_id: cleared owner mapping for manifest_id={}",
                manifest_id
            );
        }
    } else {
        warn!(
            "remove_workloads_by_manifest_id: retaining owner mapping for manifest_id={} due to errors {:?}",
            manifest_id, errors
        );
    }

    info!(
        "remove_workloads_by_manifest_id completed: removed {} workloads for manifest_id '{}'",
        removed_workloads.len(),
        manifest_id
    );
    Ok(removed_workloads)
}

/// Get logs from a workload (requires engine name)
pub async fn get_workload_logs_by_id(
    workload_id: &str,
    engine_name: &str,
    tail: Option<usize>,
) -> Result<String, Box<dyn std::error::Error>> {
    let registry_guard = RUNTIME_REGISTRY.read().await;
    let registry = registry_guard
        .as_ref()
        .ok_or("Runtime registry not initialized")?;

    let engine = registry
        .get_engine(engine_name)
        .ok_or(format!("Runtime engine '{}' not available", engine_name))?;

    let logs = engine.get_workload_logs(workload_id, tail).await?;
    Ok(logs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_runtime_registry_initialization() {
        let result = initialize_podman_manager(false, false, true).await;
        assert!(result.is_ok());

        let stats = get_runtime_registry_stats().await;
        assert!(!stats.is_empty());

        let engines = list_available_engines().await;
        assert!(!engines.is_empty());
        assert!(engines.contains(&"mock".to_string()));
    }

    #[tokio::test]
    async fn test_runtime_engine_selection() {
        // Initialize registry for testing
        let _ = initialize_podman_manager(false, false, true).await;

        // Test Kubernetes manifest
        let k8s_manifest = r#"
apiVersion: v1
kind: Pod
metadata:
  name: test-pod
spec:
  containers:
  - name: nginx
    image: nginx:latest
"#;
        let engine = select_runtime_engine(k8s_manifest.as_bytes()).await;
        assert!(engine.is_ok());
        // Should select mock engine in test environment
        let engine_name = engine.unwrap();
        assert!(engine_name == "mock" || engine_name == "podman");

        // Test Docker Compose manifest
        let docker_manifest = r#"
version: '3.8'
services:
  web:
    image: nginx:latest
    ports:
      - "80:80"
"#;
        let engine = select_runtime_engine(docker_manifest.as_bytes()).await;
        assert!(engine.is_ok());
        // Should prefer docker for compose files, but fall back to available engines
    }

    #[test]
    fn test_deployment_config_creation() {
        // This would require creating a mock ApplyRequest
        let config = DeploymentConfig::default();
        assert_eq!(config.replicas, 1);
        assert!(config.env.is_empty());
    }
}

//! Workload Integration Module
//!
//! This module provides integration between the existing libp2p apply message handling
//! and the new workload manager system. It updates the apply message handler to use
//! the runtime engines and provider announcement system.

use crate::libp2p_beemesh::behaviour::MyBehaviour;
use crate::provider::{ProviderConfig, ProviderManager};
use crate::resource_verifier::ResourceVerifier;
use crate::runtime::{DeploymentConfig, RuntimeRegistry, create_default_registry};
use base64::Engine;
use libp2p::Swarm;
use libp2p::request_response;
use log::{debug, error, info, warn};
use once_cell::sync::Lazy;
use protocol::machine;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Global runtime registry for all available engines
static RUNTIME_REGISTRY: Lazy<Arc<RwLock<Option<RuntimeRegistry>>>> =
    Lazy::new(|| Arc::new(RwLock::new(None)));

/// Global provider manager for announcements
static PROVIDER_MANAGER: Lazy<Arc<RwLock<Option<ProviderManager>>>> =
    Lazy::new(|| Arc::new(RwLock::new(None)));

/// Global resource verifier for capacity checks
static RESOURCE_VERIFIER: Lazy<Arc<ResourceVerifier>> =
    Lazy::new(|| Arc::new(ResourceVerifier::new()));

/// Node-local cache mapping manifest IDs to owner public keys.
static MANIFEST_OWNER_MAP: Lazy<RwLock<HashMap<String, Vec<u8>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

/// Initialize the runtime registry and provider manager
pub async fn initialize_workload_manager(
    force_mock_runtime: bool,
    mock_only_runtime: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("Initializing runtime registry and provider manager for manifest deployment");

    // Create runtime registry - use mock-only for tests if environment variable is set
    let use_mock_registry = force_mock_runtime || mock_only_runtime;
    let registry = if use_mock_registry {
        info!("Using mock-only runtime registry for testing");
        crate::runtime::create_mock_only_registry().await
    } else {
        create_default_registry().await
    };
    let available_engines = registry.check_available_engines().await;

    info!("Available runtime engines: {:?}", available_engines);

    // Store the registry globally
    {
        let mut global_registry = RUNTIME_REGISTRY.write().await;
        *global_registry = Some(registry);
    }

    // Create provider manager
    let provider_config = ProviderConfig {
        default_ttl_seconds: 3600, // 1 hour
        ..Default::default()
    };
    let provider_manager = ProviderManager::new(provider_config);

    {
        let mut global_provider_manager = PROVIDER_MANAGER.write().await;
        *global_provider_manager = Some(provider_manager);
    }

    // Initialize resource verifier with system resources
    info!("Initializing resource verifier");
    let verifier = get_global_resource_verifier();
    if let Err(e) = verifier.update_system_resources().await {
        warn!("Failed to update system resources: {}", e);
    }

    info!("Runtime registry and provider manager initialized successfully");
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

/// Get access to the global resource verifier
pub fn get_global_resource_verifier() -> Arc<ResourceVerifier> {
    Arc::clone(&RESOURCE_VERIFIER)
}

/// Enhanced apply message handler that uses the workload manager
pub async fn handle_apply_message_with_workload_manager(
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

            // Verify request as a FlatBuffer Envelope
            let (effective_request, owner_pubkey) =
                match crate::libp2p_beemesh::security::verify_envelope_and_check_nonce_for_peer(
                    &request,
                    &peer.to_string(),
                ) {
                    Ok((payload_bytes, pubkey, _sig)) => (payload_bytes, pubkey),
                    Err(e) => {
                        if crate::libp2p_beemesh::security::require_signed_messages() {
                            error!("Rejecting unsigned/invalid apply request: {:?}", e);
                            let error_response = machine::build_apply_response(
                                false,
                                "unknown",
                                "unsigned or invalid envelope",
                            );
                            let _ = swarm
                                .behaviour_mut()
                                .apply_rr
                                .send_response(channel, error_response);
                            return;
                        }
                        warn!(
                            "Accepting unsigned apply request from peer={} due to relaxed policy",
                            peer
                        );
                        (request.clone(), Vec::new())
                    }
                };

            // Parse the FlatBuffer apply request
            match machine::root_as_apply_request(&effective_request) {
                Ok(apply_req) => {
                    info!(
                        "Apply request - operation_id={:?} replicas={}",
                        apply_req.operation_id(),
                        apply_req.replicas()
                    );

                    // Extract and validate manifest
                    if let Some(manifest_json) = apply_req.manifest_json() {
                        match process_manifest_deployment(
                            swarm,
                            &apply_req,
                            manifest_json,
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
    apply_req: &machine::ApplyRequest<'_>,
    manifest_json: &str,
    owner_pubkey: &[u8],
) -> Result<String, Box<dyn std::error::Error>> {
    info!("Processing manifest deployment with encrypted envelope");

    // Decrypt the manifest content first
    let manifest_content = decrypt_manifest_content(manifest_json, "temp").await?;

    // Use the manifest_id from the apply request for provider announcements
    // This ensures consistency between apply and delete operations
    let manifest_id = apply_req.manifest_id().unwrap_or("unknown").to_string();

    info!(
        "Processing manifest deployment for manifest_id: {} (calculated from decrypted content)",
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
    // use peer-aware deployment for mock engine
    let workload_info = if engine_name == "mock" {
        // For mock engine, use the peer-aware deployment method if available
        if let Some(mock_engine) = engine
            .as_any()
            .downcast_ref::<crate::runtime::mock::MockEngine>()
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
            // Fallback to regular deployment
            engine
                .deploy_workload(&manifest_id, &modified_manifest_content, &deployment_config)
                .await?
        }
    } else {
        // For other engines, use regular deployment
        engine
            .deploy_workload(&manifest_id, &modified_manifest_content, &deployment_config)
            .await?
    };

    info!(
        "Workload deployed successfully: {} using engine '{}', status: {:?}",
        workload_info.id, engine_name, workload_info.status
    );

    // Announce as provider if deployment successful
    if let Some(provider_manager) = PROVIDER_MANAGER.read().await.as_ref() {
        let mut metadata = HashMap::new();
        metadata.insert("runtime_engine".to_string(), engine_name.clone());
        metadata.insert("workload_id".to_string(), workload_info.id.clone());
        metadata.insert("node_type".to_string(), "beemesh-machine".to_string());

        if let Err(e) = provider_manager.announce_provider(swarm, &manifest_id, metadata) {
            warn!(
                "Failed to announce as provider for manifest {}: {}",
                manifest_id, e
            );
        } else {
            info!(
                "Announced as provider for manifest_id: {} using engine '{}'",
                manifest_id, engine_name
            );
        }
    }

    Ok(workload_info.id)
}

/// Modify the manifest to set replicas=1 for single-node deployment
/// Each node in the cluster will deploy one replica of the workload
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

/// Decrypt manifest content from the encrypted envelope
async fn decrypt_manifest_content(
    manifest_json: &str,
    manifest_id: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    debug!(
        "Decrypting manifest content for manifest_id: {}",
        manifest_id
    );

    // Decode base64-encoded envelope
    let envelope_bytes = base64::engine::general_purpose::STANDARD.decode(manifest_json)?;

    // Parse as flatbuffer envelope
    let envelope = machine::root_as_envelope(&envelope_bytes)?;

    let payload_type = envelope.payload_type().unwrap_or("");
    debug!("Envelope payload type: {}", payload_type);

    if payload_type == "manifest" {
        // Extract encrypted payload from envelope and decrypt directly
        if let Some(payload_vector) = envelope.payload() {
            let payload_bytes = payload_vector.bytes();

            // Attempt to decrypt the manifest using KEM decryption
            match decrypt_manifest_from_envelope(manifest_id, payload_bytes).await {
                Ok(decrypted_content) => {
                    info!(
                        "Successfully decrypted manifest for manifest_id: {}",
                        manifest_id
                    );
                    Ok(decrypted_content.into_bytes())
                }
                Err(e) => {
                    error!(
                        "Failed to decrypt manifest for manifest_id {}: {}",
                        manifest_id, e
                    );
                    Err(format!("decryption failed: {}", e).into())
                }
            }
        } else {
            Err("Missing payload in envelope".into())
        }
    } else {
        // Assume it's plain YAML/JSON manifest
        debug!("Treating as plain manifest content");
        Ok(manifest_json.as_bytes().to_vec())
    }
}

/// Create deployment configuration from apply request
fn create_deployment_config(apply_req: &machine::ApplyRequest) -> DeploymentConfig {
    let mut config = DeploymentConfig::default();

    // Set replicas
    config.replicas = apply_req.replicas();

    // Metadata from apply request can be added here if needed
    if let Some(operation_id) = apply_req.operation_id() {
        config
            .env
            .insert("BEEMESH_OPERATION_ID".to_string(), operation_id.to_string());
    }

    config
}

/// Decrypt manifest directly from envelope payload using KEM
async fn decrypt_manifest_from_envelope(
    manifest_id: &str,
    payload_bytes: &[u8],
) -> Result<String, Box<dyn std::error::Error>> {
    debug!(
        "Attempting to decrypt manifest from envelope payload for manifest_id: {}",
        manifest_id
    );

    // Validate recipient-blob version byte
    if payload_bytes.is_empty() || payload_bytes[0] != 0x02 {
        return Err(
            "unsupported payload format: expected recipient-blob (version byte 0x02)".into(),
        );
    }

    // Use the node's KEM private key to decapsulate and decrypt the recipient-blob
    let (_pub_bytes, priv_bytes) = crypto::ensure_kem_keypair_on_disk()
        .map_err(|e| format!("failed to load KEM keypair: {}", e))?;

    let plaintext = crypto::decrypt_payload_from_recipient_blob(payload_bytes, &priv_bytes)
        .map_err(|e| format!("recipient-blob decryption failed: {}", e))?;

    // Interpret plaintext as UTF-8 manifest content (YAML/JSON)
    let manifest_str = String::from_utf8(plaintext)
        .map_err(|e| format!("decrypted manifest is not valid UTF-8: {}", e))?;

    debug!(
        "Successfully decrypted manifest from envelope (len={})",
        manifest_str.len()
    );

    Ok(manifest_str)
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
                apply_req.operation_id(),
                apply_req.replicas()
            );

            if let Some(manifest_json) = apply_req.manifest_json() {
                let owner_pubkey =
                    crypto::keypair_manager::KeypairManager::get_default_signing_keypair()
                        .map(|(pub_bytes, _)| pub_bytes)
                        .unwrap_or_default();

                match process_manifest_deployment(swarm, &apply_req, manifest_json, &owner_pubkey)
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

    // Also withdraw provider announcement if we were providing this manifest
    if let Some(provider_manager) = PROVIDER_MANAGER.read().await.as_ref() {
        if let Err(e) = provider_manager.stop_providing(manifest_id) {
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

/// Discover providers for a manifest
pub async fn discover_manifest_providers(
    swarm: &mut Swarm<MyBehaviour>,
    manifest_id: &str,
) -> Result<Vec<crate::provider::ProviderInfo>, Box<dyn std::error::Error>> {
    if let Some(provider_manager) = PROVIDER_MANAGER.read().await.as_ref() {
        let providers = provider_manager
            .discover_providers(swarm, manifest_id)
            .await?;
        Ok(providers)
    } else {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_runtime_registry_initialization() {
        let result = initialize_workload_manager(false, false).await;
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
        let _ = initialize_workload_manager(false, false).await;

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

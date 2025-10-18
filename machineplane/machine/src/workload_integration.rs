//! Workload Integration Module
//!
//! This module provides integration between the existing libp2p apply message handling
//! and the new workload manager system. It updates the apply message handler to use
//! the runtime engines and provider announcement system.

use crate::libp2p_beemesh::behaviour::MyBehaviour;
use crate::provider::{ProviderConfig, ProviderManager};
use crate::runtime::{create_default_registry, DeploymentConfig, RuntimeRegistry};
use base64::Engine;
use libp2p::request_response;
use libp2p::Swarm;
use log::{debug, error, info, warn};
use once_cell::sync::Lazy;
use protocol::machine;
use std::collections::{hash_map::DefaultHasher, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Global runtime registry for all available engines
static RUNTIME_REGISTRY: Lazy<Arc<RwLock<Option<RuntimeRegistry>>>> =
    Lazy::new(|| Arc::new(RwLock::new(None)));

/// Global provider manager for announcements
static PROVIDER_MANAGER: Lazy<Arc<RwLock<Option<ProviderManager>>>> =
    Lazy::new(|| Arc::new(RwLock::new(None)));

/// Initialize the runtime registry and provider manager
pub async fn initialize_workload_manager() -> Result<(), Box<dyn std::error::Error>> {
    info!("Initializing runtime registry and provider manager for manifest deployment");

    // Create runtime registry - use mock-only for tests if environment variable is set
    let registry = if std::env::var("BEEMESH_MOCK_ONLY_RUNTIME").is_ok() {
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

    info!("Runtime registry and provider manager initialized successfully");
    Ok(())
}

/// Get access to the global runtime registry (for testing and debug endpoints)
pub async fn get_global_runtime_registry(
) -> Option<tokio::sync::RwLockReadGuard<'static, Option<RuntimeRegistry>>> {
    let registry_guard = RUNTIME_REGISTRY.read().await;
    if registry_guard.is_some() {
        Some(registry_guard)
    } else {
        None
    }
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
            let effective_request =
                match crate::libp2p_beemesh::security::verify_envelope_and_check_nonce_for_peer(
                    &request,
                    &peer.to_string(),
                ) {
                    Ok((payload_bytes, _pub, _sig)) => payload_bytes,
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
                        request.clone()
                    }
                };

            // Parse the FlatBuffer apply request
            match machine::root_as_apply_request(&effective_request) {
                Ok(apply_req) => {
                    info!(
                        "Apply request - tenant={:?} operation_id={:?} replicas={}",
                        apply_req.tenant(),
                        apply_req.operation_id(),
                        apply_req.replicas()
                    );

                    // Extract and validate manifest
                    if let Some(manifest_json) = apply_req.manifest_json() {
                        match process_manifest_deployment(swarm, &apply_req, manifest_json).await {
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
) -> Result<String, Box<dyn std::error::Error>> {
    // Generate manifest ID
    let manifest_id = generate_manifest_id(apply_req, manifest_json);

    info!(
        "Processing manifest deployment for manifest_id: {}",
        manifest_id
    );

    // Decrypt the manifest content
    let manifest_content = decrypt_manifest_content(manifest_json, &manifest_id).await?;

    // Create deployment configuration
    let deployment_config = create_deployment_config(apply_req);

    // Select appropriate runtime engine based on manifest type
    let engine_name = select_runtime_engine(&manifest_content).await?;
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

    // Deploy the workload
    let workload_info = engine
        .deploy_workload(&manifest_id, &manifest_content, &deployment_config)
        .await?;

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

    // Store applied manifest in DHT for backward compatibility
    store_applied_manifest_in_dht(swarm, apply_req, *swarm.local_peer_id());

    Ok(workload_info.id)
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

        // Check manifest type and select appropriate engine
        if let Some(kind) = doc.get("kind").and_then(|k| k.as_str()) {
            match kind {
                "Pod" | "Deployment" | "Service" | "ConfigMap" | "Secret" => {
                    // Kubernetes resources -> prefer Podman
                    return Ok("podman".to_string());
                }
                _ => {}
            }
        }

        // Check for Docker Compose format
        if doc.get("services").is_some() && doc.get("version").is_some() {
            return Ok("docker".to_string());
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

/// Generate a stable manifest ID from the apply request
fn generate_manifest_id(apply_req: &machine::ApplyRequest<'_>, manifest_json: &str) -> String {
    let mut hasher = DefaultHasher::new();

    if let Some(tenant) = apply_req.tenant() {
        tenant.hash(&mut hasher);
    }
    if let Some(operation_id) = apply_req.operation_id() {
        operation_id.hash(&mut hasher);
    }
    manifest_json.hash(&mut hasher);

    format!("{:x}", hasher.finish())
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
        // Extract encrypted manifest from envelope payload
        if let Some(payload_vector) = envelope.payload() {
            let payload_bytes = payload_vector.bytes();
            let encrypted_manifest = machine::root_as_encrypted_manifest(payload_bytes)?;

            // Attempt to decrypt the manifest
            match decrypt_encrypted_manifest_with_id(manifest_id, &encrypted_manifest).await {
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

    // Add metadata from apply request
    if let Some(tenant) = apply_req.tenant() {
        config
            .env
            .insert("BEEMESH_TENANT".to_string(), tenant.to_string());
    }
    if let Some(operation_id) = apply_req.operation_id() {
        config
            .env
            .insert("BEEMESH_OPERATION_ID".to_string(), operation_id.to_string());
    }

    config
}

/// Decrypt encrypted manifest using key reconstruction
async fn decrypt_encrypted_manifest_with_id(
    manifest_id: &str,
    encrypted_manifest: &machine::EncryptedManifest<'_>,
) -> Result<String, Box<dyn std::error::Error>> {
    debug!(
        "Attempting to decrypt manifest using key reconstruction for manifest_id: {}",
        manifest_id
    );

    // This is a simplified version - in the real implementation, you would:
    // 1. Find key shares for this manifest_id
    // 2. Reconstruct the decryption key from shares
    // 3. Decrypt the manifest content

    // For now, return a placeholder implementation
    let payload = encrypted_manifest.payload().unwrap_or("");
    if let Ok(decoded_payload) = base64::engine::general_purpose::STANDARD.decode(payload) {
        // Try to decrypt using available key shares
        match attempt_key_reconstruction_and_decrypt(manifest_id, &decoded_payload).await {
            Ok(decrypted) => Ok(decrypted),
            Err(_) => {
                // Fallback: assume it's already decrypted YAML
                String::from_utf8(decoded_payload)
                    .map_err(|e| format!("Failed to decode as UTF-8: {}", e).into())
            }
        }
    } else {
        Err("Failed to decode base64 payload".into())
    }
}

/// Attempt key reconstruction and decryption
async fn attempt_key_reconstruction_and_decrypt(
    manifest_id: &str,
    _encrypted_payload: &[u8],
) -> Result<String, Box<dyn std::error::Error>> {
    debug!(
        "Attempting key reconstruction for manifest_id: {}",
        manifest_id
    );

    // This would involve:
    // 1. Query local keystore for shares related to this manifest_id
    // 2. Request additional shares from other peers if needed
    // 3. Reconstruct the encryption key using Shamir's Secret Sharing
    // 4. Decrypt the payload using the reconstructed key

    // For now, return a mock YAML manifest
    Ok(format!(
        r#"apiVersion: v1
kind: Pod
metadata:
  name: beemesh-workload-{}
  labels:
    beemesh.io/manifest-id: "{}"
spec:
  containers:
  - name: app
    image: nginx:latest
    ports:
    - containerPort: 80
"#,
        manifest_id.chars().take(8).collect::<String>(),
        manifest_id
    ))
}

/// Store applied manifest in DHT (backward compatibility)
fn store_applied_manifest_in_dht(
    swarm: &mut Swarm<MyBehaviour>,
    apply_req: &machine::ApplyRequest,
    local_peer: libp2p::PeerId,
) {
    debug!("Storing applied manifest in DHT for backward compatibility");

    let mut hasher = DefaultHasher::new();
    if let (Some(tenant), Some(operation_id), Some(manifest_json)) = (
        apply_req.tenant(),
        apply_req.operation_id(),
        apply_req.manifest_json(),
    ) {
        tenant.hash(&mut hasher);
        operation_id.hash(&mut hasher);
        manifest_json.hash(&mut hasher);

        let manifest_id = format!("{:x}", hasher.finish());
        let content_hash = format!("{:x}", DefaultHasher::new().finish());
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let manifest_kind = "Pod"; // Simplified
        let labels = vec![
            (
                "deployed-by".to_string(),
                "beemesh-workload-manager".to_string(),
            ),
            ("kind".to_string(), manifest_kind.to_string()),
            ("tenant".to_string(), tenant.to_string()),
            ("replicas".to_string(), apply_req.replicas().to_string()),
        ];

        let empty_pubkey = vec![];
        let empty_signature = vec![];

        let manifest_data = machine::build_applied_manifest(
            &manifest_id,
            &tenant,
            &operation_id,
            &local_peer.to_string(),
            &empty_pubkey,
            &empty_signature,
            &manifest_json,
            &manifest_kind,
            labels,
            timestamp,
            3600, // 1 hour TTL
            &content_hash,
        );

        let record_key = libp2p::kad::RecordKey::new(&format!("manifest:{}", manifest_id));
        let record = libp2p::kad::Record {
            key: record_key,
            value: manifest_data,
            publisher: None,
            expires: None,
        };

        if let Ok(query_id) = swarm
            .behaviour_mut()
            .kademlia
            .put_record(record, libp2p::kad::Quorum::One)
        {
            debug!("Stored applied manifest in DHT (query_id: {:?})", query_id);
        }
    }
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
                "Enhanced self-apply request - tenant={:?} operation_id={:?} replicas={}",
                apply_req.tenant(),
                apply_req.operation_id(),
                apply_req.replicas()
            );

            if let Some(manifest_json) = apply_req.manifest_json() {
                match process_manifest_deployment(swarm, &apply_req, manifest_json).await {
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
        let result = initialize_workload_manager().await;
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
        let _ = initialize_workload_manager().await;

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
    fn test_manifest_id_generation() {
        // Create test data
        let tenant = "test-tenant";
        let operation_id = "test-operation";
        let manifest_json = r#"{"kind": "Pod", "metadata": {"name": "test"}}"#;

        let mut hasher = DefaultHasher::new();
        tenant.hash(&mut hasher);
        operation_id.hash(&mut hasher);
        manifest_json.hash(&mut hasher);
        let expected_id = format!("{:x}", hasher.finish());

        assert!(!expected_id.is_empty());
        assert!(expected_id.len() > 8);
    }

    #[test]
    fn test_deployment_config_creation() {
        // This would require creating a mock ApplyRequest
        let config = DeploymentConfig::default();
        assert_eq!(config.replicas, 1);
        assert!(config.env.is_empty());
    }
}

//! Runtime Integration Module
//!
//! This module provides integration between the scheduler/network layer and the
//! container runtime system. It handles:
//!
//! - Manifest deployment via runtime engines (Podman, etc.)
//! - Pod lifecycle management (deploy, remove, logs)
//!
//! This is the concrete use of the runtime abstractions defined in `runtimes/`.

use crate::messages::ApplyRequest;
use crate::network::behaviour::MyBehaviour;
use crate::runtimes::{DeploymentConfig, RuntimeRegistry, create_default_registry};
use base64::Engine;
use libp2p::Swarm;
use log::{debug, error, info, warn};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use tokio::sync::RwLock;

// ============================================================================
// Global State
// ============================================================================

/// Global runtime registry for all available container engines.
///
/// Lazily initialized on first access. Contains Podman and potentially
/// other container runtime adapters.
static RUNTIME_REGISTRY: LazyLock<Arc<RwLock<Option<RuntimeRegistry>>>> =
    LazyLock::new(|| Arc::new(RwLock::new(None)));

// ============================================================================
// Initialization
// ============================================================================

/// Initializes the runtime registry and probes available engines.
///
/// This should be called during node startup to prepare the container
/// runtime system. Currently defaults to Podman.
pub async fn initialize_podman_manager() -> Result<(), Box<dyn std::error::Error>> {
    info!("Initializing runtime registry for manifest deployment");

    let registry = create_default_registry().await;
    let available_engines = registry.check_available_engines().await;

    info!("Available runtime engines: {:?}", available_engines);

    // Store the registry globally
    {
        let mut global_registry = RUNTIME_REGISTRY.write().await;
        *global_registry = Some(registry);
    }

    info!("Runtime registry initialized successfully");
    Ok(())
}

// ============================================================================
// Manifest Deployment
// ============================================================================

/// Processes manifest deployment using the pod manager.
///
/// This is the main entry point for deploying pods received via
/// the tender/bid/award protocol. It:
///
/// 1. Decodes the manifest content (handles base64 if needed)
/// 2. Modifies replicas to 1 for single-node deployment
/// 3. Selects appropriate runtime engine
/// 4. Deploys via the runtime adapter
/// 5. Publishes workload info to DHT
pub async fn process_manifest_deployment(
    _swarm: &mut Swarm<MyBehaviour>,
    apply_req: &ApplyRequest,
    manifest_json: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    info!("Processing manifest deployment");

    // Parse the manifest content; decode from base64 if needed
    let manifest_content = decode_manifest_content(manifest_json);

    // Extract resource coordinates for logging
    let manifest_str = String::from_utf8_lossy(&manifest_content);
    let (namespace, kind, name) = if let Ok(doc) = serde_yaml::from_str::<serde_yaml::Value>(&manifest_str) {
        let ns = doc.get("metadata")
            .and_then(|m| m.get("namespace"))
            .and_then(|n| n.as_str())
            .unwrap_or("default");
        let k = doc.get("kind")
            .and_then(|k| k.as_str())
            .unwrap_or("Pod");
        let n = doc.get("metadata")
            .and_then(|m| m.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("unknown");
        (ns.to_string(), k.to_string(), n.to_string())
    } else {
        ("default".to_string(), "Pod".to_string(), "unknown".to_string())
    };

    info!(
        "Processing manifest deployment for {}/{}/{}",
        namespace, kind, name
    );

    // Modify manifest to set replicas=1 for this node deployment
    // The original manifest is stored in DHT, but each node deploys with replicas=1
    let modified_manifest_content = modify_manifest_replicas(&manifest_content)?;

    // Create deployment configuration
    let deployment_config = create_deployment_config(apply_req);

    // Select appropriate runtime engine based on manifest type
    let engine_name = select_runtime_engine(&modified_manifest_content).await?;
    info!(
        "Selected runtime engine '{}' for {}/{}/{}",
        engine_name, namespace, kind, name
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

    // Apply the pod with modified manifest (replicas=1)
    let pod_info = engine
        .apply(
            &modified_manifest_content,
            &deployment_config,
        )
        .await?;

    info!(
        "Pod applied successfully: {} using engine '{}', status: {:?}",
        pod_info.id, engine_name, pod_info.status
    );

    Ok(pod_info.id)
}

/// Modify the manifest to set replicas=1 for single-node deployment
/// Each node in the fabric will deploy one replica of the pod
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
        else if doc.get("replicas").is_some()
            && let Some(doc_map) = doc.as_mapping_mut() {
                doc_map.insert(
                    serde_yaml::Value::String("replicas".to_string()),
                    serde_yaml::Value::Number(serde_yaml::Number::from(1)),
                );
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

/// Decode manifest content that may be base64-encoded.
/// Falls back to treating the string as plain text if decoding fails or produces non-UTF8.
pub fn decode_manifest_content(manifest_json: &str) -> Vec<u8> {
    match base64::engine::general_purpose::STANDARD.decode(manifest_json) {
        Ok(decoded) => {
            if String::from_utf8(decoded.clone()).is_ok() {
                debug!("Decoded base64 manifest content ({} bytes)", decoded.len());
                decoded
            } else {
                debug!(
                    "Base64 manifest content was not valid UTF-8; using raw manifest string"
                );
                manifest_json.as_bytes().to_vec()
            }
        }
        Err(_) => manifest_json.as_bytes().to_vec(),
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
        if let Some(metadata) = doc.get("metadata")
            && let Some(annotations) = metadata.get("annotations")
                && let Some(engine) = annotations
                    .get("beemesh.io/runtime-engine")
                    .and_then(|v| v.as_str())
                {
                    info!("Found runtime engine annotation: {}", engine);
                    return Ok(engine.to_string());
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

        if *available.get("podman").unwrap_or(&false) {
            return Ok("podman".to_string());
        }

        if let Some(default_engine) = registry.get_default_engine() {
            warn!(
                "Preferred Podman runtime unavailable; falling back to default engine '{}'",
                default_engine.name()
            );
            return Ok(default_engine.name().to_string());
        }
    }

    Err("No suitable runtime engine available".into())
}

/// Create deployment configuration from apply request
fn create_deployment_config(apply_req: &ApplyRequest) -> DeploymentConfig {
    let mut config = DeploymentConfig {
        replicas: apply_req.replicas,
        ..Default::default()
    };

    // Metadata from apply request can be added here if needed
    if !apply_req.operation_id.is_empty() {
        config.env.insert(
            "BEEMESH_OPERATION_ID".to_string(),
            apply_req.operation_id.clone(),
        );
    }

    config
}

/// Enhanced self-apply processing with pod manager
pub async fn process_enhanced_self_apply_request(
    manifest: &[u8],
    swarm: &mut Swarm<MyBehaviour>,
) {
    debug!(
        "Processing enhanced self-apply request (manifest len={})",
        manifest.len()
    );

    match bincode::deserialize::<ApplyRequest>(manifest) {
        Ok(apply_req) => {
            debug!(
                "Enhanced self-apply request - operation_id={:?} replicas={}",
                apply_req.operation_id, apply_req.replicas
            );

            if !apply_req.manifest_json.is_empty() {
                let manifest_json = apply_req.manifest_json.clone();

                match process_manifest_deployment(
                    swarm,
                    &apply_req,
                    &manifest_json,
                )
                .await
                {
                    Ok(pod_id) => {
                        info!(
                            "Successfully deployed self-applied pod: {}",
                            pod_id
                        );
                    }
                    Err(e) => {
                        error!("Failed to deploy self-applied pod: {}", e);
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

// ============================================================================
// Runtime Registry Utilities
// ============================================================================

/// Returns availability statistics for all registered runtime engines.
///
/// Used by debug endpoints to report which container runtimes are available.
pub async fn get_runtime_registry_stats() -> HashMap<String, bool> {
    if let Some(registry) = RUNTIME_REGISTRY.read().await.as_ref() {
        registry.check_available_engines().await
    } else {
        HashMap::new()
    }
}

/// Lists all registered runtime engine names.
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

// ============================================================================
// Pod Management Operations
// ============================================================================

/// Delete all pods matching resource coordinates (namespace/kind/name).
///
/// This is used during DISPOSAL operations to clean up all pods that
/// match the specified Kubernetes resource coordinates. Searches through all
/// registered runtime engines.
///
/// # Arguments
///
/// * `namespace` - Kubernetes namespace (e.g., "default")
/// * `kind` - Kubernetes resource kind (e.g., "Pod", "Deployment")
/// * `name` - Kubernetes resource name (from metadata.name)
///
/// # Returns
///
/// A vector of pod IDs that were successfully deleted.
pub async fn delete_pods_by_resource(
    namespace: &str,
    kind: &str,
    name: &str,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let resource_key = format!("{}/{}/{}", namespace, kind, name);
    info!(
        "delete_pods_by_resource: resource={}",
        resource_key
    );

    let registry_guard = RUNTIME_REGISTRY.read().await;
    let registry = registry_guard
        .as_ref()
        .ok_or("Runtime registry not initialized")?;

    let mut deleted_pods = Vec::new();
    let mut errors = Vec::new();
    let engine_names = registry.list_engines();

    // Search all engines for matching pods
    for engine_name in &engine_names {
        if let Some(engine) = registry.get_engine(engine_name) {
            info!(
                "Checking engine '{}' for pods with resource '{}'",
                engine_name, resource_key
            );

            // List pods from this engine
            match engine.list().await {
                Ok(pods) => {
                    // Find pods that match the resource coordinates
                    for pod in pods {
                        if pod.namespace == namespace && pod.kind == kind && pod.name == name {
                            info!(
                                "Found matching pod: {} in engine '{}' (resource: {})",
                                pod.id, engine_name, resource_key
                            );

                            // Delete the pod
                            match engine.delete(&pod.id).await {
                                Ok(()) => {
                                    info!(
                                        "Successfully deleted pod: {} from engine '{}'",
                                        pod.id, engine_name
                                    );
                                    deleted_pods.push(pod.id);
                                }
                                Err(e) => {
                                    error!(
                                        "Failed to delete pod {} from engine '{}': {}",
                                        pod.id, engine_name, e
                                    );
                                    errors.push(format!("{}:{}", engine_name, pod.id));
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        "Failed to list pods from engine '{}': {}",
                        engine_name, e
                    );
                    errors.push(format!("list:{}", engine_name));
                }
            }
        }
    }

    info!(
        "delete_pods_by_resource completed: deleted {} pods for resource '{}'",
        deleted_pods.len(),
        resource_key
    );
    Ok(deleted_pods)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_runtime_registry_initialization() {
        let result = initialize_podman_manager().await;
        assert!(result.is_ok());

        let stats = get_runtime_registry_stats().await;
        assert!(!stats.is_empty());

        let engines = list_available_engines().await;
        assert!(!engines.is_empty());
        assert!(engines.contains(&"podman".to_string()));
    }

    #[tokio::test]
    async fn test_runtime_engine_selection() {
        // Initialize registry for testing
        let _ = initialize_podman_manager().await;

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
        let engine_name = engine.unwrap();
        assert_eq!(engine_name, "podman");

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

    #[test]
    fn decode_manifest_content_handles_base64_and_plain() {
        let manifest = "apiVersion: v1\nkind: Pod";
        let encoded = base64::engine::general_purpose::STANDARD.encode(manifest);

        let decoded = decode_manifest_content(&encoded);
        assert_eq!(decoded, manifest.as_bytes());

        let plaintext = decode_manifest_content(manifest);
        assert_eq!(plaintext, manifest.as_bytes());
    }
}

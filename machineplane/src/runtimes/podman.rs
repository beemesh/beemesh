//! Podman runtime engine implementation
//!
//! # Overview
//!
//! This module implements the `RuntimeEngine` trait using Podman as the underlying container
//! runtime. Podman is currently the **only supported production runtime** for BeeMesh.
//!
//! # Why Podman?
//!
//! Podman is the only container runtime that provides **native Kubernetes Pod support**
//! without requiring Kubernetes. Key features:
//!
//! ## Native Pod Support via `podman kube play`
//!
//! - **Direct Kubernetes YAML execution**: Can run `podman kube play manifest.yaml`
//! - **Infra containers**: Automatically creates pause/infra containers to hold pod namespaces
//! - **Shared network namespace**: All containers in a pod share the same IP and can
//!   communicate via localhost (essential for sidecars)
//! - **Shared volumes**: Containers can mount the same volumes for data sharing
//! - **Atomic lifecycle**: Pod containers start/stop as a unit
//!
//! ## Rootless Mode
//!
//! - Can run without root privileges for improved security
//! - User namespaces for isolation
//! - Compatible with BeeMesh's security model
//!
//! ## Remote API Support
//!
//! - Uses the Podman REST API over Unix domain sockets for efficient communication
//! - Enables distributed workload execution across nodes
//!
//! # Implementation Details
//!
//! ## Pod Naming Convention
//!
//! - Workload ID: `beemesh-{manifest_id}-{timestamp}`
//! - Pod name: `beemesh-{manifest_id}-{timestamp}` (Podman may add `-pod` suffix)
//! - Timestamp ensures unique pod names for each deployment
//!
//! ## Manifest Modification
//!
//! The engine modifies incoming Kubernetes manifests to:
//! - Set pod name to `beemesh-{manifest_id}-{timestamp}` for tracking
//! - Ensure unique naming across deployments
//! - Enable workload identification and cleanup
//!
//! ## API Communication
//!
//! - Uses the Podman REST API over Unix sockets for all operations
//! - Propagates API errors directly so callers can surface actionable diagnostics
//!
//! # Why Not Docker?
//!
//! Docker was removed because it **does not support Kubernetes Pods**:
//! - Docker only runs individual containers
//! - Docker Compose creates separate containers with separate IPs
//! - No shared network namespace (containers can't use localhost to communicate)
//! - No infra/pause container pattern
//! - Cannot properly run sidecar patterns (service mesh, logging agents)
//!
//! # Example Usage
//!
//! ```rust,no_run
//! use machineplane::runtimes::{RuntimeEngine, DeploymentConfig};
//! use machineplane::runtimes::podman::PodmanEngine;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let engine = PodmanEngine::new();
//!
//! // Check if Podman is available
//! if !engine.is_available().await {
//!     eprintln!("Podman not available!");
//!     return Ok(());
//! }
//!
//! // Deploy a Kubernetes Pod manifest
//! let manifest = br#"
//! apiVersion: v1
//! kind: Pod
//! metadata:
//!   name: nginx
//! spec:
//!   containers:
//!   - name: nginx
//!     image: nginx:latest
//!     ports:
//!     - containerPort: 80
//! "#;
//!
//! let config = DeploymentConfig::default();
//! let workload = engine.deploy_workload("manifest-123", manifest, &config).await?;
//! println!("Deployed workload: {}", workload.id);
//! # Ok(())
//! # }
//! ```

use crate::runtimes::{
    DeploymentConfig, PortMapping, RuntimeEngine, RuntimeError, RuntimeResult, WorkloadInfo,
    WorkloadStatus,
};
use crate::runtimes::podman_api::PodmanApiClient;
use async_trait::async_trait;
use log::{debug, error, info, warn};
use once_cell::sync::Lazy;
use serde_yaml::Value;
use std::collections::HashMap;
use std::sync::RwLock;

/// Podman runtime engine
pub struct PodmanEngine {
    podman_socket: Option<String>,
}

static PODMAN_SOCKET_OVERRIDE: Lazy<RwLock<Option<String>>> = Lazy::new(|| RwLock::new(None));

impl PodmanEngine {
    /// Create a new Podman engine instance
    pub fn new() -> Self {
        Self {
            podman_socket: Self::detect_podman_socket(),
        }
    }

    /// Configure the Podman runtime socket used for Libpod API interactions.
    pub fn configure_runtime(socket: Option<String>) {
        let normalized = socket.and_then(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(Self::normalize_socket(trimmed))
            }
        });

        let mut socket_guard = PODMAN_SOCKET_OVERRIDE
            .write()
            .expect("podman socket override rwlock poisoned");
        *socket_guard = normalized;
    }

    fn socket_override() -> Option<String> {
        PODMAN_SOCKET_OVERRIDE
            .read()
            .expect("podman socket override rwlock poisoned")
            .clone()
    }

    pub fn normalize_socket(value: &str) -> String {
        if value.contains("://") {
            value.to_string()
        } else {
            format!("unix://{}", value)
        }
    }

    /// Detect a podman socket URL from configuration or `CONTAINER_HOST`.
    pub fn detect_podman_socket() -> Option<String> {
        if let Some(socket) = Self::socket_override() {
            return Some(socket);
        }

        if let Ok(value) = std::env::var("CONTAINER_HOST") {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(Self::normalize_socket(trimmed));
            }
        }

        None
    }

    /// Create a REST API client for the configured socket
    fn create_api_client(&self) -> Option<PodmanApiClient> {
        self.podman_socket.as_ref().map(|socket| PodmanApiClient::new(socket))
    }

    fn require_api_client(&self) -> RuntimeResult<PodmanApiClient> {
        self.create_api_client().ok_or_else(|| {
            RuntimeError::EngineNotAvailable(
                "Podman socket not configured; cannot reach Libpod API".to_string(),
            )
        })
    }

    /// Parse Kubernetes manifest to extract metadata
    fn parse_manifest_metadata(
        &self,
        manifest_content: &[u8],
    ) -> RuntimeResult<HashMap<String, String>> {
        let manifest_str = String::from_utf8_lossy(manifest_content);
        let mut metadata = HashMap::new();

        // Try to parse as YAML
        if let Ok(doc) = serde_yaml::from_str::<Value>(&manifest_str) {
            if let Some(kind) = doc.get("kind").and_then(|k| k.as_str()) {
                metadata.insert("kind".to_string(), kind.to_string());
            }
            if let Some(api_version) = doc.get("apiVersion").and_then(|v| v.as_str()) {
                metadata.insert("apiVersion".to_string(), api_version.to_string());
            }
            if let Some(meta) = doc.get("metadata") {
                if let Some(name) = meta.get("name").and_then(|n| n.as_str()) {
                    metadata.insert("name".to_string(), name.to_string());
                }
                if let Some(namespace) = meta.get("namespace").and_then(|n| n.as_str()) {
                    metadata.insert("namespace".to_string(), namespace.to_string());
                }
            }
        }

        Ok(metadata)
    }

    /// Extract port mappings from pod info
    async fn extract_port_mappings(&self, _pod_name: &str) -> RuntimeResult<Vec<PortMapping>> {
        // Port extraction would require inspecting the pod via API
        // For now, return empty - this matches the original behavior
        Ok(Vec::new())
    }

    /// Generate a unique workload ID based on manifest ID and timestamp
    fn generate_workload_id(&self, manifest_id: &str, _manifest_content: &[u8]) -> String {
        // Use naming with timestamp for uniqueness: beemesh-{manifest_id}-{timestamp}
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        format!("beemesh-{}-{}", manifest_id, timestamp)
    }

    /// Modify the manifest to set the pod name to our workload ID
    fn modify_manifest_for_deployment(
        &self,
        manifest_content: &[u8],
        workload_id: &str,
    ) -> RuntimeResult<Vec<u8>> {
        let manifest_str = String::from_utf8_lossy(manifest_content);
        let mut doc: serde_yaml::Value = serde_yaml::from_str(&manifest_str)
            .map_err(|e| RuntimeError::InvalidManifest(format!("YAML parse error: {}", e)))?;

        // Use the workload_id as the pod name (already includes beemesh-{manifest_id}-{timestamp})
        let pod_name = workload_id;

        // Update metadata name to use our generated pod name
        if let Some(metadata) = doc.get_mut("metadata") {
            if let Some(metadata_map) = metadata.as_mapping_mut() {
                metadata_map.insert(
                    serde_yaml::Value::String("name".to_string()),
                    serde_yaml::Value::String(pod_name.to_string()),
                );
            }
        }

        // For Deployments, also update the pod template metadata name if it exists
        if let Some(spec) = doc.get_mut("spec") {
            if let Some(template) = spec.get_mut("template") {
                if let Some(template_metadata) = template.get_mut("metadata") {
                    if let Some(template_metadata_map) = template_metadata.as_mapping_mut() {
                        template_metadata_map.insert(
                            serde_yaml::Value::String("name".to_string()),
                            serde_yaml::Value::String(format!("{}-pod", pod_name)),
                        );
                    }
                }
            }
        }

        // Serialize back to YAML
        let modified_manifest = serde_yaml::to_string(&doc)
            .map_err(|e| RuntimeError::InvalidManifest(format!("YAML serialize error: {}", e)))?;

        debug!("Modified manifest for deployment with pod name: {}", pod_name);
        Ok(modified_manifest.into_bytes())
    }

    /// Deploy using REST API
    async fn deploy_via_api(
        &self,
        client: &PodmanApiClient,
        manifest_content: &[u8],
    ) -> RuntimeResult<()> {
        debug!("Deploying workload via Podman REST API");
        
        let response = client.play_kube(manifest_content, true).await?;
        
        // Check if any pods were created
        if response.pods.is_empty() {
            return Err(RuntimeError::DeploymentFailed(
                "No pods created from manifest".to_string(),
            ));
        }

        // Log pod IDs for debugging
        for pod in &response.pods {
            if let Some(id) = &pod.id {
                debug!("Created pod with ID: {}", id);
            }
        }

        Ok(())
    }

    /// List pods using REST API
    async fn list_pods_via_api(
        &self,
        client: &PodmanApiClient,
    ) -> RuntimeResult<Vec<WorkloadInfo>> {
        debug!("Listing pods via Podman REST API");
        
        let pods = client.list_pods().await?;
        let mut workloads = Vec::new();

        for pod in pods {
            if let Some(pod_name) = &pod.name {
                // Only include pods that match our naming convention "beemesh-*"
                if pod_name.starts_with("beemesh-") {
                    // Extract manifest_id from pod name (format: beemesh-{manifest_id}-{timestamp})
                    // First strip the "beemesh-" prefix, then handle optional "-pod" suffix
                    let name_without_prefix = pod_name.strip_prefix("beemesh-").unwrap_or(pod_name);
                    let name_without_suffix = if name_without_prefix.ends_with("-pod") {
                        name_without_prefix.strip_suffix("-pod").unwrap_or(name_without_prefix)
                    } else {
                        name_without_prefix
                    };
                    
                    // The manifest_id is everything except the last segment (timestamp)
                    // Format: {manifest_id}-{timestamp}
                    let manifest_id = if let Some(last_hyphen) = name_without_suffix.rfind('-') {
                        // Check if the part after the last hyphen looks like a timestamp (all digits)
                        let potential_timestamp = &name_without_suffix[last_hyphen + 1..];
                        if potential_timestamp.chars().all(|c| c.is_ascii_digit()) {
                            name_without_suffix[..last_hyphen].to_string()
                        } else {
                            name_without_suffix.to_string()
                        }
                    } else {
                        name_without_suffix.to_string()
                    };

                    // Parse pod status
                    let status = match pod.status.as_deref() {
                        Some("Running") => WorkloadStatus::Running,
                        Some("Stopped") | Some("Exited") => WorkloadStatus::Stopped,
                        Some("Error") => WorkloadStatus::Failed("Pod in error state".to_string()),
                        Some("Failed") => WorkloadStatus::Failed("Pod failed".to_string()),
                        _ => WorkloadStatus::Unknown,
                    };

                    // Extract metadata from pod labels if available
                    let metadata = pod.labels.clone().unwrap_or_default();

                    // Use the full pod name (without -pod suffix) as the workload id
                    let workload_id = if pod_name.ends_with("-pod") {
                        pod_name.strip_suffix("-pod").unwrap_or(pod_name).to_string()
                    } else {
                        pod_name.to_string()
                    };

                    let workload_info = WorkloadInfo {
                        id: workload_id,
                        manifest_id,
                        status,
                        metadata,
                        created_at: std::time::SystemTime::now(),
                        updated_at: std::time::SystemTime::now(),
                        ports: Vec::new(),
                    };

                    debug!("Found beemesh workload: {} (pod: {})", workload_info.id, pod_name);
                    workloads.push(workload_info);
                }
            }
        }

        debug!("Found {} workloads via API", workloads.len());
        Ok(workloads)
    }

    /// Remove pod using REST API
    async fn remove_pod_via_api(
        &self,
        client: &PodmanApiClient,
        workload_id: &str,
    ) -> RuntimeResult<()> {
        debug!("Removing pod via Podman REST API: {}", workload_id);

        // Try with -pod suffix first (most common)
        let pod_name_with_suffix = format!("{}-pod", workload_id);
        if client.remove_pod(&pod_name_with_suffix, true).await.is_ok() {
            info!("Successfully removed pod: {}", pod_name_with_suffix);
            return Ok(());
        }

        // Try exact name
        if client.remove_pod(workload_id, true).await.is_ok() {
            info!("Successfully removed pod: {}", workload_id);
            return Ok(());
        }

        // List pods to find matches
        let pods = client.list_pods().await?;
        for pod in pods {
            if let Some(name) = &pod.name {
                if name.contains(workload_id) {
                    if let Err(e) = client.remove_pod(name, true).await {
                        warn!("Failed to remove pod {}: {}", name, e);
                    } else {
                        info!("Successfully removed pod: {}", name);
                    }
                }
            }
        }

        Ok(())
    }

    /// Generate kube manifest using REST API
    async fn generate_kube_via_api(
        &self,
        client: &PodmanApiClient,
        workload_id: &str,
    ) -> RuntimeResult<Vec<u8>> {
        debug!("Generating kube manifest via REST API for: {}", workload_id);

        // Try with -pod suffix first
        let pod_name_with_suffix = format!("{}-pod", workload_id);
        if let Ok(manifest) = client.generate_kube(&pod_name_with_suffix).await {
            info!("Successfully generated manifest for pod: {}", pod_name_with_suffix);
            return Ok(manifest.into_bytes());
        }

        // Try exact name
        if let Ok(manifest) = client.generate_kube(workload_id).await {
            info!("Successfully generated manifest for pod: {}", workload_id);
            return Ok(manifest.into_bytes());
        }

        // List pods to find matches
        let pods = client.list_pods().await?;
        for pod in pods {
            if let Some(name) = &pod.name {
                if name.contains(workload_id) {
                    if let Ok(manifest) = client.generate_kube(name).await {
                        info!("Successfully generated manifest for pod: {}", name);
                        return Ok(manifest.into_bytes());
                    }
                }
            }
        }

        Err(RuntimeError::WorkloadNotFound(format!(
            "No running pod found for workload {}",
            workload_id
        )))
    }

    /// Get pod logs using REST API
    async fn get_logs_via_api(
        &self,
        client: &PodmanApiClient,
        workload_id: &str,
        tail: Option<usize>,
    ) -> RuntimeResult<String> {
        debug!("Getting logs via REST API for: {}", workload_id);

        let pod_name_with_suffix = format!("{}-pod", workload_id);
        let candidates = [pod_name_with_suffix.as_str(), workload_id];
        let mut last_error: Option<RuntimeError> = None;

        for candidate in candidates {
            match client.get_pod_logs(candidate, tail).await {
                Ok(logs) => return Ok(logs),
                Err(err) => last_error = Some(err),
            }
        }

        Err(last_error.unwrap_or_else(|| {
            RuntimeError::WorkloadNotFound(format!(
                "No logs available for workload {}",
                workload_id
            ))
        }))
    }
}

impl Default for PodmanEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl RuntimeEngine for PodmanEngine {
    fn name(&self) -> &str {
        "podman"
    }

    async fn is_available(&self) -> bool {
        let Some(client) = self.create_api_client() else {
            debug!("Podman socket not configured; runtime unavailable");
            return false;
        };

        match client.check_availability().await {
            Ok(info) => {
                if let Some(version) = info.version {
                    debug!("Podman API available, version: {:?}", version.version);
                }
                true
            }
            Err(e) => {
                error!("Podman REST API unavailable: {}", e);
                false
            }
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn validate_manifest(&self, manifest_content: &[u8]) -> RuntimeResult<()> {
        let manifest_str = String::from_utf8_lossy(manifest_content);

        // Basic YAML validation
        match serde_yaml::from_str::<Value>(&manifest_str) {
            Ok(doc) => {
                // Check for required Kubernetes fields
                if doc.get("apiVersion").is_none() {
                    return Err(RuntimeError::InvalidManifest(
                        "Missing apiVersion field".to_string(),
                    ));
                }
                if doc.get("kind").is_none() {
                    return Err(RuntimeError::InvalidManifest(
                        "Missing kind field".to_string(),
                    ));
                }

                info!("Manifest validation passed");
                Ok(())
            }
            Err(e) => Err(RuntimeError::InvalidManifest(format!(
                "YAML parse error: {}",
                e
            ))),
        }
    }

    async fn deploy_workload(
        &self,
        manifest_id: &str,
        manifest_content: &[u8],
        _config: &DeploymentConfig,
    ) -> RuntimeResult<WorkloadInfo> {
        info!("Deploying workload for manifest_id: {}", manifest_id);

        // Validate manifest first
        self.validate_manifest(manifest_content).await?;

        // Generate unique workload ID
        let workload_id = self.generate_workload_id(manifest_id, manifest_content);

        // Modify manifest for deployment - pass the workload_id so pod name matches
        let modified_manifest = self.modify_manifest_for_deployment(manifest_content, &workload_id)?;

        let client = self.require_api_client()?;
        if let Err(e) = self.deploy_via_api(&client, &modified_manifest).await {
            error!("Podman REST API deployment failed: {}", e);
            return Err(e);
        }
        info!("Workload deployed via REST API: {}", workload_id);

        // Parse manifest metadata
        let metadata = self
            .parse_manifest_metadata(manifest_content)
            .unwrap_or_default();

        // Get port mappings - use workload_id which matches the pod name
        let ports = self.extract_port_mappings(&workload_id).await.unwrap_or_default();

        let now = std::time::SystemTime::now();
        Ok(WorkloadInfo {
            id: workload_id,
            manifest_id: manifest_id.to_string(),
            status: WorkloadStatus::Running,
            metadata,
            created_at: now,
            updated_at: now,
            ports,
        })
    }

    /// Deploy a workload with local peer ID tracking
    async fn deploy_workload_with_peer(
        &self,
        manifest_id: &str,
        manifest_content: &[u8],
        config: &DeploymentConfig,
        local_peer_id: libp2p::PeerId,
    ) -> RuntimeResult<WorkloadInfo> {
        // Use the base deploy_workload method
        let mut workload_info = self
            .deploy_workload(manifest_id, manifest_content, config)
            .await?;

        // Add local peer ID to metadata
        workload_info
            .metadata
            .insert("local_peer_id".to_string(), local_peer_id.to_string());

        Ok(workload_info)
    }

    async fn get_workload_status(&self, workload_id: &str) -> RuntimeResult<WorkloadInfo> {
        debug!("Getting status for workload: {}", workload_id);

        // List all workloads and find the matching one
        let workloads = self.list_workloads().await?;
        
        for workload in workloads {
            if workload.id == workload_id || workload.manifest_id == workload_id {
                return Ok(workload);
            }
        }

        // Return a basic status if not found
        let now = std::time::SystemTime::now();
        Ok(WorkloadInfo {
            id: workload_id.to_string(),
            manifest_id: "unknown".to_string(),
            status: WorkloadStatus::Unknown,
            metadata: HashMap::new(),
            created_at: now,
            updated_at: now,
            ports: Vec::new(),
        })
    }

    async fn list_workloads(&self) -> RuntimeResult<Vec<WorkloadInfo>> {
        debug!("Listing all workloads");
        let client = self.require_api_client()?;
        self.list_pods_via_api(&client).await
    }

    async fn remove_workload(&self, workload_id: &str) -> RuntimeResult<()> {
        info!("Removing workload: {}", workload_id);
        let client = self.require_api_client()?;
        self.remove_pod_via_api(&client, workload_id).await
    }

    async fn get_workload_logs(
        &self,
        workload_id: &str,
        tail: Option<usize>,
    ) -> RuntimeResult<String> {
        debug!("Getting logs for workload: {}", workload_id);
        let client = self.require_api_client()?;
        self.get_logs_via_api(&client, workload_id, tail).await
    }

    async fn export_manifest(&self, workload_id: &str) -> RuntimeResult<Vec<u8>> {
        info!("Exporting manifest for workload: {}", workload_id);
        let client = self.require_api_client()?;
        self.generate_kube_via_api(&client, workload_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::ffi::OsString;

    struct EnvGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var_os(key);
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(value) = &self.previous {
                unsafe {
                    std::env::set_var(self.key, value);
                }
            } else {
                unsafe {
                    std::env::remove_var(self.key);
                }
            }
        }
    }

    #[serial]
    #[tokio::test]
    async fn test_podman_engine_creation() {
        PodmanEngine::configure_runtime(None);
        let engine = PodmanEngine::new();
        assert_eq!(engine.name(), "podman");
    }

    #[serial]
    #[tokio::test]
    async fn test_manifest_validation() {
        PodmanEngine::configure_runtime(None);
        let engine = PodmanEngine::new();

        // Valid manifest
        let valid_manifest = br#"
apiVersion: v1
kind: Pod
metadata:
  name: test-pod
spec:
  containers:
  - name: nginx
    image: nginx:latest
"#;

        assert!(engine.validate_manifest(valid_manifest).await.is_ok());

        // Invalid manifest (missing apiVersion)
        let invalid_manifest = br#"
kind: Pod
metadata:
  name: test-pod
spec:
  containers:
  - name: nginx
    image: nginx:latest
"#;

        assert!(engine.validate_manifest(invalid_manifest).await.is_err());
    }

    #[serial]
    #[tokio::test]
    async fn test_parse_manifest_metadata() {
        PodmanEngine::configure_runtime(None);
        let engine = PodmanEngine::new();

        let manifest = br#"
apiVersion: v1
kind: Pod
metadata:
  name: test-pod
  namespace: default
spec:
  containers:
  - name: nginx
    image: nginx:latest
"#;

        let metadata = engine.parse_manifest_metadata(manifest).unwrap();
        assert_eq!(metadata.get("kind"), Some(&"Pod".to_string()));
        assert_eq!(metadata.get("apiVersion"), Some(&"v1".to_string()));
        assert_eq!(metadata.get("name"), Some(&"test-pod".to_string()));
        assert_eq!(metadata.get("namespace"), Some(&"default".to_string()));
    }

    #[serial]
    #[tokio::test]
    async fn test_workload_id_generation() {
        PodmanEngine::configure_runtime(None);
        let engine = PodmanEngine::new();

        let manifest1 = b"apiVersion: v1\nkind: Pod\nmetadata:\n  name: test-pod";
        let manifest2 = b"apiVersion: v1\nkind: Pod\nmetadata:\n  name: different-pod";
        let manifest_without_name = b"apiVersion: v1\nkind: Pod";

        let id1 = engine.generate_workload_id("manifest-123", manifest1);
        let id3 = engine.generate_workload_id("manifest-456", manifest2);
        let id4 = engine.generate_workload_id("manifest-789", manifest_without_name);

        // IDs should follow the format beemesh-{manifest_id}-{timestamp}
        assert!(id1.starts_with("beemesh-manifest-123-"));
        assert!(id3.starts_with("beemesh-manifest-456-"));
        assert!(id4.starts_with("beemesh-manifest-789-"));

        // Different manifest IDs should produce different workload IDs (different prefix)
        assert_ne!(id1[..id1.rfind('-').unwrap()], id3[..id3.rfind('-').unwrap()]);

        // IDs should contain a timestamp (numeric suffix after last hyphen)
        let timestamp_suffix = id1.rsplit('-').next().unwrap();
        assert!(timestamp_suffix.chars().all(|c| c.is_ascii_digit()));
    }

    #[serial]
    #[tokio::test]
    async fn detects_podman_socket_from_env() {
        PodmanEngine::configure_runtime(None);

        let _guard = EnvGuard::set("CONTAINER_HOST", "/tmp/env-podman.sock");

        let engine = PodmanEngine::new();
        assert_eq!(
            engine.podman_socket.as_deref(),
            Some("unix:///tmp/env-podman.sock")
        );

        PodmanEngine::configure_runtime(None);
    }
}

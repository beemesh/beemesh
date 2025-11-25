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
//! - Falls back to CLI commands when API is unavailable
//! - Enables distributed workload execution across nodes
//!
//! # Implementation Details
//!
//! ## Pod Naming Convention
//!
//! - Workload ID: `beemesh-{manifest_id}`
//! - Pod name: `beemesh-{manifest_id}` (Podman may add `-pod` suffix)
//! - Ensures consistent identification
//!
//! ## Manifest Modification
//!
//! The engine modifies incoming Kubernetes manifests to:
//! - Set pod name to `beemesh-{manifest_id}` for tracking
//! - Ensure consistent naming across deployments
//! - Enable workload identification and cleanup
//!
//! ## API Communication
//!
//! - **Primary mode**: Uses Podman REST API over Unix socket for efficient async operations
//! - **Fallback mode**: Uses CLI commands when REST API is unavailable
//! - **Auto-fallback**: Switches to CLI if socket connection fails
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
    podman_binary: String,
    podman_socket: Option<String>,
    force_remote: bool,
}

static PODMAN_SOCKET_OVERRIDE: Lazy<RwLock<Option<String>>> = Lazy::new(|| RwLock::new(None));
static PODMAN_FORCE_REMOTE: Lazy<RwLock<bool>> = Lazy::new(|| RwLock::new(false));

impl PodmanEngine {
    /// Create a new Podman engine instance
    pub fn new() -> Self {
        Self {
            podman_binary: "podman".to_string(),
            podman_socket: Self::detect_podman_socket(),
            force_remote: Self::is_force_remote(),
        }
    }

    /// Create a new Podman engine with custom binary path
    pub fn with_binary(binary_path: String) -> Self {
        Self {
            podman_binary: binary_path,
            podman_socket: Self::detect_podman_socket(),
            force_remote: Self::is_force_remote(),
        }
    }

    /// Configure the Podman runtime using CLI-provided parameters.
    pub fn configure_runtime(socket: Option<String>, force_remote: bool) {
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

        let mut remote_guard = PODMAN_FORCE_REMOTE
            .write()
            .expect("podman force remote rwlock poisoned");
        *remote_guard = force_remote;
    }

    fn socket_override() -> Option<String> {
        PODMAN_SOCKET_OVERRIDE
            .read()
            .expect("podman socket override rwlock poisoned")
            .clone()
    }

    fn is_force_remote() -> bool {
        *PODMAN_FORCE_REMOTE
            .read()
            .expect("podman force remote rwlock poisoned")
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

        std::env::var("CONTAINER_HOST").ok().and_then(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(Self::normalize_socket(trimmed))
            }
        })
    }

    /// Create a REST API client for the configured socket
    fn create_api_client(&self) -> Option<PodmanApiClient> {
        self.podman_socket.as_ref().map(|socket| PodmanApiClient::new(socket))
    }

    /// Execute a CLI command as fallback when REST API is not available
    async fn execute_cli_command(&self, args: &[&str]) -> RuntimeResult<String> {
        use std::process::Stdio;
        use tokio::process::Command;

        debug!(
            "Executing podman CLI command: {} {}",
            self.podman_binary,
            args.join(" ")
        );

        let output = Command::new(&self.podman_binary)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            debug!("Podman CLI command succeeded: {}", stdout.trim());
            Ok(stdout)
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            error!("Podman CLI command failed: {}", stderr);
            Err(RuntimeError::CommandFailed(stderr))
        }
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

    /// Generate a unique workload ID based on manifest ID only
    fn generate_workload_id(&self, manifest_id: &str, _manifest_content: &[u8]) -> String {
        // Use consistent naming with pod name - just manifest_id based
        format!("beemesh-{}", manifest_id)
    }

    /// Modify the manifest to set the pod name to our workload ID
    fn modify_manifest_for_deployment(
        &self,
        manifest_content: &[u8],
        manifest_id: &str,
    ) -> RuntimeResult<Vec<u8>> {
        let manifest_str = String::from_utf8_lossy(manifest_content);
        let mut doc: serde_yaml::Value = serde_yaml::from_str(&manifest_str)
            .map_err(|e| RuntimeError::InvalidManifest(format!("YAML parse error: {}", e)))?;

        // Generate pod name based on manifest_id
        let pod_name = format!("beemesh-{}", manifest_id);

        // Update metadata name to use our generated pod name
        if let Some(metadata) = doc.get_mut("metadata") {
            if let Some(metadata_map) = metadata.as_mapping_mut() {
                metadata_map.insert(
                    serde_yaml::Value::String("name".to_string()),
                    serde_yaml::Value::String(pod_name.clone()),
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

    /// Deploy using CLI as fallback
    async fn deploy_via_cli(&self, manifest_content: &[u8]) -> RuntimeResult<()> {
        use tokio::io::AsyncWriteExt;

        debug!("Deploying workload via Podman CLI");

        // Create temporary manifest file
        let temp_dir = std::env::temp_dir();
        let temp_file = temp_dir.join(format!("beemesh-manifest-{}.yaml", uuid::Uuid::new_v4()));

        let mut file = tokio::fs::File::create(&temp_file).await?;
        file.write_all(manifest_content).await?;
        file.flush().await?;

        let temp_file_path = temp_file.to_str().ok_or_else(|| {
            RuntimeError::InvalidManifest("Invalid temporary file path".to_string())
        })?;

        let result = self
            .execute_cli_command(&["kube", "play", "--replace", temp_file_path])
            .await;

        // Clean up temp file
        let _ = tokio::fs::remove_file(&temp_file).await;

        result.map(|_| ())
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
                    // Extract manifest_id from pod name
                    let manifest_id = if pod_name.ends_with("-pod") {
                        pod_name
                            .strip_prefix("beemesh-")
                            .unwrap_or(pod_name)
                            .strip_suffix("-pod")
                            .unwrap_or(pod_name)
                            .to_string()
                    } else {
                        pod_name
                            .strip_prefix("beemesh-")
                            .unwrap_or(pod_name)
                            .to_string()
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

                    let workload_info = WorkloadInfo {
                        id: format!("beemesh-{}", manifest_id),
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

    /// List pods using CLI as fallback
    async fn list_pods_via_cli(&self) -> RuntimeResult<Vec<WorkloadInfo>> {
        debug!("Listing pods via Podman CLI");
        
        let output = self
            .execute_cli_command(&["pod", "ls", "--format", "json"])
            .await?;

        let mut workloads = Vec::new();

        if !output.trim().is_empty() {
            match serde_json::from_str::<serde_json::Value>(&output) {
                Ok(json) => {
                    if let Some(pods_array) = json.as_array() {
                        for pod in pods_array {
                            if let Some(pod_name) = pod.get("Name").and_then(|n| n.as_str()) {
                                if pod_name.starts_with("beemesh-") {
                                    let manifest_id = if pod_name.ends_with("-pod") {
                                        pod_name
                                            .strip_prefix("beemesh-")
                                            .unwrap_or(pod_name)
                                            .strip_suffix("-pod")
                                            .unwrap_or(pod_name)
                                            .to_string()
                                    } else {
                                        pod_name
                                            .strip_prefix("beemesh-")
                                            .unwrap_or(pod_name)
                                            .to_string()
                                    };

                                    let status = match pod.get("Status").and_then(|s| s.as_str()) {
                                        Some("Running") => WorkloadStatus::Running,
                                        Some("Stopped") | Some("Exited") => WorkloadStatus::Stopped,
                                        Some("Error") => {
                                            WorkloadStatus::Failed("Pod in error state".to_string())
                                        }
                                        Some("Failed") => {
                                            WorkloadStatus::Failed("Pod failed".to_string())
                                        }
                                        _ => WorkloadStatus::Unknown,
                                    };

                                    let mut metadata = HashMap::new();
                                    if let Some(labels) = pod.get("Labels") {
                                        if let Some(labels_obj) = labels.as_object() {
                                            for (key, value) in labels_obj {
                                                if let Some(value_str) = value.as_str() {
                                                    metadata.insert(key.clone(), value_str.to_string());
                                                }
                                            }
                                        }
                                    }

                                    let created_at = pod
                                        .get("Created")
                                        .and_then(|c| c.as_str())
                                        .and_then(|created_str| {
                                            std::time::SystemTime::UNIX_EPOCH.checked_add(
                                                std::time::Duration::from_secs(
                                                    created_str.parse::<u64>().unwrap_or(0),
                                                ),
                                            )
                                        })
                                        .unwrap_or_else(std::time::SystemTime::now);

                                    workloads.push(WorkloadInfo {
                                        id: format!("beemesh-{}", manifest_id),
                                        manifest_id,
                                        status,
                                        metadata,
                                        created_at,
                                        updated_at: std::time::SystemTime::now(),
                                        ports: Vec::new(),
                                    });
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    debug!("Failed to parse JSON output: {}", e);
                }
            }
        }

        debug!("Found {} workloads via CLI", workloads.len());
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

    /// Remove pod using CLI as fallback
    async fn remove_pod_via_cli(&self, workload_id: &str) -> RuntimeResult<()> {
        debug!("Removing pod via Podman CLI: {}", workload_id);

        let pod_name_with_suffix = format!("{}-pod", workload_id);

        // Try with suffix first
        if self
            .execute_cli_command(&["pod", "rm", "-f", &pod_name_with_suffix])
            .await
            .is_ok()
        {
            info!("Successfully removed pod: {}", pod_name_with_suffix);
            return Ok(());
        }

        // Try exact name
        if self
            .execute_cli_command(&["pod", "rm", "-f", workload_id])
            .await
            .is_ok()
        {
            info!("Successfully removed pod: {}", workload_id);
            return Ok(());
        }

        // List and remove by pattern
        let output = self
            .execute_cli_command(&["pod", "ls", "-q", "--filter", &format!("name={}", workload_id)])
            .await?;

        for line in output.lines() {
            let pod_id = line.trim();
            if !pod_id.is_empty() {
                if let Err(e) = self.execute_cli_command(&["pod", "rm", "-f", pod_id]).await {
                    warn!("Failed to remove pod {}: {}", pod_id, e);
                } else {
                    info!("Successfully removed pod: {}", pod_id);
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

    /// Generate kube manifest using CLI as fallback
    async fn generate_kube_via_cli(&self, workload_id: &str) -> RuntimeResult<Vec<u8>> {
        debug!("Generating kube manifest via CLI for: {}", workload_id);

        let pod_name_variations = vec![
            format!("{}-pod", workload_id),
            workload_id.to_string(),
        ];

        for pod_name in &pod_name_variations {
            if let Ok(manifest) = self.execute_cli_command(&["generate", "kube", pod_name]).await {
                info!("Successfully generated manifest for pod: {}", pod_name);
                return Ok(manifest.into_bytes());
            }
        }

        // Try listing pods to find matches
        if let Ok(output) = self
            .execute_cli_command(&[
                "pod", "ls", "--format", "{{.Name}}", "--filter", &format!("name={}", workload_id),
            ])
            .await
        {
            for line in output.lines() {
                let pod_name = line.trim();
                if !pod_name.is_empty() && pod_name.contains(workload_id) {
                    if let Ok(manifest) = self.execute_cli_command(&["generate", "kube", pod_name]).await {
                        info!("Successfully generated manifest for pod: {}", pod_name);
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

        // Try with -pod suffix first
        let pod_name_with_suffix = format!("{}-pod", workload_id);
        if let Ok(logs) = client.get_pod_logs(&pod_name_with_suffix, tail).await {
            return Ok(logs);
        }

        // Try exact name
        if let Ok(logs) = client.get_pod_logs(workload_id, tail).await {
            return Ok(logs);
        }

        Ok(format!("Failed to retrieve logs for workload: {}", workload_id))
    }

    /// Get pod logs using CLI as fallback
    async fn get_logs_via_cli(
        &self,
        workload_id: &str,
        tail: Option<usize>,
    ) -> RuntimeResult<String> {
        debug!("Getting logs via CLI for: {}", workload_id);

        let mut args = vec!["pod", "logs"];

        let tail_str;
        if let Some(tail_lines) = tail {
            tail_str = tail_lines.to_string();
            args.extend(&["--tail", &tail_str]);
        }

        args.push(workload_id);

        match self.execute_cli_command(&args).await {
            Ok(logs) => Ok(logs),
            Err(e) => Ok(format!("Failed to retrieve logs: {}", e)),
        }
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
        // Try REST API first if socket is configured
        if let Some(client) = self.create_api_client() {
            match client.check_availability().await {
                Ok(info) => {
                    if let Some(version) = info.version {
                        debug!("Podman API available, version: {:?}", version.version);
                    }
                    return true;
                }
                Err(e) => {
                    if self.force_remote {
                        debug!("Podman REST API unavailable and force_remote=true: {}", e);
                        return false;
                    }
                    debug!("Podman REST API unavailable, trying CLI: {}", e);
                }
            }
        }

        if self.force_remote {
            debug!("Podman socket forced but not configured; marking unavailable");
            return false;
        }

        // Fall back to CLI check
        self.execute_cli_command(&["--version"]).await.is_ok()
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

        // Modify manifest for deployment
        let modified_manifest = self.modify_manifest_for_deployment(manifest_content, manifest_id)?;

        // Try REST API first if socket is configured
        if let Some(client) = self.create_api_client() {
            match self.deploy_via_api(&client, &modified_manifest).await {
                Ok(()) => {
                    info!("Workload deployed via REST API: {}", workload_id);
                }
                Err(e) => {
                    if self.force_remote || !PodmanApiClient::is_connection_error(&e) {
                        error!("Podman REST API deployment failed: {}", e);
                        return Err(e);
                    }
                    warn!("REST API deployment failed, falling back to CLI: {}", e);
                    self.deploy_via_cli(&modified_manifest).await?;
                    info!("Workload deployed via CLI fallback: {}", workload_id);
                }
            }
        } else {
            // No socket configured, use CLI
            self.deploy_via_cli(&modified_manifest).await?;
            info!("Workload deployed via CLI: {}", workload_id);
        }

        // Parse manifest metadata
        let metadata = self
            .parse_manifest_metadata(manifest_content)
            .unwrap_or_default();

        // Get port mappings
        let pod_name = format!("beemesh-{}", manifest_id);
        let ports = self.extract_port_mappings(&pod_name).await.unwrap_or_default();

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

        // Try REST API first if socket is configured
        if let Some(client) = self.create_api_client() {
            match self.list_pods_via_api(&client).await {
                Ok(workloads) => return Ok(workloads),
                Err(e) => {
                    if self.force_remote || !PodmanApiClient::is_connection_error(&e) {
                        return Err(e);
                    }
                    warn!("REST API list failed, falling back to CLI: {}", e);
                }
            }
        }

        // Fall back to CLI
        self.list_pods_via_cli().await
    }

    async fn remove_workload(&self, workload_id: &str) -> RuntimeResult<()> {
        info!("Removing workload: {}", workload_id);

        // Try REST API first if socket is configured
        if let Some(client) = self.create_api_client() {
            match self.remove_pod_via_api(&client, workload_id).await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    if self.force_remote || !PodmanApiClient::is_connection_error(&e) {
                        return Err(e);
                    }
                    warn!("REST API remove failed, falling back to CLI: {}", e);
                }
            }
        }

        // Fall back to CLI
        self.remove_pod_via_cli(workload_id).await
    }

    async fn get_workload_logs(
        &self,
        workload_id: &str,
        tail: Option<usize>,
    ) -> RuntimeResult<String> {
        debug!("Getting logs for workload: {}", workload_id);

        // Try REST API first if socket is configured
        if let Some(client) = self.create_api_client() {
            match self.get_logs_via_api(&client, workload_id, tail).await {
                Ok(logs) => return Ok(logs),
                Err(e) => {
                    if self.force_remote || !PodmanApiClient::is_connection_error(&e) {
                        warn!("Failed to get logs via API: {}", e);
                        return Ok(format!("Failed to retrieve logs: {}", e));
                    }
                    debug!("REST API logs failed, falling back to CLI: {}", e);
                }
            }
        }

        // Fall back to CLI
        self.get_logs_via_cli(workload_id, tail).await
    }

    async fn export_manifest(&self, workload_id: &str) -> RuntimeResult<Vec<u8>> {
        info!("Exporting manifest for workload: {}", workload_id);

        // Try REST API first if socket is configured
        if let Some(client) = self.create_api_client() {
            match self.generate_kube_via_api(&client, workload_id).await {
                Ok(manifest) => return Ok(manifest),
                Err(e) => {
                    if self.force_remote || !PodmanApiClient::is_connection_error(&e) {
                        error!("Failed to export manifest via API: {}", e);
                        return Err(e);
                    }
                    warn!("REST API export failed, falling back to CLI: {}", e);
                }
            }
        }

        // Fall back to CLI
        self.generate_kube_via_cli(workload_id).await
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
        PodmanEngine::configure_runtime(None, false);
        let engine = PodmanEngine::new();
        assert_eq!(engine.name(), "podman");
    }

    #[serial]
    #[tokio::test]
    async fn test_manifest_validation() {
        PodmanEngine::configure_runtime(None, false);
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
        PodmanEngine::configure_runtime(None, false);
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
        PodmanEngine::configure_runtime(None, false);
        let engine = PodmanEngine::new();

        let manifest1 = b"apiVersion: v1\nkind: Pod\nmetadata:\n  name: test-pod";
        let manifest2 = b"apiVersion: v1\nkind: Pod\nmetadata:\n  name: different-pod";
        let manifest_without_name = b"apiVersion: v1\nkind: Pod";

        let id1 = engine.generate_workload_id("manifest-123", manifest1);
        let id2 = engine.generate_workload_id("manifest-123", manifest1);
        let id3 = engine.generate_workload_id("manifest-456", manifest2);
        let id4 = engine.generate_workload_id("manifest-789", manifest_without_name);

        // IDs should be identical for the same manifest and ID
        assert_eq!(id1, id2);
        assert_eq!(id1, "beemesh-manifest-123");

        // Different manifest IDs should produce different workload IDs
        assert_ne!(id1, id3);
        assert_eq!(id3, "beemesh-manifest-456");

        // Manifest without name should still use manifest_id only
        assert_eq!(id4, "beemesh-manifest-789");
    }

    #[serial]
    #[tokio::test]
    async fn detects_podman_socket_from_env() {
        PodmanEngine::configure_runtime(None, false);

        let _guard = EnvGuard::set("CONTAINER_HOST", "/tmp/env-podman.sock");

        let engine = PodmanEngine::new();
        assert_eq!(
            engine.podman_socket.as_deref(),
            Some("unix:///tmp/env-podman.sock")
        );

        PodmanEngine::configure_runtime(None, false);
    }

    #[serial]
    #[tokio::test]
    async fn test_force_remote_configuration() {
        PodmanEngine::configure_runtime(None, false);
        PodmanEngine::configure_runtime(Some("/run/podman/podman.sock".to_string()), true);
        let engine = PodmanEngine::new();
        assert!(engine.force_remote);
        assert_eq!(
            engine.podman_socket.as_deref(),
            Some("unix:///run/podman/podman.sock")
        );
        PodmanEngine::configure_runtime(None, false);
    }
}

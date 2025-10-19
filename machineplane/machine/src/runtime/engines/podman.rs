//! Podman runtime engine implementation
//!
//! This module provides a runtime engine that uses Podman to deploy and manage
//! Kubernetes manifests via `podman kube play`.

use crate::runtime::{
    DeploymentConfig, PortMapping, RuntimeEngine, RuntimeError, RuntimeResult, WorkloadInfo,
    WorkloadStatus,
};
use async_trait::async_trait;
use log::{debug, error, info, warn};
use serde_yaml::Value;
use std::collections::HashMap;
use std::process::Stdio;
use tokio::process::Command;

/// Podman runtime engine
pub struct PodmanEngine {
    podman_binary: String,
}

impl PodmanEngine {
    /// Create a new Podman engine instance
    pub fn new() -> Self {
        Self {
            podman_binary: "podman".to_string(),
        }
    }

    /// Create a new Podman engine with custom binary path
    pub fn with_binary(binary_path: String) -> Self {
        Self {
            podman_binary: binary_path,
        }
    }

    /// Execute a podman command and return the output
    async fn execute_command(&self, args: &[&str]) -> RuntimeResult<String> {
        debug!(
            "Executing podman command: {} {}",
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
            debug!("Podman command succeeded: {}", stdout.trim());
            Ok(stdout)
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            error!("Podman command failed: {}", stderr);
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

    /// Extract port mappings from podman pod inspect output
    async fn extract_port_mappings(&self, pod_name: &str) -> RuntimeResult<Vec<PortMapping>> {
        // Try the pod name with -pod suffix first (most common)
        let pod_name_with_suffix = format!("{}-pod", pod_name);
        
        match self
            .execute_command(&["pod", "inspect", &pod_name_with_suffix, "--format", "json"])
            .await
        {
            Ok(_output) => {
                // Parse JSON output to extract port mappings
                // This is a simplified implementation - in practice you'd parse the full JSON
                let ports = Vec::new();
                debug!(
                    "Extracted {} port mappings for pod {}",
                    ports.len(),
                    pod_name_with_suffix
                );
                return Ok(ports);
            }
            Err(_) => {
                debug!("Failed to inspect pod with suffix: {}", pod_name_with_suffix);
            }
        }

        // Try without suffix as fallback
        match self
            .execute_command(&["pod", "inspect", pod_name, "--format", "json"])
            .await
        {
            Ok(_output) => {
                let ports = Vec::new();
                debug!(
                    "Extracted {} port mappings for pod {}",
                    ports.len(),
                    pod_name
                );
                Ok(ports)
            }
            Err(e) => {
                debug!("Failed to inspect pod {}: {}", pod_name, e);
                // Return empty ports if inspection fails - this is not a critical error
                Ok(Vec::new())
            }
        }
    }

    /// Generate a unique workload ID based on manifest ID only
    fn generate_workload_id(&self, manifest_id: &str, _manifest_content: &[u8]) -> String {
        // Use consistent naming with pod name - just manifest_id based
        format!("beemesh-{}", manifest_id)
    }

    /// Create a temporary file with the manifest content, modifying pod name to use manifest_id
    async fn create_temp_manifest_file(
        &self,
        manifest_content: &[u8],
        manifest_id: &str,
    ) -> RuntimeResult<std::path::PathBuf> {
        use tokio::io::AsyncWriteExt;

        let temp_dir = std::env::temp_dir();
        let temp_file = temp_dir.join(format!("beemesh-manifest-{}.yaml", uuid::Uuid::new_v4()));

        // Parse the manifest and modify the pod name
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

        let mut file = tokio::fs::File::create(&temp_file).await?;
        file.write_all(modified_manifest.as_bytes()).await?;
        file.flush().await?;

        debug!("Created temporary manifest file: {:?} with pod name: {}", temp_file, pod_name);
        Ok(temp_file)
    }

    /// Clean up temporary manifest file
    async fn cleanup_temp_file(&self, path: &std::path::Path) {
        if let Err(e) = tokio::fs::remove_file(path).await {
            warn!("Failed to clean up temporary file {:?}: {}", path, e);
        } else {
            debug!("Cleaned up temporary file: {:?}", path);
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
        match Command::new(&self.podman_binary)
            .args(&["--version"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
        {
            Ok(status) => status.success(),
            Err(e) => {
                debug!("Podman not available: {}", e);
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
        config: &DeploymentConfig,
    ) -> RuntimeResult<WorkloadInfo> {
        info!("Deploying workload for manifest_id: {}", manifest_id);

        // Validate manifest first
        self.validate_manifest(manifest_content).await?;

        // Generate unique workload ID
        let workload_id = self.generate_workload_id(manifest_id, manifest_content);

        // Create temporary manifest file with modified pod name
        let temp_file = self.create_temp_manifest_file(manifest_content, manifest_id).await?;

        // Build podman kube play command
        let mut args = vec!["kube", "play"];

        // Add --replace flag to overwrite existing pods
        args.push("--replace");

        // Add replicas if specified and > 1
        let replicas_str;
        if config.replicas > 1 {
            replicas_str = config.replicas.to_string();
            args.extend(&["--replicas", &replicas_str]);
        }

        // Add resource limits if specified
        let memory_str;
        if let Some(memory) = config.resources.memory {
            memory_str = format!("{}b", memory);
            args.extend(&["--memory", &memory_str]);
        }

        let cpu_str;
        if let Some(cpu) = config.resources.cpu {
            cpu_str = format!("{}", cpu);
            args.extend(&["--cpus", &cpu_str]);
        }

        // Add environment variables
        let mut env_strings = Vec::new();
        for (key, value) in &config.env {
            env_strings.push(format!("{}={}", key, value));
        }
        for env_str in &env_strings {
            args.extend(&["--env", env_str]);
        }

        // Add runtime-specific options
        for (key, value) in &config.runtime_options {
            match key.as_str() {
                "network" => args.extend(&["--network", value]),
                "volume" => args.extend(&["--volume", value]),
                "security-opt" => args.extend(&["--security-opt", value]),
                _ => debug!("Ignoring unknown runtime option: {}={}", key, value),
            }
        }

        // Add the manifest file
        args.push(temp_file.to_str().ok_or_else(|| {
            RuntimeError::InvalidManifest("Invalid temporary file path".to_string())
        })?);

        // Execute the deployment
        match self.execute_command(&args).await {
            Ok(output) => {
                info!(
                    "Podman kube play succeeded for workload {}: {}",
                    workload_id,
                    output.trim()
                );

                // Clean up temporary file
                self.cleanup_temp_file(&temp_file).await;

                // Parse manifest metadata
                let metadata = self
                    .parse_manifest_metadata(manifest_content)
                    .unwrap_or_default();

                // Use the manifest_id-based pod name (consistent with our modified manifest)
                let pod_name = format!("beemesh-{}", manifest_id);

                // Get port mappings
                let ports = self
                    .extract_port_mappings(&pod_name)
                    .await
                    .unwrap_or_default();

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
            Err(e) => {
                error!(
                    "Podman kube play failed for workload {}: {}",
                    workload_id, e
                );

                // Clean up temporary file
                self.cleanup_temp_file(&temp_file).await;

                Err(RuntimeError::DeploymentFailed(format!(
                    "Podman deployment failed: {}",
                    e
                )))
            }
        }
    }

    /// Deploy a workload with local peer ID tracking
    async fn deploy_workload_with_peer(
        &self,
        manifest_id: &str,
        manifest_content: &[u8],
        config: &DeploymentConfig,
        local_peer_id: libp2p::PeerId,
    ) -> RuntimeResult<WorkloadInfo> {
        // Validate the manifest first
        self.validate_manifest(manifest_content).await?;

        // Generate unique workload ID
        let workload_id = self.generate_workload_id(manifest_id, manifest_content);

        // Create temporary manifest file with modified pod name
        let temp_file = self.create_temp_manifest_file(manifest_content, manifest_id).await?;

        // Prepare podman command
        let mut args = vec!["kube", "play"];

        // Add --replace flag to overwrite existing pods
        args.push("--replace");

        // Add environment variables
        let mut env_strings = Vec::new();
        for (key, value) in &config.env {
            env_strings.push(format!("{}={}", key, value));
        }

        if !env_strings.is_empty() {
            for env in &env_strings {
                args.extend(&["--env", env]);
            }
        }

        // Add the manifest file
        args.push(temp_file.to_str().unwrap());

        // Execute the deployment
        match self.execute_command(&args).await {
            Ok(output) => {
                info!(
                    "Podman kube play succeeded for workload {}: {}",
                    workload_id,
                    output.trim()
                );

                // Clean up temporary file
                self.cleanup_temp_file(&temp_file).await;

                // Parse manifest metadata
                let mut metadata = self
                    .parse_manifest_metadata(manifest_content)
                    .unwrap_or_default();

                // Add local peer ID to metadata
                metadata.insert("local_peer_id".to_string(), local_peer_id.to_string());

                // Use the manifest_id-based pod name (consistent with our modified manifest)
                let pod_name = format!("beemesh-{}", manifest_id);

                // Get port mappings
                let ports = self
                    .extract_port_mappings(&pod_name)
                    .await
                    .unwrap_or_default();

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
            Err(e) => {
                error!(
                    "Podman kube play failed for workload {}: {}",
                    workload_id, e
                );

                // Clean up temporary file
                self.cleanup_temp_file(&temp_file).await;

                Err(RuntimeError::DeploymentFailed(format!(
                    "Podman deployment failed: {}",
                    e
                )))
            }
        }
    }

    async fn get_workload_status(&self, workload_id: &str) -> RuntimeResult<WorkloadInfo> {
        debug!("Getting status for workload: {}", workload_id);

        // Try to find the pod by listing all pods and matching by labels or names
        let _output = self
            .execute_command(&["pod", "ls", "--format", "json"])
            .await?;

        // Parse JSON output to find our workload
        // This is a simplified implementation - in practice you'd parse the full JSON
        // and match based on labels or naming conventions

        // For now, return a basic status
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

        let _output = self
            .execute_command(&["pod", "ls", "--format", "json"])
            .await?;

        // Parse JSON output to create WorkloadInfo objects
        // This is a simplified implementation - in practice you'd parse the full JSON
        let workloads = Vec::new();

        debug!("Found {} workloads", workloads.len());
        Ok(workloads)
    }

    async fn remove_workload(&self, workload_id: &str) -> RuntimeResult<()> {
        info!("Removing workload: {}", workload_id);

        // For our naming convention, the workload_id is "beemesh-{manifest_id}"
        // But Podman creates pods with "-pod" suffix, so we need to try both forms
        let pod_name_with_suffix = format!("{}-pod", workload_id);

        // Try to remove by pod name with suffix first (most likely to succeed)
        match self
            .execute_command(&["pod", "rm", "-f", &pod_name_with_suffix])
            .await
        {
            Ok(output) => {
                info!(
                    "Successfully removed pod {}: {}",
                    pod_name_with_suffix,
                    output.trim()
                );
                return Ok(());
            }
            Err(e) => {
                debug!("Failed to remove pod with suffix {}: {}", pod_name_with_suffix, e);
            }
        }

        // Try to remove by exact workload_id
        match self
            .execute_command(&["pod", "rm", "-f", workload_id])
            .await
        {
            Ok(output) => {
                info!(
                    "Successfully removed workload {}: {}",
                    workload_id,
                    output.trim()
                );
                return Ok(());
            }
            Err(e) => {
                debug!("Direct removal failed: {}", e);
            }
        }

        // If both specific removals fail, try to find and remove by pattern
        warn!("Specific removals failed, trying pattern match for: {}", workload_id);

        // List pods and find ones that match our naming pattern
        let output = self
            .execute_command(&[
                "pod",
                "ls",
                "-q",
                "--filter",
                &format!("name={}", workload_id),
            ])
            .await?;

        for line in output.lines() {
            let pod_id = line.trim();
            if !pod_id.is_empty() {
                if let Err(e) = self.execute_command(&["pod", "rm", "-f", pod_id]).await {
                    warn!("Failed to remove pod {}: {}", pod_id, e);
                } else {
                    info!("Successfully removed pod: {}", pod_id);
                }
            }
        }

        Ok(())
    }

    async fn get_workload_logs(
        &self,
        workload_id: &str,
        tail: Option<usize>,
    ) -> RuntimeResult<String> {
        debug!("Getting logs for workload: {}", workload_id);

        let mut args = vec!["pod", "logs"];

        let tail_str;
        if let Some(tail_lines) = tail {
            tail_str = tail_lines.to_string();
            args.extend(&["--tail", &tail_str]);
        }

        args.push(workload_id);

        match self.execute_command(&args).await {
            Ok(logs) => {
                debug!(
                    "Retrieved {} bytes of logs for workload {}",
                    logs.len(),
                    workload_id
                );
                Ok(logs)
            }
            Err(e) => {
                warn!("Failed to get logs for workload {}: {}", workload_id, e);
                Ok(format!("Failed to retrieve logs: {}", e))
            }
        }
    }

    async fn export_manifest(&self, workload_id: &str) -> RuntimeResult<Vec<u8>> {
        info!("Exporting manifest for workload: {}", workload_id);

        // Podman creates pods with different naming patterns:
        // 1. For our workload_id "beemesh-manifest-123", it might create:
        //    - "beemesh-manifest-123-pod" (most common)
        //    - "beemesh-manifest-123" (exact match)
        //    - Other variations based on the original manifest

        let pod_name_variations = vec![
            format!("{}-pod", workload_id),  // Most common pattern
            workload_id.to_string(),         // Exact match
        ];

        let mut last_error = None;

        for pod_name in &pod_name_variations {
            debug!("Trying to export manifest for pod: {}", pod_name);

            match self
                .execute_command(&["generate", "kube", pod_name])
                .await
            {
                Ok(manifest_yaml) => {
                    info!(
                        "Successfully exported manifest for workload {} (pod: {})",
                        workload_id, pod_name
                    );
                    debug!(
                        "Exported manifest ({} bytes): {}",
                        manifest_yaml.len(),
                        manifest_yaml.trim()
                    );
                    return Ok(manifest_yaml.into_bytes());
                }
                Err(e) => {
                    debug!("Failed to export manifest for pod {}: {}", pod_name, e);
                    last_error = Some(e);
                }
            }
        }

        // If all variations failed, try to find the actual pod name by listing
        debug!("All direct attempts failed, trying to find pod by listing");

        match self
            .execute_command(&[
                "pod", "ls", "--format", "{{.Name}}", "--filter", 
                &format!("name={}", workload_id)
            ])
            .await
        {
            Ok(output) => {
                for line in output.lines() {
                    let actual_pod_name = line.trim();
                    if !actual_pod_name.is_empty() && actual_pod_name.contains(workload_id) {
                        debug!("Found actual pod name: {}", actual_pod_name);
                        
                        match self
                            .execute_command(&["generate", "kube", actual_pod_name])
                            .await
                        {
                            Ok(manifest_yaml) => {
                                info!(
                                    "Successfully exported manifest for workload {} (actual pod: {})",
                                    workload_id, actual_pod_name
                                );
                                return Ok(manifest_yaml.into_bytes());
                            }
                            Err(e) => {
                                debug!("Failed to export manifest for actual pod {}: {}", actual_pod_name, e);
                                last_error = Some(e);
                            }
                        }
                    }
                }
            }
            Err(e) => {
                debug!("Failed to list pods: {}", e);
                last_error = Some(e);
            }
        }

        // All attempts failed
        let error_msg = match last_error {
            Some(e) => format!("Failed to export manifest for workload {}: {}", workload_id, e),
            None => format!("No running pod found for workload {}", workload_id),
        };

        error!("{}", error_msg);
        Err(RuntimeError::WorkloadNotFound(error_msg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_podman_engine_creation() {
        let engine = PodmanEngine::new();
        assert_eq!(engine.name(), "podman");
    }

    #[tokio::test]
    async fn test_manifest_validation() {
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

    #[tokio::test]
    async fn test_parse_manifest_metadata() {
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

    #[tokio::test]
    async fn test_workload_id_generation() {
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
}

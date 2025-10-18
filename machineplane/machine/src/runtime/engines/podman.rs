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
        let _output = self
            .execute_command(&["pod", "inspect", pod_name, "--format", "json"])
            .await?;

        // Parse JSON output to extract port mappings
        // This is a simplified implementation - in practice you'd parse the full JSON
        let ports = Vec::new();

        // For now, return empty ports - full implementation would parse the JSON
        // to extract actual port mappings from the pod inspection
        debug!(
            "Extracted {} port mappings for pod {}",
            ports.len(),
            pod_name
        );

        Ok(ports)
    }

    /// Generate a unique workload ID based on manifest name and ID
    fn generate_workload_id(&self, manifest_id: &str, manifest_content: &[u8]) -> String {
        let metadata = self
            .parse_manifest_metadata(manifest_content)
            .unwrap_or_default();
        let manifest_name = metadata
            .get("name")
            .map(|n| n.as_str())
            .unwrap_or("unnamed");
        format!("beemesh-{}-{}", manifest_name, manifest_id)
    }

    /// Create a temporary file with the manifest content
    async fn create_temp_manifest_file(
        &self,
        manifest_content: &[u8],
    ) -> RuntimeResult<std::path::PathBuf> {
        use tokio::io::AsyncWriteExt;

        let temp_dir = std::env::temp_dir();
        let temp_file = temp_dir.join(format!("beemesh-manifest-{}.yaml", uuid::Uuid::new_v4()));

        let mut file = tokio::fs::File::create(&temp_file).await?;
        file.write_all(manifest_content).await?;
        file.flush().await?;

        debug!("Created temporary manifest file: {:?}", temp_file);
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

        // Create temporary manifest file
        let temp_file = self.create_temp_manifest_file(manifest_content).await?;

        // Build podman kube play command
        let mut args = vec!["kube", "play"];

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

                // Extract pod name from metadata or use workload_id
                let pod_name = metadata
                    .get("name")
                    .cloned()
                    .unwrap_or_else(|| workload_id.clone());

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

        // Create temporary manifest file
        let temp_file = self.create_temp_manifest_file(manifest_content).await?;

        // Prepare podman command
        let mut args = vec!["kube", "play"];

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

                // Extract pod name from metadata or use workload_id
                let pod_name = metadata
                    .get("name")
                    .cloned()
                    .unwrap_or_else(|| workload_id.clone());

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

        // Try to remove by pod name (assuming workload_id maps to pod name)
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
                Ok(())
            }
            Err(e) => {
                // If removal by exact name fails, try to find and remove by pattern
                warn!("Direct removal failed, trying pattern match: {}", e);

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
        }
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
        assert_eq!(id1, "beemesh-test-pod-manifest-123");

        // Different manifest names should produce different workload IDs
        assert_ne!(id1, id3);
        assert_eq!(id3, "beemesh-different-pod-manifest-456");

        // Manifest without name should use "unnamed"
        assert_eq!(id4, "beemesh-unnamed-manifest-789");
    }
}

//! Docker runtime engine implementation
//!
//! This module provides a runtime engine that uses Docker to deploy and manage
//! container workloads. This is for future extensibility when Docker support
//! is needed alongside Podman.

use crate::runtime::{
    DeploymentConfig, PortMapping, RuntimeEngine, RuntimeError, RuntimeResult, WorkloadInfo,
    WorkloadStatus,
};
use async_trait::async_trait;
use log::{debug, error, info, warn};
use serde_json::Value;
use std::collections::HashMap;
use std::process::Stdio;
use tokio::process::Command;

/// Docker runtime engine
pub struct DockerEngine {
    docker_binary: String,
}

impl DockerEngine {
    /// Create a new Docker engine instance
    pub fn new() -> Self {
        Self {
            docker_binary: "docker".to_string(),
        }
    }

    /// Create a new Docker engine with custom binary path
    pub fn with_binary(binary_path: String) -> Self {
        Self {
            docker_binary: binary_path,
        }
    }

    /// Execute a docker command and return the output
    async fn execute_command(&self, args: &[&str]) -> RuntimeResult<String> {
        debug!(
            "Executing docker command: {} {}",
            self.docker_binary,
            args.join(" ")
        );

        let output = Command::new(&self.docker_binary)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            debug!("Docker command succeeded: {}", stdout.trim());
            Ok(stdout)
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            error!("Docker command failed: {}", stderr);
            Err(RuntimeError::CommandFailed(stderr))
        }
    }

    /// Parse Docker Compose manifest to extract metadata
    fn parse_manifest_metadata(
        &self,
        manifest_content: &[u8],
    ) -> RuntimeResult<HashMap<String, String>> {
        let manifest_str = String::from_utf8_lossy(manifest_content);
        let mut metadata = HashMap::new();

        // Try to parse as YAML (Docker Compose format or Kubernetes format)
        if let Ok(doc) = serde_yaml::from_str::<serde_yaml::Value>(&manifest_str) {
            // Docker Compose fields
            if let Some(version) = doc.get("version").and_then(|v| v.as_str()) {
                metadata.insert("version".to_string(), version.to_string());
            }
            if let Some(services) = doc.get("services").and_then(|s| s.as_mapping()) {
                let service_names: Vec<String> = services
                    .keys()
                    .filter_map(|k| k.as_str().map(|s| s.to_string()))
                    .collect();
                metadata.insert("services".to_string(), service_names.join(","));
            }

            // Kubernetes-style fields
            if let Some(meta) = doc.get("metadata").and_then(|m| m.as_mapping()) {
                if let Some(name) = meta.get("name").and_then(|n| n.as_str()) {
                    metadata.insert("name".to_string(), name.to_string());
                }
            }
        }

        Ok(metadata)
    }

    /// Extract port mappings from docker container inspect output
    #[allow(dead_code)]
    async fn extract_port_mappings(&self, container_name: &str) -> RuntimeResult<Vec<PortMapping>> {
        let output = self
            .execute_command(&[
                "inspect",
                container_name,
                "--format",
                "{{json .NetworkSettings.Ports}}",
            ])
            .await?;

        let mut ports = Vec::new();

        // Parse JSON output to extract port mappings
        if let Ok(port_data) = serde_json::from_str::<Value>(&output) {
            if let Some(ports_obj) = port_data.as_object() {
                for (container_port, host_bindings) in ports_obj {
                    if let Some(port_str) = container_port.strip_suffix("/tcp") {
                        if let Ok(port_num) = port_str.parse::<u16>() {
                            let mut host_port = None;

                            if let Some(bindings) = host_bindings.as_array() {
                                if let Some(binding) = bindings.first() {
                                    if let Some(host_port_str) =
                                        binding.get("HostPort").and_then(|p| p.as_str())
                                    {
                                        host_port = host_port_str.parse().ok();
                                    }
                                }
                            }

                            ports.push(PortMapping {
                                container_port: port_num,
                                host_port,
                                protocol: "tcp".to_string(),
                            });
                        }
                    }
                }
            }
        }

        debug!(
            "Extracted {} port mappings for container {}",
            ports.len(),
            container_name
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
        let temp_file = temp_dir.join(format!("beemesh-compose-{}.yml", uuid::Uuid::new_v4()));

        let mut file = tokio::fs::File::create(&temp_file).await?;
        file.write_all(manifest_content).await?;
        file.flush().await?;

        debug!("Created temporary compose file: {:?}", temp_file);
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

impl Default for DockerEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl RuntimeEngine for DockerEngine {
    fn name(&self) -> &str {
        "docker"
    }

    async fn is_available(&self) -> bool {
        match Command::new(&self.docker_binary)
            .args(&["--version"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
        {
            Ok(status) => status.success(),
            Err(e) => {
                debug!("Docker not available: {}", e);
                false
            }
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn validate_manifest(&self, manifest_content: &[u8]) -> RuntimeResult<()> {
        let manifest_str = String::from_utf8_lossy(manifest_content);

        // Basic YAML validation for Docker Compose format
        match serde_yaml::from_str::<serde_yaml::Value>(&manifest_str) {
            Ok(doc) => {
                // Check for Docker Compose structure
                if doc.get("services").is_none() {
                    return Err(RuntimeError::InvalidManifest(
                        "Missing services section in Docker Compose".to_string(),
                    ));
                }

                info!("Docker Compose manifest validation passed");
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
        info!("Deploying Docker workload for manifest_id: {}", manifest_id);

        // Validate manifest first
        self.validate_manifest(manifest_content).await?;

        // Generate unique workload ID
        let workload_id = self.generate_workload_id(manifest_id, manifest_content);

        // Create temporary manifest file
        let temp_file = self.create_temp_manifest_file(manifest_content).await?;

        // Build docker compose command
        let mut args = vec!["compose", "-f"];

        args.push(temp_file.to_str().ok_or_else(|| {
            RuntimeError::InvalidManifest("Invalid temporary file path".to_string())
        })?);

        // Set project name to workload_id for isolation
        args.extend(&["-p", &workload_id]);

        // Add scaling if replicas > 1
        if config.replicas > 1 {
            args.extend(&["up", "-d", "--scale"]);
            // This would need service name - simplified for now
            args.push("service=1");
        } else {
            args.extend(&["up", "-d"]);
        }

        // Execute the deployment
        match self.execute_command(&args).await {
            Ok(output) => {
                info!(
                    "Docker compose up succeeded for workload {}: {}",
                    workload_id,
                    output.trim()
                );

                // Clean up temporary file
                self.cleanup_temp_file(&temp_file).await;

                // Parse manifest metadata
                let metadata = self
                    .parse_manifest_metadata(manifest_content)
                    .unwrap_or_default();

                // For Docker, we'd need to inspect the containers to get port mappings
                let ports = Vec::new(); // Simplified for now

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
                    "Docker compose up failed for workload {}: {}",
                    workload_id, e
                );

                // Clean up temporary file
                self.cleanup_temp_file(&temp_file).await;

                Err(RuntimeError::DeploymentFailed(format!(
                    "Docker deployment failed: {}",
                    e
                )))
            }
        }
    }

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

        // Prepare docker-compose command
        let mut args = vec![
            "compose",
            "-f",
            temp_file.to_str().unwrap(),
            "-p",
            &workload_id, // Use workload ID as project name
        ];

        // Handle replicas
        if config.replicas > 1 {
            args.extend(&["up", "-d", "--scale"]);
            // This would need service name - simplified for now
            args.push("service=1");
        } else {
            args.extend(&["up", "-d"]);
        }

        // Execute the deployment
        match self.execute_command(&args).await {
            Ok(output) => {
                info!(
                    "Docker compose up succeeded for workload {}: {}",
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

                // For Docker, we'd need to inspect the containers to get port mappings
                let ports = Vec::new(); // Simplified for now

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
                    "Docker compose up failed for workload {}: {}",
                    workload_id, e
                );
                self.cleanup_temp_file(&temp_file).await;
                Err(RuntimeError::DeploymentFailed(format!(
                    "Docker deployment failed: {}",
                    e
                )))
            }
        }
    }

    async fn get_workload_status(&self, workload_id: &str) -> RuntimeResult<WorkloadInfo> {
        debug!("Getting status for Docker workload: {}", workload_id);

        // List containers for this project
        let output = self
            .execute_command(&["compose", "-p", workload_id, "ps", "--format", "json"])
            .await?;

        // Parse JSON output to determine status
        let status = if output.trim().is_empty() {
            WorkloadStatus::Stopped
        } else {
            // Parse the JSON to determine actual status
            WorkloadStatus::Running // Simplified
        };

        let now = std::time::SystemTime::now();
        Ok(WorkloadInfo {
            id: workload_id.to_string(),
            manifest_id: "unknown".to_string(),
            status,
            metadata: HashMap::new(),
            created_at: now,
            updated_at: now,
            ports: Vec::new(),
        })
    }

    async fn list_workloads(&self) -> RuntimeResult<Vec<WorkloadInfo>> {
        debug!("Listing all Docker workloads");

        // List all containers with beemesh prefix
        let output = self
            .execute_command(&["ps", "-a", "--filter", "name=beemesh-", "--format", "json"])
            .await?;

        let mut workloads = Vec::new();

        // Parse JSON output to create WorkloadInfo objects
        for line in output.lines() {
            if let Ok(container_info) = serde_json::from_str::<Value>(line) {
                if let Some(name) = container_info.get("Names").and_then(|n| n.as_str()) {
                    let status = match container_info.get("State").and_then(|s| s.as_str()) {
                        Some("running") => WorkloadStatus::Running,
                        Some("exited") => WorkloadStatus::Stopped,
                        Some("created") => WorkloadStatus::Starting,
                        _ => WorkloadStatus::Unknown,
                    };

                    let now = std::time::SystemTime::now();
                    workloads.push(WorkloadInfo {
                        id: name.to_string(),
                        manifest_id: "unknown".to_string(),
                        status,
                        metadata: HashMap::new(),
                        created_at: now,
                        updated_at: now,
                        ports: Vec::new(),
                    });
                }
            }
        }

        debug!("Found {} Docker workloads", workloads.len());
        Ok(workloads)
    }

    async fn remove_workload(&self, workload_id: &str) -> RuntimeResult<()> {
        info!("Removing Docker workload: {}", workload_id);

        // Stop and remove the compose project
        match self
            .execute_command(&["compose", "-p", workload_id, "down", "-v"])
            .await
        {
            Ok(output) => {
                info!(
                    "Successfully removed Docker workload {}: {}",
                    workload_id,
                    output.trim()
                );
                Ok(())
            }
            Err(e) => {
                warn!("Failed to remove Docker workload {}: {}", workload_id, e);

                // Try to remove containers directly if compose down fails
                let _ = self
                    .execute_command(&["container", "rm", "-f", workload_id])
                    .await;

                Ok(()) // Consider partial success as OK
            }
        }
    }

    async fn get_workload_logs(
        &self,
        workload_id: &str,
        tail: Option<usize>,
    ) -> RuntimeResult<String> {
        debug!("Getting logs for Docker workload: {}", workload_id);

        let mut args = vec!["compose", "-p", workload_id, "logs"];

        let tail_str;
        if let Some(tail_lines) = tail {
            tail_str = tail_lines.to_string();
            args.extend(&["--tail", &tail_str]);
        }

        match self.execute_command(&args).await {
            Ok(logs) => {
                debug!(
                    "Retrieved {} bytes of logs for Docker workload {}",
                    logs.len(),
                    workload_id
                );
                Ok(logs)
            }
            Err(e) => {
                warn!(
                    "Failed to get logs for Docker workload {}: {}",
                    workload_id, e
                );
                Ok(format!("Failed to retrieve logs: {}", e))
            }
        }
    }

    async fn export_manifest(&self, workload_id: &str) -> RuntimeResult<Vec<u8>> {
        info!("Exporting manifest for Docker workload: {}", workload_id);

        // For Docker, we can try to generate a Docker Compose manifest from running containers
        // First, try to get the compose config that was used
        match self
            .execute_command(&["compose", "-p", workload_id, "config"])
            .await
        {
            Ok(compose_config) => {
                info!(
                    "Successfully exported Docker Compose config for workload {}",
                    workload_id
                );
                return Ok(compose_config.into_bytes());
            }
            Err(e) => {
                debug!("Failed to get compose config for {}: {}", workload_id, e);
            }
        }

        // Fallback: inspect containers and generate a basic Docker Compose manifest
        match self
            .execute_command(&[
                "ps",
                "-a",
                "--filter",
                &format!("name={}", workload_id),
                "--format",
                "json",
            ])
            .await
        {
            Ok(containers_output) => {
                if containers_output.trim().is_empty() {
                    return Err(RuntimeError::WorkloadNotFound(format!(
                        "No containers found for workload {}",
                        workload_id
                    )));
                }

                // Parse container information and generate a basic compose file
                let mut compose_manifest = format!(
                    "# Generated Docker Compose manifest for workload: {}\nversion: '3.8'\nservices:\n",
                    workload_id
                );

                for line in containers_output.lines() {
                    if let Ok(container_info) = serde_json::from_str::<Value>(line) {
                        if let (Some(names), Some(image)) = (
                            container_info.get("Names").and_then(|n| n.as_str()),
                            container_info.get("Image").and_then(|i| i.as_str()),
                        ) {
                            // Extract service name from container name
                            let service_name = names.split('/').last().unwrap_or(names);
                            compose_manifest.push_str(&format!(
                                "  {}:\n    image: {}\n    container_name: {}\n",
                                service_name, image, names
                            ));

                            // Add basic restart policy
                            compose_manifest.push_str("    restart: unless-stopped\n");

                            // Could add more details by inspecting each container
                            // This is a simplified version
                        }
                    }
                }

                info!(
                    "Generated basic Docker Compose manifest for workload {} ({} bytes)",
                    workload_id,
                    compose_manifest.len()
                );

                Ok(compose_manifest.into_bytes())
            }
            Err(e) => {
                error!(
                    "Failed to export manifest for Docker workload {}: {}",
                    workload_id, e
                );
                Err(RuntimeError::WorkloadNotFound(format!(
                    "Failed to export manifest for workload {}: {}",
                    workload_id, e
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_docker_engine_creation() {
        let engine = DockerEngine::new();
        assert_eq!(engine.name(), "docker");
    }

    #[tokio::test]
    async fn test_manifest_validation() {
        let engine = DockerEngine::new();

        // Valid Docker Compose manifest
        let valid_manifest = br#"
version: '3.8'
services:
  web:
    image: nginx:latest
    ports:
      - "80:80"
"#;

        assert!(engine.validate_manifest(valid_manifest).await.is_ok());

        // Invalid manifest (missing services)
        let invalid_manifest = br#"
version: '3.8'
networks:
  default:
    driver: bridge
"#;

        assert!(engine.validate_manifest(invalid_manifest).await.is_err());
    }

    #[tokio::test]
    async fn test_parse_manifest_metadata() {
        let engine = DockerEngine::new();

        let manifest = br#"
version: '3.8'
services:
  web:
    image: nginx:latest
    ports:
      - "80:80"
  db:
    image: postgres:13
"#;

        let metadata = engine.parse_manifest_metadata(manifest).unwrap();
        assert_eq!(metadata.get("version"), Some(&"3.8".to_string()));
        assert_eq!(metadata.get("services"), Some(&"web,db".to_string()));
    }

    #[tokio::test]
    async fn test_workload_id_generation() {
        let engine = DockerEngine::new();

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

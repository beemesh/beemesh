//! Podman REST API client for Unix socket communication
//!
//! This module provides an async HTTP client that communicates with the Podman REST API
//! over Unix domain sockets. It replaces CLI-based commands with direct API calls for
//! improved performance, reliability, and structured error handling.
//!
//! # API Versioning
//!
//! The client uses the Libpod API (native Podman API) at version v5.0.0 by default.
//! This provides access to Podman-specific features like pods and the kube play endpoint.
//!
//! # Endpoints Used
//!
//! - `GET /libpod/info` - Check API availability and version
//! - `POST /libpod/play/kube` - Deploy Kubernetes manifests
//! - `DELETE /libpod/play/kube` - Remove resources created from manifests
//! - `GET /libpod/pods/json` - List pods
//! - `GET /libpod/pods/{name}/json` - Inspect a specific pod
//! - `DELETE /libpod/pods/{name}` - Remove a pod
//! - `GET /libpod/generate/{name}/kube` - Generate Kubernetes manifest from running pod
//! - `GET /libpod/pods/{name}/logs` - Get pod logs

use crate::runtimes::RuntimeError;
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::{Method, Request, StatusCode};
use hyper_util::client::legacy::Client;
use hyperlocal::{UnixClientExt, UnixConnector, Uri};
use log::{debug, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Default API version for Podman Libpod API
const DEFAULT_API_VERSION: &str = "v5.0.0";

/// Podman REST API client for Unix socket communication
pub struct PodmanApiClient {
    /// Unix socket path (without unix:// prefix)
    socket_path: String,
    /// API version to use
    api_version: String,
    /// HTTP client for Unix socket connections
    client: Client<UnixConnector, Full<Bytes>>,
}

/// Response from /libpod/info endpoint
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InfoResponse {
    pub host: Option<HostInfo>,
    pub version: Option<VersionInfo>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostInfo {
    pub arch: Option<String>,
    pub os: Option<String>,
    pub hostname: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    #[serde(rename = "APIVersion")]
    pub api_version: Option<String>,
    #[serde(rename = "Version")]
    pub version: Option<String>,
}

/// Response from POST /libpod/play/kube endpoint
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlayKubeResponse {
    /// List of pod IDs created
    #[serde(default, alias = "Pods")]
    pub pods: Vec<PlayKubePod>,
    /// Volumes created (if any)
    #[serde(default, alias = "Volumes")]
    pub volumes: Vec<PlayKubeVolume>,
    /// Errors encountered during play
    #[serde(default, alias = "Errors")]
    pub errors: Option<Vec<String>>,
    /// rm_report - only present in delete response
    #[serde(default, rename = "RmReport")]
    pub rm_report: Option<Vec<RmReport>>,
    /// stop_report - only present in delete response
    #[serde(default, rename = "StopReport")]
    pub stop_report: Option<Vec<StopReport>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlayKubePod {
    #[serde(rename = "ID")]
    pub id: Option<String>,
    #[serde(default)]
    pub containers: Option<Vec<String>>,
    #[serde(default)]
    pub init_containers: Option<Vec<String>>,
    #[serde(default)]
    pub logs: Option<Vec<String>>,
    #[serde(default)]
    pub container_errors: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlayKubeVolume {
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct RmReport {
    #[serde(rename = "Id")]
    pub id: Option<String>,
    #[serde(rename = "Err")]
    pub err: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct StopReport {
    #[serde(rename = "Id")]
    pub id: Option<String>,
    #[serde(rename = "Err")]
    pub err: Option<String>,
}

/// Response from GET /libpod/pods/json endpoint
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct PodListEntry {
    #[serde(rename = "Id")]
    pub id: Option<String>,
    pub name: Option<String>,
    pub status: Option<String>,
    pub created: Option<String>,
    pub labels: Option<HashMap<String, String>>,
    pub containers: Option<Vec<PodContainer>>,
    #[serde(default)]
    pub infra_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct PodContainer {
    #[serde(rename = "Id")]
    pub id: Option<String>,
    pub name: Option<String>,
    pub status: Option<String>,
}

/// Response from GET /libpod/pods/{name}/json endpoint
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PodInspectResponse {
    #[serde(rename = "Id")]
    pub id: Option<String>,
    pub name: Option<String>,
    pub state: Option<String>,
    pub created: Option<String>,
    pub infra_container_id: Option<String>,
    pub containers: Option<Vec<PodInspectContainer>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PodInspectContainer {
    #[serde(rename = "Id")]
    pub id: Option<String>,
    pub name: Option<String>,
    pub state: Option<String>,
}

/// Error response from Podman API
#[derive(Debug, Deserialize)]
pub struct ApiErrorResponse {
    pub cause: Option<String>,
    pub message: Option<String>,
    pub response: Option<i32>,
}

impl PodmanApiClient {
    /// Create a new Podman API client for the given Unix socket path.
    ///
    /// The socket path should be in one of these formats:
    /// - `/path/to/podman.sock` (bare path)
    /// - `unix:///path/to/podman.sock` (URI format)
    pub fn new(socket_url: &str) -> Self {
        let socket_path = Self::normalize_socket_path(socket_url);
        debug!("Creating PodmanApiClient with socket: {}", socket_path);

        Self {
            socket_path,
            api_version: DEFAULT_API_VERSION.to_string(),
            client: Client::unix(),
        }
    }

    /// Normalize a socket URL to a bare path
    fn normalize_socket_path(socket_url: &str) -> String {
        if let Some(path) = socket_url.strip_prefix("unix://") {
            path.to_string()
        } else {
            socket_url.to_string()
        }
    }

    /// Build a URI for the given API path
    fn build_uri(&self, path: &str) -> hyper::Uri {
        let full_path = format!("/{}/libpod{}", self.api_version, path);
        Uri::new(&self.socket_path, &full_path).into()
    }

    /// Build a URI with query parameters
    fn build_uri_with_query(&self, path: &str, query: &str) -> hyper::Uri {
        let full_path = format!("/{}/libpod{}?{}", self.api_version, path, query);
        Uri::new(&self.socket_path, &full_path).into()
    }

    /// Execute an HTTP request and return the response body as bytes
    async fn execute_request(
        &self,
        request: Request<Full<Bytes>>,
    ) -> Result<(StatusCode, Bytes), RuntimeError> {
        let response = self
            .client
            .request(request)
            .await
            .map_err(|e| RuntimeError::CommandFailed(format!("HTTP request failed: {}", e)))?;

        let status = response.status();
        let body = response
            .into_body()
            .collect()
            .await
            .map_err(|e| RuntimeError::CommandFailed(format!("Failed to read response body: {}", e)))?
            .to_bytes();

        Ok((status, body))
    }

    /// Check if the Podman API is available
    pub async fn check_availability(&self) -> Result<InfoResponse, RuntimeError> {
        let uri = self.build_uri("/info");
        debug!("Checking Podman API availability at: {:?}", uri);

        let request = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .body(Full::new(Bytes::new()))
            .map_err(|e| RuntimeError::CommandFailed(format!("Failed to build request: {}", e)))?;

        let (status, body) = self.execute_request(request).await?;

        if !status.is_success() {
            let error_msg = self.parse_error_response(&body, status);
            return Err(RuntimeError::CommandFailed(error_msg));
        }

        serde_json::from_slice(&body)
            .map_err(|e| RuntimeError::CommandFailed(format!("Failed to parse info response: {}", e)))
    }

    /// Deploy a Kubernetes manifest using the kube play endpoint
    ///
    /// This is equivalent to `podman kube play manifest.yaml`
    pub async fn play_kube(
        &self,
        manifest_yaml: &[u8],
        replace: bool,
    ) -> Result<PlayKubeResponse, RuntimeError> {
        let mut query_params = Vec::new();
        if replace {
            query_params.push("replace=true");
        }

        let uri = if query_params.is_empty() {
            self.build_uri("/play/kube")
        } else {
            self.build_uri_with_query("/play/kube", &query_params.join("&"))
        };

        debug!("Deploying Kubernetes manifest via kube play: {:?}", uri);

        let request = Request::builder()
            .method(Method::POST)
            .uri(uri)
            .header("Content-Type", "application/yaml")
            .body(Full::new(Bytes::from(manifest_yaml.to_vec())))
            .map_err(|e| RuntimeError::CommandFailed(format!("Failed to build request: {}", e)))?;

        let (status, body) = self.execute_request(request).await?;

        if !status.is_success() {
            let error_msg = self.parse_error_response(&body, status);
            return Err(RuntimeError::DeploymentFailed(error_msg));
        }

        let response: PlayKubeResponse = serde_json::from_slice(&body)
            .map_err(|e| RuntimeError::DeploymentFailed(format!("Failed to parse play kube response: {} - body: {}", e, String::from_utf8_lossy(&body))))?;

        // Check for errors in the response
        if let Some(errors) = &response.errors {
            if !errors.is_empty() {
                let error_str = errors.join("; ");
                warn!("Play kube reported errors: {}", error_str);
            }
        }

        Ok(response)
    }

    /// Remove resources created from a Kubernetes manifest
    ///
    /// This is equivalent to `podman kube down manifest.yaml`
    pub async fn play_kube_down(
        &self,
        manifest_yaml: &[u8],
        force: bool,
    ) -> Result<PlayKubeResponse, RuntimeError> {
        let mut query_params = Vec::new();
        if force {
            query_params.push("force=true");
        }

        let uri = if query_params.is_empty() {
            self.build_uri("/play/kube")
        } else {
            self.build_uri_with_query("/play/kube", &query_params.join("&"))
        };

        debug!("Removing Kubernetes manifest resources via kube play down: {:?}", uri);

        let request = Request::builder()
            .method(Method::DELETE)
            .uri(uri)
            .header("Content-Type", "application/yaml")
            .body(Full::new(Bytes::from(manifest_yaml.to_vec())))
            .map_err(|e| RuntimeError::CommandFailed(format!("Failed to build request: {}", e)))?;

        let (status, body) = self.execute_request(request).await?;

        if !status.is_success() {
            let error_msg = self.parse_error_response(&body, status);
            return Err(RuntimeError::CommandFailed(error_msg));
        }

        serde_json::from_slice(&body)
            .map_err(|e| RuntimeError::CommandFailed(format!("Failed to parse play kube down response: {}", e)))
    }

    /// List all pods
    pub async fn list_pods(&self) -> Result<Vec<PodListEntry>, RuntimeError> {
        let uri = self.build_uri("/pods/json");
        debug!("Listing pods: {:?}", uri);

        let request = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .body(Full::new(Bytes::new()))
            .map_err(|e| RuntimeError::CommandFailed(format!("Failed to build request: {}", e)))?;

        let (status, body) = self.execute_request(request).await?;

        if !status.is_success() {
            let error_msg = self.parse_error_response(&body, status);
            return Err(RuntimeError::CommandFailed(error_msg));
        }

        // Handle empty response (no pods)
        if body.is_empty() || body.as_ref() == b"null" {
            return Ok(Vec::new());
        }

        serde_json::from_slice(&body)
            .map_err(|e| RuntimeError::CommandFailed(format!("Failed to parse pods list: {} - body: {}", e, String::from_utf8_lossy(&body))))
    }

    /// Inspect a specific pod by name
    pub async fn inspect_pod(&self, pod_name: &str) -> Result<PodInspectResponse, RuntimeError> {
        let uri = self.build_uri(&format!("/pods/{}/json", pod_name));
        debug!("Inspecting pod: {:?}", uri);

        let request = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .body(Full::new(Bytes::new()))
            .map_err(|e| RuntimeError::CommandFailed(format!("Failed to build request: {}", e)))?;

        let (status, body) = self.execute_request(request).await?;

        if status == StatusCode::NOT_FOUND {
            return Err(RuntimeError::WorkloadNotFound(format!("Pod not found: {}", pod_name)));
        }

        if !status.is_success() {
            let error_msg = self.parse_error_response(&body, status);
            return Err(RuntimeError::CommandFailed(error_msg));
        }

        serde_json::from_slice(&body)
            .map_err(|e| RuntimeError::CommandFailed(format!("Failed to parse pod inspect response: {}", e)))
    }

    /// Remove a pod by name
    pub async fn remove_pod(&self, pod_name: &str, force: bool) -> Result<(), RuntimeError> {
        let uri = if force {
            self.build_uri_with_query(&format!("/pods/{}", pod_name), "force=true")
        } else {
            self.build_uri(&format!("/pods/{}", pod_name))
        };
        debug!("Removing pod: {:?}", uri);

        let request = Request::builder()
            .method(Method::DELETE)
            .uri(uri)
            .body(Full::new(Bytes::new()))
            .map_err(|e| RuntimeError::CommandFailed(format!("Failed to build request: {}", e)))?;

        let (status, body) = self.execute_request(request).await?;

        if status == StatusCode::NOT_FOUND {
            // Pod doesn't exist - treat as success for idempotency
            debug!("Pod {} not found during removal (already removed)", pod_name);
            return Ok(());
        }

        if !status.is_success() {
            let error_msg = self.parse_error_response(&body, status);
            return Err(RuntimeError::CommandFailed(error_msg));
        }

        debug!("Successfully removed pod: {}", pod_name);
        Ok(())
    }

    /// Generate a Kubernetes manifest from a running pod
    ///
    /// This is equivalent to `podman generate kube pod_name`
    pub async fn generate_kube(&self, pod_name: &str) -> Result<String, RuntimeError> {
        let uri = self.build_uri(&format!("/generate/{}/kube", pod_name));
        debug!("Generating Kubernetes manifest for pod: {:?}", uri);

        let request = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .body(Full::new(Bytes::new()))
            .map_err(|e| RuntimeError::CommandFailed(format!("Failed to build request: {}", e)))?;

        let (status, body) = self.execute_request(request).await?;

        if status == StatusCode::NOT_FOUND {
            return Err(RuntimeError::WorkloadNotFound(format!("Pod not found: {}", pod_name)));
        }

        if !status.is_success() {
            let error_msg = self.parse_error_response(&body, status);
            return Err(RuntimeError::CommandFailed(error_msg));
        }

        String::from_utf8(body.to_vec())
            .map_err(|e| RuntimeError::CommandFailed(format!("Invalid UTF-8 in generated manifest: {}", e)))
    }

    /// Get logs from a pod's containers
    pub async fn get_pod_logs(&self, pod_name: &str, tail: Option<usize>) -> Result<String, RuntimeError> {
        let mut query_params = Vec::new();
        if let Some(lines) = tail {
            query_params.push(format!("tail={}", lines));
        }
        // Add follow=false to ensure we don't stream
        query_params.push("follow=false".to_string());

        let path = format!("/pods/{}/logs", pod_name);
        let uri = if query_params.is_empty() {
            self.build_uri(&path)
        } else {
            self.build_uri_with_query(&path, &query_params.join("&"))
        };

        debug!("Getting pod logs: {:?}", uri);

        let request = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .body(Full::new(Bytes::new()))
            .map_err(|e| RuntimeError::CommandFailed(format!("Failed to build request: {}", e)))?;

        let (status, body) = self.execute_request(request).await?;

        if status == StatusCode::NOT_FOUND {
            return Err(RuntimeError::WorkloadNotFound(format!("Pod not found: {}", pod_name)));
        }

        if !status.is_success() {
            let error_msg = self.parse_error_response(&body, status);
            return Err(RuntimeError::CommandFailed(error_msg));
        }

        Ok(String::from_utf8_lossy(&body).to_string())
    }

    /// Parse an error response from the API
    fn parse_error_response(&self, body: &Bytes, status: StatusCode) -> String {
        // Try to parse as JSON error response
        if let Ok(error_response) = serde_json::from_slice::<ApiErrorResponse>(body) {
            let mut parts = Vec::new();
            if let Some(message) = error_response.message {
                parts.push(message);
            }
            if let Some(cause) = error_response.cause {
                parts.push(format!("cause: {}", cause));
            }
            if !parts.is_empty() {
                return format!("HTTP {} - {}", status, parts.join(" - "));
            }
        }

        // Fall back to raw body text
        let body_text = String::from_utf8_lossy(body);
        if body_text.is_empty() {
            format!("HTTP {} - No error details provided", status)
        } else {
            format!("HTTP {} - {}", status, body_text)
        }
    }

    /// Check if an error indicates a socket connection problem
    pub fn is_connection_error(error: &RuntimeError) -> bool {
        if let RuntimeError::CommandFailed(msg) = error {
            let lower = msg.to_ascii_lowercase();
            lower.contains("connection refused")
                || lower.contains("no such file")
                || lower.contains("connection reset")
                || lower.contains("broken pipe")
                || lower.contains("unix socket")
                || lower.contains("hyper")
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_socket_path() {
        assert_eq!(
            PodmanApiClient::normalize_socket_path("unix:///run/podman/podman.sock"),
            "/run/podman/podman.sock"
        );
        assert_eq!(
            PodmanApiClient::normalize_socket_path("/run/podman/podman.sock"),
            "/run/podman/podman.sock"
        );
    }

    #[test]
    fn test_build_uri() {
        let client = PodmanApiClient::new("/run/podman/podman.sock");
        let uri = client.build_uri("/pods/json");
        let uri_str = uri.to_string();
        // The URI should contain the path
        assert!(uri_str.contains("/v5.0.0/libpod/pods/json"), "URI was: {}", uri_str);
    }

    #[test]
    fn test_build_uri_with_query() {
        let client = PodmanApiClient::new("/run/podman/podman.sock");
        let uri = client.build_uri_with_query("/play/kube", "replace=true");
        let uri_str = uri.to_string();
        assert!(uri_str.contains("replace=true"), "URI was: {}", uri_str);
    }

    #[test]
    fn test_parse_play_kube_response() {
        let json = r#"{
            "Pods": [
                {
                    "ID": "abc123",
                    "Containers": ["container1", "container2"]
                }
            ],
            "Volumes": []
        }"#;

        let response: PlayKubeResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.pods.len(), 1);
        assert_eq!(response.pods[0].id, Some("abc123".to_string()));
    }

    #[test]
    fn test_parse_pod_list() {
        let json = r#"[
            {
                "Id": "pod123",
                "Name": "beemesh-test-pod",
                "Status": "Running",
                "Created": "2024-01-01T00:00:00Z"
            }
        ]"#;

        let pods: Vec<PodListEntry> = serde_json::from_str(json).unwrap();
        assert_eq!(pods.len(), 1);
        assert_eq!(pods[0].name, Some("beemesh-test-pod".to_string()));
        assert_eq!(pods[0].status, Some("Running".to_string()));
    }

    #[test]
    fn test_is_connection_error() {
        assert!(PodmanApiClient::is_connection_error(&RuntimeError::CommandFailed(
            "connection refused".to_string()
        )));
        assert!(PodmanApiClient::is_connection_error(&RuntimeError::CommandFailed(
            "No such file or directory".to_string()
        )));
        assert!(!PodmanApiClient::is_connection_error(&RuntimeError::CommandFailed(
            "Pod not found".to_string()
        )));
    }
}

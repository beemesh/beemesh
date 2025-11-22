//! Helper functions for interacting with the Kubernetes-compatible API.
//!
//! This module provides functions to simulate `kubectl apply` and `kubectl delete`
//! by sending HTTP requests to the machineplane's API endpoints.

use anyhow::{Context, Result, anyhow};
use reqwest::{Client, StatusCode};
use serde_json::Value;
use std::path::Path;
use tokio::fs;

/// Extracts metadata (namespace, name, kind) from a Kubernetes manifest.
fn manifest_metadata(manifest: &Value) -> Result<(String, String, String)> {
    let metadata = manifest
        .get("metadata")
        .and_then(|v| v.as_object())
        .ok_or_else(|| anyhow!("manifest missing metadata"))?;

    let name = metadata
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("manifest missing metadata.name"))?
        .to_string();

    let namespace = metadata
        .get("namespace")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "default".to_string());

    let kind = manifest
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or("Deployment")
        .to_string();

    Ok((namespace, name, kind))
}

/// Computes a deterministic manifest ID based on namespace, name, and kind.
fn compute_manifest_id(namespace: &str, name: &str, kind: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    namespace.hash(&mut hasher);
    name.hash(&mut hasher);
    kind.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Loads and parses a YAML manifest from a file.
async fn load_manifest(path: &Path) -> Result<Value> {
    let contents = fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read manifest at {}", path.display()))?;
    let manifest: Value = serde_yaml::from_str(&contents)
        .with_context(|| format!("failed to parse manifest at {}", path.display()))?;
    Ok(manifest)
}

/// Simulates `kubectl apply` by sending a POST request to the API.
///
/// Returns the `manifest_id` on success.
pub async fn apply_manifest_via_kube_api(
    client: &Client,
    port: u16,
    manifest_path: &Path,
) -> Result<String> {
    let manifest = load_manifest(manifest_path).await?;
    let (namespace, _name, _kind) = manifest_metadata(&manifest)?;

    let url = format!(
        "http://127.0.0.1:{}/apis/apps/v1/namespaces/{}/deployments",
        port, namespace
    );

    let response = client
        .post(url)
        .json(&manifest)
        .send()
        .await
        .context("failed to send apply request")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "<unavailable>".to_string());
        return Err(anyhow!(
            "kubectl-compatible apply failed with HTTP {}: {}",
            status,
            body
        ));
    }

    let response_json: Value = response
        .json()
        .await
        .context("failed to decode apply response body")?;

    response_json
        .get("manifest_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("apply response missing manifest_id"))
}

/// Simulates `kubectl delete` by sending a DELETE request to the API.
///
/// If `force` is true, it adds query parameters to force immediate deletion.
/// Returns the `manifest_id` on success.
pub async fn delete_manifest_via_kube_api(
    client: &Client,
    port: u16,
    manifest_path: &Path,
    force: bool,
) -> Result<String> {
    let manifest = load_manifest(manifest_path).await?;
    let (namespace, name, kind) = manifest_metadata(&manifest)?;
    let manifest_id = compute_manifest_id(&namespace, &name, &kind);

    let mut request = client.delete(format!(
        "http://127.0.0.1:{}/apis/apps/v1/namespaces/{}/deployments/{}",
        port, namespace, name
    ));

    if force {
        request = request.query(&[
            ("gracePeriodSeconds", "0"),
            ("propagationPolicy", "Foreground"),
        ]);
    }

    let response = request
        .send()
        .await
        .context("failed to send delete request")?;

    match response.status() {
        status if status.is_success() => Ok(manifest_id),
        StatusCode::NOT_FOUND => Err(anyhow!(
            "delete failed: deployment {} in namespace {} not found",
            name,
            namespace
        )),
        status => {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<unavailable>".to_string());
            Err(anyhow!(
                "kubectl-compatible delete failed with HTTP {}: {}",
                status,
                body
            ))
        }
    }
}

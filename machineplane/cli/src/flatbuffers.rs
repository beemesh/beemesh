use anyhow::Result;
use protocol::machine;
use serde_json::Value as JsonValue;

/// Convert JSON requests into flatbuffer payloads for direct communication with the machine
pub struct FlatbufferClient {
    base_url: String,
    client: reqwest::Client,
}

impl FlatbufferClient {
    pub fn new(base_url: String) -> Self {
        Self {
            base_url,
            client: reqwest::Client::new(),
        }
    }

    /// Create a task by sending flatbuffer data
    pub async fn create_task(
        &self,
        tenant: &str,
        manifest: &JsonValue,
        manifest_id: Option<String>,
        operation_id: Option<String>,
    ) -> Result<JsonValue> {
        let create_body = protocol::json::TaskCreateRequest {
            manifest: manifest.clone(),
            replicas: None,
            manifest_id,
            operation_id,
        };

        let url = format!("{}/tenant/{}/tasks", self.base_url.trim_end_matches('/'), tenant);
        let resp = self.client
            .post(&url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(&create_body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await?;
            anyhow::bail!("create_task failed: {} {}", status, body_text);
        }

        let response_json: JsonValue = resp.json().await?;
        Ok(response_json)
    }

    /// Get candidates using flatbuffer capacity request
    pub async fn get_candidates(
        &self,
        tenant: &str,
        task_id: &str,
    ) -> Result<Vec<String>> {
        let url = format!("{}/tenant/{}/tasks/{}/candidates", 
                         self.base_url.trim_end_matches('/'), tenant, task_id);
        let resp = self.client.get(&url).send().await?;
        
        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await?;
            anyhow::bail!("get_candidates failed: {} {}", status, body_text);
        }
        
        let response_json: JsonValue = resp.json().await?;
        let responders = response_json
            .get("responders")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        
        let peers: Vec<String> = responders
            .into_iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
        
        Ok(peers)
    }

    /// Distribute keyshares using flatbuffer envelope
    pub async fn distribute_shares(
        &self,
        tenant: &str,
        task_id: &str,
        shares_envelope: &JsonValue,
        targets: &[ShareTarget],
    ) -> Result<JsonValue> {
        let request = protocol::json::DistributeSharesRequest {
            shares_envelope: Some(shares_envelope.clone()),
            targets: targets.iter().map(|t| protocol::json::ShareTarget {
                peer_id: t.peer_id.clone(),
                payload: t.payload.clone(),
            }).collect(),
        };

        let url = format!("{}/tenant/{}/tasks/{}/distribute_shares", 
                         self.base_url.trim_end_matches('/'), tenant, task_id);
        let resp = self.client
            .post(&url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(&request)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await?;
            anyhow::bail!("distribute_shares failed: {} {}", status, body_text);
        }

        let response_json: JsonValue = resp.json().await?;
        Ok(response_json)
    }

    /// Distribute capability tokens using flatbuffer envelope
    pub async fn distribute_capabilities(
        &self,
        tenant: &str,
        task_id: &str,
        targets: &[CapabilityTarget],
    ) -> Result<JsonValue> {
        let request = protocol::json::DistributeCapabilitiesRequest {
            targets: targets.iter().map(|t| protocol::json::CapabilityTarget {
                peer_id: t.peer_id.clone(),
                payload: t.payload.clone(),
            }).collect(),
        };

        let url = format!("{}/tenant/{}/tasks/{}/distribute_capabilities", 
                         self.base_url.trim_end_matches('/'), tenant, task_id);
        let resp = self.client
            .post(&url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(&request)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await?;
            anyhow::bail!("distribute_capabilities failed: {} {}", status, body_text);
        }

        let response_json: JsonValue = resp.json().await?;
        Ok(response_json)
    }

    /// Assign task using flatbuffer apply request
    pub async fn assign_task(
        &self,
        tenant: &str,
        task_id: &str,
        chosen_peers: Vec<String>,
    ) -> Result<JsonValue> {
        let request = protocol::json::AssignRequest {
            chosen_peers: Some(chosen_peers),
        };

        let url = format!("{}/tenant/{}/tasks/{}/assign", 
                         self.base_url.trim_end_matches('/'), tenant, task_id);
        let resp = self.client
            .post(&url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(&request)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await?;
            anyhow::bail!("assign_task failed: {} {}", status, body_text);
        }

        let response_json: JsonValue = resp.json().await?;
        Ok(response_json)
    }

    /// Get manifest ID for a task
    pub async fn get_task_manifest_id(
        &self,
        tenant: &str,
        task_id: &str,
    ) -> Result<String> {
        let url = format!("{}/tenant/{}/tasks/{}/manifest_id", 
                         self.base_url.trim_end_matches('/'), tenant, task_id);
        let resp = self.client.get(&url).send().await?;
        
        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await?;
            anyhow::bail!("get_task_manifest_id failed: {} {}", status, body_text);
        }
        
        let response_json: JsonValue = resp.json().await?;
        let manifest_id = response_json
            .get("manifest_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("no manifest_id in response"))?
            .to_string();
        
        Ok(manifest_id)
    }
}

/// Helper structures for building flatbuffer requests
#[derive(Debug, Clone)]
pub struct ShareTarget {
    pub peer_id: String,
    pub payload: JsonValue,
}

#[derive(Debug, Clone)]
pub struct CapabilityTarget {
    pub peer_id: String,
    pub payload: JsonValue,
}

/// Build a flatbuffer envelope from JSON envelope data
pub fn build_flatbuffer_envelope(
    payload: &[u8],
    payload_type: &str,
    nonce: &str,
    ts: u64,
    alg: &str,
    sig: &str,
    pubkey: &str,
) -> Vec<u8> {
    // Parse signature to extract prefix and base64 part
    let (sig_prefix, sig_b64) = if sig.contains(':') {
        let parts: Vec<&str> = sig.splitn(2, ':').collect();
        (parts[0], parts[1])
    } else {
        ("ml-dsa-65", sig)
    };

    machine::build_envelope_signed(
        payload,
        payload_type,
        nonce,
        ts,
        alg,
        sig_prefix,
        sig_b64,
        pubkey,
    )
}

/// Create a capacity request flatbuffer
pub fn build_capacity_request_flatbuffer(
    cpu_milli: u32,
    memory_bytes: u64,
    storage_bytes: u64,
    replicas: u32,
) -> Vec<u8> {
    machine::build_capacity_request(cpu_milli, memory_bytes, storage_bytes, replicas)
}

/// Create an apply request flatbuffer from manifest data
pub fn build_apply_request_flatbuffer(
    replicas: u32,
    tenant: &str,
    operation_id: &str,
    manifest_json: &str,
    origin_peer: &str,
) -> Vec<u8> {
    machine::build_apply_request(replicas, tenant, operation_id, manifest_json, origin_peer)
}

/// Create a keyshare request flatbuffer
pub fn build_keyshare_request_flatbuffer(
    manifest_id: &str,
    capability: &str,
) -> Vec<u8> {
    machine::build_keyshare_request(manifest_id, capability)
}
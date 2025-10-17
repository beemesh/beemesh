use anyhow::Result;
use base64::Engine;
use protocol::machine::{self};
use serde_json::Value as JsonValue;

/// Convert JSON requests into flatbuffer payloads for direct communication with the machine
pub struct FlatbufferClient {
    base_url: String,
    client: reqwest::Client,
    /// CLI's private key for decryption (signing key private bytes)
    private_key: Vec<u8>,
    /// CLI's signing public key (base64)
    public_key: String,
    /// CLI's KEM public key bytes (raw) used to advertise to machines
    kem_public_key: Vec<u8>,
    /// CLI's KEM private key bytes (raw) used for decryption
    kem_private_key: Vec<u8>,
    /// Machine's public key for encryption
    machine_public_key: Option<Vec<u8>>,
}

impl FlatbufferClient {
    pub fn new(base_url: String) -> Result<Self> {
        // Generate ephemeral signing keypair for CLI
        crypto::ensure_pqc_init()?;
        let (public_key_bytes, private_key) = crypto::ensure_keypair_ephemeral()?;
        let public_key = base64::engine::general_purpose::STANDARD.encode(&public_key_bytes);

        // Obtain or generate a KEM keypair for the CLI so we can advertise our KEM public key
        // to machines (used to encrypt responses back to the CLI).
        // Use the existing helper which supports ephemeral KEM mode via env var.
        let (kem_pub_bytes, kem_priv_bytes) = crypto::ensure_kem_keypair_on_disk()?;

        Ok(Self {
            base_url,
            client: reqwest::Client::new(),
            private_key,
            public_key,
            kem_public_key: kem_pub_bytes,
            kem_private_key: kem_priv_bytes,
            machine_public_key: None,
        })
    }

    /// Fetch the machine's KEM public key from the /api/v1/kem_pubkey endpoint for encryption
    pub async fn fetch_machine_public_key(&mut self) -> Result<()> {
        let url = format!("{}/api/v1/kem_pubkey", self.base_url);
        log::info!("Fetching machine KEM public key from: {}", url);

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to connect to machine at {}: {}", url, e))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp
                .text()
                .await
                .unwrap_or_else(|_| "Unable to read response body".to_string());
            anyhow::bail!(
                "Failed to fetch machine pubkey: {} - Response: {}",
                status,
                body
            );
        }

        let pubkey_b64 = resp
            .text()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get response text: {}", e))?;

        // Check for error response
        if pubkey_b64.starts_with("ERROR:") {
            anyhow::bail!("Machine returned error: {}", pubkey_b64);
        }

        log::debug!("Received pubkey response: {}", pubkey_b64);

        let pubkey_bytes = base64::engine::general_purpose::STANDARD
            .decode(&pubkey_b64)
            .map_err(|e| anyhow::anyhow!("Failed to decode public key: {}", e))?;

        log::info!(
            "Successfully fetched machine KEM public key ({} bytes)",
            pubkey_bytes.len()
        );
        self.machine_public_key = Some(pubkey_bytes);
        Ok(())
    }

    /// Decrypt a response from the machine
    fn decrypt_from_machine(&self, envelope_bytes: &[u8]) -> Result<Vec<u8>> {
        // Parse the envelope first
        let envelope = protocol::machine::root_as_envelope(envelope_bytes)
            .map_err(|e| anyhow::anyhow!("Failed to parse envelope: {}", e))?;

        // Extract the payload from the envelope
        let payload_bytes = envelope
            .payload()
            .map(|v| v.iter().collect::<Vec<u8>>())
            .unwrap_or_default();

        // If the payload is encrypted (starts with 0x02), decrypt it
        if !payload_bytes.is_empty() && payload_bytes[0] == 0x02 {
            crypto::decrypt_payload_from_recipient_blob(&payload_bytes, &self.kem_private_key)
        } else {
            // Payload is not encrypted, return as-is
            Ok(payload_bytes)
        }
    }

    /// Send an unencrypted flatbuffer request (for already-signed envelopes)
    #[allow(dead_code)]
    async fn send_unencrypted_request(
        &self,
        url: &str,
        payload: &[u8],
        payload_type: &str,
    ) -> Result<Vec<u8>> {
        // Create unencrypted envelope with peer_id and public key
        use crypto::sign_envelope;
        use protocol::machine::{
            build_envelope_canonical_with_peer, build_envelope_signed_with_peer,
        };

        // Generate nonce and timestamp
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        std::time::SystemTime::now().hash(&mut hasher);
        let nonce = format!("{:x}", hasher.finish());
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Build canonical envelope for signing (with peer_id).
        let kem_pub_b64 = base64::engine::general_purpose::STANDARD.encode(&self.kem_public_key);
        let canonical = build_envelope_canonical_with_peer(
            payload,
            payload_type,
            &nonce,
            ts,
            "ml-dsa-65",
            "cli-client",
            Some(kem_pub_b64.as_str()),
        );

        // Sign the envelope
        // Decode public key from base64 for signing
        let public_key_bytes = base64::engine::general_purpose::STANDARD
            .decode(&self.public_key)
            .map_err(|e| anyhow::anyhow!("Failed to decode public key: {}", e))?;
        let (sig_b64, pub_b64) = sign_envelope(&self.private_key, &public_key_bytes, &canonical)?;

        // Create signed envelope
        let envelope = build_envelope_signed_with_peer(
            payload,
            payload_type,
            &nonce,
            ts,
            "ml-dsa-65",
            "ml-dsa-65",
            &sig_b64,
            &pub_b64,
            "cli-client",
            Some(kem_pub_b64.as_str()),
        );

        let resp = self
            .client
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .header("x-peer-id", "cli-client") // Identify as CLI client
            .body(envelope)
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(anyhow::anyhow!(
                "HTTP error {}: {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            ));
        }

        let response_bytes = resp.bytes().await?.to_vec();
        println!(
            "send_unencrypted_request: response_bytes.len()={}",
            response_bytes.len()
        );

        Ok(response_bytes)
    }

    /// Send an encrypted flatbuffer request
    async fn send_encrypted_request(
        &self,
        url: &str,
        payload: &[u8],
        payload_type: &str,
    ) -> Result<Vec<u8>> {
        let envelope = if let Some(machine_pubkey) = &self.machine_public_key {
            // Create encrypted envelope with peer_id. Include our KEM public key (base64)
            let kem_pub_b64 =
                base64::engine::general_purpose::STANDARD.encode(&self.kem_public_key);
            protocol::machine::build_encrypted_envelope_with_peer(
                payload,
                payload_type,
                machine_pubkey,
                &self.private_key,
                &self.public_key,
                "cli-client",
                Some(kem_pub_b64.as_str()),
            )?
        } else {
            // Create unencrypted envelope with peer_id and public key
            use crypto::sign_envelope;
            use protocol::machine::{
                build_envelope_canonical_with_peer, build_envelope_signed_with_peer,
            };

            // Generate nonce and timestamp
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut hasher = DefaultHasher::new();
            std::time::SystemTime::now().hash(&mut hasher);
            let nonce = format!("{:x}", hasher.finish());
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            // Build canonical envelope for signing (with peer_id).
            // Include the CLI's advertised KEM public key (base64) in the canonical
            // bytes so the header-provided KEM key is integrity-protected by the
            // envelope signature.
            let kem_pub_b64 =
                base64::engine::general_purpose::STANDARD.encode(&self.kem_public_key);
            let canonical = build_envelope_canonical_with_peer(
                payload,
                payload_type,
                &nonce,
                ts,
                "ml-dsa-65",
                "cli-client",
                Some(kem_pub_b64.as_str()),
            );

            // Decode sender public key from base64 for signing
            let sender_pubkey_bytes = base64::engine::general_purpose::STANDARD
                .decode(&self.public_key)
                .map_err(|e| anyhow::anyhow!("Failed to decode sender public key: {}", e))?;

            // Sign the canonical envelope
            let signature = sign_envelope(&self.private_key, &sender_pubkey_bytes, &canonical)?;

            // Build the final signed envelope with unencrypted payload (with peer_id)
            build_envelope_signed_with_peer(
                payload,
                payload_type,
                &nonce,
                ts,
                "ml-dsa-65",
                "ml-dsa-65",
                &signature.0,
                &signature.1,
                "cli-client",
                Some(kem_pub_b64.as_str()),
            )
        };

        let resp = self
            .client
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .header("x-peer-id", "cli-client") // Identify as CLI client
            .body(envelope)
            .send()
            .await?;

        let status = resp.status();

        // Treat 405 Method Not Allowed specially: some clients (flatbuffer clients)
        // POST envelope payloads and expect the server to return a body even when the
        // server doesn't consider the request a 'success' status. In that case we
        // return the response body bytes instead of treating it as an error.
        if status.as_u16() == 405 {
            let body_text = resp.text().await?;
            return Ok(body_text.into_bytes());
        }

        if !status.is_success() {
            let body_text = resp.text().await?;
            anyhow::bail!("Request failed: {} {}", status, body_text);
        }

        let response_bytes = resp.bytes().await?;

        // Try to decrypt if it looks like an encrypted envelope
        if response_bytes.len() > 100 && self.machine_public_key.is_some() {
            println!("send_encrypted_request: response length={}, machine_pubkey available, attempting envelope parsing", response_bytes.len());
            log::info!("send_encrypted_request: response length={}, machine_pubkey available, attempting envelope parsing", response_bytes.len());

            // Try to parse as envelope first
            match protocol::machine::root_as_envelope(&response_bytes) {
                Ok(envelope) => {
                    println!(
                        "send_encrypted_request: Detected envelope response, attempting decryption"
                    );
                    log::info!(
                        "send_encrypted_request: Detected envelope response, attempting decryption"
                    );
                    println!(
                        "send_encrypted_request: envelope payload length: {:?}",
                        envelope.payload().map(|p| p.len())
                    );
                    log::info!(
                        "send_encrypted_request: envelope payload length: {:?}",
                        envelope.payload().map(|p| p.len())
                    );
                    // It's an envelope, try to decrypt
                    match self.decrypt_from_machine(&response_bytes) {
                        Ok(decrypted) => {
                            println!("send_encrypted_request: Decryption successful, decrypted length: {}", decrypted.len());
                            log::info!("send_encrypted_request: Decryption successful, decrypted length: {}", decrypted.len());
                            println!(
                                "send_encrypted_request: first 50 bytes of decrypted: {:?}",
                                &decrypted[..std::cmp::min(50, decrypted.len())]
                            );
                            log::info!(
                                "send_encrypted_request: first 50 bytes of decrypted: {:?}",
                                &decrypted[..std::cmp::min(50, decrypted.len())]
                            );
                            Ok(decrypted)
                        }
                        Err(e) => {
                            println!("send_encrypted_request: Decryption failed: {:?}, returning raw bytes", e);
                            log::warn!("send_encrypted_request: Decryption failed: {:?}, returning raw bytes", e);
                            // Decryption failed, return raw bytes
                            Ok(response_bytes.to_vec())
                        }
                    }
                }
                Err(e) => {
                    println!(
                        "send_encrypted_request: Not an envelope ({}), server likely sent unencrypted FlatBuffer response",
                        e
                    );
                    log::info!(
                        "send_encrypted_request: Not an envelope ({}), server likely sent unencrypted FlatBuffer response",
                        e
                    );
                    // Server sent unencrypted FlatBuffer response directly, return as-is
                    Ok(response_bytes.to_vec())
                }
            }
        } else {
            println!("send_encrypted_request: response_bytes.len()={}, machine_public_key.is_some()={}, skipping decryption", response_bytes.len(), self.machine_public_key.is_some());
            log::info!("send_encrypted_request: response_bytes.len()={}, machine_public_key.is_some()={}, skipping decryption", response_bytes.len(), self.machine_public_key.is_some());
            Ok(response_bytes.to_vec())
        }
    }

    /// Create a task by sending flatbuffer data
    pub async fn create_task(
        &self,
        tenant: &str,
        manifest_bytes: &[u8], // Raw flatbuffer bytes
        manifest_id: Option<String>,
        operation_id: Option<String>,
    ) -> Result<JsonValue> {
        let mut url = format!(
            "{}/tenant/{}/tasks",
            self.base_url.trim_end_matches('/'),
            tenant
        );

        // Add query parameters
        let mut query_params = Vec::new();
        if let Some(mid) = manifest_id {
            query_params.push(format!("manifest_id={}", mid));
        }
        if let Some(oid) = operation_id {
            query_params.push(format!("operation_id={}", oid));
        }
        if !query_params.is_empty() {
            url.push('?');
            url.push_str(&query_params.join("&"));
        }

        let response_bytes = self
            .send_encrypted_request(&url, manifest_bytes, "task_create_request")
            .await?;

        // Parse as flatbuffer TaskCreateResponse
        let task_response = protocol::machine::root_as_task_create_response(&response_bytes)
            .map_err(|e| anyhow::anyhow!("Failed to parse TaskCreateResponse: {}", e))?;

        Ok(serde_json::json!({
            "ok": task_response.ok(),
            "task_id": task_response.task_id().unwrap_or(""),
            "manifest_id": task_response.manifest_id().unwrap_or(""),
            "selection_window_ms": task_response.selection_window_ms()
        }))
    }

    /// Get candidates using flatbuffer capacity request
    pub async fn get_candidates(&self, tenant: &str, task_id: &str) -> Result<Vec<String>> {
        log::debug!(
            "get_candidates: called with tenant={}, task_id={}",
            tenant,
            task_id
        );
        let url = format!(
            "{}/tenant/{}/tasks/{}/candidates",
            self.base_url.trim_end_matches('/'),
            tenant,
            task_id
        );
        log::debug!("get_candidates: requesting URL: {}", url);
        println!("CLI: get_candidates requesting URL: {}", url);

        // Send empty payload for GET-like request
        log::debug!("get_candidates: sending request...");
        let response_bytes = self
            .send_encrypted_request(&url, &[], "candidates_request")
            .await?;

        log::info!(
            "get_candidates: received response, {} bytes",
            response_bytes.len()
        );

        // Try to parse as flatbuffer CandidatesResponse first
        log::debug!(
            "get_candidates: attempting to parse {} bytes as flatbuffer",
            response_bytes.len()
        );
        match protocol::machine::root_as_candidates_response(&response_bytes) {
            Ok(candidates_response) => {
                log::debug!(
                    "get_candidates: successfully parsed flatbuffer, ok={}",
                    candidates_response.ok()
                );
                if candidates_response.ok() {
                    let responders: Vec<String> = candidates_response
                        .candidates()
                        .map(|v| {
                            v.iter()
                                .filter_map(|candidate| {
                                    let peer_id = candidate.peer_id()?.to_string();
                                    let public_key =
                                        candidate.public_key().unwrap_or("").to_string();
                                    Some(format!("{}:{}", peer_id, public_key))
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    log::debug!("get_candidates: found {} responders", responders.len());
                    Ok(responders)
                } else {
                    log::debug!("get_candidates: flatbuffer indicates error, returning empty");
                    Ok(vec![])
                }
            }
            Err(e) => {
                // Fallback to JSON parsing for compatibility
                log::warn!("Failed to parse candidates response as flatbuffer: {:?}", e);
                log::warn!(
                    "First 100 bytes of response: {:?}",
                    &response_bytes[..std::cmp::min(100, response_bytes.len())]
                );
                let response_str = String::from_utf8(response_bytes.clone())?;
                log::warn!(
                    "Response string length: {}, content: '{}'",
                    response_str.len(),
                    response_str
                );
                if response_str.trim().is_empty() {
                    log::warn!("Response is empty, returning empty candidates list");
                    return Ok(vec![]);
                }
                let response_json: JsonValue = serde_json::from_str(&response_str)?;
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
        }
    }

    /// Distribute keyshares using pure flatbuffer envelopes
    #[allow(dead_code)]
    pub async fn distribute_shares_flatbuffer(
        &self,
        tenant: &str,
        task_id: &str,
        shares_envelope_b64: &str,
        peer_ids: &[String],
        flatbuffer_payloads: &[Vec<u8>],
    ) -> Result<JsonValue> {
        // Convert flatbuffer payloads to base64 strings for transport
        let fb_targets: Vec<(String, String)> = peer_ids
            .iter()
            .zip(flatbuffer_payloads.iter())
            .map(|(peer_id, fb_bytes)| {
                (
                    peer_id.clone(),
                    base64::engine::general_purpose::STANDARD.encode(fb_bytes),
                )
            })
            .collect();

        // Create flatbuffer request
        let flatbuffer_data =
            machine::build_distribute_shares_request(&shares_envelope_b64, &fb_targets);

        let url = format!(
            "{}/tenant/{}/tasks/{}/distribute_shares",
            self.base_url.trim_end_matches('/'),
            tenant,
            task_id
        );

        let response_bytes = self
            .send_encrypted_request(&url, &flatbuffer_data, "distribute_shares_request")
            .await?;

        // Try to parse as flatbuffer DistributeSharesResponse first
        let shares_response =
            protocol::machine::root_as_distribute_shares_response(&response_bytes)
                .map_err(|e| anyhow::anyhow!("Failed to parse DistributeSharesResponse: {}", e))?;

        let results_json = shares_response.results_json().unwrap_or("{}");
        let results: JsonValue =
            serde_json::from_str(results_json).unwrap_or(serde_json::json!({}));
        Ok(serde_json::json!({
            "ok": shares_response.ok(),
            "results": results
        }))
    }

    /// Distribute capability tokens using flatbuffer envelope
    #[allow(dead_code)]
    pub async fn distribute_capabilities(
        &self,
        tenant: &str,
        task_id: &str,
        targets: &[CapabilityTarget],
    ) -> Result<JsonValue> {
        // Convert targets to the required format for flatbuffer
        let fb_targets: Vec<(String, String)> = targets
            .iter()
            .map(|t| {
                let b64_capability = base64::engine::general_purpose::STANDARD.encode(&t.payload);
                eprintln!("CLI: Encoding capability for {}: payload_len={}, b64_len={}, raw_bytes_prefix={:02x?}",
                         t.peer_id, t.payload.len(), b64_capability.len(),
                         &t.payload[..std::cmp::min(20, t.payload.len())]);
                eprintln!("CLI: Base64 capability prefix={}", &b64_capability[..std::cmp::min(100, b64_capability.len())]);
                (
                    t.peer_id.clone(),
                    b64_capability,
                )
            })
            .collect();

        // Create flatbuffer request
        let flatbuffer_data = machine::build_distribute_capabilities_request(&fb_targets);

        let url = format!(
            "{}/tenant/{}/tasks/{}/distribute_capabilities",
            self.base_url.trim_end_matches('/'),
            tenant,
            task_id
        );

        let response_bytes = self
            .send_encrypted_request(&url, &flatbuffer_data, "distribute_capabilities_request")
            .await?;

        // Try to parse as flatbuffer DistributeCapabilitiesResponse first
        let capabilities_response = protocol::machine::root_as_distribute_capabilities_response(
            &response_bytes,
        )
        .map_err(|e| anyhow::anyhow!("Failed to parse DistributeCapabilitiesResponse: {}", e))?;

        let results_json = capabilities_response.results_json().unwrap_or("{}");
        let results: JsonValue =
            serde_json::from_str(results_json).unwrap_or(serde_json::json!({}));
        Ok(serde_json::json!({
            "ok": capabilities_response.ok(),
            "results": results
        }))
    }

    /// Distribute encrypted manifests to candidate nodes
    pub async fn distribute_manifests(
        &self,
        tenant: &str,
        task_id: &str,
        encrypted_manifest_b64: &str,
        target_peer_ids: &[String],
    ) -> Result<JsonValue> {
        log::debug!(
            "distribute_manifests called with tenant={}, task_id={}, targets={}",
            tenant,
            task_id,
            target_peer_ids.len()
        );

        // Create manifest distribution targets
        let mut fb_targets = Vec::new();
        for peer_id in target_peer_ids {
            fb_targets.push((peer_id.clone(), encrypted_manifest_b64.to_string()));
        }

        // Create flatbuffer request
        let flatbuffer_data = machine::build_distribute_manifests_request(
            task_id,
            encrypted_manifest_b64,
            &fb_targets,
        );

        let url = format!(
            "{}/tenant/{}/tasks/{}/distribute_manifests",
            self.base_url.trim_end_matches('/'),
            tenant,
            task_id
        );

        let response_bytes = self
            .send_encrypted_request(&url, &flatbuffer_data, "distribute_manifests_request")
            .await?;

        // Try to parse as flatbuffer DistributeManifestsResponse first
        let manifests_response = protocol::machine::root_as_distribute_manifests_response(
            &response_bytes,
        )
        .map_err(|e| anyhow::anyhow!("Failed to parse DistributeManifestsResponse: {}", e))?;

        let results_json = manifests_response.results_json().unwrap_or("{}");
        let results: JsonValue =
            serde_json::from_str(results_json).unwrap_or(serde_json::json!({}));
        Ok(serde_json::json!({
            "ok": manifests_response.ok(),
            "results": results
        }))
    }

    /// Assign task using flatbuffer apply request
    pub async fn assign_task(
        &self,
        tenant: &str,
        task_id: &str,
        chosen_peers: Vec<String>,
    ) -> Result<JsonValue> {
        log::info!(
            "assign_task called with tenant={}, task_id={}, chosen_peers={:?}",
            tenant,
            task_id,
            chosen_peers
        );

        // Create flatbuffer request
        let flatbuffer_data = machine::build_assign_request(&chosen_peers);

        let url = format!(
            "{}/tenant/{}/tasks/{}/assign",
            self.base_url.trim_end_matches('/'),
            tenant,
            task_id
        );

        let response_bytes = self
            .send_encrypted_request(&url, &flatbuffer_data, "assign_request")
            .await?;

        log::info!(
            "assign_task response_bytes length: {}",
            response_bytes.len()
        );
        log::info!(
            "assign_task response_bytes preview: {:?}",
            &response_bytes[..std::cmp::min(50, response_bytes.len())]
        );

        // Try to parse as flatbuffer AssignResponse first
        let assign_response = protocol::machine::root_as_assign_response(&response_bytes)
            .map_err(|e| anyhow::anyhow!("Failed to parse AssignResponse: {}", e))?;

        let assigned_peers: Vec<String> = assign_response
            .assigned_peers()
            .map(|v| v.iter().map(|s| s.to_string()).collect())
            .unwrap_or_default();
        let per_peer_json = assign_response.per_peer_results_json().unwrap_or("{}");
        let per_peer: JsonValue =
            serde_json::from_str(per_peer_json).unwrap_or(serde_json::json!({}));

        Ok(serde_json::json!({
            "ok": assign_response.ok(),
            "task_id": assign_response.task_id().unwrap_or(""),
            "assigned_peers": assigned_peers,
            "per_peer": per_peer
        }))
    }
}

/// Helper structure for capability distribution
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CapabilityTarget {
    pub peer_id: String,
    pub payload: Vec<u8>,
}

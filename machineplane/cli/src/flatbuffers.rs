use anyhow::Result;
use base64::Engine;
use rand;
use serde_json::Value as JsonValue;

/// Convert JSON requests into flatbuffer payloads for direct communication with the machine
pub struct FlatbufferClient {
    pub base_url: String,
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
        // Use persistent keypairs from disk to match machine expectations
        crypto::ensure_pqc_init()?;
        let (public_key_bytes, private_key) = crypto::ensure_keypair_on_disk()?;
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
    pub async fn send_encrypted_request(
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

            // Generate unique random nonce and timestamp
            let nonce_bytes: [u8; 16] = rand::random();
            let nonce = base64::engine::general_purpose::STANDARD.encode(&nonce_bytes);
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
            println!(
                "send_encrypted_request: response length={}, machine_pubkey available, attempting envelope parsing",
                response_bytes.len()
            );
            log::info!(
                "send_encrypted_request: response length={}, machine_pubkey available, attempting envelope parsing",
                response_bytes.len()
            );

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
                            println!(
                                "send_encrypted_request: Decryption successful, decrypted length: {}",
                                decrypted.len()
                            );
                            log::info!(
                                "send_encrypted_request: Decryption successful, decrypted length: {}",
                                decrypted.len()
                            );
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
                            println!(
                                "send_encrypted_request: Decryption failed: {:?}, returning raw bytes",
                                e
                            );
                            log::warn!(
                                "send_encrypted_request: Decryption failed: {:?}, returning raw bytes",
                                e
                            );
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
            println!(
                "send_encrypted_request: response_bytes.len()={}, machine_public_key.is_some()={}, skipping decryption",
                response_bytes.len(),
                self.machine_public_key.is_some()
            );
            log::info!(
                "send_encrypted_request: response_bytes.len()={}, machine_public_key.is_some()={}, skipping decryption",
                response_bytes.len(),
                self.machine_public_key.is_some()
            );
            Ok(response_bytes.to_vec())
        }
    }

    /// Get candidates using flatbuffer capacity request
    pub async fn get_candidates(&self, task_id: &str) -> Result<Vec<String>> {
        log::debug!("get_candidates: called with task_id={}", task_id);
        let url = format!(
            "{}/tasks/{}/candidates",
            self.base_url.trim_end_matches('/'),
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

    /// Send a DELETE request with optional body
    pub async fn send_delete_request(&self, url: &str, body: &[u8]) -> Result<Vec<u8>> {
        // Wrap the delete request in a signed envelope
        let envelope = if let Some(machine_pubkey) = &self.machine_public_key {
            // Create encrypted envelope with peer_id
            let kem_pub_b64 =
                base64::engine::general_purpose::STANDARD.encode(&self.kem_public_key);
            protocol::machine::build_encrypted_envelope_with_peer(
                body,
                "delete_request",
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

            // Generate unique random nonce and timestamp
            let nonce_bytes: [u8; 16] = rand::random();
            let nonce = base64::engine::general_purpose::STANDARD.encode(&nonce_bytes);
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            // Build canonical envelope for signing (with peer_id)
            let kem_pub_b64 =
                base64::engine::general_purpose::STANDARD.encode(&self.kem_public_key);
            let canonical = build_envelope_canonical_with_peer(
                body,
                "delete_request",
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
                body,
                "delete_request",
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
            .delete(url)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .header("x-peer-id", "cli-client") // Identify as CLI client
            .body(envelope)
            .send()
            .await?;

        let status = resp.status();

        if !status.is_success() {
            let body_text = resp.text().await?;
            anyhow::bail!("Delete request failed: {} {}", status, body_text);
        }

        let response_bytes = resp.bytes().await?;

        // Try to decrypt if it looks like an encrypted envelope
        if response_bytes.len() > 100 && self.machine_public_key.is_some() {
            // Try to parse as envelope first
            match protocol::machine::root_as_envelope(&response_bytes) {
                Ok(_envelope) => {
                    // It's an envelope, try to decrypt
                    match self.decrypt_from_machine(&response_bytes) {
                        Ok(decrypted) => {
                            log::info!(
                                "send_delete_request: Decryption successful, decrypted length: {}",
                                decrypted.len()
                            );
                            return Ok(decrypted);
                        }
                        Err(e) => {
                            log::warn!(
                                "send_delete_request: Decryption failed: {:?}, returning raw bytes",
                                e
                            );
                            // Decryption failed, return raw bytes
                            return Ok(response_bytes.to_vec());
                        }
                    }
                }
                Err(_) => {
                    log::info!(
                        "send_delete_request: Not an envelope, server sent unencrypted response"
                    );
                    // Server sent unencrypted response directly, return as-is
                    return Ok(response_bytes.to_vec());
                }
            }
        }

        Ok(response_bytes.to_vec())
    }
}

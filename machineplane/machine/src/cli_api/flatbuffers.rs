use anyhow::Result;
use base64::Engine;
use rand;
use serde_json::Value as JsonValue;

pub struct FlatbufferClient {
    pub base_url: String,
    client: reqwest::Client,
    private_key: Vec<u8>,
    public_key: String,
    kem_public_key: Vec<u8>,
    kem_private_key: Vec<u8>,
    machine_public_key: Option<Vec<u8>>,
}

impl FlatbufferClient {
    pub fn new(base_url: String) -> Result<Self> {
        crypto::ensure_pqc_init()?;
        let (public_key_bytes, private_key) = crypto::ensure_keypair_on_disk()?;
        let public_key = base64::engine::general_purpose::STANDARD.encode(&public_key_bytes);

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

    fn decrypt_from_machine(&self, envelope_bytes: &[u8]) -> Result<Vec<u8>> {
        let envelope = protocol::machine::root_as_envelope(envelope_bytes)
            .map_err(|e| anyhow::anyhow!("Failed to parse envelope: {}", e))?;

        let payload_bytes = envelope
            .payload()
            .map(|v| v.iter().collect::<Vec<u8>>())
            .unwrap_or_default();

        if !payload_bytes.is_empty() && payload_bytes[0] == 0x02 {
            crypto::decrypt_payload_from_recipient_blob(&payload_bytes, &self.kem_private_key)
        } else {
            Ok(payload_bytes)
        }
    }

    #[allow(dead_code)]
    async fn send_unencrypted_request(
        &self,
        url: &str,
        payload: &[u8],
        payload_type: &str,
    ) -> Result<Vec<u8>> {
        use crypto::sign_envelope;
        use protocol::machine::{
            build_envelope_canonical_with_peer, build_envelope_signed_with_peer,
        };

        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        std::time::SystemTime::now().hash(&mut hasher);
        let nonce = format!("{:x}", hasher.finish());
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

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

        let public_key_bytes = base64::engine::general_purpose::STANDARD
            .decode(&self.public_key)
            .map_err(|e| anyhow::anyhow!("Failed to decode public key: {}", e))?;
        let (sig_b64, pub_b64) = sign_envelope(&self.private_key, &public_key_bytes, &canonical)?;

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
            .header("x-peer-id", "cli-client")
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

        Ok(response_bytes)
    }

    pub async fn send_encrypted_request(
        &self,
        url: &str,
        payload: &[u8],
        payload_type: &str,
    ) -> Result<Vec<u8>> {
        let kem_pub_b64 = base64::engine::general_purpose::STANDARD.encode(&self.kem_public_key);

        let envelope = if let Some(machine_pubkey) = &self.machine_public_key {
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
            use crypto::sign_envelope;
            use protocol::machine::{
                build_envelope_canonical_with_peer, build_envelope_signed_with_peer,
            };

            let nonce_bytes: [u8; 16] = rand::random();
            let nonce = base64::engine::general_purpose::STANDARD.encode(&nonce_bytes);
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            let canonical = build_envelope_canonical_with_peer(
                payload,
                payload_type,
                &nonce,
                ts,
                "ml-dsa-65",
                "cli-client",
                Some(kem_pub_b64.as_str()),
            );

            let sender_pubkey_bytes = base64::engine::general_purpose::STANDARD
                .decode(&self.public_key)
                .map_err(|e| anyhow::anyhow!("Failed to decode sender public key: {}", e))?;
            let signature = sign_envelope(&self.private_key, &sender_pubkey_bytes, &canonical)?;

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
            .header("x-peer-id", "cli-client")
            .body(envelope)
            .send()
            .await?;

        let status = resp.status();

        if !status.is_success() {
            let body_text = resp.text().await?;
            anyhow::bail!("Encrypted request failed: {} {}", status, body_text);
        }

        let response_bytes = resp.bytes().await?;

        if response_bytes.len() > 100 && self.machine_public_key.is_some() {
            match protocol::machine::root_as_envelope(&response_bytes) {
                Ok(_envelope) => match self.decrypt_from_machine(&response_bytes) {
                    Ok(decrypted) => {
                        log::info!(
                            "send_encrypted_request: Decryption successful, decrypted length: {}",
                            decrypted.len()
                        );
                        Ok(decrypted)
                    }
                    Err(e) => {
                        log::warn!(
                            "send_encrypted_request: Decryption failed: {:?}, returning raw bytes",
                            e
                        );
                        Ok(response_bytes.to_vec())
                    }
                },
                Err(_) => {
                    log::error!(
                        "send_encrypted_request: Not an envelope, server sent unencrypted response"
                    );
                    Ok(response_bytes.to_vec())
                }
            }
        } else {
            Ok(response_bytes.to_vec())
        }
    }

    pub async fn get_candidates(&self, task_id: &str) -> Result<Vec<String>> {
        let url = format!(
            "{}/tasks/{}/candidates",
            self.base_url.trim_end_matches('/'),
            task_id
        );

        let request_json = serde_json::json!({
            "task_id": task_id,
            "cli_pubkey": self.public_key,
            "cli_kem_pubkey": base64::engine::general_purpose::STANDARD
                .encode(&self.kem_public_key),
        });

        let mut builder = flatbuffers::FlatBufferBuilder::new();
        let json_str = serde_json::to_string(&request_json)?;
        let json_fb = builder.create_string(&json_str);
        let request = protocol::machine::build_generic_json_request(&mut builder, json_fb);
        builder.finish(request, None);

        let body = builder.finished_data();

        let response_bytes = self
            .send_encrypted_request(url.as_str(), body, "json_request")
            .await?;

        let parsed = protocol::machine::root_as_generic_json_response(&response_bytes)
            .map_err(|e| anyhow::anyhow!("Failed to parse GenericJsonResponse: {}", e))?;

        let json_bytes = parsed
            .json_body()
            .ok_or_else(|| anyhow::anyhow!("Missing json_body in response"))?
            .bytes()
            .map_err(|e| anyhow::anyhow!("Failed to get json_body bytes: {}", e))?;

        let json_value: JsonValue = serde_json::from_slice(&json_bytes)
            .map_err(|e| anyhow::anyhow!("Failed to parse json_body as JSON: {}", e))?;

        let peers = json_value
            .get("peers")
            .and_then(|v| v.as_array())
            .ok_or_else(|| anyhow::anyhow!("Missing peers array in response"))?
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();

        Ok(peers)
    }

    pub async fn send_delete_request(&self, url: &str, body: &[u8]) -> Result<Vec<u8>> {
        let envelope = if let Some(machine_pubkey) = &self.machine_public_key {
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
            use crypto::sign_envelope;
            use protocol::machine::{
                build_envelope_canonical_with_peer, build_envelope_signed_with_peer,
            };

            let nonce_bytes: [u8; 16] = rand::random();
            let nonce = base64::engine::general_purpose::STANDARD.encode(&nonce_bytes);
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

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

            let sender_pubkey_bytes = base64::engine::general_purpose::STANDARD
                .decode(&self.public_key)
                .map_err(|e| anyhow::anyhow!("Failed to decode sender public key: {}", e))?;

            let signature = sign_envelope(&self.private_key, &sender_pubkey_bytes, &canonical)?;

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
            .header("x-peer-id", "cli-client")
            .body(envelope)
            .send()
            .await?;

        let status = resp.status();

        if !status.is_success() {
            let body_text = resp.text().await?;
            anyhow::bail!("Delete request failed: {} {}", status, body_text);
        }

        let response_bytes = resp.bytes().await?;

        if response_bytes.len() > 100 && self.machine_public_key.is_some() {
            match protocol::machine::root_as_envelope(&response_bytes) {
                Ok(_envelope) => match self.decrypt_from_machine(&response_bytes) {
                    Ok(decrypted) => {
                        log::info!(
                            "send_delete_request: Decryption successful, decrypted length: {}",
                            decrypted.len()
                        );
                        Ok(decrypted)
                    }
                    Err(e) => {
                        log::warn!(
                            "send_delete_request: Decryption failed: {:?}, returning raw bytes",
                            e
                        );
                        Ok(response_bytes.to_vec())
                    }
                },
                Err(_) => {
                    log::error!(
                        "send_delete_request: Not an envelope, server sent unencrypted response"
                    );
                    Ok(response_bytes.to_vec())
                }
            }
        } else {
            Ok(response_bytes.to_vec())
        }
    }
}

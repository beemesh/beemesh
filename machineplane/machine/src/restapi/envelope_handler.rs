use anyhow::{Result, anyhow};
use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::Response,
};
use base64::Engine;
use log::{debug, error, warn};
use protocol::machine::{FbEnvelope, root_as_envelope};

use std::sync::Arc;

// Secure envelope metadata stored in request extensions
#[derive(Clone)]
pub struct EnvelopeMetadata {
    pub signing_pubkey: Vec<u8>,
    pub kem_pubkey: Vec<u8>,
    pub peer_id: Option<String>,
}

/// Encrypted envelope handler for REST API communication
pub struct EnvelopeHandler {
    /// Node's KEM private key for decryption
    kem_private_key: Vec<u8>,
    /// Optional node signing private key (used to sign/encrypt responses properly)
    signing_private_key: Option<Vec<u8>>,
    /// Node's public (signing) key for responses (base64 string)
    public_key: String,
}

impl EnvelopeHandler {
    pub fn new(kem_private_key: Vec<u8>, public_key: String) -> Self {
        // Prefer an in-memory node keypair if available (set at startup) to avoid
        // relying on filesystem keys in ephemeral/test environments. If no in-memory
        // keypair is present, fall back to loading the on-disk keypair. If that
        // also fails, leave the signing key as None.
        let signing_private_key = if let Some(opt_pair) = crate::libp2p_beemesh::NODE_KEYPAIR.get()
        {
            // NODE_KEYPAIR holds Option<(pub, priv)>; prefer it when present
            if let Some((_pub_bytes, priv_bytes)) = opt_pair.as_ref() {
                log::debug!("EnvelopeHandler::new: using in-memory NODE_KEYPAIR for signing");
                Some(priv_bytes.clone())
            } else {
                // No in-memory pair available; try on-disk as a fallback
                match crypto::ensure_keypair_on_disk() {
                    Ok((pub_bytes, priv_bytes)) => {
                        let on_disk_b64 =
                            base64::engine::general_purpose::STANDARD.encode(&pub_bytes);
                        if on_disk_b64 == public_key {
                            Some(priv_bytes)
                        } else {
                            warn!(
                                "EnvelopeHandler::new: provided public_key differs from on-disk public key; using on-disk private key for signing responses"
                            );
                            Some(priv_bytes)
                        }
                    }
                    Err(e) => {
                        warn!(
                            "EnvelopeHandler::new: could not load signing key from disk: {}",
                            e
                        );
                        None
                    }
                }
            }
        } else {
            // NODE_KEYPAIR not initialized, fall back to on-disk keypair
            match crypto::ensure_keypair_on_disk() {
                Ok((pub_bytes, priv_bytes)) => {
                    let on_disk_b64 = base64::engine::general_purpose::STANDARD.encode(&pub_bytes);
                    if on_disk_b64 == public_key {
                        Some(priv_bytes)
                    } else {
                        warn!(
                            "EnvelopeHandler::new: provided public_key differs from on-disk public key; using on-disk private key for signing responses"
                        );
                        Some(priv_bytes)
                    }
                }
                Err(e) => {
                    warn!(
                        "EnvelopeHandler::new: could not load signing key from disk: {}",
                        e
                    );
                    None
                }
            }
        };

        Self {
            kem_private_key,
            signing_private_key,
            public_key,
        }
    }

    /// Get the machine's public key
    pub fn get_public_key(&self) -> &str {
        &self.public_key
    }

    /// Decrypt an incoming envelope and extract the payload
    pub async fn decrypt_request_envelope(&self, envelope_bytes: &[u8]) -> Result<Vec<u8>> {
        // Parse the flatbuffer envelope
        let envelope = root_as_envelope(envelope_bytes)
            .map_err(|e| anyhow!("Failed to parse envelope: {}", e))?;

        // Verify signature using peer-aware verification if peer_id is present
        let nonce_window = std::time::Duration::from_secs(300);
        let peer_id = envelope.peer_id().unwrap_or("global");
        crate::libp2p_beemesh::envelope::verify_flatbuffer_envelope_for_peer(
            envelope_bytes,
            nonce_window,
            peer_id,
        )
        .map_err(|e| anyhow!("Envelope verification failed: {}", e))?;

        // Extract encrypted payload
        let encrypted_payload = envelope
            .payload()
            .ok_or_else(|| anyhow!("No payload in envelope"))?
            .iter()
            .collect::<Vec<u8>>();

        // Decrypt payload using our KEM private key
        let decrypted_payload =
            crypto::decrypt_payload_from_recipient_blob(&encrypted_payload, &self.kem_private_key)
                .map_err(|e| anyhow!("Failed to decrypt payload: {}", e))?;

        debug!(
            "Successfully decrypted envelope payload ({} bytes)",
            decrypted_payload.len()
        );
        Ok(decrypted_payload)
    }

    /// Encrypt a response payload and wrap it in an envelope
    /// Build a response envelope for a specific peer using provided KEM public key
    pub async fn encrypt_response_envelope_with_key(
        &self,
        payload: &[u8],
        payload_type: &str,
        recipient_peer_id: &str,
        kem_pubkey: &[u8],
    ) -> Result<Vec<u8>> {
        if kem_pubkey.is_empty() {
            return Err(anyhow!(
                "No KEM public key provided for peer: {}",
                recipient_peer_id
            ));
        }

        debug!(
            "Using provided KEM pubkey for peer {} of length {} bytes",
            recipient_peer_id,
            kem_pubkey.len()
        );

        // Try parsing provided pubkey bytes as an ml_kem_512 public key
        let _ml_kem_pubkey = match saorsa_pqc::api::kem::MlKemPublicKey::from_bytes(
            saorsa_pqc::api::kem::MlKemVariant::MlKem512,
            kem_pubkey,
        ) {
            Ok(pubkey) => pubkey,
            Err(e) => {
                return Err(anyhow!(
                    "Failed to parse KEM public key for peer {}: {}",
                    recipient_peer_id,
                    e
                ));
            }
        };

        // Encrypt payload using the provided KEM public key
        let encrypted_blob = crypto::encrypt_payload_for_recipient(kem_pubkey, payload)?;

        // Build envelope with encrypted payload
        let nonce = uuid::Uuid::new_v4().to_string();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        // Create canonical bytes for signing
        let canonical_bytes = protocol::machine::build_envelope_canonical(
            &encrypted_blob,
            payload_type,
            &nonce,
            ts,
            "ml-dsa-65",
            None,
        );

        // Sign the canonical bytes
        let signing_key = self.signing_private_key.as_ref().ok_or_else(|| {
            anyhow!(
                "No signing private key available for peer: {}",
                recipient_peer_id
            )
        })?;
        let public_key_bytes = base64::engine::general_purpose::STANDARD
            .decode(&self.public_key)
            .map_err(|e| anyhow!("Failed to decode public key: {}", e))?;
        let (sig_b64, pub_b64) =
            crypto::sign_envelope(signing_key, &public_key_bytes, &canonical_bytes)?;

        // Build the final signed envelope
        Ok(protocol::machine::build_envelope_signed(
            &encrypted_blob,
            payload_type,
            &nonce,
            ts,
            "ml-dsa-65",
            "ml-dsa-65",
            &sig_b64,
            &pub_b64,
            None,
        ))
    }

    /// Extract peer ID from request headers or envelope
    pub fn extract_peer_id(&self, headers: &HeaderMap, envelope: &FbEnvelope) -> Option<String> {
        // First try to get peer ID from headers
        if let Some(peer_id_header) = headers.get("x-peer-id") {
            if let Ok(peer_id) = peer_id_header.to_str() {
                return Some(peer_id.to_string());
            }
        }

        // Fallback to peer ID from envelope
        envelope.peer_id().map(|s| s.to_string())
    }
}

/// Middleware to handle encrypted envelope requests
pub async fn envelope_middleware(
    State(envelope_handler): State<Arc<EnvelopeHandler>>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let headers = request.headers().clone();
    let content_type = headers
        .get("content-type")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    // Only process flatbuffer envelopes
    if content_type == "application/octet-stream" {
        // Extract body bytes and parts
        let (mut parts, body) = request.into_parts();
        let body_bytes = match axum::body::to_bytes(body, usize::MAX).await {
            Ok(bytes) => bytes,
            Err(_) => return Err(StatusCode::BAD_REQUEST),
        };

        // Try to parse as envelope
        match root_as_envelope(&body_bytes) {
            Ok(envelope) => {
                debug!("Processing envelope from peer: {:?}", envelope.peer_id());

                // Extract and store envelope metadata securely in request extensions
                let signing_pubkey = envelope
                    .pubkey()
                    .and_then(|pk| base64::engine::general_purpose::STANDARD.decode(pk).ok())
                    .unwrap_or_default();
                let kem_pubkey = envelope
                    .kem_pubkey()
                    .and_then(|pk| base64::engine::general_purpose::STANDARD.decode(pk).ok())
                    .unwrap_or_default();
                let peer_id = envelope.peer_id().map(|s| s.to_string());

                let metadata = EnvelopeMetadata {
                    signing_pubkey,
                    kem_pubkey,
                    peer_id,
                };
                parts.extensions.insert(metadata);
                debug!("Stored envelope metadata in request extensions");

                // Decrypt the envelope
                match envelope_handler.decrypt_request_envelope(&body_bytes).await {
                    Ok(decrypted_payload) => {
                        // Replace request body with decrypted payload
                        let new_body = axum::body::Body::from(decrypted_payload);
                        let new_request = Request::from_parts(parts, new_body);

                        // Continue with decrypted request
                        let response = next.run(new_request).await;
                        return Ok(response);
                    }
                    Err(e) => {
                        error!(
                            "Envelope verification failed: {} - rejecting request for security",
                            e
                        );
                        // SECURITY: Reject requests with invalid envelopes to prevent bypass
                        return Err(StatusCode::UNAUTHORIZED);
                    }
                }
            }
            Err(_) => {
                // Not an envelope, pass through as-is but add empty metadata extension
                let metadata = EnvelopeMetadata {
                    signing_pubkey: Vec::new(),
                    kem_pubkey: Vec::new(),
                    peer_id: None,
                };
                parts.extensions.insert(metadata);
                debug!("Added empty envelope metadata for non-envelope request");

                let new_body = axum::body::Body::from(body_bytes);
                let new_request = Request::from_parts(parts, new_body);
                let response = next.run(new_request).await;
                return Ok(response);
            }
        }
    }

    // For non-flatbuffer requests, add empty metadata extension and pass through
    let (mut parts, body) = request.into_parts();
    let metadata = EnvelopeMetadata {
        signing_pubkey: Vec::new(),
        kem_pubkey: Vec::new(),
        peer_id: None,
    };
    parts.extensions.insert(metadata);
    debug!("Added empty envelope metadata for non-flatbuffer request");

    let new_request = Request::from_parts(parts, body);
    let response = next.run(new_request).await;
    Ok(response)
}

/// Helper to create encrypted flatbuffer responses using provided KEM key
pub async fn create_encrypted_response_with_key(
    envelope_handler: &Arc<EnvelopeHandler>,
    payload: &[u8],
    payload_type: &str,
    peer_id: Option<&str>,
    kem_pubkey: &[u8],
) -> Result<axum::response::Response<axum::body::Body>, StatusCode> {
    match peer_id {
        Some(peer_id) => {
            // Encrypt response for the specific peer using provided KEM key
            match envelope_handler
                .encrypt_response_envelope_with_key(payload, payload_type, peer_id, kem_pubkey)
                .await
            {
                Ok(encrypted_envelope) => {
                    let response = axum::response::Response::builder()
                        .header("content-type", "application/octet-stream")
                        .body(axum::body::Body::from(encrypted_envelope))
                        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
                    Ok(response)
                }
                Err(e) => {
                    // If encryption fails, log the issue and fall back to unencrypted response
                    error!(
                        "Failed to encrypt response for peer {}: {} - falling back to unencrypted response",
                        peer_id, e
                    );

                    // Send unencrypted flatbuffer response as a fallback
                    let response = axum::response::Response::builder()
                        .header("content-type", "application/octet-stream")
                        .body(axum::body::Body::from(payload.to_vec()))
                        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
                    Ok(response)
                }
            }
        }
        None => {
            // No peer ID provided, send unencrypted response
            let response = axum::response::Response::builder()
                .header("content-type", "application/octet-stream")
                .body(axum::body::Body::from(payload.to_vec()))
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            Ok(response)
        }
    }
}

/// Helper to create responses that fall back to unencrypted when no envelope metadata is available
pub async fn create_response_with_fallback(
    payload: &[u8],
) -> Result<axum::response::Response<axum::body::Body>, StatusCode> {
    // Since we removed caching, we can only return unencrypted responses
    // for endpoints that don't have envelope metadata
    let response = axum::response::Response::builder()
        .header("content-type", "application/octet-stream")
        .body(axum::body::Body::from(payload.to_vec()))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(response)
}

/// Create an encrypted response when metadata provides the recipient KEM key
pub async fn create_response_for_envelope_metadata(
    envelope_handler: &Arc<EnvelopeHandler>,
    payload: &[u8],
    payload_type: &str,
    metadata: &EnvelopeMetadata,
) -> Result<axum::response::Response<axum::body::Body>, StatusCode> {
    if !metadata.kem_pubkey.is_empty() {
        return create_encrypted_response_with_key(
            envelope_handler,
            payload,
            payload_type,
            metadata.peer_id.as_deref(),
            &metadata.kem_pubkey,
        )
        .await;
    }

    create_response_with_fallback(payload).await
}

/// Extract peer ID from request state/headers
pub fn get_peer_id_from_request(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-peer-id")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {

    use crypto::{ensure_keypair_ephemeral, ensure_pqc_init};

    struct KeypairConfigGuard(crypto::KeypairConfig);

    impl KeypairConfigGuard {
        fn kem_ephemeral() -> Self {
            let previous = crypto::get_keypair_config();
            crypto::set_keypair_config(crypto::KeypairConfig {
                signing_mode: crypto::KeypairMode::Ephemeral,
                kem_mode: crypto::KeypairMode::Ephemeral,
                key_directory: previous.key_directory.clone(),
            });
            KeypairConfigGuard(previous)
        }
    }

    impl Drop for KeypairConfigGuard {
        fn drop(&mut self) {
            crypto::set_keypair_config(self.0.clone());
        }
    }

    #[tokio::test]
    async fn test_envelope_roundtrip() {
        ensure_pqc_init().expect("PQC init failed");

        // Set ephemeral KEM mode for this test
        let _config_guard = KeypairConfigGuard::kem_ephemeral();

        // Generate test signing keypairs for envelope signing
        // Note: ensure_keypair_ephemeral returns (public_key, private_key)
        let (sender_pub, sender_priv) =
            ensure_keypair_ephemeral().expect("Failed to generate sender keypair");

        // Generate KEM keypairs for encryption using the currently configured keypair mode
        let (recipient_kem_pub, recipient_kem_priv) =
            crypto::ensure_kem_keypair_on_disk().expect("Failed to generate recipient KEM keypair");

        // Test payload
        let payload = b"test payload data";
        let payload_type = "test";

        // Step 1: Create encrypted envelope manually to avoid nonce conflicts
        let encrypted_payload = crypto::encrypt_payload_for_recipient(&recipient_kem_pub, payload)
            .expect("Failed to encrypt payload");

        // Generate unique nonce using nanoseconds like the working test
        let nonce = format!(
            "test-nonce-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let alg = "ml-dsa-65";

        // Build canonical envelope for signing
        let canonical = protocol::machine::build_envelope_canonical(
            &encrypted_payload,
            payload_type,
            &nonce,
            timestamp,
            alg,
            None,
        );

        // Sign the canonical envelope
        let (sig_b64, pub_b64) = crypto::sign_envelope(&sender_priv, &sender_pub, &canonical)
            .expect("Failed to sign envelope");

        // Build the final signed envelope with encrypted payload
        let encrypted_envelope = protocol::machine::build_envelope_signed(
            &encrypted_payload,
            payload_type,
            &nonce,
            timestamp,
            alg,
            "ml-dsa-65",
            &sig_b64,
            &pub_b64,
            None,
        );

        // Step 2: Verify the envelope can be verified directly
        let (verified_payload, _pub, _sig) =
            crate::libp2p_beemesh::envelope::verify_flatbuffer_envelope(
                &encrypted_envelope,
                std::time::Duration::from_secs(300),
            )
            .expect("Direct verification failed");

        // Step 3: Decrypt the verified encrypted payload
        let decrypted_payload =
            crypto::decrypt_payload_from_recipient_blob(&verified_payload, &recipient_kem_priv)
                .expect("Failed to decrypt payload");

        assert_eq!(payload, decrypted_payload.as_slice());
    }
}

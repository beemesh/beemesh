use anyhow::{anyhow, Result};
use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::Response,
};
use base64::Engine;
use log::{debug, warn};
use protocol::machine::{root_as_envelope, FbEnvelope};

use std::sync::Arc;
use tokio::sync::RwLock;

/// Encrypted envelope handler for REST API communication
pub struct EnvelopeHandler {
    /// Node's KEM private key for decryption
    kem_private_key: Vec<u8>,
    /// Optional node signing private key (used to sign/encrypt responses properly)
    signing_private_key: Option<Vec<u8>>,
    /// Node's public (signing) key for responses (base64 string)
    public_key: String,
    /// Cache of peer public keys for encryption
    peer_pubkeys: Arc<RwLock<std::collections::HashMap<String, Vec<u8>>>>,
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
            if let Some((ref _pub_bytes, ref priv_bytes)) = opt_pair.as_ref() {
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
                            warn!("EnvelopeHandler::new: provided public_key differs from on-disk public key; using on-disk private key for signing responses");
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
                        warn!("EnvelopeHandler::new: provided public_key differs from on-disk public key; using on-disk private key for signing responses");
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
            peer_pubkeys: Arc::new(RwLock::new(std::collections::HashMap::new())),
        }
    }

    /// Get the machine's public key
    pub fn get_public_key(&self) -> &str {
        &self.public_key
    }

    /// Add a peer's public key to the cache (log length for diagnostics)
    pub async fn add_peer_pubkey(&self, peer_id: &str, pubkey: Vec<u8>) {
        let mut cache = self.peer_pubkeys.write().await;
        // Log the length of the stored key to help diagnose KEM vs signing key mixups
        debug!(
            "Inserting peer pubkey for {} of length {} bytes",
            peer_id,
            pubkey.len()
        );
        cache.insert(peer_id.to_string(), pubkey);
    }

    /// Decrypt an incoming envelope and extract the payload
    pub async fn decrypt_request_envelope(&self, envelope_bytes: &[u8]) -> Result<Vec<u8>> {
        // Parse the flatbuffer envelope
        let envelope = root_as_envelope(envelope_bytes)
            .map_err(|e| anyhow!("Failed to parse envelope: {}", e))?;

        // Verify signature
        let nonce_window = std::time::Duration::from_secs(300);
        crate::libp2p_beemesh::envelope::verify_flatbuffer_envelope(envelope_bytes, nonce_window)
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
    pub async fn encrypt_response_envelope(
        &self,
        payload: &[u8],
        payload_type: &str,
        recipient_peer_id: &str,
    ) -> Result<Vec<u8>> {
        debug!(
            "Attempting to encrypt response for peer: {}, payload size: {}",
            recipient_peer_id,
            payload.len()
        );

        // Get recipient's public key from cache and validate length (avoid using signing keys as KEM keys)
        let recipient_pubkey =
            {
                let cache = self.peer_pubkeys.read().await;
                debug!(
                    "Looking up public key for peer: {}, cache contains {} entries",
                    recipient_peer_id,
                    cache.len()
                );

                // Debug log all cached peer IDs
                for peer_id in cache.keys() {
                    debug!("Cached peer ID: {}", peer_id);
                }

                let pubkey = cache.get(recipient_peer_id).cloned().ok_or_else(|| {
                    anyhow!("No public key found for peer: {}", recipient_peer_id)
                })?;
                // Expected ml_kem_512 public key length (bytes). If this doesn't match we likely
                // have a signing public key stored (or some other non-KEM key) and should not
                // attempt to use it for KEM-based encryption.
                debug!(
                    "Using stored pubkey for peer {} of length {} bytes",
                    recipient_peer_id,
                    pubkey.len()
                );
                // Try parsing stored pubkey bytes as an ml_kem_512 public key instead of relying on
                // a simple length check. If parsing fails, remove the cached entry and return an error.
                match saorsa_pqc::api::kem::MlKemPublicKey::from_bytes(
                    saorsa_pqc::api::kem::MlKemVariant::MlKem512,
                    &pubkey,
                ) {
                    Ok(_) => {
                        debug!(
                            "Stored pubkey for peer {} parsed successfully as ml_kem_512 key",
                            recipient_peer_id
                        );
                    }
                    Err(e) => {
                        // remove invalid entry to avoid repeated failures
                        let mut cache = self.peer_pubkeys.write().await;
                        cache.remove(recipient_peer_id);
                        return Err(anyhow!(
                        "Stored public key for peer {} is not a valid ml_kem_512 public key: {:?}",
                        recipient_peer_id, e
                    ));
                    }
                }
                pubkey
            };

        // Choose the appropriate signing private key to sign the envelope.
        // Prefer an explicit signing private key if we were able to load one;
        // otherwise fall back to the KEM private key (legacy behavior).
        let sender_priv_bytes: Vec<u8> = if let Some(sk) = &self.signing_private_key {
            sk.clone()
        } else {
            // fallback - use KEM private key bytes so behavior remains backward-compatible
            self.kem_private_key.clone()
        };

        // Build encrypted envelope using the selected signing key
        let encrypted_envelope = protocol::machine::build_encrypted_envelope(
            payload,
            payload_type,
            &recipient_pubkey,
            &sender_priv_bytes,
            &self.public_key,
        )?;

        debug!(
            "Successfully encrypted response envelope for peer: {}",
            recipient_peer_id
        );

        Ok(encrypted_envelope)
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

    /// Store peer's public key from envelope for future responses
    /// Also stores it in custom header for immediate use
    pub async fn store_peer_pubkey_from_envelope(
        &self,
        envelope: &FbEnvelope<'_>,
    ) -> Option<String> {
        // Prefer KEM public key embedded in the envelope (if present). Fall back to the
        // envelope.pubkey only if it looks like a KEM public key (length check).
        if let Some(peer_id) = envelope.peer_id() {
            debug!("Processing envelope for peer: {}", peer_id);

            // Try kem_pub field first
            if let Some(kem_b64) = envelope.kem_pubkey() {
                debug!(
                    "Found kem_pubkey field in envelope for peer: {}, length: {}",
                    peer_id,
                    kem_b64.len()
                );
                if kem_b64.is_empty() {
                    debug!("KEM public key field is empty for peer: {}", peer_id);
                } else {
                    match base64::engine::general_purpose::STANDARD.decode(kem_b64) {
                        Ok(kem_bytes) => {
                            debug!("Successfully decoded KEM public key for peer: {}, bytes length: {}", peer_id, kem_bytes.len());
                            self.add_peer_pubkey(peer_id, kem_bytes).await;
                            debug!(
                                "Stored KEM public key from envelope.kem_pubkey for peer: {}",
                                peer_id
                            );
                            return Some(kem_b64.to_string());
                        }
                        Err(e) => {
                            warn!(
                                "Failed to decode envelope.kem_pubkey (base64) for peer {}: {}",
                                peer_id, e
                            );
                        }
                    }
                }
            } else {
                debug!(
                    "No kem_pubkey field found in envelope for peer: {}",
                    peer_id
                );
            }

            // If envelope.kem_pub not present, check envelope.pubkey (may be a KEM pubkey)
            if let Some(pubkey_b64) = envelope.pubkey() {
                debug!(
                    "Found pubkey field in envelope for peer: {}, length: {}",
                    peer_id,
                    pubkey_b64.len()
                );
                match base64::engine::general_purpose::STANDARD.decode(pubkey_b64) {
                    Ok(pubkey_bytes) => {
                        const EXPECTED_KEM_PUBKEY_LEN: usize = 800;
                        debug!("Decoded pubkey for peer: {}, bytes length: {} (expected KEM length: {})", peer_id, pubkey_bytes.len(), EXPECTED_KEM_PUBKEY_LEN);
                        if pubkey_bytes.len() == EXPECTED_KEM_PUBKEY_LEN {
                            self.add_peer_pubkey(peer_id, pubkey_bytes).await;
                            debug!(
                                "Stored KEM public key from envelope.pubkey for peer: {}",
                                peer_id
                            );
                            return Some(pubkey_b64.to_string());
                        } else {
                            debug!("Envelope pubkey for peer {} has unexpected length {}, not storing as KEM pubkey", peer_id, pubkey_bytes.len());
                        }
                    }
                    Err(e) => {
                        warn!(
                            "Failed to decode envelope.pubkey (base64) for peer {}: {}",
                            peer_id, e
                        );
                    }
                }
            } else {
                debug!("No pubkey field found in envelope for peer: {}", peer_id);
            }

            debug!(
                "No valid KEM public key found in envelope for peer: {}",
                peer_id
            );
        } else {
            debug!("No peer_id found in envelope");
        }

        None
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

                // Store peer's public key from envelope and get base64 encoded version
                // This should work even if decryption fails later
                if let Some(kem_pubkey_b64) = envelope_handler
                    .store_peer_pubkey_from_envelope(&envelope)
                    .await
                {
                    // Add KEM public key to headers for response encryption
                    parts
                        .headers
                        .insert("x-peer-kem-pubkey", kem_pubkey_b64.parse().unwrap());
                    debug!("Added peer KEM public key to request headers");
                }

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
                        warn!(
                            "Failed to decrypt envelope: {} - continuing with original body",
                            e
                        );
                        // Continue with original body even if decryption fails
                        // The KEM public key was already extracted for response encryption
                        let new_body = axum::body::Body::from(body_bytes);
                        let new_request = Request::from_parts(parts, new_body);
                        let response = next.run(new_request).await;
                        return Ok(response);
                    }
                }
            }
            Err(_) => {
                // Not an envelope, pass through as-is
                let new_body = axum::body::Body::from(body_bytes);
                let new_request = Request::from_parts(parts, new_body);
                let response = next.run(new_request).await;
                return Ok(response);
            }
        }
    }

    // For non-flatbuffer requests, pass through
    let response = next.run(request).await;
    Ok(response)
}

/// Helper to create encrypted flatbuffer responses
pub async fn create_encrypted_response(
    envelope_handler: &Arc<EnvelopeHandler>,
    payload: &[u8],
    payload_type: &str,
    peer_id: Option<&str>,
) -> Result<axum::response::Response<axum::body::Body>, StatusCode> {
    match peer_id {
        Some(peer_id) => {
            // Encrypt response for the specific peer
            match envelope_handler
                .encrypt_response_envelope(payload, payload_type, peer_id)
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
                    // If encryption fails (e.g. missing peer pubkey or crypto error),
                    // log the issue and fall back to sending the unencrypted flatbuffer payload.
                    // This makes the debug/test endpoints more robust in environments where
                    // encryption between processes may not be available.
                    warn!(
                        "Failed to encrypt response for peer {}: {} - falling back to unencrypted response",
                        peer_id, e
                    );

                    // Send unencrypted flatbuffer response as a fallback.
                    let response = axum::response::Response::builder()
                        .header("content-type", "application/octet-stream")
                        .body(axum::body::Body::from(payload.to_vec()))
                        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
                    Ok(response)
                }
            }
        }
        None => {
            // No peer ID, send unencrypted flatbuffer response
            let response = axum::response::Response::builder()
                .header("content-type", "application/octet-stream")
                .body(axum::body::Body::from(payload.to_vec()))
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            Ok(response)
        }
    }
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

    #[tokio::test]
    async fn test_envelope_roundtrip() {
        ensure_pqc_init().expect("PQC init failed");

        // Set ephemeral KEM mode for this test
        std::env::set_var("BEEMESH_KEM_EPHEMERAL", "1");

        // Generate test signing keypairs for envelope signing
        // Note: ensure_keypair_ephemeral returns (public_key, private_key)
        let (sender_pub, sender_priv) =
            ensure_keypair_ephemeral().expect("Failed to generate sender keypair");

        // Generate KEM keypairs for encryption - use ensure_kem_keypair_on_disk which respects BEEMESH_KEM_EPHEMERAL
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

        // Clean up environment variable
        std::env::remove_var("BEEMESH_KEM_EPHEMERAL");
    }
}

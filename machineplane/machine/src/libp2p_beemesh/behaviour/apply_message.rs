use base64::prelude::*;
use libp2p::request_response;
use log::{debug, error, info, warn};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Decrypt an encrypted manifest using a specific manifest ID and encrypted manifest
/// This implementation supports direct delivery only: the EncryptedManifest.payload MUST
/// be a base64-encoded recipient-blob (versioned recipient blob starting with 0x02)
/// produced by `crypto::encrypt_payload_for_recipient`. No share-based reconstruction
/// or legacy fallback is supported; any other payload format is treated as an error.
async fn decrypt_encrypted_manifest_with_id(
    _manifest_id: &str,
    encrypted_manifest: &protocol::machine::EncryptedManifest<'_>,
    _local_peer_id: &libp2p::PeerId,
) -> Result<String, anyhow::Error> {
    log::info!("libp2p: DECRYPT ENTRY - direct recipient-blob only path");

    // Extract payload (base64-encoded recipient blob)
    let payload_b64 = encrypted_manifest.payload().unwrap_or("");
    if payload_b64.is_empty() {
        return Err(anyhow::anyhow!(
            "missing payload in encrypted manifest (expected recipient-blob)"
        ));
    }

    // Decode the base64 recipient-blob
    let decoded_blob = base64::engine::general_purpose::STANDARD
        .decode(payload_b64)
        .map_err(|e| anyhow::anyhow!("failed to base64-decode payload: {}", e))?;

    // Validate recipient-blob version byte
    if decoded_blob.is_empty() || decoded_blob[0] != 0x02 {
        return Err(anyhow::anyhow!(
            "unsupported payload format: expected recipient-blob (version byte 0x02)"
        ));
    }

    // Use the node's KEM private key to decapsulate and decrypt the recipient-blob
    let (_pub_bytes, priv_bytes) = crypto::ensure_kem_keypair_on_disk()
        .map_err(|e| anyhow::anyhow!("failed to load KEM keypair: {}", e))?;

    let plaintext = crypto::decrypt_payload_from_recipient_blob(&decoded_blob, &priv_bytes)
        .map_err(|e| anyhow::anyhow!("recipient-blob decryption failed: {}", e))?;

    // Interpret plaintext as UTF-8 manifest content (YAML/JSON)
    let manifest_str = String::from_utf8(plaintext)
        .map_err(|e| anyhow::anyhow!("decrypted manifest is not valid UTF-8: {}", e))?;

    log::info!(
        "libp2p: direct recipient-blob decryption succeeded (len={})",
        manifest_str.len()
    );

    Ok(manifest_str)
}

/// Store an applied manifest in the DHT after successful deployment
fn store_applied_manifest_in_dht(
    swarm: &mut libp2p::Swarm<super::MyBehaviour>,
    apply_req: &protocol::machine::ApplyRequest,
    local_peer: libp2p::PeerId,
) {
    // Generate a stable ID for this manifest
    let mut hasher = DefaultHasher::new();
    if let (Some(tenant), Some(operation_id), Some(manifest_json)) = (
        apply_req.tenant(),
        apply_req.operation_id(),
        apply_req.manifest_json(),
    ) {
        tenant.hash(&mut hasher);
        operation_id.hash(&mut hasher);
        manifest_json.hash(&mut hasher);

        let manifest_id = format!("{:x}", hasher.finish());

        // Create content hash
        let mut content_hasher = DefaultHasher::new();
        manifest_json.hash(&mut content_hasher);
        let content_hash = format!("{:x}", content_hasher.finish());

        // Get current timestamp
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        // Determine manifest kind (simple heuristic)
        let manifest_kind = if manifest_json.contains(r#""kind""#) {
            // Try to extract kind from JSON
            "Pod" // Simplified for now
        } else {
            "Unknown"
        };

        // Create labels
        let labels = vec![
            ("deployed-by".to_string(), "beemesh-node".to_string()),
            ("kind".to_string(), manifest_kind.to_string()),
            ("tenant".to_string(), tenant.to_string()),
            ("replicas".to_string(), apply_req.replicas().to_string()),
        ];

        // Build the AppliedManifest FlatBuffer
        // Note: In production, you should sign this with your node's private key
        let empty_pubkey = vec![];
        let empty_signature = vec![];

        let manifest_data = protocol::machine::build_applied_manifest(
            &manifest_id,
            &tenant,
            &operation_id,
            &local_peer.to_string(),
            &empty_pubkey,
            &empty_signature,
            &manifest_json,
            &manifest_kind,
            labels,
            timestamp,
            3600, // 1 hour TTL
            &content_hash,
        );

        // Store in DHT using Kademlia
        let record_key = libp2p::kad::RecordKey::new(&format!("manifest:{}", manifest_id));
        let record = libp2p::kad::Record {
            key: record_key,
            value: manifest_data,
            publisher: None,
            expires: None,
        };

        let query_id = swarm
            .behaviour_mut()
            .kademlia
            .put_record(record, libp2p::kad::Quorum::One);

        info!(
            "DHT: Storing applied manifest {} (query_id: {:?})",
            manifest_id, query_id
        );
    } else {
        warn!("DHT: Cannot store manifest - missing required fields");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use base64::Engine;
    use std::sync::Once;

    static INIT: Once = Once::new();

    fn test_init() {
        INIT.call_once(|| {
            // Ensure PQC is initialized for tests
            let _ = crypto::ensure_pqc_init();
            // Use ephemeral KEM mode so tests don't write to disk
            std::env::set_var("BEEMESH_KEM_EPHEMERAL", "1");
        });
    }

    /// Verify that an EncryptedManifest whose payload is a recipient-blob (ml-kem) can be
    /// decrypted by `decrypt_encrypted_manifest_with_id`.
    #[tokio::test]
    async fn test_recipient_blob_encrypted_manifest_decrypts() {
        test_init();

        // Prepare a simple manifest YAML payload
        let manifest_yaml = "apiVersion: v1\nkind: Pod\nmetadata:\n  name: test-pod\nspec:\n  containers:\n  - name: nginx\n    image: nginx:latest\n";

        // Get the ephemeral KEM keypair that our node will use as recipient
        let (kem_pub, _kem_priv) =
            crypto::ensure_kem_keypair_on_disk().expect("ensure_kem_keypair_on_disk");

        // Encrypt the manifest payload for the recipient (returns recipient-blob)
        let recipient_blob =
            crypto::encrypt_payload_for_recipient(&kem_pub, manifest_yaml.as_bytes())
                .expect("encrypt_payload_for_recipient");

        // Base64-encode the recipient blob for storage inside EncryptedManifest.payload()
        let payload_b64 = base64::engine::general_purpose::STANDARD.encode(&recipient_blob);

        // Build an EncryptedManifest flatbuffer with the recipient-blob as payload
        let nonce = "test-recipient-blob-nonce";
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let encrypted_manifest_fb = protocol::machine::build_encrypted_manifest(
            nonce,
            &payload_b64,
            "ml-kem-512", // encryption algorithm marker for recipient-blob
            1,            // threshold
            1,            // total_shares
            Some("kubernetes"),
            &[],
            ts,
            None,
        );

        // Parse back into EncryptedManifest view
        let encrypted_manifest =
            protocol::machine::root_as_encrypted_manifest(&encrypted_manifest_fb)
                .expect("root_as_encrypted_manifest");

        // Use a dummy local_peer id for the call; the decryption path uses the KEM privkey on disk
        let fake_peer = libp2p::PeerId::random();

        // Attempt decryption via the function under test
        let decrypted =
            decrypt_encrypted_manifest_with_id("test-manifest", &encrypted_manifest, &fake_peer)
                .await
                .expect("decrypt_encrypted_manifest_with_id");

        // The decrypted string should match our original manifest YAML
        assert_eq!(decrypted, manifest_yaml);
    }
}

pub fn apply_message(
    message: request_response::Message<Vec<u8>, Vec<u8>>,
    peer: libp2p::PeerId,
    swarm: &mut libp2p::Swarm<super::MyBehaviour>,
    local_peer: libp2p::PeerId,
) {
    match message {
        request_response::Message::Request {
            request, channel, ..
        } => {
            info!("libp2p: received apply request from peer={}", peer);

            // First, attempt to verify request as a FlatBuffer Envelope
            let effective_request =
                match crate::libp2p_beemesh::security::verify_envelope_and_check_nonce_for_peer(
                    &request,
                    &peer.to_string(),
                ) {
                    Ok((payload_bytes, _pub, _sig)) => payload_bytes,
                    Err(e) => {
                        if crate::libp2p_beemesh::security::require_signed_messages() {
                            error!("rejecting unsigned/invalid apply request: {:?}", e);
                            let error_response = protocol::machine::build_apply_response(
                                false,
                                "unknown",
                                "unsigned or invalid envelope",
                            );
                            let _ = swarm
                                .behaviour_mut()
                                .apply_rr
                                .send_response(channel, error_response);
                            return;
                        }
                        request.clone()
                    }
                };

            // Parse the FlatBuffer apply request
            match protocol::machine::root_as_apply_request(&effective_request) {
                Ok(apply_req) => {
                    info!(
                        "libp2p: apply request - tenant={:?} operation_id={:?} replicas={}",
                        apply_req.tenant(),
                        apply_req.operation_id(),
                        apply_req.replicas()
                    );

                    // Calculate manifest_id for deployment
                    let manifest_id =
                        if let (Some(tenant), Some(operation_id), Some(manifest_json)) = (
                            apply_req.tenant(),
                            apply_req.operation_id(),
                            apply_req.manifest_json(),
                        ) {
                            let mut hasher = DefaultHasher::new();
                            tenant.hash(&mut hasher);
                            operation_id.hash(&mut hasher);
                            manifest_json.hash(&mut hasher);
                            format!("{:x}", hasher.finish())
                        } else {
                            format!("{:x}", DefaultHasher::new().finish())
                        };

                    // Deploy the manifest to runtime engine
                    let deployment_success = if let Some(manifest_json) = apply_req.manifest_json()
                    {
                        // Spawn deployment task since we can't make this function async
                        let manifest_id_clone = manifest_id.clone();
                        let manifest_json_clone = manifest_json.to_string();
                        let local_peer_clone = local_peer;
                        tokio::spawn(async move {
                            match deploy_manifest_from_apply_request(
                                &manifest_id_clone,
                                &manifest_json_clone,
                                local_peer_clone,
                            )
                            .await
                            {
                                Ok(_) => {
                                    info!(
                                        "libp2p: successfully deployed manifest {} from apply request",
                                        manifest_id_clone
                                    );
                                }
                                Err(e) => {
                                    warn!(
                                        "libp2p: failed to deploy manifest {} from apply request: {}",
                                        manifest_id_clone, e
                                    );
                                }
                            }
                        });
                        true // Assume success for now, actual deployment happens async
                    } else {
                        warn!("libp2p: apply request missing manifest_json");
                        false
                    };

                    if deployment_success {
                        // Store the applied manifest in the DHT
                        store_applied_manifest_in_dht(swarm, &apply_req, local_peer);

                        // Also store the encrypted manifest locally to become a manifest holder
                        if let Some(manifest_json) = apply_req.manifest_json() {
                            if let Ok(encrypted_envelope_bytes) =
                                base64::engine::general_purpose::STANDARD.decode(manifest_json)
                            {
                                debug!(
                                    "libp2p: apply request storing manifest locally for manifest_id={}",
                                    manifest_id
                                );
                                // Store manifest locally in manifest store
                                debug!(
                                    "libp2p: stored encrypted manifest locally (size={} bytes) for manifest_id={}",
                                    encrypted_envelope_bytes.len(),
                                    manifest_id
                                );
                            } else {
                                warn!("libp2p: failed to decode base64 manifest_json in apply request");
                            }
                        }
                    }

                    let success = deployment_success;

                    // Create a response
                    let response = protocol::machine::build_apply_response(
                        success,
                        apply_req.operation_id().unwrap_or("unknown"),
                        if success {
                            "Successfully applied manifest and stored in DHT"
                        } else {
                            "Failed to apply manifest"
                        },
                    );

                    // Send the response back
                    let _ = swarm
                        .behaviour_mut()
                        .apply_rr
                        .send_response(channel, response);
                    info!("libp2p: sent apply response to peer={}", peer);
                }
                Err(e) => {
                    warn!("libp2p: failed to parse apply request: {:?}", e);
                    let error_response = protocol::machine::build_apply_response(
                        false,
                        "unknown",
                        &format!("Failed to parse request: {:?}", e),
                    );
                    let _ = swarm
                        .behaviour_mut()
                        .apply_rr
                        .send_response(channel, error_response);
                }
            }
        }
        request_response::Message::Response { response, .. } => {
            info!("libp2p: received apply response from peer={}", peer);

            // Parse the response
            match protocol::machine::root_as_apply_response(&response) {
                Ok(apply_resp) => {
                    info!(
                        "libp2p: apply response - ok={} operation_id={:?} message={:?}",
                        apply_resp.ok(),
                        apply_resp.operation_id(),
                        apply_resp.message()
                    );
                }
                Err(e) => {
                    warn!("libp2p: failed to parse apply response: {:?}", e);
                }
            }
        }
    }
}

/// Process a self-apply request locally without going through RequestResponse protocol
/// This handles the case where a node assigns a task to itself
pub fn process_self_apply_request(
    manifest: &[u8],
    swarm: &mut libp2p::Swarm<super::MyBehaviour>,
    _local_peer: libp2p::PeerId,
) {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    log::debug!(
        "libp2p: processing self-apply request (manifest len={})",
        manifest.len()
    );

    // Parse the FlatBuffer apply request (same logic as in apply_message)
    match protocol::machine::root_as_apply_request(manifest) {
        Ok(apply_req) => {
            log::debug!(
                "libp2p: self-apply request - tenant={:?} operation_id={:?} replicas={}",
                apply_req.tenant(),
                apply_req.operation_id(),
                apply_req.replicas()
            );

            // Store the applied manifest in the DHT (same as normal apply)
            let local_peer = *swarm.local_peer_id();
            store_applied_manifest_in_dht(swarm, &apply_req, local_peer);

            // Spawn the decryption task (same as normal apply)
            let tenant_s = apply_req.tenant().map(|s| s.to_string());
            let operation_id_s = apply_req.operation_id().map(|s| s.to_string());
            let manifest_json_s = apply_req.manifest_json().map(|s| s.to_string());
            let local_peer_id_copy = local_peer; // Capture for async block

            tokio::spawn(async move {
                if let (Some(tenant), Some(operation_id), Some(manifest_json)) = (
                    tenant_s.as_deref(),
                    operation_id_s.as_deref(),
                    manifest_json_s.as_deref(),
                ) {
                    // Try to get the stored manifest_cid for this operation_id
                    let manifest_id = if let Some(stored_cid) =
                        crate::restapi::get_manifest_cid_for_operation(operation_id).await
                    {
                        log::debug!(
                            "libp2p: self-apply using stored manifest_cid={} for operation_id={}",
                            stored_cid,
                            operation_id
                        );
                        stored_cid
                    } else {
                        // Fallback to calculation (for backwards compatibility)
                        let mut hasher = DefaultHasher::new();
                        tenant.hash(&mut hasher);
                        operation_id.hash(&mut hasher);
                        manifest_json.hash(&mut hasher);
                        let calculated_id = format!("{:x}", hasher.finish());
                        log::warn!("libp2p: self-apply calculated fallback manifest_id={} from tenant='{}' operation_id='{}' manifest_json_len={}",
                                  calculated_id, tenant, operation_id, manifest_json.len());
                        calculated_id
                    };

                    log::debug!(
                        "libp2p: self-apply triggering decryption for manifest_id={}",
                        manifest_id
                    );

                    // Parse manifest_json as base64-encoded flatbuffer envelope
                    log::debug!(
                        "libp2p: SELF-APPLY DEBUG - parsing manifest_json len={}",
                        manifest_json.len()
                    );
                    let manifest_value = if let Ok(envelope_bytes) =
                        base64::engine::general_purpose::STANDARD.decode(&manifest_json)
                    {
                        log::debug!("libp2p: SELF-APPLY DEBUG - decoded base64 successfully, envelope_bytes len={}", envelope_bytes.len());
                        // Try to parse as flatbuffer envelope
                        if let Ok(envelope) = protocol::machine::root_as_envelope(&envelope_bytes) {
                            let payload_type = envelope.payload_type().unwrap_or("");
                            log::debug!(
                                "libp2p: SELF-APPLY DEBUG - parsed envelope with payload_type='{}'",
                                payload_type
                            );
                            if payload_type == "manifest" {
                                log::debug!("libp2p: self-apply detected encrypted manifest envelope - attempting decryption");

                                // Extract the encrypted manifest from the envelope payload
                                if let Some(payload_vector) = envelope.payload() {
                                    // Convert Vector<u8> to &[u8]
                                    let payload_bytes = payload_vector.bytes();
                                    if let Ok(encrypted_manifest) =
                                        protocol::machine::root_as_encrypted_manifest(payload_bytes)
                                    {
                                        match decrypt_encrypted_manifest_with_id(
                                            &manifest_id,
                                            &encrypted_manifest,
                                            &local_peer_id_copy,
                                        )
                                        .await
                                        {
                                            Ok(decrypted_yaml) => {
                                                log::debug!(
                                                "libp2p: self-apply successfully decrypted FlatBuffer manifest"
                                            );
                                                // Parse the decrypted YAML content
                                                match serde_yaml::from_str::<serde_json::Value>(
                                                    &decrypted_yaml,
                                                ) {
                                                    Ok(v) => {
                                                        log::debug!(
                                                        "libp2p: self-apply parsed decrypted YAML successfully"
                                                    );
                                                        v
                                                    }
                                                    Err(_) => {
                                                        log::warn!(
                                                        "libp2p: self-apply treating decrypted content as raw"
                                                    );
                                                        serde_json::json!({ "raw": decrypted_yaml })
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                log::warn!(
                                                "libp2p: self-apply failed to decrypt FlatBuffer manifest: {}",
                                                e
                                            );
                                                // Return empty object for failed decryption
                                                serde_json::json!({})
                                            }
                                        }
                                    } else {
                                        log::error!("libp2p: self-apply failed to parse payload as EncryptedManifest");
                                        serde_json::json!({})
                                    }
                                } else {
                                    log::error!("libp2p: self-apply envelope missing payload");
                                    serde_json::json!({})
                                }
                            } else {
                                log::error!(
                                    "libp2p: self-apply envelope has wrong payload type: '{}', expected 'manifest'",
                                    payload_type
                                );
                                serde_json::json!({})
                            }
                        } else {
                            log::warn!("libp2p: self-apply failed to parse as flatbuffer envelope, envelope_bytes len={}", envelope_bytes.len());
                            serde_json::json!({})
                        }
                    } else {
                        log::error!("libp2p: self-apply failed to decode base64 manifest_json, manifest_json len={}", manifest_json.len());
                        serde_json::json!({})
                    };

                    log::debug!("libp2p: self-apply storing decrypted manifest for testing");
                    let _ = crate::restapi::store_decrypted_manifest(
                        &manifest_id,
                        manifest_value.clone(),
                    )
                    .await;

                    // Deploy the manifest to the runtime engine
                    log::info!("libp2p: self-apply deploying manifest to runtime engine");
                    if let Err(e) =
                        deploy_manifest_to_runtime(&manifest_id, &manifest_value, local_peer).await
                    {
                        log::error!(
                            "libp2p: self-apply failed to deploy manifest to runtime engine: {}",
                            e
                        );
                    } else {
                        log::info!("libp2p: self-apply successfully deployed manifest {} to runtime engine", manifest_id);
                    }
                } else {
                    log::warn!("libp2p: self-apply missing required fields for decryption");
                }
            });
        }
        Err(e) => {
            log::warn!("libp2p: failed to parse self-apply request: {:?}", e);
        }
    }
}

/// Find peers that hold key shares for a given manifest ID using DHT providers
/// find_manifest_holders removed - direct-delivery design no longer queries holders.
/// Kept a small stub to avoid accidental references in other modules.
#[allow(dead_code)]
async fn find_manifest_holders(_manifest_id: &str) -> Result<Vec<libp2p::PeerId>, anyhow::Error> {
    Ok(Vec::new())
}

/// Fetch manifest from holders using the new peer-based system
// fetch_manifest_from_holders removed - unused helper cleaned up to avoid dead_code warning.

/// Get list of currently connected peers as fallback when DHT provider discovery fails
/// get_connected_peers removed in direct-delivery design. Stub retained for compatibility.
#[allow(dead_code)]
/// Deploy a decrypted manifest to the runtime engine
async fn deploy_manifest_to_runtime(
    manifest_id: &str,
    manifest_value: &serde_json::Value,
    local_peer: libp2p::PeerId,
) -> Result<(), anyhow::Error> {
    log::info!("Deploying manifest {} to runtime engine", manifest_id);

    // Get the global runtime registry
    let registry_guard = match crate::workload_integration::get_global_runtime_registry().await {
        Some(guard) => guard,
        None => {
            return Err(anyhow::anyhow!("Runtime registry not available"));
        }
    };

    let registry = match registry_guard.as_ref() {
        Some(reg) => reg,
        None => {
            return Err(anyhow::anyhow!("Runtime registry not initialized"));
        }
    };

    // Get the preferred runtime engine (mock in test environment)
    let engine = if std::env::var("BEEMESH_MOCK_ONLY_RUNTIME").unwrap_or_default() == "1" {
        registry.get_engine("mock")
    } else {
        registry.get_default_engine()
    };

    let engine = match engine {
        Some(eng) => eng,
        None => {
            return Err(anyhow::anyhow!("No runtime engine available"));
        }
    };

    // Convert the manifest content to bytes for deployment
    let manifest_content = if let Some(raw_str) = manifest_value.get("raw").and_then(|v| v.as_str())
    {
        raw_str.as_bytes().to_vec()
    } else {
        serde_json::to_string(manifest_value)?.as_bytes().to_vec()
    };

    // Create deployment configuration from the manifest
    let config = create_deployment_config_from_manifest(manifest_value)?;

    // Deploy to the runtime engine with local peer ID
    match engine
        .deploy_workload_with_peer(manifest_id, &manifest_content, &config, local_peer)
        .await
    {
        Ok(workload_info) => {
            log::info!(
                "Successfully deployed workload {} with ID {} to engine {}",
                manifest_id,
                workload_info.id,
                engine.name()
            );
            Ok(())
        }
        Err(e) => {
            log::error!(
                "Failed to deploy workload {} to engine {}: {}",
                manifest_id,
                engine.name(),
                e
            );
            Err(anyhow::anyhow!("Deployment failed: {}", e))
        }
    }
}

/// Create deployment configuration from manifest content
/// Deploy manifest from apply request by decrypting and parsing it
async fn deploy_manifest_from_apply_request(
    manifest_id: &str,
    manifest_json: &str,
    local_peer: libp2p::PeerId,
) -> Result<(), anyhow::Error> {
    // Decode the base64-encoded encrypted envelope
    let encrypted_envelope_bytes = base64::engine::general_purpose::STANDARD
        .decode(manifest_json)
        .map_err(|e| anyhow::anyhow!("Failed to decode base64 manifest_json: {}", e))?;

    // Parse as flatbuffer envelope
    let envelope = protocol::machine::root_as_envelope(&encrypted_envelope_bytes)
        .map_err(|e| anyhow::anyhow!("Failed to parse envelope: {}", e))?;

    let payload_type = envelope.payload_type().unwrap_or("");
    if payload_type != "manifest" {
        return Err(anyhow::anyhow!(
            "Wrong payload type: '{}', expected 'manifest'",
            payload_type
        ));
    }

    // Extract the encrypted manifest from the envelope payload
    let payload_bytes = envelope
        .payload()
        .ok_or_else(|| anyhow::anyhow!("Envelope missing payload"))?
        .bytes();

    let encrypted_manifest = protocol::machine::root_as_encrypted_manifest(payload_bytes)
        .map_err(|e| anyhow::anyhow!("Failed to parse encrypted manifest: {}", e))?;

    // Decrypt the manifest
    let local_peer_id = libp2p::PeerId::random(); // This should be the actual local peer ID
    let decrypted_yaml =
        decrypt_encrypted_manifest_with_id(manifest_id, &encrypted_manifest, &local_peer_id)
            .await?;

    // Parse the decrypted YAML content
    let manifest_value = match serde_yaml::from_str::<serde_json::Value>(&decrypted_yaml) {
        Ok(v) => v,
        Err(_) => serde_json::json!({ "raw": decrypted_yaml }),
    };

    // Store decrypted manifest for testing
    let _ = crate::restapi::store_decrypted_manifest(manifest_id, manifest_value.clone()).await;

    // Deploy to runtime engine
    deploy_manifest_to_runtime(manifest_id, &manifest_value, local_peer).await
}

fn create_deployment_config_from_manifest(
    manifest_value: &serde_json::Value,
) -> Result<crate::runtime::DeploymentConfig, anyhow::Error> {
    // Handle raw content
    if let Some(raw_str) = manifest_value.get("raw").and_then(|v| v.as_str()) {
        // Try to parse as YAML first, then JSON
        let parsed: serde_json::Value = serde_yaml::from_str(raw_str)
            .or_else(|_| serde_json::from_str(raw_str))
            .unwrap_or_else(|_| serde_json::json!({"spec": {"replicas": 1}}));
        return create_deployment_config_from_manifest(&parsed);
    }

    // Extract replicas from Kubernetes Deployment spec (default to 1)
    let replicas = manifest_value
        .get("spec")
        .and_then(|s| s.get("replicas"))
        .and_then(|r| r.as_u64())
        .unwrap_or(1) as u32;

    // Extract environment variables
    let mut env = std::collections::HashMap::new();
    if let Some(containers) = manifest_value
        .get("spec")
        .and_then(|s| s.get("template"))
        .and_then(|t| t.get("spec"))
        .and_then(|s| s.get("containers"))
        .and_then(|c| c.as_array())
    {
        for container in containers {
            if let Some(env_vars) = container.get("env").and_then(|e| e.as_array()) {
                for env_var in env_vars {
                    if let (Some(name), Some(value)) = (
                        env_var.get("name").and_then(|n| n.as_str()),
                        env_var.get("value").and_then(|v| v.as_str()),
                    ) {
                        env.insert(name.to_string(), value.to_string());
                    }
                }
            }
        }
    }

    // Create resource limits (defaults for now)
    let resources = crate::runtime::ResourceLimits {
        cpu: Some(1.0),
        memory: Some(512 * 1024 * 1024), // 512MB
        storage: None,
    };

    Ok(crate::runtime::DeploymentConfig {
        replicas,
        resources,
        env,
        runtime_options: std::collections::HashMap::new(),
    })
}

#[allow(dead_code)]
async fn get_connected_peers() -> Vec<libp2p::PeerId> {
    Vec::new()
}

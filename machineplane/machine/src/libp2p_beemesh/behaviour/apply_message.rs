use crate::libp2p_beemesh::error_helpers;
use base64::prelude::*;
use libp2p::request_response;
use log::{debug, info, warn};
use protocol::machine::build_keyshare_request;
use rand;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Search for local key shares for a specific manifest ID
fn search_local_key_shares(manifest_id: &str) -> Result<Vec<(String, Vec<u8>)>, anyhow::Error> {
    // Open the keystore
    let keystore = crate::libp2p_beemesh::open_keystore()?;

    // Get all CIDs
    let all_cids = keystore.list_cids()?;

    log::warn!(
        "libp2p: DECRYPT DEBUG - searching through {} CIDs in keystore for manifest_id={}",
        all_cids.len(),
        manifest_id
    );

    let mut key_shares = Vec::new();

    // Instead of searching all CIDs, directly look for all CIDs matching the manifest_id in keystore metadata
    match keystore.find_cids_for_manifest(manifest_id) {
        Ok(cids) => {
            log::warn!("libp2p: DECRYPT DEBUG - find_cids_for_manifest returned {} CIDs for manifest_id={}", cids.len(), manifest_id);
            for cid in cids.iter() {
                if let Ok(Some(blob)) = keystore.get(cid) {
                    match crypto::ensure_kem_keypair_on_disk() {
                        Ok((_pubb, privb)) => {
                            match crypto::decrypt_share_from_blob(&blob, &privb) {
                                Ok(decrypted_share) => {
                                    log::warn!("libp2p: DECRYPT DEBUG - found matching local key share for manifest_id={} (cid={})", manifest_id, cid);
                                    key_shares.push((cid.clone(), decrypted_share));
                                }
                                Err(e) => {
                                    log::warn!("libp2p: DECRYPT DEBUG - failed to decrypt share blob for cid {}: {}", cid, e);
                                }
                            }
                        }
                        Err(e) => {
                            log::warn!("libp2p: decrypt could not get KEM keypair: {}", e);
                        }
                    }
                }
            }
        }
        Err(e) => {
            log::warn!(
                "libp2p: DECRYPT DEBUG - error searching keystore for manifest_id {}: {}",
                manifest_id,
                e
            );
        }
    }

    log::warn!(
        "libp2p: DECRYPT DEBUG - returning {} key_shares for manifest_id={}",
        key_shares.len(),
        manifest_id
    );
    Ok(key_shares)
}

/// Decrypt an encrypted manifest using a specific manifest ID and encrypted manifest
async fn decrypt_encrypted_manifest_with_id(
    manifest_id: &str,
    encrypted_manifest: &protocol::machine::EncryptedManifest<'_>,
    local_peer_id: &libp2p::PeerId,
) -> Result<String, anyhow::Error> {
    log::warn!(
        "libp2p: DECRYPT ENTRY - starting decryption for manifest_id={}",
        manifest_id
    );
    let nonce_str = encrypted_manifest.nonce().unwrap_or("");
    let payload_str = encrypted_manifest.payload().unwrap_or("");

    if nonce_str.is_empty() || payload_str.is_empty() {
        return Err(anyhow::anyhow!(
            "missing nonce or payload in encrypted manifest"
        ));
    }

    // Step 1: Search for key shares in local keystore
    log::warn!(
        "libp2p: DECRYPT ENTRY - about to search local keyshares for manifest_id={}",
        manifest_id
    );
    let mut key_shares = search_local_key_shares(manifest_id)?;
    log::warn!(
        "libp2p: DECRYPT ENTRY - found {} local key shares for manifest_id={}, need 2 for k=2",
        key_shares.len(),
        manifest_id
    );

    // Step 2: If we don't have enough local shares, fetch from other nodes
    if key_shares.len() < 2 {
        let needed_shares = 2 - key_shares.len();
        log::warn!(
            "libp2p: DECRYPTION DEBUG - need {} more key shares, initiating distributed retrieval",
            needed_shares
        );

        // Try DHT provider discovery first
        let mut holders = find_manifest_holders(manifest_id).await?;
        log::info!(
            "libp2p: decrypt_with_id found {} potential holders via DHT: {:?}",
            holders.len(),
            holders
        );

        // If DHT discovery didn't find enough providers, use connected peers as fallback
        if holders.len() < 2 {
            log::warn!(
                "libp2p: DHT found only {} providers, using connected peers as fallback",
                holders.len()
            );
            let connected_peers = get_connected_peers().await;
            log::info!(
                "libp2p: found {} connected peers for fallback: {:?}",
                connected_peers.len(),
                connected_peers
            );

            // Add connected peers that aren't already in holders
            for peer_id in connected_peers {
                if !holders.contains(&peer_id) {
                    holders.push(peer_id);
                }
            }

            log::info!(
                "libp2p: after fallback, have {} total providers: {:?}",
                holders.len(),
                holders
            );
        }

        let peers_to_query = holders;
        log::info!(
            "libp2p: decrypt_with_id will query {} providers: {:?}",
            peers_to_query.len(),
            peers_to_query
        );

        // Fetch shares from remote peers until we have enough
        let mut fetched_shares = 0;
        for peer_id in peers_to_query {
            if key_shares.len() >= 2 {
                break; // We have enough shares
            }

            // Prepare the keyshare request - for inter-node fetching, we MUST include capability token
            log::debug!("libp2p: KEYSHARE DEBUG - building keyshare request with manifest_id='{}' capability=''", manifest_id);
            let keyshare_request_fb = build_keyshare_request(manifest_id, "");

            // Attempt to attach a locally-held capability token (if present) so
            // the remote peer can authorize this fetch. The capability token is
            // stored in the keystore under metadata `keyshare_capability:<manifest_id>` by
            // the originator when it distributed capabilities during apply.
            let mut request_fb_to_send = keyshare_request_fb.clone();
            if let Ok(ks) = crate::libp2p_beemesh::open_keystore() {
                // Try keyshare_capability first, then fall back to legacy capability format
                let keyshare_cap_meta = format!("keyshare_capability:{}", manifest_id);
                let legacy_cap_meta = format!("capability:{}", manifest_id);

                log::debug!(
                    "libp2p: attempting keystore capability lookup for meta='{}' (legacy fallback: '{}')",
                    keyshare_cap_meta, legacy_cap_meta
                );

                let cap_lookup_result = ks
                    .find_cid_for_manifest(&keyshare_cap_meta)
                    .or_else(|_| ks.find_cid_for_manifest(&legacy_cap_meta));

                match cap_lookup_result {
                    Ok(Some(cap_cid)) => {
                        log::debug!("libp2p: keystore lookup succeeded -> cid={}", cap_cid);

                        if let Ok(Some(cap_blob)) = ks.get(&cap_cid) {
                            // Decrypt the stored capability blob using our KEM privkey
                            if let Ok((_pubb, privb)) = crypto::ensure_kem_keypair_on_disk() {
                                if let Ok(cap_plain) =
                                    crypto::decrypt_share_from_blob(&cap_blob, &privb)
                                {
                                    // cap_plain contains the signed envelope bytes
                                    log::debug!("libp2p: decrypt local capability plain_len={} prefix={:02x}{:02x}{:02x}", cap_plain.len(),
                                        if cap_plain.len() > 0 { cap_plain[0] } else { 0u8 },
                                        if cap_plain.len() > 1 { cap_plain[1] } else { 0u8 },
                                        if cap_plain.len() > 2 { cap_plain[2] } else { 0u8 }
                                    );

                                    // Extract capability token from envelope and add holder signature
                                    match add_holder_signature_to_capability(
                                        &cap_plain,
                                        manifest_id,
                                        local_peer_id,
                                    ) {
                                        Ok(signed_cap_envelope) => {
                                            let cap_b64 = base64::engine::general_purpose::STANDARD
                                                .encode(&signed_cap_envelope);

                                            log::debug!("libp2p: CAPABILITY DEBUG - added holder signature for manifest_id={} cap_len={} cap_b64_len={}",
                                                manifest_id, signed_cap_envelope.len(), cap_b64.len());

                                            request_fb_to_send =
                                                build_keyshare_request(manifest_id, &cap_b64);
                                        }
                                        Err(e) => {
                                            log::warn!("libp2p: failed to add holder signature to capability: {:?}", e);
                                            // Fall back to original unsigned token
                                            let cap_b64 = base64::engine::general_purpose::STANDARD
                                                .encode(&cap_plain);
                                            request_fb_to_send =
                                                build_keyshare_request(manifest_id, &cap_b64);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Ok(None) => {
                        log::debug!("libp2p: keystore lookup found no cid for keyshare capability");
                    }
                    Err(e) => {
                        log::warn!(
                            "libp2p: keystore lookup error for keyshare capability: {:?}",
                            e
                        );
                    }
                }
            }

            // Check if this is a self-request and handle it locally to avoid libp2p request-response issues
            if peer_id == *local_peer_id {
                log::info!(
                    "libp2p: handling self-fetch locally for manifest_id={}",
                    manifest_id
                );
                // Handle self-fetch locally by directly querying our keystore
                if let Ok(ks) = crate::libp2p_beemesh::open_keystore() {
                    match ks.find_cids_for_manifest(manifest_id) {
                        Ok(cids) => {
                            for cid in cids.iter() {
                                // Skip CIDs we already have from local search to avoid duplicates
                                if key_shares
                                    .iter()
                                    .any(|(existing_cid, _)| existing_cid == cid)
                                {
                                    log::info!("libp2p: self-fetch skipping duplicate CID {} for manifest_id={}", cid, manifest_id);
                                    continue;
                                }

                                if let Ok(Some(blob)) = ks.get(cid) {
                                    if let Ok((_pubb, privb)) = crypto::ensure_kem_keypair_on_disk()
                                    {
                                        if let Ok(plain) =
                                            crypto::decrypt_share_from_blob(&blob, &privb)
                                        {
                                            log::info!("libp2p: self-fetch found new keyshare for manifest_id={} cid={}", manifest_id, cid);
                                            let self_share_key = format!("self_{}", cid);
                                            key_shares.push((self_share_key, plain));
                                            fetched_shares += 1;

                                            // Break if we have enough shares
                                            if key_shares.len() >= 2 {
                                                break;
                                            }
                                        } else {
                                            log::warn!("libp2p: self-fetch failed to decrypt keyshare for manifest_id={} cid={}", manifest_id, cid);
                                        }
                                    } else {
                                        log::warn!("libp2p: self-fetch could not load KEM keypair for decryption");
                                    }
                                } else {
                                    log::warn!("libp2p: self-fetch could not get blob for manifest_id={} cid={}", manifest_id, cid);
                                }
                            }
                        }
                        Err(e) => {
                            log::warn!(
                                "libp2p: self-fetch keystore error for manifest_id={}: {:?}",
                                manifest_id,
                                e
                            );
                        }
                    }
                } else {
                    log::warn!("libp2p: self-fetch could not open keystore");
                }
            } else {
                // Fetch share from remote peer
                match fetch_keyshare_from_peer(&peer_id, request_fb_to_send).await {
                    Ok(keyshare_response_bytes) => {
                        // Parse the FlatBuffer response
                        if let Ok(keyshare_response) =
                            protocol::machine::root_as_key_share_response(&keyshare_response_bytes)
                        {
                            if keyshare_response.ok() {
                                if let Some(share_b64) = keyshare_response.message() {
                                    if let Ok(share_bytes) =
                                        base64::engine::general_purpose::STANDARD.decode(share_b64)
                                    {
                                        // Store the fetched share in our collection
                                        let share_cid =
                                            format!("remote_{}_{}", peer_id, fetched_shares);
                                        key_shares.push((share_cid, share_bytes));
                                        fetched_shares += 1;
                                        log::info!("libp2p: decrypt_with_id successfully fetched share from peer={} (now have {} shares)", peer_id, key_shares.len());
                                    }
                                }
                            } else {
                                log::warn!(
                                    "libp2p: decrypt_with_id peer={} returned error: {:?}",
                                    peer_id,
                                    keyshare_response.message()
                                );
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!(
                            "libp2p: decrypt_with_id failed to fetch from peer={}: {}",
                            peer_id,
                            e
                        );
                    }
                }
            }
        }

        if key_shares.len() < 2 {
            log::error!(
                "libp2p: DECRYPTION DEBUG - INSUFFICIENT SHARES! Have {} but need 2",
                key_shares.len()
            );
            return Err(error_helpers::insufficient_key_shares(key_shares.len(), 2));
        }
        log::debug!(
            "libp2p: DECRYPTION DEBUG - after fetching, have {} shares",
            key_shares.len()
        );
    } else {
        log::warn!(
            "libp2p: DECRYPTION DEBUG - have enough local shares ({}) proceeding",
            key_shares.len()
        );
    }

    // Step 3: Recover the symmetric key from the shares
    let mut shares = Vec::new();
    for (cid, share_data) in &key_shares {
        log::warn!(
            "libp2p: SHARE DEBUG - cid={} share_len={} first_4_bytes={:02x}{:02x}{:02x}{:02x}",
            cid,
            share_data.len(),
            if share_data.len() > 0 {
                share_data[0]
            } else {
                0u8
            },
            if share_data.len() > 1 {
                share_data[1]
            } else {
                0u8
            },
            if share_data.len() > 2 {
                share_data[2]
            } else {
                0u8
            },
            if share_data.len() > 3 {
                share_data[3]
            } else {
                0u8
            }
        );
        shares.push(share_data.clone());
    }

    log::warn!(
        "libp2p: SHARE DEBUG - about to call recover_symmetric_key with {} shares",
        shares.len()
    );
    let symmetric_key = crypto::recover_symmetric_key(&shares, 2)?;
    log::info!(
        "libp2p: decrypt_with_id successfully recovered symmetric key using {} share(s)",
        shares.len()
    );

    // Step 4: Decrypt the manifest
    let nonce_bytes = base64::engine::general_purpose::STANDARD.decode(nonce_str)?;
    let payload_bytes = base64::engine::general_purpose::STANDARD.decode(payload_str)?;

    let decrypted_bytes = crypto::decrypt_manifest(&symmetric_key, &nonce_bytes, &payload_bytes)?;
    let decrypted_yaml_str = String::from_utf8(decrypted_bytes)?;

    log::info!(
        "libp2p: decrypt_with_id successfully decrypted manifest to YAML (len={})",
        decrypted_yaml_str.len()
    );

    Ok(decrypted_yaml_str)
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
            tenant,
            operation_id,
            &local_peer.to_string(),
            &empty_pubkey,
            protocol::machine::SignatureScheme::NONE,
            &empty_signature,
            manifest_json,
            manifest_kind,
            labels,
            timestamp,
            protocol::machine::OperationType::APPLY,
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
                match crate::libp2p_beemesh::security::verify_envelope_and_check_nonce(&request) {
                    Ok((payload_bytes, _pub, _sig)) => payload_bytes,
                    Err(e) => {
                        if crate::libp2p_beemesh::security::require_signed_messages() {
                            warn!("rejecting unsigned/invalid apply request: {:?}", e);
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

                    // In a real implementation, you would:
                    // 1. Validate the manifest
                    // 2. Actually deploy the workload
                    // 3. Store the applied manifest in the DHT

                    // For now, simulate successful application and store in DHT
                    let success = true; // This would be the result of actual deployment

                    if success {
                        // Store the applied manifest in the DHT
                        store_applied_manifest_in_dht(swarm, &apply_req, local_peer);

                        // Also store the encrypted manifest locally to become a manifest holder
                        // The manifest_json field contains the base64-encoded encrypted envelope
                        if let Some(manifest_json) = apply_req.manifest_json() {
                            if let Ok(encrypted_envelope_bytes) =
                                base64::engine::general_purpose::STANDARD.decode(manifest_json)
                            {
                                // Calculate manifest_id from tenant, operation_id, and manifest_json
                                let manifest_id = if let (Some(tenant), Some(operation_id)) =
                                    (apply_req.tenant(), apply_req.operation_id())
                                {
                                    let mut hasher = DefaultHasher::new();
                                    tenant.hash(&mut hasher);
                                    operation_id.hash(&mut hasher);
                                    manifest_json.hash(&mut hasher);
                                    format!("{:x}", hasher.finish())
                                } else {
                                    format!("{:x}", DefaultHasher::new().finish())
                                };

                                debug!(
                                    "libp2p: apply request storing manifest locally for manifest_id={}",
                                    manifest_id
                                );
                                // TODO: Store manifest locally in manifest store
                                // For now, just log that we would store it
                                debug!(
                                    "libp2p: would store encrypted manifest locally (size={} bytes) for manifest_id={}",
                                    encrypted_envelope_bytes.len(),
                                    manifest_id
                                );
                            } else {
                                warn!("libp2p: failed to decode base64 manifest_json in apply request");
                            }
                        }
                    }

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
pub fn process_self_apply_request(manifest: &[u8], swarm: &mut libp2p::Swarm<super::MyBehaviour>) {
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
                    let _ = crate::restapi::store_decrypted_manifest(&manifest_id, manifest_value)
                        .await;
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
async fn find_manifest_holders(manifest_id: &str) -> Result<Vec<libp2p::PeerId>, anyhow::Error> {
    use tokio::sync::mpsc;

    let mut all_holders = std::collections::HashSet::new();

    // Retry up to 5 times with delays to allow provider announcements to propagate
    // Continue searching until we find at least 2 providers (k=2) or exhaust attempts
    for attempt in 1..=5 {
        let (tx, mut rx) = mpsc::unbounded_channel();

        // Send control message to find manifest holders using new announcement system
        let control_msg =
            crate::libp2p_beemesh::control::Libp2pControl::FindManifestHoldersWithVersion {
                manifest_id: manifest_id.to_string(),
                version: None, // Find any version
                reply_tx: tx,
            };
        crate::libp2p_beemesh::control::enqueue_control(control_msg);

        // Wait for response with longer timeout for local tests
        match tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await {
            Ok(Some(holder_infos)) => {
                if !holder_infos.is_empty() {
                    for holder_info in holder_infos {
                        // Convert holder info to PeerId
                        if let Ok(peer_id) = holder_info.peer_id.parse::<libp2p::PeerId>() {
                            all_holders.insert(peer_id);
                        }
                    }
                    log::info!("libp2p: find_manifest_holders found {} total holders for manifest_id={} (attempt {})", all_holders.len(), manifest_id, attempt);

                    // Continue searching until we have at least 2 providers or this is the last attempt
                    if all_holders.len() >= 2 || attempt == 5 {
                        return Ok(all_holders.into_iter().collect());
                    }
                } else {
                    log::info!("libp2p: find_manifest_holders attempt {} returned empty result for manifest_id={}", attempt, manifest_id);
                }
            }
            Ok(None) => {
                log::warn!(
                    "libp2p: find_manifest_holders channel closed for manifest_id={} (attempt {})",
                    manifest_id,
                    attempt
                );
            }
            Err(_timeout) => {
                log::warn!(
                    "libp2p: find_manifest_holders timeout for manifest_id={} (attempt {})",
                    manifest_id,
                    attempt
                );
            }
        }

        // If not the last attempt, wait before retrying to allow provider announcements to propagate
        if attempt < 5 {
            log::info!(
                "libp2p: find_manifest_holders waiting 3s before retry {} for manifest_id={}",
                attempt + 1,
                manifest_id
            );
            // Shorter retry delay for local tests but allow DHT to propagate
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    }

    // Return whatever providers we found, even if less than ideal
    log::warn!(
        "libp2p: find_manifest_holders exhausted all attempts for manifest_id={}, found {} providers",
        manifest_id,
        all_holders.len()
    );
    Ok(all_holders.into_iter().collect())
}

/// Fetch manifest from holders using the new peer-based system
async fn fetch_manifest_from_holders(
    manifest_id: &str,
    version: Option<u64>,
) -> Result<Vec<u8>, anyhow::Error> {
    use tokio::sync::mpsc;

    // First, find manifest holders
    let (tx, mut rx) = mpsc::unbounded_channel();
    let control_msg =
        crate::libp2p_beemesh::control::Libp2pControl::FindManifestHoldersWithVersion {
            manifest_id: manifest_id.to_string(),
            version,
            reply_tx: tx,
        };
    crate::libp2p_beemesh::control::enqueue_control(control_msg);

    // Wait for response
    let holders = match tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await {
        Ok(Some(holder_infos)) => holder_infos,
        Ok(None) => {
            return Err(anyhow::anyhow!(
                "No response received when finding manifest holders"
            ));
        }
        Err(_) => {
            return Err(anyhow::anyhow!("Timeout when finding manifest holders"));
        }
    };

    if holders.is_empty() {
        return Err(anyhow::anyhow!(
            "No holders found for manifest {}",
            manifest_id
        ));
    }

    // Try to fetch from the first available holder
    for holder in holders {
        if let Ok(peer_id) = holder.peer_id.parse::<libp2p::PeerId>() {
            log::info!(
                "Attempting to fetch manifest {} version {:?} from peer {}",
                manifest_id,
                version,
                peer_id
            );

            // For now, return an error since we need async coordination with the swarm
            // In a full implementation, this would use the manifest_fetch protocol
            return Err(anyhow::anyhow!(
                "Manifest fetching from peer {} not yet implemented",
                peer_id
            ));
        }
    }

    Err(anyhow::anyhow!("No valid peer IDs found in holders list"))
}

/// Get list of currently connected peers as fallback when DHT provider discovery fails
async fn get_connected_peers() -> Vec<libp2p::PeerId> {
    use tokio::sync::mpsc;

    let (tx, mut rx) = mpsc::unbounded_channel();

    // Send control message to get connected peers
    let control_msg =
        crate::libp2p_beemesh::control::Libp2pControl::GetConnectedPeers { reply_tx: tx };

    // Get the control sender from the global context
    if let Some(control_tx) = crate::libp2p_beemesh::get_control_sender() {
        if let Err(e) = control_tx.send(control_msg) {
            log::warn!(
                "libp2p: failed to send GetConnectedPeers control message: {}",
                e
            );
            return vec![];
        }
    } else {
        log::warn!("libp2p: control sender not available");
        return vec![];
    }

    // Wait for response with timeout
    match tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv()).await {
        Ok(Some(peer_ids)) => {
            log::info!(
                "libp2p: get_connected_peers found {} connected peers",
                peer_ids.len()
            );
            peer_ids
        }
        Ok(None) => {
            log::warn!("libp2p: get_connected_peers channel closed");
            vec![]
        }
        Err(_) => {
            log::warn!("libp2p: get_connected_peers timeout");
            vec![]
        }
    }
}

/// Fetch a keyshare from a specific peer using FlatBuffer request/response
async fn fetch_keyshare_from_peer(
    peer_id: &libp2p::PeerId,
    request_fb: Vec<u8>,
) -> Result<Vec<u8>, anyhow::Error> {
    use tokio::sync::mpsc;

    let (tx, mut rx) = mpsc::unbounded_channel();

    // Send control message to fetch keyshare
    let control_msg = crate::libp2p_beemesh::control::Libp2pControl::FetchKeyshare {
        peer_id: *peer_id,
        request_fb,
        reply_tx: tx,
    };

    // Get the control sender from the global context
    if let Some(control_tx) = crate::libp2p_beemesh::get_control_sender() {
        if let Err(e) = control_tx.send(control_msg) {
            return Err(anyhow::anyhow!(
                "failed to send FetchKeyshare control message: {}",
                e
            ));
        }
    } else {
        return Err(error_helpers::control_sender_unavailable());
    }

    // Wait for response with timeout
    match tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv()).await {
        Ok(Some(Ok(response_bytes))) => {
            log::debug!("libp2p: fetch_keyshare_from_peer successfully received response from peer={} (len={})", peer_id, response_bytes.len());
            Ok(response_bytes)
        }
        Ok(Some(Err(e))) => {
            log::warn!(
                "libp2p: fetch_keyshare_from_peer error from peer={}: {}",
                peer_id,
                e
            );
            Err(anyhow::anyhow!("keyshare fetch error: {}", e))
        }
        Ok(None) => {
            log::warn!(
                "libp2p: fetch_keyshare_from_peer channel closed for peer={}",
                peer_id
            );
            Err(anyhow::anyhow!("keyshare fetch channel closed"))
        }
        Err(_) => {
            log::warn!(
                "libp2p: fetch_keyshare_from_peer timeout for peer={}",
                peer_id
            );
            Err(anyhow::anyhow!("keyshare fetch timeout"))
        }
    }
}

/// Add holder signature to capability token
/// Extracts token from envelope, adds holder signature, and returns new envelope
fn add_holder_signature_to_capability(
    envelope_bytes: &[u8],
    manifest_id: &str,
    local_peer_id: &libp2p::PeerId,
) -> anyhow::Result<Vec<u8>> {
    // Verify the envelope and extract the capability token (skip nonce check for re-signing)
    let (token_bytes, _pub, _sig) =
        crate::libp2p_beemesh::envelope::verify_flatbuffer_envelope_skip_nonce_check(
            envelope_bytes,
        )?;

    // Parse the capability token
    let capability_token = protocol::machine::root_as_capability_token(&token_bytes)
        .map_err(|e| anyhow::anyhow!("Failed to parse capability token: {}", e))?;

    // Get our keypair for signing
    let (pub_bytes, priv_bytes) = crypto::ensure_keypair_on_disk()?;

    // Use the provided local peer ID

    // Create presentation context with unique nonce including peer ID and random component
    let presentation_nonce = format!(
        "holder_sig_{}_{}_{:x}",
        local_peer_id,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        rand::random::<u64>()
    );
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let presentation_context = crypto::create_capability_presentation_context(
        &token_bytes,
        &presentation_nonce,
        timestamp,
        manifest_id,
        "KeyShareRequest",
    );

    // Sign the presentation context
    let (holder_sig_b64, holder_pub_b64) =
        crypto::sign_capability_presentation(&priv_bytes, &pub_bytes, &presentation_context)?;

    let holder_sig_bytes = base64::engine::general_purpose::STANDARD.decode(holder_sig_b64)?;
    let holder_pub_bytes = base64::engine::general_purpose::STANDARD.decode(holder_pub_b64)?;

    // Extract original token data
    let root_cap = capability_token
        .root_capability()
        .ok_or_else(|| anyhow::anyhow!("Missing root capability"))?;

    let issuer_peer_id = root_cap.issuer_peer_id().unwrap_or("");
    let issued_at = root_cap.issued_at();
    let expires_at = root_cap.expires_at();

    // Get authorized peer from caveats
    let authorized_peer = if let Some(caveats) = capability_token.caveats() {
        if caveats.len() > 0 {
            let caveat = caveats.get(0);
            if caveat.condition_type().unwrap_or("") == "authorized_peer" {
                if let Some(value) = caveat.value() {
                    let value_bytes: Vec<u8> = value.iter().collect();
                    std::str::from_utf8(&value_bytes).unwrap_or("").to_string()
                } else {
                    String::new()
                }
            } else {
                String::new()
            }
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    // Build new capability token with holder signature
    let signed_token_bytes = protocol::machine::build_capability_token_with_holder_signature(
        manifest_id,
        issuer_peer_id,
        &authorized_peer,
        issued_at,
        expires_at,
        &local_peer_id.to_string(),
        &holder_pub_bytes,
        &holder_sig_bytes,
        &presentation_nonce,
        timestamp,
    );

    // Create new envelope with the signed token
    let envelope_nonce: [u8; 16] = rand::random();
    let nonce_str = base64::engine::general_purpose::STANDARD.encode(&envelope_nonce);
    let ts = timestamp;

    // Build canonical envelope
    let canonical = protocol::machine::build_envelope_canonical(
        &signed_token_bytes,
        "capability",
        &nonce_str,
        ts,
        "ml-dsa-65",
        None,
    );

    // Sign the envelope
    let (sig_b64, pub_b64) = crypto::sign_envelope(&priv_bytes, &pub_bytes, &canonical)?;

    // Build final signed envelope
    let signed_envelope = protocol::machine::build_envelope_signed(
        &signed_token_bytes,
        "capability",
        &nonce_str,
        ts,
        "ml-dsa-65",
        "ml-dsa-65",
        &sig_b64,
        &pub_b64,
        None,
    );

    Ok(signed_envelope)
}

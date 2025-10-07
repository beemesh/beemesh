use libp2p::request_response;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use log::{info, warn};
use base64::prelude::*;
use protocol::machine::build_keyshare_request;

/// Search for local key shares for a specific manifest ID  
fn search_local_key_shares(manifest_id: &str) -> Result<Vec<(String, Vec<u8>)>, anyhow::Error> {
    // Open the keystore
    let keystore = crate::libp2p_beemesh::open_keystore()?;
    
    // Get all CIDs
    let all_cids = keystore.list_cids()?;
    
    log::info!("libp2p: decrypt searching through {} CIDs in keystore for manifest_id={}", all_cids.len(), manifest_id);
    
    let mut key_shares = Vec::new();
    
    // Instead of searching all CIDs, directly look for all CIDs matching the manifest_id in keystore metadata
    match keystore.find_cids_for_manifest(manifest_id) {
        Ok(cids) => {
            for cid in cids.iter() {
                if let Ok(Some(blob)) = keystore.get(cid) {
                    match crypto::ensure_kem_keypair_on_disk() {
                        Ok((_pubb, privb)) => {
                            match crypto::decrypt_share_from_blob(&blob, &privb) {
                                Ok(decrypted_share) => {
                                    log::info!("libp2p: decrypt found matching local key share for manifest_id={} (cid={})", manifest_id, cid);
                                    key_shares.push((cid.clone(), decrypted_share));
                                }
                                Err(e) => {
                                    log::warn!("libp2p: decrypt failed to decrypt share blob for cid {}: {}", cid, e);
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
            log::warn!("libp2p: decrypt error searching keystore for manifest_id {}: {}", manifest_id, e);
        }
    }
    
    Ok(key_shares)
}

/// Decrypt an encrypted manifest using only local key shares (for testing)
async fn decrypt_encrypted_manifest_local_only(manifest_json_str: &str) -> Result<String, anyhow::Error> {
    use base64::Engine as _;
    
    // Parse the JSON to extract nonce and payload
    let json_val = serde_json::from_str::<serde_json::Value>(manifest_json_str)?;
    let nonce_str = json_val.get("nonce")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing nonce in encrypted manifest"))?;
    let payload_str = json_val.get("payload")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing payload in encrypted manifest"))?;

    // Calculate manifest ID from the JSON content
    let mut hasher = DefaultHasher::new();
    manifest_json_str.hash(&mut hasher);
    let manifest_id = format!("{:x}", hasher.finish());

    log::info!("libp2p: decrypt attempting to recover key shares for manifest_id={}", manifest_id);
    
    // Step 1: Search for key shares in local keystore
    let key_shares = search_local_key_shares(&manifest_id)?;
    log::info!("libp2p: decrypt found {} local key shares for manifest_id={}", key_shares.len(), manifest_id);
    
    if key_shares.len() >= 2 {
        log::info!("libp2p: decrypt have enough local shares, proceeding to decrypt");
    } else {
        log::warn!("libp2p: decrypt_local_only need {} more key shares for manifest_id={} - not enough local shares", 2 - key_shares.len(), manifest_id);
        return Err(anyhow::anyhow!("not enough local key shares - need {} more", 2 - key_shares.len()));
    }
    
    // Step 2: Recover the symmetric key from the shares
    let mut shares_data = Vec::new();
    for (_, share_data) in &key_shares {
        shares_data.push(share_data.clone());
    }
    
    let symmetric_key = crypto::recover_symmetric_key(&shares_data, 2)?;
    log::info!("libp2p: decrypt successfully recovered symmetric key");
    
    // Step 3: Decrypt the manifest  
    let nonce_bytes = base64::engine::general_purpose::STANDARD.decode(nonce_str)?;
    let payload_bytes = base64::engine::general_purpose::STANDARD.decode(payload_str)?;
    
    let decrypted_bytes = crypto::decrypt_manifest(&symmetric_key, &nonce_bytes, &payload_bytes)?;
    let decrypted_yaml_str = String::from_utf8(decrypted_bytes)?;
    
    log::info!("libp2p: decrypt successfully decrypted manifest to YAML (len={})", decrypted_yaml_str.len());
    
    Ok(decrypted_yaml_str)
}

/// Decrypt an encrypted manifest using a specific manifest ID and nonce/payload
async fn decrypt_encrypted_manifest_with_id(manifest_id: &str, nonce_str: &str, payload_str: &str, local_peer_id: &libp2p::PeerId) -> Result<String, anyhow::Error> {
    log::info!("libp2p: decrypt_with_id attempting decryption for manifest_id={}", manifest_id);
    
    // Step 1: Search for key shares in local keystore
    let mut key_shares = search_local_key_shares(manifest_id)?;
    log::info!("libp2p: decrypt_with_id found {} local key shares for manifest_id={}", key_shares.len(), manifest_id);
    
        // Step 2: If we don't have enough local shares, fetch from other nodes
    if key_shares.len() < 2 {
        let needed_shares = 2 - key_shares.len();
        log::info!("libp2p: decrypt_with_id need {} more key shares, initiating distributed retrieval", needed_shares);
        
        // Try DHT provider discovery first
        let holders = find_manifest_holders(manifest_id).await?;
        log::info!("libp2p: decrypt_with_id found {} potential holders via DHT: {:?}", holders.len(), holders);
        
        // Always get all connected peers and combine with DHT results to maximize our chances of finding keyshares
        let connected_peers = get_connected_peers().await;
        log::info!("libp2p: decrypt_with_id found {} connected peers: {:?}", connected_peers.len(), connected_peers);
        
        // Combine DHT holders and connected peers, removing duplicates
        let mut peers_to_query = holders;
        for peer in connected_peers {
            if !peers_to_query.contains(&peer) {
                peers_to_query.push(peer);
            }
        }
        log::info!("libp2p: decrypt_with_id will query {} total peers: {:?}", peers_to_query.len(), peers_to_query);
        
        // Fetch shares from remote peers until we have enough
        let mut fetched_shares = 0;
        for peer_id in peers_to_query {
            if key_shares.len() >= 2 {
                break; // We have enough shares
            }
            
            // Prepare the keyshare request
            let keyshare_request_fb = build_keyshare_request(manifest_id, "");
            
            // Attempt to attach a locally-held capability token (if present) so
            // the remote peer can authorize this fetch. The capability token is
            // stored in the keystore under metadata `capability:<manifest_id>` by
            // the originator when it distributed capabilities during apply.
            let mut request_fb_to_send = keyshare_request_fb.clone();
            if let Ok(ks) = crate::libp2p_beemesh::open_keystore() {
                let cap_meta = format!("capability:{}", manifest_id);
                log::info!("libp2p: attempting keystore capability lookup for meta='{}'", cap_meta);
                match ks.find_cid_for_manifest(&cap_meta) {
                    Ok(Some(cap_cid)) => {
                        log::info!("libp2p: keystore lookup succeeded for meta='{}' -> cid={}", cap_meta, cap_cid);

                        if let Ok(Some(cap_blob)) = ks.get(&cap_cid) {
                            // Decrypt the stored capability blob using our KEM privkey
                            if let Ok((_pubb, privb)) = crypto::ensure_kem_keypair_on_disk() {
                                if let Ok(cap_plain) = crypto::decrypt_share_from_blob(&cap_blob, &privb) {
                                    // cap_plain contains the signed JSON envelope bytes
                                    log::info!("libp2p: decrypt local capability plain_len={} prefix={:02x}{:02x}{:02x}", cap_plain.len(),
                                        if cap_plain.len() > 0 { cap_plain[0] } else { 0u8 },
                                        if cap_plain.len() > 1 { cap_plain[1] } else { 0u8 },
                                        if cap_plain.len() > 2 { cap_plain[2] } else { 0u8 }
                                    );
                                    let cap_b64 = base64::engine::general_purpose::STANDARD.encode(&cap_plain);
                                    // Log diagnostics about attached capability so remote holders can
                                    // be inspected when they reject/parsing fails.
                                    log::info!("libp2p: attaching local capability for manifest_id={} cap_len={} first_byte=0x{:02x}",
                                        manifest_id,
                                        cap_plain.len(),
                                        if cap_plain.is_empty() { 0u8 } else { cap_plain[0] });
                                    request_fb_to_send = build_keyshare_request(manifest_id, &cap_b64);
                                }
                            }
                        }
                    }
                    Ok(None) => {
                        log::info!("libp2p: keystore lookup found no cid for meta='{}'", cap_meta);
                    }
                    Err(e) => {
                        log::warn!("libp2p: keystore lookup error for meta='{}': {:?}", cap_meta, e);
                    }
                }
            }

            // Check if this is a self-request and handle it locally to avoid libp2p request-response issues
            if peer_id == *local_peer_id {
                log::info!("libp2p: handling self-fetch locally for manifest_id={}", manifest_id);
                // Handle self-fetch locally by directly querying our keystore
                if let Ok(ks) = crate::libp2p_beemesh::open_keystore() {
                    match ks.find_cids_for_manifest(manifest_id) {
                        Ok(cids) => {
                            for cid in cids.iter() {
                                // Skip CIDs we already have from local search to avoid duplicates
                                if key_shares.iter().any(|(existing_cid, _)| existing_cid == cid) {
                                    log::info!("libp2p: self-fetch skipping duplicate CID {} for manifest_id={}", cid, manifest_id);
                                    continue;
                                }
                                
                                if let Ok(Some(blob)) = ks.get(cid) {
                                    if let Ok((_pubb, privb)) = crypto::ensure_kem_keypair_on_disk() {
                                        if let Ok(plain) = crypto::decrypt_share_from_blob(&blob, &privb) {
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
                            log::warn!("libp2p: self-fetch keystore error for manifest_id={}: {:?}", manifest_id, e);
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
                    if let Ok(keyshare_response) = protocol::machine::root_as_key_share_response(&keyshare_response_bytes) {
                        if keyshare_response.ok() {
                            if let Some(share_b64) = keyshare_response.message() {
                                if let Ok(share_bytes) = base64::engine::general_purpose::STANDARD.decode(share_b64) {
                                    // Store the fetched share in our collection
                                    let share_cid = format!("remote_{}_{}", peer_id, fetched_shares);
                                    key_shares.push((share_cid, share_bytes));
                                    fetched_shares += 1;
                                    log::info!("libp2p: decrypt_with_id successfully fetched share from peer={} (now have {} shares)", peer_id, key_shares.len());
                                }
                            }
                        } else {
                            log::warn!("libp2p: decrypt_with_id peer={} returned error: {:?}", peer_id, keyshare_response.message());
                        }
                    }
                }
                Err(e) => {
                    log::warn!("libp2p: decrypt_with_id failed to fetch from peer={}: {}", peer_id, e);
                }
                }
            }
        }
        
        if key_shares.len() < 2 {
            return Err(anyhow::anyhow!("insufficient key shares after distributed retrieval: have {} need 2", key_shares.len()));
        }
    } else {
        log::info!("libp2p: have enough local shares proceeding");
    }
    
    // Step 3: Recover the symmetric key from the shares
    let mut shares = Vec::new();
    for (_, share_data) in &key_shares {
        shares.push(share_data.clone());
    }
    
    let symmetric_key = crypto::recover_symmetric_key(&shares, 2)?;
    log::info!("libp2p: decrypt_with_id successfully recovered symmetric key using {} share(s)", shares.len());
    
    // Step 4: Decrypt the manifest  
    let nonce_bytes = base64::engine::general_purpose::STANDARD.decode(nonce_str)?;
    let payload_bytes = base64::engine::general_purpose::STANDARD.decode(payload_str)?;
    
    let decrypted_bytes = crypto::decrypt_manifest(&symmetric_key, &nonce_bytes, &payload_bytes)?;
    let decrypted_yaml_str = String::from_utf8(decrypted_bytes)?;
    
    log::info!("libp2p: decrypt_with_id successfully decrypted manifest to YAML (len={})", decrypted_yaml_str.len());
    
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
        apply_req.manifest_json()
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

        let query_id = swarm.behaviour_mut().kademlia.put_record(record, libp2p::kad::Quorum::One);
        
        info!("DHT: Storing applied manifest {} (query_id: {:?})", manifest_id, query_id);
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
        request_response::Message::Request { request, channel, .. } => {
            info!("libp2p: received apply request from peer={}", peer);

            // First, attempt to verify request as an Envelope (JSON or FlatBuffer)
            let effective_request = match serde_json::from_slice::<serde_json::Value>(&request) {
                Ok(val) => {
                    match crate::libp2p_beemesh::security::verify_envelope_and_check_nonce(&val) {
                        Ok((payload_bytes, _pub, _sig)) => payload_bytes,
                        Err(e) => {
                            if crate::libp2p_beemesh::security::require_signed_messages() {
                                warn!("rejecting unsigned/invalid apply request: {:?}", e);
                                let error_response = protocol::machine::build_apply_response(
                                    false,
                                    "unknown",
                                    "unsigned or invalid envelope",
                                );
                                let _ = swarm.behaviour_mut().apply_rr.send_response(channel, error_response);
                                return;
                            }
                            request.clone()
                        }
                    }
                }
                Err(_) => request.clone(),
            };

            // Parse the FlatBuffer apply request
            match protocol::machine::root_as_apply_request(&effective_request) {
                Ok(apply_req) => {
                    info!("libp2p: apply request - tenant={:?} operation_id={:?} replicas={}",
                        apply_req.tenant(), apply_req.operation_id(), apply_req.replicas());

                    // In a real implementation, you would:
                    // 1. Validate the manifest
                    // 2. Actually deploy the workload
                    // 3. Store the applied manifest in the DHT
                    
                    // For now, simulate successful application and store in DHT
                    let success = true; // This would be the result of actual deployment
                    
                    if success {
                        // Store the applied manifest in the DHT
                        store_applied_manifest_in_dht(
                            swarm,
                            &apply_req,
                            local_peer,
                        );
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
                    let _ = swarm.behaviour_mut().apply_rr.send_response(channel, response);
                    info!("libp2p: sent apply response to peer={}", peer);
                }
                Err(e) => {
                    warn!("libp2p: failed to parse apply request: {:?}", e);
                    let error_response = protocol::machine::build_apply_response(
                        false,
                        "unknown",
                        &format!("Failed to parse request: {:?}", e),
                    );
                    let _ = swarm.behaviour_mut().apply_rr.send_response(channel, error_response);
                }
            }
        }
        request_response::Message::Response { response, .. } => {
            info!("libp2p: received apply response from peer={}", peer);

            // Parse the response
            match protocol::machine::root_as_apply_response(&response) {
                Ok(apply_resp) => {
                    info!("libp2p: apply response - ok={} operation_id={:?} message={:?}",
                        apply_resp.ok(), apply_resp.operation_id(), apply_resp.message());
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
) {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    log::info!("libp2p: processing self-apply request (manifest len={})", manifest.len());

    // Parse the FlatBuffer apply request (same logic as in apply_message)
    match protocol::machine::root_as_apply_request(manifest) {
        Ok(apply_req) => {
            log::info!("libp2p: self-apply request - tenant={:?} operation_id={:?} replicas={}",
                apply_req.tenant(), apply_req.operation_id(), apply_req.replicas());

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
                    let manifest_id = if let Some(stored_cid) = crate::restapi::get_manifest_cid_for_operation(operation_id).await {
                        log::info!("libp2p: self-apply using stored manifest_cid={} for operation_id={}", stored_cid, operation_id);
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

                    log::info!("libp2p: self-apply triggering decryption for manifest_id={}", manifest_id);
                    
                    // Check if manifest_json is a signed envelope or directly encrypted
                    let manifest_value = if let Ok(envelope) = serde_json::from_str::<serde_json::Value>(manifest_json) {
                        // Check if this is a signed envelope (has 'alg', 'payload', 'sig')
                        if envelope.get("alg").is_some() && envelope.get("sig").is_some() {
                            log::info!("libp2p: self-apply detected signed envelope - extracting payload");
                            
                            // Extract and decode the payload from the signed envelope
                            if let Some(payload_b64) = envelope.get("payload").and_then(|p| p.as_str()) {
                                match base64::engine::general_purpose::STANDARD.decode(payload_b64) {
                                    Ok(payload_bytes) => {
                                        match String::from_utf8(payload_bytes) {
                                            Ok(inner_content) => {
                                                log::info!("libp2p: self-apply extracted payload from envelope: {}", inner_content.chars().take(100).collect::<String>());
                                                
                                                // Now check if the inner content is encrypted (has nonce/payload structure)
                                                if let Ok(inner_json) = serde_json::from_str::<serde_json::Value>(&inner_content) {
                                                    if let (Some(nonce_val), Some(payload_val)) = (inner_json.get("nonce"), inner_json.get("payload")) {
                                                        log::info!("libp2p: self-apply detected encrypted content inside envelope - attempting decryption");
                                                        
                                                        if let (Some(nonce_str), Some(payload_str)) = (nonce_val.as_str(), payload_val.as_str()) {
                                                            match decrypt_encrypted_manifest_with_id(&manifest_id, nonce_str, payload_str, &local_peer_id_copy).await {
                                                                Ok(decrypted_yaml) => {
                                                                    log::info!("libp2p: self-apply successfully decrypted manifest from envelope");
                                                                    // Parse the decrypted YAML content
                                                                    match serde_yaml::from_str::<serde_json::Value>(&decrypted_yaml) {
                                                                        Ok(v) => {
                                                                            log::info!("libp2p: self-apply parsed decrypted YAML successfully");
                                                                            v
                                                                        }
                                                                        Err(_) => {
                                                                            log::info!("libp2p: self-apply treating decrypted content as raw");
                                                                            serde_json::json!({ "raw": decrypted_yaml })
                                                                        }
                                                                    }
                                                                }
                                                                Err(e) => {
                                                                    log::warn!("libp2p: self-apply failed to decrypt manifest from envelope: {}", e);
                                                                    // Fallback to inner content for debugging
                                                                    inner_json
                                                                }
                                                            }
                                                        } else {
                                                            log::warn!("libp2p: self-apply inner content has invalid nonce/payload format");
                                                            inner_json
                                                        }
                                                    } else {
                                                        // Inner content is not encrypted, try to parse as YAML
                                                        match serde_yaml::from_str::<serde_json::Value>(&inner_content) {
                                                            Ok(v) => {
                                                                log::info!("libp2p: self-apply parsed inner YAML successfully");
                                                                v
                                                            }
                                                            Err(_) => {
                                                                log::info!("libp2p: self-apply treating inner content as raw");
                                                                serde_json::json!({ "raw": inner_content })
                                                            }
                                                        }
                                                    }
                                                } else {
                                                    // Inner content is not JSON, try as YAML
                                                    match serde_yaml::from_str::<serde_json::Value>(&inner_content) {
                                                        Ok(v) => {
                                                            log::info!("libp2p: self-apply parsed inner YAML successfully");
                                                            v
                                                        }
                                                        Err(_) => {
                                                            log::info!("libp2p: self-apply treating inner content as raw");
                                                            serde_json::json!({ "raw": inner_content })
                                                        }
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                log::warn!("libp2p: self-apply failed to decode payload as UTF-8: {}", e);
                                                envelope
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        log::warn!("libp2p: self-apply failed to base64 decode payload: {}", e);
                                        envelope
                                    }
                                }
                            } else {
                                log::warn!("libp2p: self-apply envelope missing payload field");
                                envelope
                            }
                        } else if let (Some(nonce_val), Some(payload_val)) = (envelope.get("nonce"), envelope.get("payload")) {
                            // This is directly encrypted (old format)
                            log::info!("libp2p: self-apply detected directly encrypted manifest - attempting decryption");
                            
                            // Extract nonce and payload
                            if let (Some(nonce_str), Some(payload_str)) = (nonce_val.as_str(), payload_val.as_str()) {
                                match decrypt_encrypted_manifest_with_id(&manifest_id, nonce_str, payload_str, &local_peer_id_copy).await {
                                    Ok(decrypted_yaml) => {
                                        log::info!("libp2p: self-apply successfully decrypted manifest");
                                        // Parse the decrypted YAML content
                                        match serde_yaml::from_str::<serde_json::Value>(&decrypted_yaml) {
                                            Ok(v) => {
                                                log::info!("libp2p: self-apply parsed decrypted YAML successfully");
                                                v
                                            }
                                            Err(_) => {
                                                log::info!("libp2p: self-apply treating decrypted content as raw");
                                                serde_json::json!({ "raw": decrypted_yaml })
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        log::warn!("libp2p: self-apply failed to decrypt manifest: {}", e);
                                        // Fallback to encrypted structure for debugging
                                        envelope
                                    }
                                }
                            } else {
                                log::warn!("libp2p: self-apply encrypted manifest has invalid nonce/payload format");
                                envelope
                            }
                        } else {
                            // Not encrypted, try to parse as YAML
                            match serde_yaml::from_str::<serde_json::Value>(manifest_json) {
                                Ok(v) => {
                                    log::info!("libp2p: self-apply parsed YAML manifest successfully");
                                    v
                                }
                                Err(_) => {
                                    log::info!("libp2p: self-apply treating as raw content");
                                    serde_json::json!({ "raw": manifest_json })
                                }
                            }
                        }
                    } else {
                        // If not valid JSON, treat as raw YAML
                        match serde_yaml::from_str::<serde_json::Value>(manifest_json) {
                            Ok(v) => {
                                log::info!("libp2p: self-apply parsed YAML manifest successfully");
                                v
                            }
                            Err(_) => {
                                log::info!("libp2p: self-apply treating as raw content");
                                serde_json::json!({ "raw": manifest_json })
                            }
                        }
                    };
                    
                    log::info!("libp2p: self-apply storing decrypted manifest for testing");
                    let _ = crate::restapi::store_decrypted_manifest(&manifest_id, manifest_value).await;
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
    
    // Retry up to 3 times with delays to allow provider announcements to propagate
    for attempt in 1..=3 {
        let (tx, mut rx) = mpsc::unbounded_channel();
        
        // Send control message to find manifest holders
        let control_msg = crate::libp2p_beemesh::control::Libp2pControl::FindManifestHolders {
            manifest_id: manifest_id.to_string(),
            reply_tx: tx,
        };
        
        // Get the control sender from the global context
        if let Some(control_tx) = crate::libp2p_beemesh::get_control_sender() {
            if let Err(e) = control_tx.send(control_msg) {
                return Err(anyhow::anyhow!("failed to send FindManifestHolders control message: {}", e));
            }
        } else {
            return Err(anyhow::anyhow!("control sender not available"));
        }
        
        // Wait for response with timeout
        match tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await {
            Ok(Some(peer_ids)) => {
                if !peer_ids.is_empty() {
                    log::info!("libp2p: find_manifest_holders found {} holders for manifest_id={} (attempt {})", peer_ids.len(), manifest_id, attempt);
                    return Ok(peer_ids);
                } else {
                    log::info!("libp2p: find_manifest_holders attempt {} returned empty result for manifest_id={}", attempt, manifest_id);
                }
            }
            Ok(None) => {
                log::warn!("libp2p: find_manifest_holders channel closed for manifest_id={} (attempt {})", manifest_id, attempt);
            }
            Err(_) => {
                log::info!("libp2p: find_manifest_holders timeout for manifest_id={} (attempt {})", manifest_id, attempt);
            }
        }
        
        // If not the last attempt, wait before retrying to allow provider announcements to propagate
        if attempt < 3 {
            log::info!("libp2p: find_manifest_holders waiting 1s before retry {} for manifest_id={}", attempt + 1, manifest_id);
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }
    
    // All attempts failed
    log::warn!("libp2p: find_manifest_holders exhausted all attempts for manifest_id={}", manifest_id);
    Ok(vec![])
}

/// Get list of currently connected peers as fallback when DHT provider discovery fails
async fn get_connected_peers() -> Vec<libp2p::PeerId> {
    use tokio::sync::mpsc;
    
    let (tx, mut rx) = mpsc::unbounded_channel();
    
    // Send control message to get connected peers
    let control_msg = crate::libp2p_beemesh::control::Libp2pControl::GetConnectedPeers {
        reply_tx: tx,
    };
    
    // Get the control sender from the global context
    if let Some(control_tx) = crate::libp2p_beemesh::get_control_sender() {
        if let Err(e) = control_tx.send(control_msg) {
            log::warn!("libp2p: failed to send GetConnectedPeers control message: {}", e);
            return vec![];
        }
    } else {
        log::warn!("libp2p: control sender not available");
        return vec![];
    }
    
    // Wait for response with timeout
    match tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv()).await {
        Ok(Some(peer_ids)) => {
            log::info!("libp2p: get_connected_peers found {} connected peers", peer_ids.len());
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
async fn fetch_keyshare_from_peer(peer_id: &libp2p::PeerId, request_fb: Vec<u8>) -> Result<Vec<u8>, anyhow::Error> {
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
            return Err(anyhow::anyhow!("failed to send FetchKeyshare control message: {}", e));
        }
    } else {
        return Err(anyhow::anyhow!("control sender not available"));
    }
    
    // Wait for response with timeout
    match tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv()).await {
        Ok(Some(Ok(response_bytes))) => {
            log::info!("libp2p: fetch_keyshare_from_peer successfully received response from peer={} (len={})", peer_id, response_bytes.len());
            Ok(response_bytes)
        }
        Ok(Some(Err(e))) => {
            log::warn!("libp2p: fetch_keyshare_from_peer error from peer={}: {}", peer_id, e);
            Err(anyhow::anyhow!("keyshare fetch error: {}", e))
        }
        Ok(None) => {
            log::warn!("libp2p: fetch_keyshare_from_peer channel closed for peer={}", peer_id);
            Err(anyhow::anyhow!("keyshare fetch channel closed"))
        }
        Err(_) => {
            log::warn!("libp2p: fetch_keyshare_from_peer timeout for peer={}", peer_id);
            Err(anyhow::anyhow!("keyshare fetch timeout"))
        }
    }
}

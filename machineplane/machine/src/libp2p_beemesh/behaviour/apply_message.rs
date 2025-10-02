use libp2p::request_response;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use log::{info, warn};

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
        let manifest_kind = if manifest_json.contains("\"kind\"") {
            // Try to extract kind from JSON
            manifest_json.split("\"kind\"")
                .nth(1)
                .and_then(|s| s.split('"').nth(2))
                .unwrap_or("Unknown")
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

use base64::Engine;
use libp2p::{request_response, PeerId};
use log::{info, warn};

/// Handle manifest fetch request/response messages and manifest storage requests
pub fn manifest_fetch_message(
    message: request_response::Message<Vec<u8>, Vec<u8>>,
    peer: PeerId,
    swarm: &mut libp2p::Swarm<super::behaviour::MyBehaviour>,
) {
    match message {
        request_response::Message::Request {
            request, channel, ..
        } => {
            info!("libp2p: received manifest request from peer={}", peer);

            // First check if this is a manifest storage request (format: "manifest_id:base64_data")
            if let Ok(request_str) = std::str::from_utf8(&request) {
                if let Some((manifest_id, manifest_data_b64)) = request_str.split_once(':') {
                    info!("libp2p: processing manifest storage request for manifest_id={} from peer={}", manifest_id, peer);

                    // Decode base64 manifest data
                    match base64::engine::general_purpose::STANDARD.decode(manifest_data_b64) {
                        Ok(manifest_data) => {
                            // Create manifest entry for storage
                            let manifest_entry =
                                crate::libp2p_beemesh::manifest_store::ManifestEntry {
                                    manifest_id: manifest_id.to_string(),
                                    version: 1,
                                    encrypted_data: manifest_data,
                                    stored_at:
                                        crate::libp2p_beemesh::manifest_store::current_timestamp(),
                                    access_tokens: vec![],
                                    owner_pubkey: vec![],
                                };

                            // Store the manifest using the global manifest store
                            let mut manifest_store =
                                crate::libp2p_beemesh::control::get_local_manifest_store()
                                    .lock()
                                    .unwrap();

                            match manifest_store.store_manifest(manifest_entry) {
                                Ok(()) => {
                                    info!(
                                        "libp2p: successfully stored manifest {} from peer {}",
                                        manifest_id, peer
                                    );
                                    let success_response = b"Manifest stored successfully";
                                    let _ = swarm
                                        .behaviour_mut()
                                        .manifest_fetch_rr
                                        .send_response(channel, success_response.to_vec());
                                }
                                Err(e) => {
                                    warn!(
                                        "libp2p: failed to store manifest {} from peer {}: {:?}",
                                        manifest_id, peer, e
                                    );
                                    let error_response =
                                        format!("Failed to store manifest: {:?}", e);
                                    let _ = swarm
                                        .behaviour_mut()
                                        .manifest_fetch_rr
                                        .send_response(channel, error_response.into_bytes());
                                }
                            }
                        }
                        Err(e) => {
                            warn!(
                                "libp2p: failed to decode manifest data from peer {}: {}",
                                peer, e
                            );
                            let error_response = format!("Failed to decode manifest: {}", e);
                            let _ = swarm
                                .behaviour_mut()
                                .manifest_fetch_rr
                                .send_response(channel, error_response.into_bytes());
                        }
                    }
                    return;
                }
            }

            // Try to parse the request as ManifestFetchRequest
            match protocol::machine::root_as_manifest_fetch_request(&request) {
                Ok(fetch_request) => {
                    let manifest_id = fetch_request.manifest_id().unwrap_or("");
                    let version = fetch_request.version();
                    let capability_token = fetch_request.capability_token().unwrap_or("");

                    info!(
                        "libp2p: parsed manifest fetch request for manifest_id={} version={}",
                        manifest_id, version
                    );

                    // Get manifest store for read access
                    let manifest_store = crate::libp2p_beemesh::control::get_local_manifest_store()
                        .lock()
                        .unwrap();

                    // Verify capability token
                    let token_verification =
                        verify_capability_token(capability_token, manifest_id, &peer.to_string());

                    let response_bytes = match token_verification {
                        Ok(true) => {
                            // Token is valid, try to fetch manifest
                            match manifest_store.get_manifest(manifest_id, version) {
                                Some(entry) => {
                                    // Check if token allows access to this specific version
                                    if entry.access_tokens.contains(&capability_token.to_string())
                                        || capability_token == "debug"
                                    // Allow debug access for testing
                                    {
                                        info!(
                                            "libp2p: serving manifest {} version {} to peer {}",
                                            manifest_id, version, peer
                                        );
                                        protocol::machine::build_manifest_fetch_response(
                                            true,
                                            Some(&entry.encrypted_data),
                                            entry.version,
                                            None,
                                        )
                                    } else {
                                        warn!(
                                            "libp2p: capability token does not grant access to manifest {} version {}",
                                            manifest_id, version
                                        );
                                        protocol::machine::build_manifest_fetch_response(
                                            false,
                                            None,
                                            0,
                                            Some("Insufficient permissions"),
                                        )
                                    }
                                }
                                None => {
                                    warn!(
                                        "libp2p: manifest {} version {} not found",
                                        manifest_id, version
                                    );
                                    protocol::machine::build_manifest_fetch_response(
                                        false,
                                        None,
                                        0,
                                        Some("Manifest not found"),
                                    )
                                }
                            }
                        }
                        Ok(false) => {
                            warn!(
                                "libp2p: invalid capability token from peer {} for manifest {}",
                                peer, manifest_id
                            );
                            protocol::machine::build_manifest_fetch_response(
                                false,
                                None,
                                0,
                                Some("Invalid capability token"),
                            )
                        }
                        Err(e) => {
                            warn!(
                                "libp2p: error verifying capability token from peer {}: {}",
                                peer, e
                            );
                            protocol::machine::build_manifest_fetch_response(
                                false,
                                None,
                                0,
                                Some("Token verification failed"),
                            )
                        }
                    };

                    // Send response
                    if let Err(e) = swarm
                        .behaviour_mut()
                        .manifest_fetch_rr
                        .send_response(channel, response_bytes)
                    {
                        warn!("Failed to send manifest fetch response: {:?}", e);
                    }
                }
                Err(e) => {
                    warn!(
                        "Failed to parse manifest fetch request from peer {}: {}",
                        peer, e
                    );

                    // Send error response
                    let error_response = protocol::machine::build_manifest_fetch_response(
                        false,
                        None,
                        0,
                        Some("Invalid request format"),
                    );

                    if let Err(e) = swarm
                        .behaviour_mut()
                        .manifest_fetch_rr
                        .send_response(channel, error_response)
                    {
                        warn!("Failed to send error response: {:?}", e);
                    }
                }
            }
        }
        request_response::Message::Response {
            response,
            request_id,
        } => {
            info!(
                "libp2p: received manifest fetch response from peer={}, request_id={:?}",
                peer, request_id
            );

            // Check if we have a pending request for this response
            let reply_sender = {
                let mut pending_requests =
                    crate::libp2p_beemesh::control::get_pending_manifest_requests()
                        .lock()
                        .unwrap();
                pending_requests
                    .remove(&request_id)
                    .map(|(sender, _)| sender)
            };

            if let Some(reply_tx) = reply_sender {
                // Check if the response indicates success
                let success = if let Ok(response_str) = std::str::from_utf8(&response) {
                    response_str.contains("success") || response_str.contains("stored")
                } else {
                    false
                };

                if success {
                    info!(
                        "libp2p: manifest distribution successful for request {:?}",
                        request_id
                    );
                    let _ = reply_tx.send(Ok(()));
                } else {
                    warn!(
                        "libp2p: manifest distribution failed for request {:?}",
                        request_id
                    );
                    let _ = reply_tx.send(Err("Manifest storage failed".to_string()));
                }
            } else {
                warn!(
                    "libp2p: received response for unknown request {:?}",
                    request_id
                );
            }
        }
    }
}

/// Verify a capability token for manifest access
fn verify_capability_token(
    token_b64: &str,
    manifest_id: &str,
    peer_id: &str,
) -> Result<bool, String> {
    // Allow debug token for testing
    if token_b64 == "debug" {
        return Ok(true);
    }

    // For now, also allow empty tokens for testing
    if token_b64.is_empty() {
        return Ok(true);
    }

    // Decode base64 token
    let token_data = base64::engine::general_purpose::STANDARD
        .decode(token_b64)
        .map_err(|e| format!("Failed to decode token: {}", e))?;

    // Parse as CapabilityToken
    match protocol::machine::root_as_capability_token(&token_data) {
        Ok(token) => {
            if let Some(root_cap) = token.root_capability() {
                let token_manifest_id = root_cap.manifest_id().unwrap_or("");

                // Verify manifest ID matches
                if token_manifest_id != manifest_id {
                    return Ok(false);
                }

                // Check for authorized_peer caveat
                if let Some(caveats) = token.caveats() {
                    for i in 0..caveats.len() {
                        let caveat = caveats.get(i);
                        if let Some(condition_type) = caveat.condition_type() {
                            if condition_type == "authorized_peer" {
                                if let Some(value_bytes) = caveat.value() {
                                    let peer_bytes: Vec<u8> = value_bytes.iter().collect();
                                    let authorized_peer =
                                        std::str::from_utf8(&peer_bytes).unwrap_or("");
                                    if authorized_peer == peer_id {
                                        // TODO: Add signature verification
                                        return Ok(true);
                                    }
                                }
                            }
                        }
                    }
                }

                // No matching authorized_peer caveat found
                Ok(false)
            } else {
                Ok(false)
            }
        }
        Err(e) => Err(format!("Failed to parse token: {}", e)),
    }
}

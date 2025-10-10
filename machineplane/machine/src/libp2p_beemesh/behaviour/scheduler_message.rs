use base64::Engine;
use libp2p::request_response;
use log::{info, warn};
use std::collections::HashMap as StdHashMap;
use tokio::sync::mpsc;

pub fn scheduler_message(
    message: request_response::Message<Vec<u8>, Vec<u8>>,
    peer: libp2p::PeerId,
    swarm: &mut libp2p::Swarm<super::MyBehaviour>,
    local_peer: libp2p::PeerId,
    pending_queries: &mut StdHashMap<String, Vec<mpsc::UnboundedSender<String>>>,
) {
    match message {
        request_response::Message::Request {
            request, channel, ..
        } => {
            info!("libp2p: received scheduler request from peer={}", peer);
            // First, attempt to verify request as an Envelope (JSON or FlatBuffer)
            let effective_request =
                match crate::libp2p_beemesh::security::verify_envelope_and_check_nonce(&request) {
                    Ok((payload_bytes, _pub, _sig)) => payload_bytes,
                    Err(e) => {
                        if crate::libp2p_beemesh::security::require_signed_messages() {
                            warn!("rejecting unsigned/invalid scheduler request: {:?}", e);
                            return;
                        }
                        request.clone()
                    }
                };

            // Try parse CapacityRequest
            match protocol::machine::root_as_capacity_request(&effective_request) {
                Ok(cap_req) => {
                    let orig_request_id = cap_req.request_id().unwrap_or("");
                    info!(
                        "libp2p: scheduler capacity request id={} from {}",
                        orig_request_id, peer
                    );

                    // Dummy capacity check - always true for now
                    let has_capacity = true;

                    // build CapacityReply using protocol helper and include local KEM pubkey if available
                    let kem_b64 = match crypto::ensure_kem_keypair_on_disk() {
                        Ok((pubb, _priv)) => {
                            Some(base64::engine::general_purpose::STANDARD.encode(&pubb))
                        }
                        Err(_) => None,
                    };
                    let finished = protocol::machine::build_capacity_reply(
                        has_capacity,
                        1000u32,
                        1024u64 * 1024 * 512,
                        1024u64 * 1024 * 1024,
                        orig_request_id,
                        &local_peer.to_string(),
                        "local",
                        kem_b64.as_deref(),
                        &["default"],
                    );

                    // Send response via request-response
                    let _ = swarm
                        .behaviour_mut()
                        .scheduler_rr
                        .send_response(channel, finished);
                    info!(
                        "libp2p: sent scheduler capacity reply for id={} to {}",
                        orig_request_id, peer
                    );
                }
                Err(e) => {
                    warn!("libp2p: failed to parse scheduler request: {:?}", e);
                }
            }
        }
        request_response::Message::Response { response, .. } => {
            info!("libp2p: received scheduler response from peer={}", peer);
            if let Ok(cap_reply) = protocol::machine::root_as_capacity_reply(&response) {
                let request_part = cap_reply.request_id().unwrap_or("").to_string();
                info!(
                    "libp2p: scheduler reply ok={} from {} for request_id={}",
                    cap_reply.ok(),
                    peer,
                    request_part
                );
                // If the reply contains a kem_pubkey, decode it and store in global cache
                if let Some(kem_b64) = cap_reply.kem_pubkey() {
                    match base64::engine::general_purpose::STANDARD.decode(kem_b64) {
                        Ok(kem_bytes) => {
                            let mut map = crate::libp2p_beemesh::PEER_KEM_PUBKEYS.write().unwrap();
                            map.insert(peer.clone(), kem_bytes);
                        }
                        Err(e) => {
                            warn!("failed to decode kem_pubkey from {}: {:?}", peer, e);
                        }
                    }
                }
                if let Some(senders) = pending_queries.get_mut(&request_part) {
                    for tx in senders.iter() {
                        let _ = tx.send(peer.to_string());
                    }
                }
            }
        }
    }
}

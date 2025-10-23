use super::message_verifier::verify_signed_message;
use crate::libp2p_beemesh::reply::{build_capacity_reply_with, warn_missing_kem, CapacityReply};
use libp2p::request_response;
use log::{debug, error, warn};
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
            debug!("libp2p: received scheduler request from peer={}", peer);
            // First, attempt to verify request as an Envelope (JSON or FlatBuffer)
            let verified = match verify_signed_message(&peer, &request, |err| {
                error!("rejecting invalid scheduler request: {}", err);
            }) {
                Some(envelope) => envelope,
                None => return,
            };
            let effective_request = verified.payload;

            // Try parse CapacityRequest
            match protocol::machine::root_as_capacity_request(&effective_request) {
                Ok(cap_req) => {
                    let orig_request_id = cap_req.request_id().unwrap_or("");
                    debug!(
                        "libp2p: scheduler capacity request id={} from {}",
                        orig_request_id, peer
                    );

                    // Dummy capacity check - always true for now
                    let has_capacity = true;

                    // build CapacityReply using protocol helper and include local KEM pubkey if available
                    let responder_peer = local_peer.to_string();
                    let reply =
                        build_capacity_reply_with(orig_request_id, &responder_peer, |params| {
                            params.ok = has_capacity;
                        });
                    let CapacityReply {
                        payload,
                        kem_pub_b64,
                    } = reply;
                    warn_missing_kem("scheduler", &responder_peer, kem_pub_b64.as_deref());

                    // Send response via request-response
                    let _ = swarm
                        .behaviour_mut()
                        .scheduler_rr
                        .send_response(channel, payload);
                    debug!(
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
            debug!("libp2p: received scheduler response from peer={}", peer);
            if let Ok(cap_reply) = protocol::machine::root_as_capacity_reply(&response) {
                let request_part = cap_reply.request_id().unwrap_or("").to_string();
                debug!(
                    "libp2p: scheduler reply ok={} from {} for request_id={}",
                    cap_reply.ok(),
                    peer,
                    request_part
                );
                // KEM pubkey caching has been removed - keys are now extracted directly from envelopes
                if let Some(senders) = pending_queries.get_mut(&request_part) {
                    // Extract public key from the capacity reply
                    let peer_pubkey = cap_reply.kem_pubkey().unwrap_or("").to_string();
                    let peer_with_key = format!("{}:{}", peer.to_string(), peer_pubkey);
                    for tx in senders.iter() {
                        let _ = tx.send(peer_with_key.clone());
                    }
                }
            }
        }
    }
}

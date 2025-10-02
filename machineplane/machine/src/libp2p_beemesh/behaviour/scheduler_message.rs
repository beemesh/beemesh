use log::{info, warn};
use libp2p::request_response;
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
        request_response::Message::Request { request, channel, .. } => {
            info!("libp2p: received scheduler request from peer={}", peer);
            // First, attempt to verify request as an Envelope (JSON or FlatBuffer)
            let effective_request = match serde_json::from_slice::<serde_json::Value>(&request) {
                Ok(val) => {
                    match crate::libp2p_beemesh::security::verify_envelope_and_check_nonce(&val) {
                        Ok((payload_bytes, _pub, _sig)) => payload_bytes,
                        Err(e) => {
                            if crate::libp2p_beemesh::security::require_signed_messages() {
                                warn!("rejecting unsigned/invalid scheduler request: {:?}", e);
                                return;
                            }
                            request.clone()
                        }
                    }
                }
                Err(_) => request.clone(),
            };

            // Try parse CapacityRequest
            match protocol::machine::root_as_capacity_request(&effective_request) {
                Ok(cap_req) => {
                    let orig_request_id = cap_req.request_id().unwrap_or("");
                    info!("libp2p: scheduler capacity request id={} from {}", orig_request_id, peer);

                    // Dummy capacity check - always true for now
                    let has_capacity = true;

                    // build CapacityReply
                    let mut fbb = flatbuffers::FlatBufferBuilder::new();
                    let req_id_off = fbb.create_string(orig_request_id);
                    let node_id_off = fbb.create_string(&local_peer.to_string());
                    let region_off = fbb.create_string("local");
                    let caps_vec = {
                        let mut tmp: Vec<flatbuffers::WIPOffset<&str>> = Vec::new();
                        tmp.push(fbb.create_string("default"));
                        fbb.create_vector(&tmp)
                    };
                    let reply_args = protocol::machine::CapacityReplyArgs {
                        request_id: Some(req_id_off),
                        ok: has_capacity,
                        node_id: Some(node_id_off),
                        region: Some(region_off),
                        capabilities: Some(caps_vec),
                        cpu_available_milli: 1000u32,
                        memory_available_bytes: 1024u64 * 1024 * 512,
                        storage_available_bytes: 1024u64 * 1024 * 1024,
                    };
                    let reply_off = protocol::machine::CapacityReply::create(&mut fbb, &reply_args);
                    protocol::machine::finish_capacity_reply_buffer(&mut fbb, reply_off);
                    let finished = fbb.finished_data().to_vec();

                    // Send response via request-response
                    let _ = swarm.behaviour_mut().scheduler_rr.send_response(channel, finished);
                    info!("libp2p: sent scheduler capacity reply for id={} to {}", orig_request_id, peer);
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
                info!("libp2p: scheduler reply ok={} from {} for request_id={}",
                    cap_reply.ok(), peer, request_part);
                if let Some(senders) = pending_queries.get_mut(&request_part) {
                    for tx in senders.iter() {
                        let _ = tx.send(peer.to_string());
                    }
                }
            }
        }
    }
}

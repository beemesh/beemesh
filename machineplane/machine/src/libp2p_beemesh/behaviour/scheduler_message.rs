use super::message_verifier::verify_signed_message;
use crate::libp2p_beemesh::capacity;
use crate::libp2p_beemesh::utils;
use crate::resource_verifier::ResourceRequest;
use crate::workload_integration::get_global_resource_verifier;
use libp2p::request_response;
use log::{debug, error, info, warn};
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
            if crate::libp2p_beemesh::is_scheduling_disabled_for(&local_peer) {
                debug!(
                    "libp2p: scheduling disabled, ignoring scheduler request from {}",
                    peer
                );
                return;
            }
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

                    // Perform real capacity check using resource verifier
                    let resource_request = ResourceRequest::new(
                        Some(cap_req.cpu_milli()),
                        Some(cap_req.memory_bytes()),
                        Some(cap_req.storage_bytes()),
                        cap_req.replicas(),
                    );

                    let verifier = get_global_resource_verifier();
                    let responder_peer = local_peer.to_string();

                    // Perform synchronous capacity check using cached resources
                    // This is called from within the libp2p event loop, so we can't use async
                    let check_result = {
                        let handle = tokio::runtime::Handle::current();
                        // Spawn a blocking task to avoid nesting runtimes
                        std::thread::spawn(move || {
                            handle.block_on(verifier.verify_capacity(&resource_request))
                        })
                        .join()
                        .unwrap_or_else(|_| {
                            warn!("Capacity check thread panicked, assuming no capacity");
                            crate::resource_verifier::CapacityCheckResult {
                                has_capacity: false,
                                rejection_reason: Some("Internal error".to_string()),
                                available_cpu_milli: 0,
                                available_memory_bytes: 0,
                                available_storage_bytes: 0,
                            }
                        })
                    };

                    let has_capacity = check_result.has_capacity;

                    if has_capacity {
                        info!(
                            "Capacity check passed for request_id={}: CPU={}m, Mem={} MB, Storage={} GB available",
                            orig_request_id,
                            check_result.available_cpu_milli,
                            check_result.available_memory_bytes / (1024 * 1024),
                            check_result.available_storage_bytes / (1024 * 1024 * 1024)
                        );

                        // build CapacityReply using protocol helper and include local KEM pubkey if available
                        let reply = capacity::compose_capacity_reply(
                            "scheduler",
                            &orig_request_id,
                            &responder_peer,
                            |params| {
                                params.ok = true;
                                params.cpu_milli = check_result.available_cpu_milli;
                                params.memory_bytes = check_result.available_memory_bytes;
                                params.storage_bytes = check_result.available_storage_bytes;
                            },
                        );
                        let payload_len = reply.payload.len();

                        match capacity::send_scheduler_capacity_reply(
                            &mut swarm.behaviour_mut().scheduler_rr,
                            channel,
                            reply,
                        ) {
                            Ok(_) => {
                                debug!(
                                    "libp2p: sent scheduler capacity reply for id={} to {} ({} bytes)",
                                    orig_request_id, peer, payload_len
                                );
                            }
                            Err(e) => {
                                error!(
                                    "libp2p: failed to send scheduler capacity reply for id={} to {}: {:?}",
                                    orig_request_id, peer, e
                                );
                            }
                        }
                    } else {
                        info!(
                            "Capacity check failed for request_id={}: {} - not sending response",
                            orig_request_id,
                            check_result
                                .rejection_reason
                                .unwrap_or_else(|| "Unknown reason".to_string())
                        );
                        // Do not send a response when capacity is unavailable
                    }
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
                let peer_pubkey = cap_reply.kem_pubkey().unwrap_or("").to_string();
                let peer_with_key = format!("{}:{}", peer.to_string(), peer_pubkey);
                utils::notify_capacity_observers(pending_queries, &request_part, move || {
                    peer_with_key.clone()
                });
            }
        }
    }
}

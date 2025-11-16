use libp2p::{Swarm, gossipsub};

use std::collections::HashMap as StdHashMap;
use tokio::sync::mpsc;

use crate::libp2p_beemesh::behaviour::MyBehaviour;
use crate::libp2p_beemesh::capacity;
use crate::libp2p_beemesh::utils;

/// Handle QueryCapacityWithPayload control message
pub async fn handle_query_capacity_with_payload(
    request_id: String,
    reply_tx: mpsc::UnboundedSender<String>,
    payload: Vec<u8>,
    swarm: &mut Swarm<MyBehaviour>,
    _topic: &gossipsub::IdentTopic,
    pending_queries: &mut StdHashMap<String, Vec<mpsc::UnboundedSender<String>>>,
) {
    // register the reply channel so incoming reply messages can be forwarded
    log::debug!(
        "libp2p: control QueryCapacityWithPayload received request_id={} payload_len={}",
        request_id,
        payload.len()
    );
    pending_queries
        .entry(request_id.clone())
        .or_insert_with(Vec::new)
        .push(reply_tx);

    // Parse the provided payload as a CapacityRequest FlatBuffer and rebuild it into a TopicMessage wrapper
    match crate::protocol::machine::root_as_capacity_request(&payload) {
        Ok(cap_req) => {
            // Rebuild a CapacityRequest flatbuffer embedding the request_id.
            let mut fbb = flatbuffers::FlatBufferBuilder::new();
            let req_id_off = fbb.create_string(&request_id);
            let cpu = cap_req.cpu_milli();
            let mem = cap_req.memory_bytes();
            let stor = cap_req.storage_bytes();
            let reps = cap_req.replicas();
            let cap_args = crate::protocol::machine::CapacityRequestArgs {
                request_id: Some(req_id_off),
                cpu_milli: cpu,
                memory_bytes: mem,
                storage_bytes: stor,
                replicas: reps,
            };
            let cap_off = crate::protocol::machine::CapacityRequest::create(&mut fbb, &cap_args);
            crate::protocol::machine::finish_capacity_request_buffer(&mut fbb, cap_off);
            let finished = fbb.finished_data().to_vec();
            // Broadcast signed scheduler requests to peers (centralized helper)
            match utils::broadcast_signed_request_to_peers(swarm, &finished, "capacity") {
                Ok(sent) => {
                    log::info!(
                        "libp2p: broadcasted capreq request_id={} to {} peers",
                        request_id,
                        sent
                    );
                }
                Err(e) => {
                    log::error!("failed to broadcast capacity request: {:?}", e);
                    return;
                }
            }

            // Also notify local pending senders directly so the originator is always considered
            // a potential responder. This ensures single-node operation and makes the
            // origining host countable when collecting responders.
            // When scheduling is disabled, skip the local response to honour the configuration.
            // Include the local KEM public key in the same format as remote responses.
            if crate::libp2p_beemesh::is_scheduling_disabled_for(swarm.local_peer_id()) {
                log::info!(
                    "libp2p: local scheduling disabled, skipping local capacity response for {}",
                    request_id
                );
            } else {
                let responder_peer = swarm.local_peer_id().to_string();
                let reply = capacity::compose_capacity_reply(
                    "local",
                    &request_id,
                    &responder_peer,
                    |params| {
                        params.cpu_milli = cap_req.cpu_milli();
                        params.memory_bytes = cap_req.memory_bytes();
                        params.storage_bytes = cap_req.storage_bytes();
                    },
                );
                let local_response = format!(
                    "{}:{}",
                    swarm.local_peer_id(),
                    reply.kem_pub_b64.unwrap_or_default()
                );
                utils::notify_capacity_observers(pending_queries, &request_id, move || {
                    local_response.clone()
                });
            }
        }
        Err(e) => {
            log::error!("libp2p: failed to parse provided capacity payload: {:?}", e);
        }
    }
}

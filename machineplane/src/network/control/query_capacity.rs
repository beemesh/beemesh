use libp2p::{Swarm, gossipsub};

use std::collections::HashMap as StdHashMap;
use tokio::sync::mpsc;

use crate::network::behaviour::MyBehaviour;
use crate::network::capacity;
use crate::network::utils;

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

    // Parse the provided payload and rebuild it into a TopicMessage wrapper
    match crate::messages::machine::root_as_capacity_request(&payload) {
        Ok(cap_req) => {
            let finished = crate::messages::machine::build_capacity_request_with_id(
                &request_id,
                cap_req.cpu_milli,
                cap_req.memory_bytes,
                cap_req.storage_bytes,
                cap_req.replicas,
            );
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
            if crate::network::is_scheduling_disabled_for(swarm.local_peer_id()) {
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
                        params.cpu_milli = cap_req.cpu_milli;
                        params.memory_bytes = cap_req.memory_bytes;
                        params.storage_bytes = cap_req.storage_bytes;
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

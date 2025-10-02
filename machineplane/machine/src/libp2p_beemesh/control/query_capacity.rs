use log::{debug, info, error};
use libp2p::{gossipsub, Swarm};
use std::collections::HashMap as StdHashMap;
use tokio::sync::mpsc;

use crate::libp2p_beemesh::behaviour::MyBehaviour;

/// Handle QueryCapacityWithPayload control message
pub async fn handle_query_capacity_with_payload(
    request_id: String,
    reply_tx: mpsc::UnboundedSender<String>,
    payload: Vec<u8>,
    swarm: &mut Swarm<MyBehaviour>,
    topic: &gossipsub::IdentTopic,
    pending_queries: &mut StdHashMap<String, Vec<mpsc::UnboundedSender<String>>>,
) {
    // register the reply channel so incoming reply messages can be forwarded
    log::debug!("libp2p: control QueryCapacityWithPayload received request_id={} payload_len={}", request_id, payload.len());
    pending_queries.entry(request_id.clone()).or_insert_with(Vec::new).push(reply_tx);

    // Parse the provided payload as a CapacityRequest FlatBuffer and rebuild it into a TopicMessage wrapper
    match protocol::machine::root_as_capacity_request(&payload) {
        Ok(cap_req) => {
            // Rebuild a CapacityRequest flatbuffer embedding the request_id.
            let mut fbb = flatbuffers::FlatBufferBuilder::new();
            let req_id_off = fbb.create_string(&request_id);
            let cpu = cap_req.cpu_milli();
            let mem = cap_req.memory_bytes();
            let stor = cap_req.storage_bytes();
            let reps = cap_req.replicas();
            let cap_args = protocol::machine::CapacityRequestArgs {
                request_id: Some(req_id_off),
                cpu_milli: cpu,
                memory_bytes: mem,
                storage_bytes: stor,
                replicas: reps,
            };
            let cap_off = protocol::machine::CapacityRequest::create(&mut fbb, &cap_args);
            protocol::machine::finish_capacity_request_buffer(&mut fbb, cap_off);
            let finished = fbb.finished_data().to_vec();
            // Send the capacity request as a request-response to all known peers.
            // Use the scheduler request-response behaviour we wired as `/beemesh/scheduler-tasks/1.0.0`.
            let mut sent = 0usize;
            // Collect peers first to avoid simultaneous immutable + mutable borrows of swarm
            let peers: Vec<libp2p::PeerId> = swarm.behaviour().gossipsub.all_peers().map(|(p, _)| p.clone()).collect();
            for peer in peers.iter() {
                let req_clone = finished.clone();
                let req_id = swarm.behaviour_mut().scheduler_rr.send_request(&peer, req_clone);
                log::debug!("libp2p: sent scheduler request to peer={} request_id={:?}", peer, req_id);
                sent += 1;
            }
            log::info!("libp2p: broadcasted capreq request_id={} to {} peers", request_id, sent);

            // Also notify local pending senders directly so the originator is always considered
            // a potential responder. This ensures single-node operation and makes the
            // origining host countable when collecting responders.
            if let Some(senders) = pending_queries.get_mut(&request_id) {
                for tx in senders.iter() {
                    let _ = tx.send(swarm.local_peer_id().to_string());
                }
            }
        }
        Err(e) => {
            log::error!("libp2p: failed to parse provided capacity payload: {:?}", e);
        }
    }
}

/// Dummy capacity check that always returns true for now.
fn has_free_capacity_dummy() -> bool {
    // TODO: replace with real resource accounting checks
    true
}
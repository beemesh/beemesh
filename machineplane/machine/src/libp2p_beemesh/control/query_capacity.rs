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
            // Rebuild a CapacityRequest flatbuffer embedding the request_id, and publish it directly (no wrapper)
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
            let res = swarm.behaviour_mut().gossipsub.publish(topic.clone(), finished.as_slice());
            log::info!("libp2p: published capreq request_id={} publish_res={:?}", request_id, res);

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
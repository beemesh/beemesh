use base64::Engine;
use libp2p::{gossipsub, Swarm};

use std::collections::HashMap as StdHashMap;
use tokio::sync::mpsc;

use crate::libp2p_beemesh::behaviour::MyBehaviour;
use crate::libp2p_beemesh::envelope::{sign_with_existing_keypair, SignEnvelopeConfig};

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
            // Wrap the capacity request in a signed envelope before sending
            match crypto::ensure_keypair_on_disk() {
                Ok((pub_bytes, sk_bytes)) => {
                    let timestamp = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    let mut sent = 0usize;
                    let peers: Vec<libp2p::PeerId> = swarm
                        .behaviour()
                        .gossipsub
                        .all_peers()
                        .map(|(p, _)| p.clone())
                        .collect();
                    for peer in peers.iter() {
                        let nonce = format!("capacity_query_{}_{}", peer, rand::random::<u32>());
                        let sign_cfg = SignEnvelopeConfig {
                            nonce: Some(&nonce),
                            timestamp: Some(timestamp),
                            ..Default::default()
                        };

                        match sign_with_existing_keypair(
                            &finished, "capacity", sign_cfg, &pub_bytes, &sk_bytes,
                        ) {
                            Ok(signed) => {
                                let req_id = swarm
                                    .behaviour_mut()
                                    .scheduler_rr
                                    .send_request(&peer, signed.bytes);
                                log::debug!(
                                    "libp2p: sent scheduler request to peer={} request_id={:?} nonce={}",
                                    peer,
                                    req_id,
                                    nonce
                                );
                                sent += 1;
                            }
                            Err(e) => {
                                log::error!(
                                    "failed to sign capacity query envelope for peer {}: {:?}",
                                    peer,
                                    e
                                );
                            }
                        }
                    }

                    log::info!(
                        "libp2p: broadcasted capreq request_id={} to {} peers",
                        request_id,
                        sent
                    );
                }
                Err(e) => {
                    log::error!("failed to load keypair for capacity query: {:?}", e);
                    return;
                }
            }

            // Also notify local pending senders directly so the originator is always considered
            // a potential responder. This ensures single-node operation and makes the
            // origining host countable when collecting responders.
            // Include the local KEM public key in the same format as remote responses.
            if let Some(senders) = pending_queries.get_mut(&request_id) {
                let local_kem_b64 = match crypto::ensure_kem_keypair_on_disk() {
                    Ok((pubb, _priv)) => base64::engine::general_purpose::STANDARD.encode(&pubb),
                    Err(e) => {
                        log::warn!(
                            "libp2p: local capacity response failed to load KEM keypair: {:?}",
                            e
                        );
                        String::new()
                    }
                };
                let local_response = format!("{}:{}", swarm.local_peer_id(), local_kem_b64);
                for tx in senders.iter() {
                    let _ = tx.send(local_response.clone());
                }
            }
        }
        Err(e) => {
            log::error!("libp2p: failed to parse provided capacity payload: {:?}", e);
        }
    }
}

// Dummy capacity check that always returns true for now.
#[allow(dead_code)]
fn has_free_capacity_dummy() -> bool {
    // TODO: replace with real resource accounting checks
    true
}

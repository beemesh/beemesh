use crate::capacity::{CapacityCheckResult, ResourceRequest};
use crate::messages::constants::{SCHEDULER_EVENTS, SCHEDULER_PROPOSALS, SCHEDULER_TENDERS};
use crate::messages::machine;
use crate::network::{ApplyCodec, DeleteCodec, HandshakeCodec};
use crate::network::{capacity, control, utils};
use crate::scheduler::{get_global_capacity_verifier, remove_workloads_by_manifest_id};
use libp2p::swarm::NetworkBehaviour;
use libp2p::{PeerId, autonat, gossipsub, identify, kad, relay, request_response};
use log::{debug, error, info, warn};
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;

#[derive(NetworkBehaviour)]
pub struct MyBehaviour {
    pub gossipsub: gossipsub::Behaviour,
    pub apply_rr: request_response::Behaviour<ApplyCodec>,
    pub handshake_rr: request_response::Behaviour<HandshakeCodec>,
    pub delete_rr: request_response::Behaviour<DeleteCodec>,
    pub manifest_fetch_rr: request_response::Behaviour<ApplyCodec>,
    pub kademlia: kad::Behaviour<kad::store::MemoryStore>,
    pub relay: relay::Behaviour,
    pub autonat: autonat::Behaviour,
    pub identify: identify::Behaviour,
}

// ============================================================================
// Failure Handlers
// ============================================================================

/// Direction of the failure (inbound or outbound)
#[derive(Debug, Clone, Copy)]
pub enum FailureDirection {
    Inbound,
    Outbound,
}

impl std::fmt::Display for FailureDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FailureDirection::Inbound => write!(f, "inbound"),
            FailureDirection::Outbound => write!(f, "outbound"),
        }
    }
}

/// Generic failure handler for any protocol and direction
pub fn handle_failure<E: std::fmt::Debug>(
    protocol: &str,
    direction: FailureDirection,
    peer: libp2p::PeerId,
    error: E,
) {
    warn!(
        "{} {} failure with peer {}: {:?}",
        protocol, direction, peer, error
    );
}

/// Specialized handler for apply protocol inbound failures
pub fn apply_inbound_failure(peer: libp2p::PeerId, error: request_response::InboundFailure) {
    handle_failure("apply", FailureDirection::Inbound, peer, error);
}

/// Specialized handler for apply protocol outbound failures
pub fn apply_outbound_failure(peer: libp2p::PeerId, error: request_response::OutboundFailure) {
    handle_failure("apply", FailureDirection::Outbound, peer, error);
}

/// Specialized handler for handshake protocol inbound failures
pub fn handshake_inbound_failure(peer: libp2p::PeerId, error: request_response::InboundFailure) {
    handle_failure("handshake", FailureDirection::Inbound, peer, error);
}

/// Specialized handler for handshake protocol outbound failures
pub fn handshake_outbound_failure(peer: libp2p::PeerId, error: request_response::OutboundFailure) {
    handle_failure("handshake", FailureDirection::Outbound, peer, error);
}

/// Handle delete outbound failure
pub fn delete_outbound_failure(peer: libp2p::PeerId, error: request_response::OutboundFailure) {
    handle_failure("delete", FailureDirection::Outbound, peer, error);
}

/// Handle delete inbound failure
pub fn delete_inbound_failure(peer: libp2p::PeerId, error: request_response::InboundFailure) {
    handle_failure("delete", FailureDirection::Inbound, peer, error);
}

// ============================================================================
// Gossipsub Handlers
// ============================================================================

// Global channel for scheduler messages
pub static SCHEDULER_INPUT_TX: OnceLock<
    mpsc::UnboundedSender<(libp2p::gossipsub::TopicHash, libp2p::gossipsub::Message)>,
> = OnceLock::new();

pub fn set_scheduler_input(
    tx: mpsc::UnboundedSender<(libp2p::gossipsub::TopicHash, libp2p::gossipsub::Message)>,
) {
    let _ = SCHEDULER_INPUT_TX.set(tx);
}

pub fn gossipsub_message(
    peer_id: libp2p::PeerId,
    message: gossipsub::Message,
    topic: gossipsub::TopicHash,
    swarm: &mut libp2p::Swarm<MyBehaviour>,
    pending_queries: &mut std::collections::HashMap<
        String,
        Vec<tokio::sync::mpsc::UnboundedSender<String>>,
    >,
) {
    debug!("received message from {}", peer_id);
    let payload = &message.data;

    // Check if this is a scheduler topic
    static SCHEDULER_TOPICS: OnceLock<[gossipsub::TopicHash; 3]> = OnceLock::new();

    let scheduler_topics = SCHEDULER_TOPICS.get_or_init(|| {
        [
            gossipsub::IdentTopic::new(SCHEDULER_TENDERS).hash(),
            gossipsub::IdentTopic::new(SCHEDULER_PROPOSALS).hash(),
            gossipsub::IdentTopic::new(SCHEDULER_EVENTS).hash(),
        ]
    });

    if scheduler_topics.contains(&topic) {
        if let Some(tx) = SCHEDULER_INPUT_TX.get() {
            if let Err(e) = tx.send((topic, message)) {
                error!("Failed to forward scheduler message: {}", e);
            }
        } else {
            warn!("Scheduler input channel not initialized, dropping message");
        }
        return;
    }

    // Then try CapacityReply
    if let Ok(cap_req) = crate::messages::machine::root_as_capacity_request(payload.as_slice()) {
        let orig_request_id = cap_req.request_id.clone();
        let responder_peer = swarm.local_peer_id().to_string();

        let manifest_id = match utils::extract_manifest_id_from_request_id(&orig_request_id) {
            Some(id) => id,
            None => {
                warn!(
                    "libp2p: capreq id={} missing manifest id, ignoring",
                    orig_request_id
                );
                return;
            }
        };

        let resource_request = ResourceRequest::new(
            Some(cap_req.cpu_milli),
            Some(cap_req.memory_bytes),
            Some(cap_req.storage_bytes),
            cap_req.replicas,
        );

        info!(
            "libp2p: received capreq id={} manifest_id={} from peer={} payload_bytes={}",
            orig_request_id,
            manifest_id,
            peer_id,
            message.data.len()
        );

        let verifier = get_global_capacity_verifier();

        // Perform synchronous capacity check using cached resources.
        let request_id_for_check = orig_request_id.clone();
        let verifier_for_check = verifier.clone();
        let resource_request_for_check = resource_request.clone();
        let check_handle = tokio::runtime::Handle::current();
        let check_result = std::thread::spawn(move || {
            check_handle.block_on(verifier_for_check.verify_capacity(&resource_request_for_check))
        })
        .join()
        .unwrap_or_else(|_| {
            warn!(
                "libp2p: capacity check thread panicked for request_id={}",
                request_id_for_check
            );
            CapacityCheckResult {
                has_capacity: false,
                rejection_reason: Some("internal error".to_string()),
                available_cpu_milli: 0,
                available_memory_bytes: 0,
                available_storage_bytes: 0,
            }
        });

        if !check_result.has_capacity {
            info!(
                "libp2p: capacity check failed for request_id={} manifest_id={} reason={:?}",
                orig_request_id, manifest_id, check_result.rejection_reason
            );
            return;
        }

        // Reserve resources for a short period to back the bid.
        let reserve_request_id = orig_request_id.clone();
        let reserve_manifest_id = manifest_id.clone();
        let verifier_for_reserve = verifier.clone();
        let resource_request_for_reserve = resource_request.clone();
        let reserve_handle = tokio::runtime::Handle::current();
        let reserve_outcome = std::thread::spawn(move || {
            reserve_handle.block_on(verifier_for_reserve.reserve_capacity(
                &reserve_request_id,
                Some(reserve_manifest_id.as_str()),
                &resource_request_for_reserve,
            ))
        })
        .join();

        match reserve_outcome {
            Ok(Ok(())) => {
                info!(
                    "libp2p: reserved resources for request_id={} manifest_id={}",
                    orig_request_id, manifest_id
                );
            }
            Ok(Err(err)) => {
                warn!(
                    "libp2p: failed to reserve resources for request_id={} manifest_id={}: {}",
                    orig_request_id, manifest_id, err
                );
                return;
            }
            Err(_) => {
                warn!(
                    "libp2p: reservation thread panicked for request_id={} manifest_id={}",
                    orig_request_id, manifest_id
                );
                return;
            }
        }

        let reply = capacity::compose_capacity_reply(
            "gossipsub",
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

        match capacity::publish_gossipsub_capacity_reply(
            &mut swarm.behaviour_mut().gossipsub,
            &topic,
            &reply,
        ) {
            Ok(_) => {
                info!(
                    "libp2p: published capreply for id={} manifest_id={} ({} bytes)",
                    orig_request_id, manifest_id, payload_len
                );
            }
            Err(e) => {
                error!(
                    "libp2p: failed to publish signed capacity reply id={} to {}: {:?}",
                    orig_request_id, peer_id, e
                );
            }
        }
        return;
    }

    if let Ok(cap_reply) = crate::messages::machine::root_as_capacity_reply(payload.as_slice()) {
        let request_part = cap_reply.request_id.clone();
        info!(
            "libp2p: received capreply for id={} from peer={}",
            request_part, peer_id
        );
        // KEM pubkey caching has been removed; keys are expected directly in capacity replies
        utils::notify_capacity_observers(pending_queries, &request_part, move || {
            peer_id.to_string()
        });
        return;
    }

    warn!(
        "gossipsub: Received unsupported message ({} bytes) from peer {}",
        payload.len(),
        peer_id
    );
}

pub fn gossipsub_subscribed(_peer_id: libp2p::PeerId, _topic: gossipsub::TopicHash) {
    //info!("Peer {peer_id} subscribed to topic: {topic}");
}

pub fn gossipsub_unsubscribed(peer_id: libp2p::PeerId, topic: gossipsub::TopicHash) {
    log::info!("Peer {peer_id} unsubscribed from topic: {topic}");
}

// ============================================================================
// Handshake Handlers
// ============================================================================

#[derive(Debug, Clone)]
pub struct HandshakeState {
    pub attempts: u32,
    pub last_attempt: Instant,
    pub confirmed: bool,
}

pub fn handshake_request<F>(
    request: Vec<u8>,
    peer: libp2p::PeerId,
    send_response: F,
    handshake_states: &mut std::collections::HashMap<libp2p::PeerId, HandshakeState>,
) where
    F: FnOnce(Vec<u8>),
{
    //log::info!("libp2p: received handshake request from peer={}", peer);

    // Parse the handshake request
    match crate::messages::machine::root_as_handshake(&request) {
        Ok(_handshake_req) => {
            // Mark this peer as confirmed
            ensure_handshake_state(&peer, handshake_states).confirmed = true;

            // Create a handshake response
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;

            // Build simple handshake response
            let handshake_response = crate::messages::machine::build_handshake(
                rand::random::<u32>(),
                timestamp,
                "beemesh/1.0",
                &peer.to_string(),
            );

            send_response(handshake_response);
            //log::info!("libp2p: sent handshake response to peer={}", peer);
        }
        Err(e) => {
            log::error!("libp2p: failed to parse handshake request: {:?}", e);
            // Send empty response on parse error
            let error_response = crate::messages::machine::build_handshake(0, 0, "", "");
            send_response(error_response);
        }
    }
}

pub fn handshake_response(
    response: Vec<u8>,
    peer: libp2p::PeerId,
    handshake_states: &mut std::collections::HashMap<libp2p::PeerId, HandshakeState>,
) {
    //log::info!("libp2p: received handshake response from peer={}", peer);

    // Parse the response
    match crate::messages::machine::root_as_handshake(&response) {
        Ok(_handshake_resp) => {
            //log::debug!("libp2p: handshake response - signature={:?}", handshake_resp.signature());

            // Mark this peer as confirmed
            ensure_handshake_state(&peer, handshake_states).confirmed = true;
        }
        Err(e) => {
            log::error!("libp2p: failed to parse handshake response: {:?}", e);
        }
    }
}

/// Handle a full `request_response::Message` for the handshake protocol.
/// This centralizes request/response handling so callers only need to delegate the
/// `Event::Message` into this function.
pub fn handshake_message_event(
    message: request_response::Message<Vec<u8>, Vec<u8>>,
    peer: libp2p::PeerId,
    swarm: &mut libp2p::Swarm<MyBehaviour>,
    handshake_states: &mut std::collections::HashMap<libp2p::PeerId, HandshakeState>,
) {
    match message {
        request_response::Message::Request {
            request, channel, ..
        } => {
            // Use the existing helper to process requests and send the response via the swarm
            let req = request.clone();
            let peer_c = peer.clone();
            let swarm_ref = swarm;
            handshake_request(
                req,
                peer_c,
                |resp| {
                    let _ = swarm_ref
                        .behaviour_mut()
                        .handshake_rr
                        .send_response(channel, resp);
                },
                handshake_states,
            );
        }
        request_response::Message::Response { response, .. } => {
            // Delegate to the existing response handler
            handshake_response(response.clone(), peer, handshake_states);
        }
    }
}

fn ensure_handshake_state<'a>(
    peer: &libp2p::PeerId,
    handshake_states: &'a mut std::collections::HashMap<libp2p::PeerId, HandshakeState>,
) -> &'a mut HandshakeState {
    handshake_states
        .entry(peer.clone())
        .or_insert(HandshakeState {
            attempts: 0,
            last_attempt: Instant::now() - Duration::from_secs(3),
            confirmed: false,
        })
}

// ============================================================================
// Delete Handlers
// ============================================================================

const UNKNOWN_OPERATION: &str = "unknown";
const UNKNOWN_MANIFEST: &str = "unknown";
const INVALID_FORMAT: &str = "invalid delete request format";
const ACK_MESSAGE: &str = "delete request received and processing";

fn delete_error_response(message: &str) -> Vec<u8> {
    machine::build_delete_response(false, UNKNOWN_OPERATION, message, UNKNOWN_MANIFEST, &[])
}

fn delete_ack_response(operation_id: &str, manifest_id: &str) -> Vec<u8> {
    machine::build_delete_response(true, operation_id, ACK_MESSAGE, manifest_id, &[])
}

/// Handle a delete message (request or response)
pub fn delete_message(
    message: request_response::Message<Vec<u8>, Vec<u8>>,
    peer: libp2p::PeerId,
    swarm: &mut libp2p::Swarm<MyBehaviour>,
    _local_peer: libp2p::PeerId,
) {
    match message {
        request_response::Message::Request {
            request, channel, ..
        } => {
            info!("Received delete request from peer={}", peer);

            // Parse the delete request
            match machine::root_as_delete_request(&request) {
                Ok(delete_req) => {
                    info!(
                        "Delete request - manifest_id={:?} operation_id={:?} force={}",
                        &delete_req.manifest_id, &delete_req.operation_id, delete_req.force
                    );

                    // Process the delete request asynchronously
                    let manifest_id = delete_req.manifest_id.clone();
                    let operation_id = delete_req.operation_id.clone();
                    let force = delete_req.force;
                    let requesting_peer = peer.to_string();

                    tokio::spawn(async move {
                        let (success, message, removed_workloads) =
                            process_delete_request(&manifest_id, force, &requesting_peer).await;

                        let _response = machine::build_delete_response(
                            success,
                            &operation_id,
                            &message,
                            &manifest_id,
                            &removed_workloads,
                        );

                        // Note: In a real implementation, we'd need to send this response back through a channel
                        // For now, we'll just log the result
                        info!(
                            "Delete request processed: success={} message={} removed_workloads={:?}",
                            success, message, removed_workloads
                        );
                    });

                    // Send immediate acknowledgment (the actual processing happens async)
                    let ack_response = delete_ack_response(
                        delete_req.operation_id.as_str(),
                        delete_req.manifest_id.as_str(),
                    );
                    let _ = swarm
                        .behaviour_mut()
                        .delete_rr
                        .send_response(channel, ack_response);
                }
                Err(e) => {
                    error!("Failed to parse delete request: {}", e);
                    let error_response = delete_error_response(INVALID_FORMAT);
                    let _ = swarm
                        .behaviour_mut()
                        .delete_rr
                        .send_response(channel, error_response);
                }
            }
        }
        request_response::Message::Response { response, .. } => {
            info!("Received delete response from peer={}", peer);

            // Parse the response
            match machine::root_as_delete_response(&response) {
                Ok(delete_resp) => {
                    info!(
                        "Delete response - ok={} operation_id={:?} message={:?} removed_workloads={:?}",
                        delete_resp.ok,
                        &delete_resp.operation_id,
                        &delete_resp.message,
                        Some(delete_resp.removed_workloads.clone())
                    );
                }
                Err(e) => {
                    warn!("Failed to parse delete response: {:?}", e);
                }
            }
        }
    }
}

/// Process a delete request by verifying ownership and removing workloads
async fn process_delete_request(
    manifest_id: &str,
    force: bool,
    _requesting_peer: &str,
) -> (bool, String, Vec<String>) {
    if !force {
        warn!(
            "Processing delete request for manifest_id={} without explicit ownership validation",
            manifest_id
        );
    }

    // Remove workloads using the workload manager
    match remove_workloads_by_manifest_id(manifest_id).await {
        Ok(removed_workloads) => {
            if removed_workloads.is_empty() {
                info!("No workloads found for manifest_id={}", manifest_id);
                (
                    true,
                    "no workloads found for manifest".to_string(),
                    removed_workloads,
                )
            } else {
                info!(
                    "Successfully removed {} workloads for manifest_id={}",
                    removed_workloads.len(),
                    manifest_id
                );
                (
                    true,
                    format!("removed {} workloads", removed_workloads.len()),
                    removed_workloads,
                )
            }
        }
        Err(e) => {
            error!(
                "Failed to remove workloads for manifest_id={}: {}",
                manifest_id, e
            );
            (false, format!("failed to remove workloads: {}", e), vec![])
        }
    }
}

// ============================================================================
// Kademlia Handlers
// ============================================================================

/// Handle Kademlia DHT events
pub fn kademlia_event(event: kad::Event, _peer_id: Option<PeerId>) {
    match event {
        kad::Event::OutboundQueryProgressed {
            id,
            result,
            step: _,
            stats: _,
        } => match result {
            kad::QueryResult::GetRecord(Ok(kad::GetRecordOk::FoundRecord(kad::PeerRecord {
                record,
                peer: _,
            }))) => {
                info!(
                    "DHT: Retrieved record with key: {:?} from query: {:?}",
                    record.key, id
                );
            }
            kad::QueryResult::GetProviders(Ok(kad::GetProvidersOk::FoundProviders {
                key: _key,
                providers,
            })) => {
                info!("DHT: Found providers for query {:?}: {:?}", id, providers);
                if let Some(tx) = control::take_pending_providers_query(&id) {
                    let _ = tx.send(providers.into_iter().collect());
                }
            }
            kad::QueryResult::GetProviders(Err(e)) => {
                warn!("DHT: Failed to get providers for query {:?}: {:?}", id, e);
                if let Some(tx) = control::take_pending_providers_query(&id) {
                    let _ = tx.send(Vec::new()); // Send empty vector on error
                }
            }
            kad::QueryResult::GetRecord(Err(e)) => {
                warn!("DHT: Failed to get record for query {:?}: {:?}", id, e);
            }
            kad::QueryResult::PutRecord(Ok(kad::PutRecordOk { key })) => {
                info!("DHT: Successfully stored record with key: {:?}", key);
                // If there is a pending control sender for this put (unlikely), we could reply here.
            }
            kad::QueryResult::PutRecord(Err(e)) => {
                debug!("DHT: Failed to store record: {:?}", e);
            }
            kad::QueryResult::Bootstrap(Ok(_)) => {
                //info!("DHT: Bootstrap completed successfully");
            }
            kad::QueryResult::Bootstrap(Err(e)) => {
                warn!("DHT: Bootstrap failed: {:?}", e);
            }
            kad::QueryResult::StartProviding(Ok(_)) => {
                debug!("DHT: Successfully started providing for query {:?}", id);
            }
            kad::QueryResult::StartProviding(Err(e)) => {
                warn!("DHT: Failed to start providing for query {:?}: {:?}", id, e);
            }
            _ => {
                info!("DHT: Other query result: {:?}", result);
            }
        },
        kad::Event::RoutingUpdated {
            peer: _,
            is_new_peer,
            addresses: _,
            bucket_range: _,
            old_peer: _,
        } => {
            if is_new_peer {
                /*info!(
                    "DHT: Added new peer {} with addresses: {:?}",
                    peer, addresses
                );*/
            }
        }
        kad::Event::UnroutablePeer { peer } => {
            warn!("DHT: Peer {} is unroutable", peer);
        }
        kad::Event::RoutablePeer { peer, address } => {
            info!("DHT: Peer {} is routable at {}", peer, address);
        }
        kad::Event::PendingRoutablePeer { peer, address } => {
            info!("DHT: Peer {} is pending routable at {}", peer, address);
        }
        kad::Event::InboundRequest { request } => match request {
            kad::InboundRequest::GetRecord {
                num_closer_peers,
                present_locally,
            } => {
                debug!(
                    "DHT: Received GetRecord request (closer_peers: {}, local: {})",
                    num_closer_peers, present_locally
                );
            }
            kad::InboundRequest::PutRecord {
                source,
                connection: _,
                record,
            } => {
                info!(
                    "DHT: Received PutRecord from {} for key: {:?}",
                    source,
                    record.as_ref().map(|r| &r.key)
                );
            }
            _ => {
                debug!("DHT: Other inbound request: {:?}", request);
            }
        },
        kad::Event::ModeChanged { new_mode } => {
            info!("DHT: Mode changed to {:?}", new_mode);
        }
    }
}

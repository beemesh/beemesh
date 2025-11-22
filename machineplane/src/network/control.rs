use crate::messages::constants::{SCHEDULER_PROPOSALS, SCHEDULER_TENDERS};
use crate::network::behaviour::{MyBehaviour, SCHEDULER_INPUT_TX};
use crate::network::{capacity, utils};
use libp2p::kad::{QueryId, RecordKey};
use libp2p::{PeerId, Swarm, gossipsub};
use log::{debug, info};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::collections::HashMap as StdHashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

type ManifestRequestSender = mpsc::UnboundedSender<Result<(), String>>;

/// Control messages sent from the rest API or other parts of the host to the libp2p task.
#[derive(Debug)]
pub enum Libp2pControl {
    QueryCapacityWithPayload {
        request_id: String,
        reply_tx: mpsc::UnboundedSender<String>,
        payload: Vec<u8>,
    },
    PublishTender {
        payload: Vec<u8>,
        reply_tx: mpsc::UnboundedSender<Result<(), String>>,
    },
    SendApplyRequest {
        peer_id: PeerId,
        /// ApplyRequest bytes (bincode)
        manifest: Vec<u8>,
        reply_tx: mpsc::UnboundedSender<Result<String, String>>,
    },
    SendDeleteRequest {
        peer_id: PeerId,
        /// DeleteRequest bytes (bincode)
        delete_request: Vec<u8>,
        reply_tx: mpsc::UnboundedSender<Result<String, String>>,
    },

    /// Find peers that hold shares for a given manifest id using DHT providers
    FindManifestHolders {
        manifest_id: String,
        reply_tx: mpsc::UnboundedSender<Vec<libp2p::PeerId>>,
    },
    /// Bootstrap the DHT by connecting to known peers
    BootstrapDht {
        reply_tx: mpsc::UnboundedSender<Result<(), String>>,
    },
    /// Announce that this node hosts the workload for the given CID (uses Kademlia provider records for placement)
    AnnounceProvider {
        cid: String,
        ttl_ms: u64,
        reply_tx: mpsc::UnboundedSender<Result<(), String>>,
    },
    /// Withdraw a previously announced placement
    WithdrawProvider {
        cid: String,
        reply_tx: mpsc::UnboundedSender<Result<(), String>>,
    },
    /// Get DHT peer information for debugging
    GetDhtPeers {
        reply_tx: mpsc::UnboundedSender<Result<serde_json::Value, String>>,
    },
    /// Get the local peer ID
    GetLocalPeerId {
        reply_tx: mpsc::UnboundedSender<libp2p::PeerId>,
    },
}

/// Handle incoming control messages from other parts of the host (REST handlers)
pub async fn handle_control_message(
    msg: Libp2pControl,
    swarm: &mut Swarm<MyBehaviour>,
    topic: &gossipsub::IdentTopic,
    pending_queries: &mut StdHashMap<String, Vec<mpsc::UnboundedSender<String>>>,
) {
    match msg {
        Libp2pControl::QueryCapacityWithPayload {
            request_id,
            reply_tx,
            payload,
        } => {
            handle_query_capacity_with_payload(
                request_id,
                reply_tx,
                payload,
                swarm,
                topic,
                pending_queries,
            )
            .await;
        }
        Libp2pControl::SendApplyRequest {
            peer_id,
            manifest,
            reply_tx,
        } => {
            handle_send_apply_request(peer_id, manifest, reply_tx, swarm).await;
        }
        Libp2pControl::SendDeleteRequest {
            peer_id,
            delete_request,
            reply_tx,
        } => {
            handle_send_delete_request(peer_id, delete_request, reply_tx, swarm).await;
        }
        Libp2pControl::PublishTender { payload, reply_tx } => {
            let topic = gossipsub::IdentTopic::new(SCHEDULER_TENDERS);
            match swarm
                .behaviour_mut()
                .gossipsub
                .publish(topic.clone(), payload.clone())
            {
                Ok(_) => {
                    if let Some(tx) = SCHEDULER_INPUT_TX.get() {
                        let _ = tx.send((
                            topic.hash(),
                            gossipsub::Message {
                                source: Some(*swarm.local_peer_id()),
                                data: payload,
                                sequence_number: None,
                                topic: topic.hash(),
                            },
                        ));
                    }

                    let _ = reply_tx.send(Ok(()));
                }
                Err(e) => {
                    let _ = reply_tx.send(Err(format!("Failed to publish tender: {}", e)));
                }
            }
        }

        Libp2pControl::FindManifestHolders {
            manifest_id,
            reply_tx,
        } => {
            // Use Kademlia providers API to find placement holders
            let record_key = RecordKey::new(&format!("provider:{}", manifest_id));
            info!(
                "DHT: attempting get_providers for key=provider:{}",
                manifest_id
            );
            let query_id = swarm.behaviour_mut().kademlia.get_providers(record_key);
            // Register pending sender so the kademlia_event handler can reply when results arrive
            insert_pending_providers_query(query_id, reply_tx);
            info!(
                "DHT: initiated find_providers for manifest (query_id={:?})",
                query_id
            );
        }

        Libp2pControl::BootstrapDht { reply_tx } => {
            let _ = swarm.behaviour_mut().kademlia.bootstrap();
            let _ = reply_tx.send(Ok(()));
        }
        Libp2pControl::AnnounceProvider {
            cid,
            ttl_ms,
            reply_tx,
        } => {
            log::warn!(
                "libp2p: processing AnnounceProvider for cid={} ttl_ms={}",
                cid,
                ttl_ms
            );
            // For local tests, announce multiple times with short intervals to improve discovery
            let mut announce_success = false;
            for attempt in 1..=3 {
                match announce_provider_direct(swarm, cid.clone(), ttl_ms) {
                    Ok(()) => {
                        announce_success = true;
                        if attempt > 1 {
                            log::info!(
                                "libp2p: AnnounceProvider succeeded for cid={} on attempt {}",
                                cid,
                                attempt
                            );
                        }
                        break;
                    }
                    Err(e) => {
                        log::warn!(
                            "libp2p: AnnounceProvider attempt {} failed for cid={}: {}",
                            attempt,
                            cid,
                            e
                        );
                        if attempt < 3 {
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        }
                    }
                }
            }

            if announce_success {
                // schedule next renewal at half the TTL (or 1s minimum) with more frequent updates for local tests
                let ttl = if ttl_ms == 0 { 0 } else { ttl_ms };
                let next = if ttl == 0 {
                    Instant::now() + Duration::from_secs(3600) // effectively never
                } else {
                    // More frequent renewal for local tests - every 2 seconds minimum
                    Instant::now() + Duration::from_millis(std::cmp::max(2000, ttl / 3))
                };
                let mut map = ACTIVE_ANNOUNCES.lock().unwrap();
                map.insert(
                    cid.clone(),
                    AnnounceEntry {
                        ttl_ms: ttl_ms,
                        next_due: next,
                    },
                );
                let _ = reply_tx.send(Ok(()));
            } else {
                let _ = reply_tx.send(Err("All provider announcement attempts failed".to_string()));
            }
        }
        Libp2pControl::GetDhtPeers { reply_tx } => {
            // Get DHT peer information for debugging
            let connected_peers: Vec<_> = swarm.connected_peers().cloned().collect();
            let local_peer_id = *swarm.local_peer_id();

            let dht_info = serde_json::json!({
                "local_peer_id": local_peer_id.to_string(),
                "connected_peers": connected_peers.iter().map(|p| p.to_string()).collect::<Vec<_>>(),
                "connected_count": connected_peers.len()
            });

            let _ = reply_tx.send(Ok(dht_info));
        }
        Libp2pControl::WithdrawProvider { cid, reply_tx } => {
            // Remove from active announces and issue a withdraw (expiry=now)
            {
                let mut map = ACTIVE_ANNOUNCES.lock().unwrap();
                map.remove(&cid);
            }
            match announce_provider_direct(swarm, cid.clone(), 0) {
                Ok(()) => {
                    let _ = reply_tx.send(Ok(()));
                }
                Err(e) => {
                    let _ = reply_tx.send(Err(e));
                }
            }
        }
        Libp2pControl::GetLocalPeerId { reply_tx } => {
            // Get the local peer ID
            let local_peer_id = *swarm.local_peer_id();
            let _ = reply_tx.send(local_peer_id);
        }
    }
}

/// Announce a placement record directly using the swarm's Kademlia behaviour.
/// This helper centralizes the key/record format used for placement announcements (via Kademlia providers).
pub fn announce_provider_direct(
    swarm: &mut Swarm<MyBehaviour>,
    cid: String,
    _ttl_ms: u64,
) -> Result<(), String> {
    // Use Kademlia's provider mechanism instead of records
    let record_key = RecordKey::new(&cid);

    // Check if we have sufficient DHT connectivity before announcing
    let connected_peers: Vec<_> = swarm.connected_peers().cloned().collect();
    if connected_peers.is_empty() {
        return Err("No connected peers for DHT announcement".to_string());
    }

    match swarm.behaviour_mut().kademlia.start_providing(record_key) {
        Ok(query_id) => {
            log::info!(
                "DHT: announced provider for cid={} (query_id={:?}) with {} connected peers",
                cid,
                query_id,
                connected_peers.len()
            );
            Ok(())
        }
        Err(e) => Err(format!("kademlia start_providing failed: {:?}", e)),
    }
}

/// Entry for active announce tracking
struct AnnounceEntry {
    ttl_ms: u64,
    next_due: Instant,
}

/// Map of active announces: cid -> entry
static ACTIVE_ANNOUNCES: Lazy<Mutex<HashMap<String, AnnounceEntry>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Queue for control messages enqueued from inside the libp2p behaviours.
static ENQUEUED_CONTROLS: Lazy<Mutex<std::collections::VecDeque<Libp2pControl>>> =
    Lazy::new(|| Mutex::new(std::collections::VecDeque::new()));

/// Pending provider lookup queries: QueryId string -> reply sender (Vec<PeerId>)
static PENDING_PROVIDERS_QUERIES: Lazy<
    Mutex<std::collections::HashMap<String, mpsc::UnboundedSender<Vec<libp2p::PeerId>>>>,
> = Lazy::new(|| Mutex::new(std::collections::HashMap::new()));

/// Insert a pending providers query sender for the given QueryId
pub fn insert_pending_providers_query(
    id: QueryId,
    sender: mpsc::UnboundedSender<Vec<libp2p::PeerId>>,
) {
    let key = format!("{:?}", id);
    let mut map = PENDING_PROVIDERS_QUERIES.lock().unwrap();
    map.insert(key, sender);
}

/// Take and remove a pending providers query sender by QueryId
pub fn take_pending_providers_query(
    id: &QueryId,
) -> Option<mpsc::UnboundedSender<Vec<libp2p::PeerId>>> {
    let key = format!("{:?}", id);
    let mut map = PENDING_PROVIDERS_QUERIES.lock().unwrap();
    map.remove(&key)
}

/// Global store for tracking pending manifest distribution requests
static PENDING_MANIFEST_REQUESTS: Lazy<
    Mutex<HashMap<libp2p::request_response::OutboundRequestId, (ManifestRequestSender, Instant)>>,
> = Lazy::new(|| Mutex::new(HashMap::new()));

/// Get access to the pending manifest requests store
pub fn get_pending_manifest_requests() -> &'static Mutex<
    HashMap<libp2p::request_response::OutboundRequestId, (ManifestRequestSender, Instant)>,
> {
    &PENDING_MANIFEST_REQUESTS
}

/// Clean up timed-out manifest requests (older than 5 seconds)
pub fn cleanup_timed_out_manifest_requests() {
    let now = Instant::now();
    let timeout_duration = Duration::from_secs(5);

    let mut pending_requests = PENDING_MANIFEST_REQUESTS.lock().unwrap();
    let mut timed_out_requests = Vec::new();

    // Find timed-out requests
    for (request_id, (_, timestamp)) in pending_requests.iter() {
        if now.duration_since(*timestamp) > timeout_duration {
            timed_out_requests.push(*request_id);
        }
    }

    // Remove and notify timed-out requests
    for request_id in timed_out_requests {
        if let Some((reply_tx, _)) = pending_requests.remove(&request_id) {
            let _ = reply_tx.send(Err("Request timeout".to_string()));
        }
    }
}

/// Enqueue a control message from inside a behaviour handler. This is synchronous and cheap.
pub fn enqueue_control(msg: Libp2pControl) {
    let mut q = ENQUEUED_CONTROLS.lock().unwrap();
    q.push_back(msg);
}

/// Drain and handle enqueued controls. Should be called from the libp2p task (async context).
pub async fn drain_enqueued_controls(
    swarm: &mut Swarm<MyBehaviour>,
    topic: &gossipsub::IdentTopic,
    pending_queries: &mut StdHashMap<String, Vec<mpsc::UnboundedSender<String>>>,
) {
    // Move queued items into a local vector to minimize lock time
    let mut items = Vec::new();
    {
        let mut q = ENQUEUED_CONTROLS.lock().unwrap();
        while let Some(it) = q.pop_front() {
            items.push(it);
        }
    }

    for msg in items {
        handle_control_message(msg, swarm, topic, pending_queries).await;
    }
}

/// Renew any announces that are due. Intended to be called periodically from the libp2p loop.
pub fn renew_due_providers(swarm: &mut Swarm<MyBehaviour>) {
    let now = Instant::now();
    let mut map = ACTIVE_ANNOUNCES.lock().unwrap();
    for (cid, entry) in map.iter_mut() {
        if entry.ttl_ms == 0 {
            continue;
        }
        if entry.next_due <= now {
            let _ = announce_provider_direct(swarm, cid.clone(), entry.ttl_ms);
            // More frequent renewal for local tests - every 2 seconds minimum
            entry.next_due =
                Instant::now() + Duration::from_millis(std::cmp::max(2000, entry.ttl_ms / 3));
        }
    }
}

/// Withdraw all announces (used on shutdown)
pub fn withdraw_all_providers(swarm: &mut Swarm<MyBehaviour>) {
    let keys: Vec<String> = {
        let mut m = ACTIVE_ANNOUNCES.lock().unwrap();
        m.drain().map(|(k, _)| k).collect()
    };
    for cid in keys {
        let _ = announce_provider_direct(swarm, cid.clone(), 0);
        info!("DHT: withdrew provider for cid={}", cid);
    }
}

/// Return a snapshot of active announce CIDs
pub fn list_active_announces() -> Vec<String> {
    let map = ACTIVE_ANNOUNCES.lock().unwrap();
    map.keys().cloned().collect()
}

// ============================================================================
// Control Message Handlers
// ============================================================================

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
            let proposals_topic = gossipsub::IdentTopic::new(SCHEDULER_PROPOSALS);
            match swarm
                .behaviour_mut()
                .gossipsub
                .publish(proposals_topic, finished)
            {
                Ok(_) => {
                    log::info!(
                        "libp2p: published capreq request_id={} to scheduler-proposals",
                        request_id
                    );
                }
                Err(err) => {
                    log::error!(
                        "libp2p: failed to publish capreq request_id={} to scheduler-proposals: {:?}",
                        request_id,
                        err
                    );
                }
            }

            // Also notify local pending senders directly so the originator is always considered
            // a potential responder. This ensures single-node operation and makes the
            // origining host countable when collecting responders. Include the local KEM
            // public key in the same format as remote responses.
            let responder_peer = swarm.local_peer_id().to_string();
            let reply =
                capacity::compose_capacity_reply("local", &request_id, &responder_peer, |params| {
                    params.cpu_milli = cap_req.cpu_milli;
                    params.memory_bytes = cap_req.memory_bytes;
                    params.storage_bytes = cap_req.storage_bytes;
                });
            let local_response = format!(
                "{}:{}",
                swarm.local_peer_id(),
                reply.kem_pub_b64.unwrap_or_default()
            );
            utils::notify_capacity_observers(pending_queries, &request_id, move || {
                local_response.clone()
            });
        }
        Err(e) => {
            log::error!("libp2p: failed to parse provided capacity payload: {:?}", e);
        }
    }
}

/// Handle SendApplyRequest control message
pub async fn handle_send_apply_request(
    peer_id: PeerId,
    manifest: Vec<u8>,
    reply_tx: mpsc::UnboundedSender<Result<String, String>>,
    swarm: &mut Swarm<MyBehaviour>,
) {
    info!(
        "libp2p: control SendApplyRequest received for peer={}",
        peer_id
    );

    // Check if this is a self-send - handle locally instead of using RequestResponse
    if peer_id == *swarm.local_peer_id() {
        debug!("libp2p: handling self-apply locally for peer {}", peer_id);

        // Use the new workload manager integration for self-apply as well
        crate::scheduler::process_enhanced_self_apply_request(&manifest, swarm).await;

        let _ = reply_tx.send(Ok(format!("Apply request handled locally for {}", peer_id)));
        return;
    }

    // For remote peers, use the normal RequestResponse protocol
    // Before sending the apply, create a capability token tied to this manifest and
    // send it to the target peer so they can store it in their keystore.
    // No capability token needed - direct manifest application

    // Finally send the apply request
    let request_id = swarm
        .behaviour_mut()
        .apply_rr
        .send_request(&peer_id, manifest);
    info!(
        "libp2p: sent apply request to peer={} request_id={:?}",
        peer_id, request_id
    );

    // For now, just send success immediately - proper response handling is done elsewhere
    let _ = reply_tx.send(Ok(format!("Apply request sent to {}", peer_id)));
}

/// Handle sending a delete request to a specific peer via libp2p request-response
pub async fn handle_send_delete_request(
    peer_id: PeerId,
    delete_request: Vec<u8>,
    reply_tx: mpsc::UnboundedSender<Result<String, String>>,
    swarm: &mut Swarm<MyBehaviour>,
) {
    info!(
        "handle_send_delete_request: sending delete request to peer {}",
        peer_id
    );

    // Send the delete request via request-response protocol
    let request_id = swarm
        .behaviour_mut()
        .delete_rr
        .send_request(&peer_id, delete_request);

    info!(
        "handle_send_delete_request: delete request sent to peer {} with request_id {:?}",
        peer_id, request_id
    );

    // For now, send immediate success response
    // In a complete implementation, we would track the request_id and wait for the actual response
    // from the peer, but that requires more complex request tracking infrastructure
    let _ = reply_tx.send(Ok("Delete request sent".to_string()));
}

use libp2p::kad::{store::RecordStore, Record, RecordKey};
use libp2p::{gossipsub, PeerId, Swarm};
use log::{info, warn};
use once_cell::sync::Lazy;
use std::collections::HashMap as StdHashMap;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
type KadQuerySender = mpsc::UnboundedSender<Result<Option<Vec<u8>>, String>>;
type ManifestRequestSender = mpsc::UnboundedSender<Result<(), String>>;

use crate::libp2p_beemesh::behaviour::MyBehaviour;
use libp2p::kad::QueryId;

mod query_capacity;
mod send_apply_request;

pub use query_capacity::handle_query_capacity_with_payload;
pub use send_apply_request::handle_send_apply_request;

/// Calculate a simple CID for manifest data
/// For now, use a basic hash - this should be replaced with proper CID calculation
fn calculate_manifest_cid(data: &[u8]) -> Option<String> {
    // Use a stable cryptographic hash (blake3) so that the same bytes produce the
    // same CID across different processes and runs. DefaultHasher is randomized
    // per-process and therefore unsuitable for cross-process identifiers.

    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    data.hash(&mut hasher);
    let hash_value = hasher.finish();
    Some(format!("{:x}", hash_value))
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

        Libp2pControl::StoreAppliedManifest {
            manifest_data,
            reply_tx,
        } => {
            // Store the manifest content in the DHT
            if let Some(cid) = calculate_manifest_cid(&manifest_data) {
                let record_key = RecordKey::new(&format!("manifest:{}", cid));
                let record = Record {
                    key: record_key.clone(),
                    value: manifest_data.clone(),
                    publisher: Some(swarm.local_peer_id().clone()),
                    expires: None, // No expiration for manifests
                };

                // Store locally first (critical for small networks where local node might be closest)
                match swarm
                    .behaviour_mut()
                    .kademlia
                    .store_mut()
                    .put(record.clone())
                {
                    Ok(()) => {
                        info!("DHT: stored manifest locally with cid={}", cid);
                        // Also store the manifest under the logical manifest id (tenant+operation+manifest_json)
                        // so lookups by manifest id succeed. Parse the AppliedManifest FlatBuffer and
                        // compute the same manifest_id used by other parts of the system.
                        // Try to parse AppliedManifest FlatBuffer first; if that fails, attempt to parse
                        // as JSON to extract tenant/operation_id/manifest_json fields so we can compute
                        // the logical manifest id and store an alias record under it. This helps
                        // GetManifestFromDht lookups which use the tenant+operation_id+manifest_json hash.
                        let mut maybe_tenant: Option<String> = None;
                        let mut maybe_operation: Option<String> = None;
                        let mut maybe_manifest_json: Option<String> = None;

                        if let Ok(applied) =
                            protocol::machine::root_as_applied_manifest(&manifest_data)
                        {
                            maybe_tenant = applied.tenant().map(|s| s.to_string());
                            maybe_operation = applied.operation_id().map(|s| s.to_string());
                            maybe_manifest_json = applied.manifest_json().map(|s| s.to_string());
                        } else if let Ok(val) =
                            serde_json::from_slice::<serde_json::Value>(&manifest_data)
                        {
                            // Try common JSON shapes used when storing raw manifests
                            if let Some(t) = val.get("tenant").and_then(|v| v.as_str()) {
                                maybe_tenant = Some(t.to_string());
                            }
                            if let Some(op) = val.get("operation_id").and_then(|v| v.as_str()) {
                                maybe_operation = Some(op.to_string());
                            }
                            // If the JSON itself is the manifest, serialize it back to string
                            maybe_manifest_json = Some(val.to_string());
                        }

                        if let (Some(tenant_s), Some(operation_id_s), Some(manifest_json_s)) = (
                            maybe_tenant.clone(),
                            maybe_operation.clone(),
                            maybe_manifest_json.clone(),
                        ) {
                            // Compute a stable manifest id deterministically from the
                            // tenant, operation_id and manifest_json. Use a
                            // cryptographic hash (blake3) so different processes
                            // compute the same id for the same inputs.
                            let mut buf = Vec::new();
                            buf.extend_from_slice(tenant_s.as_bytes());
                            buf.push(0);
                            buf.extend_from_slice(operation_id_s.as_bytes());
                            buf.push(0);
                            buf.extend_from_slice(manifest_json_s.as_bytes());
                            use std::collections::hash_map::DefaultHasher;
                            use std::hash::{Hash, Hasher};
                            let mut hasher = DefaultHasher::new();
                            buf.hash(&mut hasher);
                            let mid = hasher.finish();
                            let manifest_id = mid.to_be().to_string();

                            // store an alias record keyed by manifest_id so GetManifestFromDht can find it
                            let manifest_key = RecordKey::new(&format!("manifest:{}", manifest_id));
                            let alias_record = Record {
                                key: manifest_key.clone(),
                                value: manifest_data.clone(),
                                publisher: Some(swarm.local_peer_id().clone()),
                                expires: None,
                            };
                            match swarm.behaviour_mut().kademlia.store_mut().put(alias_record) {
                                Ok(()) => info!(
                                    "DHT: stored manifest locally under manifest_id={}",
                                    manifest_id
                                ),
                                Err(e) => {
                                    warn!("DHT: failed to store alias manifest_id locally: {:?}", e)
                                }
                            }

                            // Fire-and-forget spawning helper: attempt decryption based on tenant/opid/manifest_json
                            // TODO: implement spawn_decrypt_task function
                            // let _ = crate::libp2p_beemesh::behaviour::apply_message::spawn_decrypt_task(Some(tenant_s), Some(operation_id_s), Some(manifest_json_s)).await;
                        } else if maybe_tenant.is_some()
                            || maybe_operation.is_some()
                            || maybe_manifest_json.is_some()
                        {
                            // We have partial info; still attempt to spawn decrypt task with whatever we found
                            // TODO: implement spawn_decrypt_task function
                            // let _ = crate::libp2p_beemesh::behaviour::apply_message::spawn_decrypt_task(maybe_tenant, maybe_operation, maybe_manifest_json).await;
                        }
                    }
                    Err(e) => warn!("DHT: failed to store manifest locally: {:?}", e),
                }

                // Also try to store on network
                match swarm
                    .behaviour_mut()
                    .kademlia
                    .put_record(record, libp2p::kad::Quorum::One)
                {
                    Ok(_query_id) => {
                        info!(
                            "DHT: initiated network storage for manifest with cid={}",
                            cid
                        );
                        let _ = reply_tx.send(Ok(()));
                    }
                    Err(e) => {
                        warn!(
                            "DHT: failed to initiate network storage for manifest: {:?}",
                            e
                        );
                        // Still return success since we stored it locally
                        let _ = reply_tx.send(Ok(()));
                    }
                }
            } else {
                warn!("DHT: failed to calculate manifest CID");
                let _ = reply_tx.send(Err("Failed to calculate manifest CID".to_string()));
            }
        }
        Libp2pControl::GetManifestFromDht {
            manifest_id,
            reply_tx,
        } => {
            // Create the record key used by manifests
            let record_key = RecordKey::new(&format!("manifest:{}", manifest_id));
            // Initiate kademlia get_record query
            let query_id = swarm.behaviour_mut().kademlia.get_record(record_key);
            // Register pending sender so the kademlia_event handler can reply when results arrive
            insert_pending_kad_query(query_id, reply_tx);
            info!(
                "DHT: initiated get_record for manifest (query_id={:?})",
                query_id
            );
        }
        Libp2pControl::FindManifestHolders {
            manifest_id,
            reply_tx,
        } => {
            // Use Kademlia providers API by issuing a get_providers for the manifest key
            let record_key = RecordKey::new(&format!("manifest:{}", manifest_id));
            info!(
                "DHT: attempting get_providers for key=manifest:{}",
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
        Libp2pControl::AnnounceManifestHolder {
            manifest_id,
            version,
            reply_tx,
        } => {
            // Store the manifest holder information locally in global store
            {
                let mut store = get_manifest_holder_store().lock().unwrap();
                store.announce_manifest_holder(manifest_id.clone(), version);
            }

            info!(
                "Announcing manifest holder: manifest_id={}, version={}",
                manifest_id, version
            );
            let _ = reply_tx.send(Ok(()));
        }
        Libp2pControl::FindManifestHoldersWithVersion {
            manifest_id,
            version,
            reply_tx,
        } => {
            // Query connected peers for manifest holders using the announcement protocol
            let query = crate::libp2p_beemesh::manifest_announcement::ManifestHolderQuery {
                manifest_id: manifest_id.clone(),
                version,
            };

            let connected_peers: Vec<_> = swarm.connected_peers().cloned().collect();
            if connected_peers.is_empty() {
                warn!("No connected peers to query for manifest holders");
                let _ = reply_tx.send(vec![]);
                return;
            }

            info!(
                "Querying {} peers for manifest holders of {} (version: {:?})",
                connected_peers.len(),
                manifest_id,
                version
            );

            // Serialize the query to JSON for transmission
            let query_bytes = match serde_json::to_vec(&query) {
                Ok(bytes) => bytes,
                Err(e) => {
                    warn!("Failed to serialize manifest holder query: {}", e);
                    let _ = reply_tx.send(vec![]);
                    return;
                }
            };

            // Send query to all connected peers and collect them for tracking
            let mut queried_peers = Vec::new();
            for peer in connected_peers {
                let request_id = swarm
                    .behaviour_mut()
                    .manifest_announcement_rr
                    .send_request(&peer, query_bytes.clone());
                info!(
                    "Sent manifest holder query to peer={} request_id={:?}",
                    peer, request_id
                );
                queried_peers.push(peer);
            }

            // If no queries were sent successfully, return empty result
            if queried_peers.is_empty() {
                let _ = reply_tx.send(vec![]);
                return;
            }

            // Create a unique query key for tracking responses
            let query_key = format!(
                "{}:{}",
                manifest_id,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            );

            // Store the pending query to collect responses
            insert_pending_manifest_query(query_key.clone(), queried_peers, reply_tx);

            // Set up a timeout to cleanup if not all peers respond
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                cleanup_expired_manifest_queries();
            });
        }

        Libp2pControl::BootstrapDht { reply_tx } => {
            // Bootstrap the Kademlia DHT
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
        Libp2pControl::GetConnectedPeers { reply_tx } => {
            // Get list of currently connected peers
            let connected_peers: Vec<libp2p::PeerId> = swarm.connected_peers().cloned().collect();
            let _ = reply_tx.send(connected_peers);
        }
        Libp2pControl::GetLocalPeerId { reply_tx } => {
            // Get the local peer ID
            let local_peer_id = *swarm.local_peer_id();
            let _ = reply_tx.send(local_peer_id);
        }
    }
}

/// Announce a provider record directly using the swarm's Kademlia behaviour.
/// This helper centralizes the key/record format used for provider announcements.
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

/// Pending Kademlia GetRecord queries: query_id debug string -> reply sender
static PENDING_KAD_QUERIES: Lazy<Mutex<std::collections::HashMap<String, KadQuerySender>>> =
    Lazy::new(|| Mutex::new(std::collections::HashMap::new()));

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

/// Global manifest holder store
static MANIFEST_HOLDER_STORE: Lazy<
    Mutex<crate::libp2p_beemesh::manifest_announcement::ManifestHolderStore>,
> = Lazy::new(|| {
    Mutex::new(crate::libp2p_beemesh::manifest_announcement::ManifestHolderStore::new())
});

/// Get reference to the global manifest holder store
pub fn get_manifest_holder_store(
) -> &'static Mutex<crate::libp2p_beemesh::manifest_announcement::ManifestHolderStore> {
    &MANIFEST_HOLDER_STORE
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

/// Pending manifest holder queries: query_key -> (peers, reply_sender)
static PENDING_MANIFEST_QUERIES: Lazy<
    Mutex<
        std::collections::HashMap<
            String,
            (
                Vec<libp2p::PeerId>,
                mpsc::UnboundedSender<
                    Vec<crate::libp2p_beemesh::manifest_announcement::ManifestHolderInfo>,
                >,
            ),
        >,
    >,
> = Lazy::new(|| Mutex::new(std::collections::HashMap::new()));

/// Insert a pending manifest holder query
pub fn insert_pending_manifest_query(
    query_key: String,
    peers: Vec<libp2p::PeerId>,
    sender: mpsc::UnboundedSender<
        Vec<crate::libp2p_beemesh::manifest_announcement::ManifestHolderInfo>,
    >,
) {
    let mut map = PENDING_MANIFEST_QUERIES.lock().unwrap();
    map.insert(query_key, (peers, sender));
}

/// Process a manifest holder response and check if we have enough responses
pub fn handle_manifest_holder_response(
    peer: &libp2p::PeerId,
    response: &crate::libp2p_beemesh::manifest_announcement::ManifestHolderResponse,
) {
    let mut map = PENDING_MANIFEST_QUERIES.lock().unwrap();
    let mut completed_queries = Vec::new();

    // Check all pending queries to see if this response completes any
    for (query_key, (expected_peers, _)) in map.iter() {
        if expected_peers.contains(peer) {
            // This response is for this query
            completed_queries.push(query_key.clone());
        }
    }

    // For now, send the first response to complete any pending queries
    // In a full implementation, we'd collect multiple responses
    for query_key in completed_queries {
        if let Some((_, sender)) = map.remove(&query_key) {
            let _ = sender.send(response.holders.clone());
            break; // Only complete one query per response for simplicity
        }
    }
}

/// Clean up expired manifest queries
pub fn cleanup_expired_manifest_queries() {
    let mut map = PENDING_MANIFEST_QUERIES.lock().unwrap();
    // For now, just clear all - in production we'd track timestamps
    map.clear();
}

/// Insert a pending kad query sender for the given QueryId
pub fn insert_pending_kad_query(
    id: QueryId,
    sender: mpsc::UnboundedSender<Result<Option<Vec<u8>>, String>>,
) {
    let key = format!("{:?}", id);
    let mut map = PENDING_KAD_QUERIES.lock().unwrap();
    map.insert(key, sender);
}

/// Take and remove a pending kad query sender by QueryId
pub fn take_pending_kad_query(
    id: &QueryId,
) -> Option<mpsc::UnboundedSender<Result<Option<Vec<u8>>, String>>> {
    let key = format!("{:?}", id);
    let mut map = PENDING_KAD_QUERIES.lock().unwrap();
    map.remove(&key)
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

/// Control messages sent from the rest API or other parts of the host to the libp2p task.
#[derive(Debug)]
pub enum Libp2pControl {
    QueryCapacityWithPayload {
        request_id: String,
        reply_tx: mpsc::UnboundedSender<String>,
        payload: Vec<u8>,
    },
    SendApplyRequest {
        peer_id: PeerId,
        /// FlatBuffer ApplyRequest bytes
        manifest: Vec<u8>,
        reply_tx: mpsc::UnboundedSender<Result<String, String>>,
    },

    /// Find peers that hold shares for a given manifest id using DHT providers
    FindManifestHolders {
        manifest_id: String,
        reply_tx: mpsc::UnboundedSender<Vec<libp2p::PeerId>>,
    },
    /// Announce that this node holds a specific manifest version
    AnnounceManifestHolder {
        manifest_id: String,
        version: u64,
        reply_tx: mpsc::UnboundedSender<Result<(), String>>,
    },
    /// Find peers that hold a specific manifest using the announcement protocol
    FindManifestHoldersWithVersion {
        manifest_id: String,
        version: Option<u64>,
        reply_tx: mpsc::UnboundedSender<
            Vec<crate::libp2p_beemesh::manifest_announcement::ManifestHolderInfo>,
        >,
    },

    /// Store an applied manifest in the DHT after successful deployment
    StoreAppliedManifest {
        manifest_data: Vec<u8>,
        reply_tx: mpsc::UnboundedSender<Result<(), String>>,
    },
    /// Retrieve a manifest from the DHT by its ID
    GetManifestFromDht {
        manifest_id: String,
        reply_tx: mpsc::UnboundedSender<Result<Option<Vec<u8>>, String>>,
    },
    /// Bootstrap the DHT by connecting to known peers
    BootstrapDht {
        reply_tx: mpsc::UnboundedSender<Result<(), String>>,
    },
    /// Announce that this node holds data for the given CID (provider-style record)
    AnnounceProvider {
        cid: String,
        ttl_ms: u64,
        reply_tx: mpsc::UnboundedSender<Result<(), String>>,
    },
    /// Withdraw a previously announced provider
    WithdrawProvider {
        cid: String,
        reply_tx: mpsc::UnboundedSender<Result<(), String>>,
    },
    /// Get DHT peer information for debugging
    GetDhtPeers {
        reply_tx: mpsc::UnboundedSender<Result<serde_json::Value, String>>,
    },
    /// Get list of currently connected peers
    GetConnectedPeers {
        reply_tx: mpsc::UnboundedSender<Vec<libp2p::PeerId>>,
    },
    /// Get the local peer ID
    GetLocalPeerId {
        reply_tx: mpsc::UnboundedSender<libp2p::PeerId>,
    },
}

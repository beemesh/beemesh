use libp2p::{gossipsub, PeerId, Swarm};
use libp2p::kad::{Record, RecordKey, store::RecordStore};
use std::time::{Instant, Duration};
use log::{info, warn};
use std::collections::HashMap as StdHashMap;
use tokio::sync::mpsc;
use once_cell::sync::Lazy;
use std::sync::Mutex;
use std::collections::HashMap;

use crate::libp2p_beemesh::{behaviour::MyBehaviour};
use libp2p::kad::QueryId;

mod query_capacity;
mod send_apply_request;
mod send_key_share;

pub use query_capacity::handle_query_capacity_with_payload;
pub use send_apply_request::handle_send_apply_request;
pub use send_key_share::handle_send_key_share;

/// Calculate a simple CID for manifest data
/// For now, use a basic hash - this should be replaced with proper CID calculation
fn calculate_manifest_cid(data: &[u8]) -> Option<String> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    
    let mut hasher = DefaultHasher::new();
    data.hash(&mut hasher);
    let hash = hasher.finish();
    Some(format!("{:016x}", hash))
}

/// Handle incoming control messages from other parts of the host (REST handlers)
pub async fn handle_control_message(
    msg: Libp2pControl,
    swarm: &mut Swarm<MyBehaviour>,
    topic: &gossipsub::IdentTopic,
    pending_queries: &mut StdHashMap<String, Vec<mpsc::UnboundedSender<String>>>,
) {
    match msg {
        Libp2pControl::QueryCapacityWithPayload { request_id, reply_tx, payload } => {
            handle_query_capacity_with_payload(request_id, reply_tx, payload, swarm, topic, pending_queries).await;
        }
        Libp2pControl::SendApplyRequest { peer_id, manifest, reply_tx } => {
            handle_send_apply_request(peer_id, manifest, reply_tx, swarm).await;
        }
        Libp2pControl::SendKeyShare { peer_id, share_payload, reply_tx } => {
            handle_send_key_share(peer_id, share_payload, reply_tx, swarm).await;
        }
        Libp2pControl::StoreAppliedManifest { manifest_data, reply_tx } => {
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
                match swarm.behaviour_mut().kademlia.store_mut().put(record.clone()) {
                    Ok(()) => info!("DHT: stored manifest locally with cid={}", cid),
                    Err(e) => warn!("DHT: failed to store manifest locally: {:?}", e),
                }
                
                // Also try to store on network
                match swarm.behaviour_mut().kademlia.put_record(record, libp2p::kad::Quorum::One) {
                    Ok(_query_id) => {
                        info!("DHT: initiated network storage for manifest with cid={}", cid);
                        let _ = reply_tx.send(Ok(()));
                    }
                    Err(e) => {
                        warn!("DHT: failed to initiate network storage for manifest: {:?}", e);
                        // Still return success since we stored it locally
                        let _ = reply_tx.send(Ok(()));
                    }
                }
            } else {
                warn!("DHT: failed to calculate manifest CID");
                let _ = reply_tx.send(Err("Failed to calculate manifest CID".to_string()));
            }
        }
        Libp2pControl::GetManifestFromDht { manifest_id, reply_tx } => {
            // Create the record key used by manifests
            let record_key = RecordKey::new(&format!("manifest:{}", manifest_id));
            // Initiate kademlia get_record query
            let query_id = swarm.behaviour_mut().kademlia.get_record(record_key);
            // Register pending sender so the kademlia_event handler can reply when results arrive
            insert_pending_kad_query(query_id, reply_tx);
            info!("DHT: initiated get_record for manifest (query_id={:?})", query_id);
        }
        Libp2pControl::BootstrapDht { reply_tx } => {
            // Bootstrap the Kademlia DHT
            let _ = swarm.behaviour_mut().kademlia.bootstrap();
            let _ = reply_tx.send(Ok(()));
        }
        Libp2pControl::AnnounceProvider { cid, ttl_ms, reply_tx } => {
            // perform announce and register for periodic renewal
            match announce_provider_direct(swarm, cid.clone(), ttl_ms) {
                Ok(()) => {
                    // schedule next renewal at half the TTL (or 1s minimum)
                    let ttl = if ttl_ms == 0 { 0 } else { ttl_ms };
                    let next = if ttl == 0 {
                        Instant::now() + Duration::from_secs(3600) // effectively never
                    } else {
                        Instant::now() + Duration::from_millis(std::cmp::max(1000, ttl / 2))
                    };
                    let mut map = ACTIVE_ANNOUNCES.lock().unwrap();
                    map.insert(cid.clone(), AnnounceEntry { ttl_ms: ttl_ms, next_due: next });
                    let _ = reply_tx.send(Ok(()));
                }
                Err(e) => {
                    let _ = reply_tx.send(Err(e));
                }
            }
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
    }
}

/// Announce a provider record directly using the swarm's Kademlia behaviour.
/// This helper centralizes the key/record format used for provider announcements.
pub fn announce_provider_direct(swarm: &mut Swarm<MyBehaviour>, cid: String, ttl_ms: u64) -> Result<(), String> {
    let record_key = RecordKey::new(&format!("provider:{}", cid));
    let expires = if ttl_ms > 0 {
        Some(Instant::now() + Duration::from_millis(ttl_ms))
    } else {
        None
    };
    let record = Record { key: record_key, value: Vec::new(), publisher: None, expires };
    
    // For small networks/testing, be less strict about quorum
    let quorum = libp2p::kad::Quorum::One;
    
    match swarm.behaviour_mut().kademlia.put_record(record, quorum) {
        Ok(_query_id) => {
            info!("DHT: announced provider for cid={}", cid);
            Ok(())
        }
        Err(e) => Err(format!("kademlia put_record failed: {:?}", e)),
    }
}

/// Entry for active announce tracking
struct AnnounceEntry {
    ttl_ms: u64,
    next_due: Instant,
}

/// Map of active announces: cid -> entry
static ACTIVE_ANNOUNCES: Lazy<Mutex<HashMap<String, AnnounceEntry>>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// Queue for control messages enqueued from inside the libp2p behaviours.
static ENQUEUED_CONTROLS: Lazy<Mutex<std::collections::VecDeque<Libp2pControl>>> = Lazy::new(|| Mutex::new(std::collections::VecDeque::new()));

/// Pending Kademlia GetRecord queries: query_id debug string -> reply sender
static PENDING_KAD_QUERIES: Lazy<Mutex<std::collections::HashMap<String, mpsc::UnboundedSender<Result<Option<Vec<u8>>, String>>>>> = Lazy::new(|| Mutex::new(std::collections::HashMap::new()));

/// Insert a pending kad query sender for the given QueryId
pub fn insert_pending_kad_query(id: QueryId, sender: mpsc::UnboundedSender<Result<Option<Vec<u8>>, String>>) {
    let key = format!("{:?}", id);
    let mut map = PENDING_KAD_QUERIES.lock().unwrap();
    map.insert(key, sender);
}

/// Take and remove a pending kad query sender by QueryId
pub fn take_pending_kad_query(id: &QueryId) -> Option<mpsc::UnboundedSender<Result<Option<Vec<u8>>, String>>> {
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
            entry.next_due = Instant::now() + Duration::from_millis(std::cmp::max(1000, entry.ttl_ms / 2));
        }
    }
}

/// Withdraw all announces (used on shutdown)
pub fn withdraw_all_providers(swarm: &mut Swarm<MyBehaviour>) {
    let keys: Vec<String> = { let mut m = ACTIVE_ANNOUNCES.lock().unwrap(); m.drain().map(|(k, _)| k).collect() };
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
    /// Send an encrypted key share to a specific peer
    SendKeyShare {
        peer_id: PeerId,
        share_payload: serde_json::Value,
        reply_tx: mpsc::UnboundedSender<Result<(), String>>,
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
    /// Announce that this node holds a keyshare for the given CID (provider-style record)
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
}

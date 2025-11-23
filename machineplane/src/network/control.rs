use crate::messages::constants::BEEMESH_FABRIC;
use crate::network::behaviour::{MyBehaviour, SCHEDULER_INPUT_TX};
use libp2p::kad::{QueryId, RecordKey};
use libp2p::{Swarm, gossipsub};
use log::info;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

type ManifestRequestSender = mpsc::UnboundedSender<Result<(), String>>;

/// Control messages sent from the rest API or other parts of the host to the libp2p task.
#[derive(Debug)]
pub enum Libp2pControl {
    PublishTender {
        payload: Vec<u8>,
        reply_tx: mpsc::UnboundedSender<Result<(), String>>,
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
pub async fn handle_control_message(msg: Libp2pControl, swarm: &mut Swarm<MyBehaviour>) {
    match msg {
        Libp2pControl::PublishTender { payload, reply_tx } => {
            let topic = gossipsub::IdentTopic::new(BEEMESH_FABRIC);
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
            let _ = reply_tx.send(announce_provider_direct(swarm, cid.clone(), ttl_ms));
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
            log::info!("Withdraw requested for cid={}, letting provider record expire", cid);
            let _ = reply_tx.send(Ok(()));
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

// ============================================================================
// Control Message Handlers
// ============================================================================

use crate::messages::constants::BEEMESH_FABRIC;
use crate::network::ManifestTransferCodec;
use libp2p::swarm::NetworkBehaviour;
use libp2p::{PeerId, gossipsub, kad, request_response};
use log::{debug, error, info, warn};
use std::sync::OnceLock;
use tokio::sync::mpsc;

#[derive(NetworkBehaviour)]
pub struct MyBehaviour {
    pub gossipsub: gossipsub::Behaviour,
    pub manifest_fetch_rr: request_response::Behaviour<ManifestTransferCodec>,
    pub kademlia: kad::Behaviour<kad::store::MemoryStore>,
}

// ============================================================================
// Gossipsub Handlers
// ============================================================================

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Mutex;

// Map from local peer ID to scheduler input channel
// This allows multiple nodes in the same process (for testing) to have distinct schedulers.
static SCHEDULER_INPUT_CHANNELS: Lazy<
    Mutex<HashMap<PeerId, mpsc::UnboundedSender<(gossipsub::TopicHash, gossipsub::Message)>>>,
> = Lazy::new(|| Mutex::new(HashMap::new()));

// Legacy global channel for backward compatibility (single-node mode)
pub static SCHEDULER_INPUT_TX: OnceLock<
    mpsc::UnboundedSender<(libp2p::gossipsub::TopicHash, libp2p::gossipsub::Message)>,
> = OnceLock::new();

/// Set the scheduler input channel for a specific peer ID
pub fn set_scheduler_input_for_peer(
    peer_id: PeerId,
    tx: mpsc::UnboundedSender<(gossipsub::TopicHash, gossipsub::Message)>,
) {
    let mut channels = SCHEDULER_INPUT_CHANNELS.lock().unwrap();
    channels.insert(peer_id, tx);
}

/// Get the scheduler input channel for a specific peer ID
pub fn get_scheduler_input_for_peer(
    peer_id: &PeerId,
) -> Option<mpsc::UnboundedSender<(gossipsub::TopicHash, gossipsub::Message)>> {
    let channels = SCHEDULER_INPUT_CHANNELS.lock().unwrap();
    channels.get(peer_id).cloned()
}

pub fn set_scheduler_input(
    tx: mpsc::UnboundedSender<(libp2p::gossipsub::TopicHash, libp2p::gossipsub::Message)>,
) {
    let _ = SCHEDULER_INPUT_TX.set(tx);
}

pub fn gossipsub_message(
    peer_id: libp2p::PeerId,
    local_peer_id: libp2p::PeerId,
    message: gossipsub::Message,
    topic: gossipsub::TopicHash,
) {
    debug!("received message from {}", peer_id);
    let payload = &message.data;

    static FABRIC_TOPIC: OnceLock<gossipsub::TopicHash> = OnceLock::new();

    let scheduler_topic =
        FABRIC_TOPIC.get_or_init(|| gossipsub::IdentTopic::new(BEEMESH_FABRIC).hash());

    if topic == *scheduler_topic {
        debug!(
            "gossipsub: Forwarding scheduler message on topic {} from peer {} ({} bytes) to local scheduler {}",
            topic,
            peer_id,
            payload.len(),
            local_peer_id
        );
        
        // First try to find the scheduler for this specific local peer
        if let Some(tx) = get_scheduler_input_for_peer(&local_peer_id) {
            if let Err(e) = tx.send((topic, message)) {
                error!("Failed to forward scheduler message: {}", e);
            }
            return;
        }
        
        // Fall back to legacy global channel for backward compatibility
        if let Some(tx) = SCHEDULER_INPUT_TX.get() {
            if let Err(e) = tx.send((topic, message)) {
                error!("Failed to forward scheduler message: {}", e);
            }
        } else {
            warn!("Scheduler input channel not initialized for peer {}, dropping message", local_peer_id);
        }
        return;
    }

    warn!(
        "gossipsub: Dropping message on unsupported topic {} ({} bytes) from peer {}. Supported topic is {}",
        topic,
        payload.len(),
        peer_id,
        BEEMESH_FABRIC
    );
}

pub fn gossipsub_subscribed(_peer_id: libp2p::PeerId, _topic: gossipsub::TopicHash) {}

pub fn gossipsub_unsubscribed(peer_id: libp2p::PeerId, topic: gossipsub::TopicHash) {
    log::info!("Peer {peer_id} unsubscribed from topic: {topic}");
}

// ============================================================================
// Kademlia Handlers
// ============================================================================

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
            kad::QueryResult::GetRecord(Err(e)) => {
                warn!("DHT: Failed to get record for query {:?}: {:?}", id, e);
            }
            kad::QueryResult::PutRecord(Ok(kad::PutRecordOk { key })) => {
                info!("DHT: Successfully stored record with key: {:?}", key);
            }
            kad::QueryResult::PutRecord(Err(e)) => {
                debug!("DHT: Failed to store record: {:?}", e);
            }
            kad::QueryResult::Bootstrap(Ok(_)) => {}
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
        } => if is_new_peer {},
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

//! # Network Behaviour Module
//!
//! This module defines the libp2p network behaviour (`MyBehaviour`) and handlers
//! for gossipsub and Kademlia events.
//!
//! ## MyBehaviour
//!
//! The composite network behaviour combines:
//!
//! - **Gossipsub**: Pub/sub for scheduler messages (Tender/Bid/Award/Event)
//! - **Kademlia**: DHT for peer discovery and record storage
//! - **Request-Response**: Direct messaging for manifest transfer
//!
//! ## Event Handling
//!
//! - `gossipsub_message()`: Routes incoming messages to the appropriate scheduler
//! - `kademlia_event()`: Handles DHT query results and routing updates
//! - `gossipsub_subscribed()/unsubscribed()`: Tracks topic membership changes
//!
//! ## Multi-Node Support
//!
//! For testing multiple nodes in a single process, each node has its own
//! scheduler input channel indexed by `PeerId`. Legacy single-node mode
//! uses a global channel for backward compatibility.

use crate::messages::constants::BEEMESH_FABRIC;
use crate::network::ManifestTransferCodec;
use libp2p::swarm::NetworkBehaviour;
use libp2p::{PeerId, gossipsub, kad, request_response};
use log::{debug, error, info, warn};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, OnceLock};
use tokio::sync::mpsc;

// ============================================================================
// Network Behaviour Definition
// ============================================================================

/// Composite libp2p network behaviour for the machineplane.
///
/// Combines gossipsub (pub/sub), Kademlia (DHT), and request-response (manifest transfer)
/// into a single network behaviour that can be used with a libp2p swarm.
#[derive(NetworkBehaviour)]
pub struct MyBehaviour {
    /// Gossipsub pub/sub for scheduler messages (Tender, Bid, Award, Event)
    pub gossipsub: gossipsub::Behaviour,
    /// Request-response protocol for manifest transfer to award winners
    pub manifest_fetch_rr: request_response::Behaviour<ManifestTransferCodec>,
    /// Kademlia DHT for peer discovery and distributed record storage
    pub kademlia: kad::Behaviour<kad::store::MemoryStore>,
}

// ============================================================================
// Scheduler Input Channel Management
// ============================================================================

/// Per-peer scheduler input channels for multi-node testing.
///
/// Maps local peer ID to the scheduler input channel for that node.
/// This allows multiple nodes in the same process to have distinct schedulers.
static SCHEDULER_INPUT_CHANNELS: LazyLock<
    Mutex<HashMap<PeerId, mpsc::UnboundedSender<(gossipsub::TopicHash, gossipsub::Message)>>>,
> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Legacy global channel for backward compatibility (single-node mode).
pub static SCHEDULER_INPUT_TX: OnceLock<
    mpsc::UnboundedSender<(libp2p::gossipsub::TopicHash, libp2p::gossipsub::Message)>,
> = OnceLock::new();

/// Registers a scheduler input channel for a specific peer ID.
///
/// Called during node startup to associate a scheduler with its local peer.
pub fn set_scheduler_input_for_peer(
    peer_id: PeerId,
    tx: mpsc::UnboundedSender<(gossipsub::TopicHash, gossipsub::Message)>,
) {
    let mut channels = SCHEDULER_INPUT_CHANNELS.lock().unwrap();
    channels.insert(peer_id, tx);
}

/// Retrieves the scheduler input channel for a specific peer ID.
pub fn get_scheduler_input_for_peer(
    peer_id: &PeerId,
) -> Option<mpsc::UnboundedSender<(gossipsub::TopicHash, gossipsub::Message)>> {
    let channels = SCHEDULER_INPUT_CHANNELS.lock().unwrap();
    channels.get(peer_id).cloned()
}

/// Sets the legacy global scheduler input channel.
pub fn set_scheduler_input(
    tx: mpsc::UnboundedSender<(libp2p::gossipsub::TopicHash, libp2p::gossipsub::Message)>,
) {
    let _ = SCHEDULER_INPUT_TX.set(tx);
}

// ============================================================================
// Gossipsub Event Handlers
// ============================================================================

/// Handles incoming gossipsub messages.
///
/// Routes messages on the BEEMESH_FABRIC topic to the appropriate scheduler.
/// Messages on other topics are logged and dropped.
///
/// # Arguments
///
/// * `peer_id` - The peer that sent the message
/// * `local_peer_id` - This node's peer ID (for scheduler routing)
/// * `message` - The gossipsub message containing scheduler data
/// * `topic` - The topic the message was published on
pub fn gossipsub_message(
    peer_id: libp2p::PeerId,
    local_peer_id: libp2p::PeerId,
    message: gossipsub::Message,
    topic: gossipsub::TopicHash,
) {
    debug!("received message from {}", peer_id);
    let payload = &message.data;

    // Cache the topic hash for the scheduler fabric
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
            warn!(
                "Scheduler input channel not initialized for peer {}, dropping message",
                local_peer_id
            );
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

/// Handles peer subscription events.
pub fn gossipsub_subscribed(_peer_id: libp2p::PeerId, _topic: gossipsub::TopicHash) {}

/// Handles peer unsubscription events.
pub fn gossipsub_unsubscribed(peer_id: libp2p::PeerId, topic: gossipsub::TopicHash) {
    log::info!("Peer {peer_id} unsubscribed from topic: {topic}");
}

// ============================================================================
// Kademlia Event Handlers
// ============================================================================

/// Handles Kademlia DHT events.
///
/// Processes various DHT operations including:
///
/// - **GetClosestPeers**: Random walk discovery results for MDHT
/// - **GetRecord/PutRecord**: DHT record operations
/// - **Bootstrap**: Initial DHT population progress
/// - **StartProviding**: Provider record announcements
/// - **RoutingUpdated**: K-bucket changes from peer discovery
///
/// All log messages are prefixed with "MDHT:" to distinguish them from
/// other network events.
///
/// # Spec Reference
///
/// - §3: MDHT Discovery via Kademlia random walks
pub fn kademlia_event(event: kad::Event, _peer_id: Option<PeerId>) {
    match event {
        kad::Event::OutboundQueryProgressed {
            id,
            result,
            step: _,
            stats: _,
        } => match result {
            // Random walk discovery completed successfully
            kad::QueryResult::GetClosestPeers(Ok(kad::GetClosestPeersOk { key, peers })) => {
                if peers.is_empty() {
                    debug!("MDHT: Random walk query {:?} found no new peers", id);
                } else {
                    info!(
                        "MDHT: Random walk discovered {} peer(s) near key {:?}",
                        peers.len(),
                        key
                    );
                    for peer in &peers {
                        debug!("MDHT: Discovered peer: {:?}", peer);
                    }
                }
            }
            // Random walk timed out (may still have partial results)
            kad::QueryResult::GetClosestPeers(Err(kad::GetClosestPeersError::Timeout { key, peers })) => {
                debug!(
                    "MDHT: Random walk timed out for key {:?}, found {} peer(s)",
                    key,
                    peers.len()
                );
            }
            // DHT record retrieval successful
            kad::QueryResult::GetRecord(Ok(kad::GetRecordOk::FoundRecord(kad::PeerRecord {
                record,
                peer: _,
            }))) => {
                info!(
                    "MDHT: Retrieved record with key: {:?} from query: {:?}",
                    record.key, id
                );
            }
            kad::QueryResult::GetRecord(Err(e)) => {
                warn!("MDHT: Failed to get record for query {:?}: {:?}", id, e);
            }
            // DHT record storage successful
            kad::QueryResult::PutRecord(Ok(kad::PutRecordOk { key })) => {
                info!("MDHT: Successfully stored record with key: {:?}", key);
            }
            kad::QueryResult::PutRecord(Err(e)) => {
                debug!("MDHT: Failed to store record: {:?}", e);
            }
            // Bootstrap progress updates
            kad::QueryResult::Bootstrap(Ok(kad::BootstrapOk { peer, num_remaining })) => {
                info!(
                    "MDHT: Bootstrap progress - peer {} completed, {} remaining",
                    peer, num_remaining
                );
            }
            kad::QueryResult::Bootstrap(Err(e)) => {
                warn!("MDHT: Bootstrap failed: {:?}", e);
            }
            // Provider announcements
            kad::QueryResult::StartProviding(Ok(_)) => {
                debug!("MDHT: Successfully started providing for query {:?}", id);
            }
            kad::QueryResult::StartProviding(Err(e)) => {
                warn!("MDHT: Failed to start providing for query {:?}: {:?}", id, e);
            }
            _ => {
                debug!("MDHT: Other query result: {:?}", result);
            }
        },
        // K-bucket routing table updates
        kad::Event::RoutingUpdated {
            peer,
            is_new_peer,
            addresses,
            bucket_range: _,
            old_peer,
        } => {
            if is_new_peer {
                info!(
                    "MDHT: New peer {} added to routing table with {} address(es)",
                    peer,
                    addresses.len()
                );
            } else {
                debug!("MDHT: Existing peer {} updated in routing table", peer);
            }
            if let Some(evicted) = old_peer {
                debug!("MDHT: Evicted old peer {} from bucket", evicted);
            }
        }
        kad::Event::UnroutablePeer { peer } => {
            debug!("MDHT: Peer {} is unroutable", peer);
        }
        kad::Event::RoutablePeer { peer, address } => {
            info!("MDHT: Peer {} is routable at {}", peer, address);
        }
        kad::Event::PendingRoutablePeer { peer, address } => {
            debug!("MDHT: Peer {} is pending routable at {}", peer, address);
        }
        kad::Event::InboundRequest { request } => match request {
            kad::InboundRequest::GetRecord {
                num_closer_peers,
                present_locally,
            } => {
                debug!(
                    "MDHT: Received GetRecord request (closer_peers: {}, local: {})",
                    num_closer_peers, present_locally
                );
            }
            kad::InboundRequest::PutRecord {
                source,
                connection: _,
                record,
            } => {
                debug!(
                    "MDHT: Received PutRecord from {} for key: {:?}",
                    source,
                    record.as_ref().map(|r| &r.key)
                );
            }
            kad::InboundRequest::FindNode { num_closer_peers } => {
                debug!("MDHT: Received FindNode request, returning {} closer peers", num_closer_peers);
            }
            _ => {
                debug!("MDHT: Other inbound request: {:?}", request);
            }
        },
        kad::Event::ModeChanged { new_mode } => {
            info!("MDHT: Mode changed to {:?}", new_mode);
        }
    }
}

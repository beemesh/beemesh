use crate::messages::constants::BEEMESH_FABRIC;
use crate::network::control;
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
) {
    debug!("received message from {}", peer_id);
    let payload = &message.data;

    static FABRIC_TOPIC: OnceLock<gossipsub::TopicHash> = OnceLock::new();

    let scheduler_topic =
        FABRIC_TOPIC.get_or_init(|| gossipsub::IdentTopic::new(BEEMESH_FABRIC).hash());

    if topic == *scheduler_topic {
        debug!(
            "gossipsub: Forwarding scheduler message on topic {} from peer {} ({} bytes)",
            topic,
            peer_id,
            payload.len()
        );
        if let Some(tx) = SCHEDULER_INPUT_TX.get() {
            if let Err(e) = tx.send((topic, message)) {
                error!("Failed to forward scheduler message: {}", e);
            }
        } else {
            warn!("Scheduler input channel not initialized, dropping message");
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
                    let _ = tx.send(Vec::new());
                }
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

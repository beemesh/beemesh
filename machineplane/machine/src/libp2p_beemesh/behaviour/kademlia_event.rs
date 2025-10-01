use libp2p::{kad, PeerId};
use log::{info, warn, debug};

/// Handle Kademlia DHT events
pub fn kademlia_event(event: kad::Event, peer_id: Option<PeerId>) {
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
                    record.key,
                    id
                );
                // TODO: Process the retrieved AppliedManifest
            }
            kad::QueryResult::GetRecord(Err(e)) => {
                warn!("DHT: Failed to get record for query {:?}: {:?}", id, e);
            }
            kad::QueryResult::PutRecord(Ok(kad::PutRecordOk { key })) => {
                info!("DHT: Successfully stored record with key: {:?}", key);
            }
            kad::QueryResult::PutRecord(Err(e)) => {
                warn!("DHT: Failed to store record: {:?}", e);
            }
            kad::QueryResult::Bootstrap(Ok(_)) => {
                info!("DHT: Bootstrap completed successfully");
            }
            kad::QueryResult::Bootstrap(Err(e)) => {
                warn!("DHT: Bootstrap failed: {:?}", e);
            }
            _ => {
                debug!("DHT: Other query result: {:?}", result);
            }
        },
        kad::Event::RoutingUpdated {
            peer,
            is_new_peer,
            addresses,
            bucket_range: _,
            old_peer: _,
        } => {
            if is_new_peer {
                info!(
                    "DHT: Added new peer {} with addresses: {:?}",
                    peer, addresses
                );
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
        kad::Event::InboundRequest { request } => {
            match request {
                kad::InboundRequest::GetRecord { num_closer_peers, present_locally } => {
                    debug!("DHT: Received GetRecord request (closer_peers: {}, local: {})", num_closer_peers, present_locally);
                }
                kad::InboundRequest::PutRecord { source, connection: _, record } => {
                    info!("DHT: Received PutRecord from {} for key: {:?}", source, record.as_ref().map(|r| &r.key));
                }
                _ => {
                    debug!("DHT: Other inbound request: {:?}", request);
                }
            }
        }
        kad::Event::ModeChanged { new_mode } => {
            info!("DHT: Mode changed to {:?}", new_mode);
        }
    }
}
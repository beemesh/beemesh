use crate::libp2p_beemesh::control;
use libp2p::{kad, PeerId};
use log::{debug, info, warn};

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
                // If there is a pending control query waiting for this kademlia QueryId, reply with bytes
                if let Some(tx) = control::take_pending_kad_query(&id) {
                    let _ = tx.send(Ok(Some(record.value.clone())));
                }
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
                if let Some(tx) = control::take_pending_kad_query(&id) {
                    let _ = tx.send(Err(format!("kademlia get_record error: {:?}", e)));
                }
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

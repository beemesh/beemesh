use libp2p::{kad, PeerId};

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
                println!(
                    "DHT: Retrieved record with key: {:?} from query: {:?}",
                    record.key,
                    id
                );
                // TODO: Process the retrieved AppliedManifest
            }
            kad::QueryResult::GetRecord(Err(e)) => {
                println!("DHT: Failed to get record for query {:?}: {:?}", id, e);
            }
            kad::QueryResult::PutRecord(Ok(kad::PutRecordOk { key })) => {
                println!("DHT: Successfully stored record with key: {:?}", key);
            }
            kad::QueryResult::PutRecord(Err(e)) => {
                println!("DHT: Failed to store record: {:?}", e);
            }
            kad::QueryResult::Bootstrap(Ok(_)) => {
                println!("DHT: Bootstrap completed successfully");
            }
            kad::QueryResult::Bootstrap(Err(e)) => {
                println!("DHT: Bootstrap failed: {:?}", e);
            }
            _ => {
                println!("DHT: Other query result: {:?}", result);
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
                println!(
                    "DHT: Added new peer {} with addresses: {:?}",
                    peer, addresses
                );
            }
        }
        kad::Event::UnroutablePeer { peer } => {
            println!("DHT: Peer {} is unroutable", peer);
        }
        kad::Event::RoutablePeer { peer, address } => {
            println!("DHT: Peer {} is routable at {}", peer, address);
        }
        kad::Event::PendingRoutablePeer { peer, address } => {
            println!("DHT: Peer {} is pending routable at {}", peer, address);
        }
        kad::Event::InboundRequest { request } => {
            match request {
                kad::InboundRequest::GetRecord { num_closer_peers, present_locally } => {
                    println!("DHT: Received GetRecord request (closer_peers: {}, local: {})", num_closer_peers, present_locally);
                }
                kad::InboundRequest::PutRecord { source, connection: _, record } => {
                    println!("DHT: Received PutRecord from {} for key: {:?}", source, record.as_ref().map(|r| &r.key));
                }
                _ => {
                    println!("DHT: Other inbound request: {:?}", request);
                }
            }
        }
        kad::Event::ModeChanged { new_mode } => {
            println!("DHT: Mode changed to {:?}", new_mode);
        }
    }
}
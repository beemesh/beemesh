use libp2p::{request_response, PeerId};
use log::{info, warn};

use crate::libp2p_beemesh::manifest_announcement::{ManifestHolderQuery, ManifestHolderResponse};

/// Handle manifest announcement request/response messages
pub fn manifest_announcement_message(
    message: request_response::Message<Vec<u8>, Vec<u8>>,
    peer: PeerId,
    swarm: &mut libp2p::Swarm<super::MyBehaviour>,
) {
    match message {
        request_response::Message::Request {
            request, channel, ..
        } => {
            info!("libp2p: received manifest holder query from peer={}", peer);

            // Try to parse the request as ManifestHolderQuery
            match serde_json::from_slice::<ManifestHolderQuery>(&request) {
                Ok(query) => {
                    info!(
                        "libp2p: parsed manifest holder query for manifest_id={} version={:?}",
                        query.manifest_id, query.version
                    );

                    // Generate response using global holder store
                    let response = {
                        let store = crate::libp2p_beemesh::control::get_manifest_holder_store()
                            .lock()
                            .unwrap();
                        store.handle_holder_query(&query)
                    };
                    let response_bytes = match serde_json::to_vec(&response) {
                        Ok(bytes) => bytes,
                        Err(e) => {
                            warn!("Failed to serialize manifest holder response: {}", e);
                            return;
                        }
                    };

                    // Send response
                    if let Err(e) = swarm
                        .behaviour_mut()
                        .manifest_announcement_rr
                        .send_response(channel, response_bytes)
                    {
                        warn!("Failed to send manifest holder response: {:?}", e);
                    }
                }
                Err(e) => {
                    warn!(
                        "Failed to parse manifest holder query from peer {}: {}",
                        peer, e
                    );
                }
            }
        }
        request_response::Message::Response { response, .. } => {
            info!(
                "libp2p: received manifest holder response from peer={}",
                peer
            );

            // Try to parse the response as ManifestHolderResponse
            match serde_json::from_slice::<ManifestHolderResponse>(&response) {
                Ok(holder_response) => {
                    info!(
                        "libp2p: parsed manifest holder response with {} holders, latest_version={:?}",
                        holder_response.holders.len(),
                        holder_response.latest_version
                    );

                    // Update our local holder store with the received information
                    for holder in &holder_response.holders {
                        // Only update if it's not our own peer
                        if holder.peer_id != peer.to_string() {
                            // For now, we'll skip updating remote holder info since we don't have manifest_id
                            // In a full implementation, we'd need to track the query context
                            info!("Received holder info from peer {}: {:?}", peer, holder);
                        }
                    }

                    // Process the response for any pending queries
                    crate::libp2p_beemesh::control::handle_manifest_holder_response(
                        &peer,
                        &holder_response,
                    );
                }
                Err(e) => {
                    warn!(
                        "Failed to parse manifest holder response from peer {}: {}",
                        peer, e
                    );
                }
            }
        }
    }
}

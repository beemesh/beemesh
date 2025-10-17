use base64::Engine;
use libp2p::{PeerId, Swarm};
use log::{info, warn};
use tokio::sync::mpsc;

use crate::libp2p_beemesh::behaviour::MyBehaviour;

/// Handle sending manifest data to a specific peer
pub async fn handle_send_manifest(
    peer_id: PeerId,
    manifest_id: String,
    manifest_payload: Vec<u8>,
    reply_tx: mpsc::UnboundedSender<Result<(), String>>,
    swarm: &mut Swarm<MyBehaviour>,
) {
    warn!("libp2p: control SendManifest for peer={}", peer_id);

    // Single delivery format enforced: expect raw envelope bytes (no base64 variants).

    // Check if this is a self-send - handle locally instead of using RequestResponse
    if peer_id == *swarm.local_peer_id() {
        warn!(
            "libp2p: handling self-send manifest locally for peer {}",
            peer_id
        );

        // For self-send, directly store the manifest in the local manifest store
        info!(
            "libp2p: processing self-send manifest directly, payload len={}",
            manifest_payload.len()
        );

        // Parse the manifest envelope and store it locally
        if let Ok(envelope) = protocol::machine::root_as_envelope(&manifest_payload) {
            warn!("libp2p: self-send parsed envelope successfully");

            // Extract payload from envelope
            let payload_bytes = envelope
                .payload()
                .map(|v| v.iter().collect::<Vec<u8>>())
                .unwrap_or_default();
            let payload_type = envelope.payload_type().unwrap_or("");

            if !payload_bytes.is_empty() && payload_type == "manifest" {
                warn!(
                    "libp2p: self-send storing manifest, type={}, len={}",
                    payload_type,
                    payload_bytes.len()
                );

                // Enforce single delivery format: payload must be raw envelope bytes (no base64).
                let manifest_data = payload_bytes.clone();
                warn!(
                    "libp2p: self-send manifest prepared for storage, manifest_id={}, data len={}",
                    manifest_id,
                    manifest_data.len()
                );

                // Create manifest entry for storage using the provided manifest_id
                let manifest_entry = crate::libp2p_beemesh::manifest_store::ManifestEntry {
                    manifest_id: manifest_id.clone(),
                    version: 1,
                    encrypted_data: manifest_data,
                    stored_at: crate::libp2p_beemesh::manifest_store::current_timestamp(),
                    access_tokens: vec![],
                    owner_pubkey: vec![],
                };

                // Store the manifest using the global manifest store
                let mut manifest_store = crate::libp2p_beemesh::control::get_local_manifest_store()
                    .lock()
                    .unwrap();

                match manifest_store.store_manifest(manifest_entry) {
                    Ok(()) => {
                        warn!(
                            "libp2p: self-send successfully stored manifest {} locally",
                            manifest_id
                        );
                    }
                    Err(e) => {
                        warn!(
                            "libp2p: self-send failed to store manifest {}: {:?}",
                            manifest_id, e
                        );
                    }
                }
            } else {
                warn!("libp2p: self-send envelope has empty payload or wrong type");
            }
        } else {
            warn!("libp2p: self-send failed to parse manifest_payload as envelope");
        }

        warn!("libp2p: self-send manifest processing completed");
        let _ = reply_tx.send(Ok(()));
        return;
    }

    // Check current connections for debugging
    let connected_peers: Vec<_> = swarm.connected_peers().cloned().collect();
    warn!("libp2p: current connected peers: {:?}", connected_peers);

    // Check connection status
    warn!(
        "libp2p: peer {} connection status: connected={}",
        peer_id,
        swarm.is_connected(&peer_id)
    );

    // Try to establish connection first
    if !swarm.is_connected(&peer_id) {
        warn!("libp2p: peer {} not connected, attempting to dial", peer_id);
        match swarm.dial(peer_id) {
            Ok(_) => {
                warn!("libp2p: dial initiated for peer {}", peer_id);
            }
            Err(e) => {
                warn!("libp2p: failed to dial peer {}: {:?}", peer_id, e);
                let _ = reply_tx.send(Err(format!("dial failed: {:?}", e)));
                return;
            }
        }

        // Give more time for the connection to establish, retry a few times
        for attempt in 1..=3 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            if swarm.is_connected(&peer_id) {
                warn!(
                    "libp2p: peer {} connected after attempt {}",
                    peer_id, attempt
                );
                break;
            }
            warn!(
                "libp2p: peer {} still not connected after attempt {}",
                peer_id, attempt
            );
        }

        if !swarm.is_connected(&peer_id) {
            warn!(
                "libp2p: peer {} still not connected after all attempts",
                peer_id
            );
            let _ = reply_tx.send(Err(format!("connection timeout after dial")));
            return;
        }
    }

    // Include manifest_id in payload for network sends (format: "manifest_id:base64_data")
    let payload_with_id = format!(
        "{}:{}",
        manifest_id,
        base64::engine::general_purpose::STANDARD.encode(&manifest_payload)
    );

    // Deliver the manifest payload using the manifest_fetch request-response behaviour
    let request_id = swarm
        .behaviour_mut()
        .manifest_fetch_rr
        .send_request(&peer_id, payload_with_id.into_bytes());

    warn!(
        "libp2p: sent manifest request {} to peer {}",
        request_id, peer_id
    );

    // Store the reply channel to send response when we get confirmation
    let mut pending_requests = crate::libp2p_beemesh::control::get_pending_manifest_requests()
        .lock()
        .unwrap();
    pending_requests.insert(request_id, (reply_tx, std::time::Instant::now()));
}

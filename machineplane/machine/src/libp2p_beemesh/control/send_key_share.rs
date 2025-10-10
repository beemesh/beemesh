use libp2p::{PeerId, Swarm};
use tokio::sync::mpsc;
use log::{info, warn};
use base64::Engine;

use crate::libp2p_beemesh::behaviour::MyBehaviour;

/// New API: accept payload bytes (flatbuffer envelope, flatbuffer KeyShareRequest, or recipient blob).
pub async fn handle_send_key_share(
    peer_id: PeerId,
    request_bytes: Vec<u8>,
    reply_tx: mpsc::UnboundedSender<Result<(), String>>,
    swarm: &mut Swarm<MyBehaviour>,
) {
    warn!("libp2p: control SendKeyShare for peer={}", peer_id);

    // Check if this is a self-send - handle locally instead of using RequestResponse
    if peer_id == *swarm.local_peer_id() {
        warn!("libp2p: handling self-send locally for peer {}", peer_id);

        // For self-send, perform the same processing as keyshare_message would for incoming bytes.
        // Reuse the existing handler logic by calling the behaviour handler path indirectly: send as a local request
        // (keep behaviour consistent). We send the bytes to our local RequestResponse handler.
        let request_id = swarm.behaviour_mut().keyshare_rr.send_request(&peer_id, request_bytes.clone());
        info!("libp2p: enqueued local self-send keyshare request_id={:?}", request_id);
        let _ = reply_tx.send(Ok(()));
        return;
    }

    // Check current connections for debugging
    let connected_peers: Vec<_> = swarm.connected_peers().cloned().collect();
    warn!("libp2p: current connected peers: {:?}", connected_peers);

    // Check connection status
    warn!("libp2p: peer {} connection status: connected={}", peer_id, swarm.is_connected(&peer_id));

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
                warn!("libp2p: peer {} connected after attempt {}", peer_id, attempt);
                break;
            }
            warn!("libp2p: peer {} still not connected after attempt {}", peer_id, attempt);
        }

        if !swarm.is_connected(&peer_id) {
            warn!("libp2p: peer {} still not connected after all attempts", peer_id);
            let _ = reply_tx.send(Err(format!("connection timeout after dial")));
            return;
        }
    }

    // Deliver the payload bytes using the dedicated keyshare request-response behaviour.
    let request_id = swarm.behaviour_mut().keyshare_rr.send_request(&peer_id, request_bytes);
    info!("libp2p: sent keyshare to peer={} request_id={:?}", peer_id, request_id);

    let _ = reply_tx.send(Ok(()));
}

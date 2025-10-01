use log::{debug, info, error};
use protocol::libp2p_constants::REQUEST_RESPONSE_TIMEOUT_SECS;
use serde_json;
use libp2p::PeerId;
use tokio::sync::mpsc;

/// Send the manifest to a peer using libp2p request-response protocol.
/// This function sends an apply request FlatBuffer to the specified peer and waits for a response.
pub async fn send_apply_to_peer(
    peer: &str, 
    manifest: &serde_json::Value,
    control_tx: &mpsc::UnboundedSender<crate::libp2p_beemesh::control::Libp2pControl>
) -> Result<(), String> {
    log::debug!("send_apply_to_peer: sending manifest to peer {}: {}", peer, manifest);
    
    // Parse the peer string into a PeerId
    let peer_id: PeerId = peer.parse()
        .map_err(|e| format!("invalid peer ID '{}': {}", peer, e))?;
    
    // Create a channel to receive the response
    let (reply_tx, mut reply_rx) = mpsc::unbounded_channel::<Result<String, String>>();
    
    // Send the apply request via libp2p
    let control_msg = crate::libp2p_beemesh::control::Libp2pControl::SendApplyRequest {
        peer_id,
        manifest: manifest.clone(),
        reply_tx,
    };
    
    control_tx.send(control_msg)
        .map_err(|e| format!("failed to send control message: {}", e))?;
    
    // Wait for the response with a timeout
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(REQUEST_RESPONSE_TIMEOUT_SECS),
        reply_rx.recv()
    ).await
    .map_err(|_| "timeout waiting for apply response".to_string())?
    .ok_or_else(|| "control channel closed".to_string())?;
    
    match response {
        Ok(msg) => {
            log::info!("send_apply_to_peer: success - {}", msg);
            Ok(())
        }
        Err(err) => {
            log::error!("send_apply_to_peer: error - {}", err);
            Err(err)
        }
    }
}

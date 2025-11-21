use libp2p::{PeerId, Swarm};
use log::{debug, info};
use tokio::sync::mpsc;

use crate::network::behaviour::MyBehaviour;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Handle SendApplyRequest control message
pub async fn handle_send_apply_request(
    peer_id: PeerId,
    manifest: Vec<u8>,
    reply_tx: mpsc::UnboundedSender<Result<String, String>>,
    swarm: &mut Swarm<MyBehaviour>,
) {
    info!(
        "libp2p: control SendApplyRequest received for peer={}",
        peer_id
    );

    // Check if this is a self-send - handle locally instead of using RequestResponse
    if peer_id == *swarm.local_peer_id() {
        debug!("libp2p: handling self-apply locally for peer {}", peer_id);

        // Use the new workload manager integration for self-apply as well
        crate::scheduler::process_enhanced_self_apply_request(&manifest, swarm).await;

        let _ = reply_tx.send(Ok(format!("Apply request handled locally for {}", peer_id)));
        return;
    }

    // For remote peers, use the normal RequestResponse protocol
    // Before sending the apply, create a capability token tied to this manifest and
    // send it to the target peer so they can store it in their keystore.
    // Compute a manifest_id deterministically when possible (follow same heuristic as apply_message)
    let _manifest_id =
        if let Ok(apply_req) = crate::messages::machine::root_as_apply_request(&manifest) {
            if !apply_req.operation_id.is_empty() && !apply_req.manifest_json.is_empty() {
                let mut hasher = DefaultHasher::new();
                apply_req.operation_id.hash(&mut hasher);
                apply_req.manifest_json.hash(&mut hasher);
                format!("{:x}", hasher.finish())
            } else {
                // Fallback to hashing raw bytes
                let mut hasher = DefaultHasher::new();
                manifest.hash(&mut hasher);
                format!("{:x}", hasher.finish())
            }
        } else {
            let mut hasher = DefaultHasher::new();
            manifest.hash(&mut hasher);
            format!("{:x}", hasher.finish())
        };

    // No capability token needed - direct manifest application

    // Finally send the apply request
    let request_id = swarm
        .behaviour_mut()
        .apply_rr
        .send_request(&peer_id, manifest);
    info!(
        "libp2p: sent apply request to peer={} request_id={:?}",
        peer_id, request_id
    );

    // For now, just send success immediately - proper response handling is done elsewhere
    let _ = reply_tx.send(Ok(format!("Apply request sent to {}", peer_id)));
}

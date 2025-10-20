use libp2p::{PeerId, Swarm};
use log::{debug, info};
use tokio::sync::mpsc;

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::libp2p_beemesh::behaviour::MyBehaviour;
use crypto;

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
        crate::workload_integration::process_enhanced_self_apply_request(
            &manifest,
            swarm,
        ).await;

        let _ = reply_tx.send(Ok(format!("Apply request handled locally for {}", peer_id)));
        return;
    }

    // For remote peers, use the normal RequestResponse protocol
    // Before sending the apply, create a capability token tied to this manifest and
    // send it to the target peer so they can store it in their keystore.
    // Compute a manifest_id deterministically when possible (follow same heuristic as apply_message)
    let _manifest_id = if let Ok(apply_req) = protocol::machine::root_as_apply_request(&manifest) {
        if let (Some(tenant), Some(operation_id), Some(manifest_json)) = (
            apply_req.tenant(),
            apply_req.operation_id(),
            apply_req.manifest_json(),
        ) {
            let mut hasher = DefaultHasher::new();
            tenant.hash(&mut hasher);
            operation_id.hash(&mut hasher);
            manifest_json.hash(&mut hasher);
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

    // Sign the apply request in an envelope before sending
    let signed_apply_request = if let Ok((pubb, privb)) = crypto::ensure_keypair_on_disk() {
        let nonce = uuid::Uuid::new_v4().to_string();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0u64);
        let payload_type = "apply_request";
        let alg = "ml-dsa-65";

        // Create canonical bytes and sign
        let canonical_bytes = protocol::machine::build_envelope_canonical(
            &manifest,
            payload_type,
            &nonce,
            timestamp,
            alg,
            None,
        );

        match crypto::sign_envelope(&privb, &pubb, &canonical_bytes) {
            Ok((sig_b64, pub_b64)) => {
                // Build signed envelope
                protocol::machine::build_envelope_signed(
                    &manifest,
                    payload_type,
                    &nonce,
                    timestamp,
                    alg,
                    "ml-dsa-65",
                    &sig_b64,
                    &pub_b64,
                    None,
                )
            }
            Err(e) => {
                log::warn!("failed to sign apply request for peer {}: {:?}", peer_id, e);
                manifest // fallback to unsigned
            }
        }
    } else {
        log::warn!(
            "failed to load keypair for signing apply request to peer {}",
            peer_id
        );
        manifest // fallback to unsigned
    };

    // Finally send the (now signed) apply request
    let request_id = swarm
        .behaviour_mut()
        .apply_rr
        .send_request(&peer_id, signed_apply_request);
    info!(
        "libp2p: sent apply request to peer={} request_id={:?}",
        peer_id, request_id
    );

    // For now, just send success immediately - proper response handling is done elsewhere
    let _ = reply_tx.send(Ok(format!("Apply request sent to {}", peer_id)));
}

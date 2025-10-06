use libp2p::{PeerId, Swarm};
use tokio::sync::mpsc;
use log::{info, warn};
use base64::Engine;

use crate::libp2p_beemesh::behaviour::MyBehaviour;

pub async fn handle_send_key_share(
    peer_id: PeerId,
    share_payload: serde_json::Value,
    reply_tx: mpsc::UnboundedSender<Result<(), String>>,
    swarm: &mut Swarm<MyBehaviour>,
) {
    warn!("libp2p: control SendKeyShare for peer={}", peer_id);

    // Check if this is a self-send - handle locally instead of using RequestResponse
    if peer_id == *swarm.local_peer_id() {
        warn!("libp2p: handling self-send locally for peer {}", peer_id);
        
        // Handle the keyshare locally without going through RequestResponse
        // This is the same logic as in keyshare_message.rs but without the channel response
        match serde_json::to_vec(&share_payload) {
            Ok(request_bytes) => {
                warn!("libp2p: processing self-send keyshare payload size={}", request_bytes.len());
                
                // Check if this is a capability token (has "type": "capability")
                if let Some(type_val) = share_payload.get("type").and_then(|v| v.as_str()) {
                    if type_val == "capability" {
                        // Process capability token - same logic as keyshare_message.rs
                        if let Some(manifest_id) = share_payload.get("manifest_id").and_then(|v| v.as_str()) {
                            if let Ok(signed_bytes) = serde_json::to_vec(&share_payload) {
                                if let Ok((blob, cid)) = crypto::encrypt_share_for_keystore(&signed_bytes) {
                                    if let Ok(ks) = crate::libp2p_beemesh::open_keystore() {
                                        let meta = format!("capability:{}", manifest_id);
                                        match ks.put(&cid, &blob, Some(&meta)) {
                                            Ok(()) => {
                                                warn!("libp2p: stored self-send capability for manifest_id={} cid={}", manifest_id, cid);
                                                // Announce provider for discovered capability holders
                                                let manifest_provider_cid = format!("manifest:{}", manifest_id);
                                                let (reply_tx_inner, _rx) = tokio::sync::mpsc::unbounded_channel();
                                                let ctrl = crate::libp2p_beemesh::control::Libp2pControl::AnnounceProvider { cid: manifest_provider_cid.clone(), ttl_ms: 3000, reply_tx: reply_tx_inner };
                                                crate::libp2p_beemesh::control::enqueue_control(ctrl);
                                            }
                                            Err(e) => {
                                                warn!("keystore put failed for capability cid {}: {:?}", cid, e);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        let _ = reply_tx.send(Ok(()));
                        return;
                    }
                }
                
                // Extract and process the share (same as keyshare_message fallback logic)
                if let Some(share_val) = share_payload.get("share").and_then(|v| v.as_str()) {
                    // Also extract manifest_id for metadata storage
                    let manifest_id = share_payload.get("manifest_id").and_then(|v| v.as_str()).unwrap_or("unknown");
                    match base64::engine::general_purpose::STANDARD.decode(share_val) {
                        Ok(payload_bytes) => {
                            match crypto::encrypt_share_for_keystore(&payload_bytes) {
                                Ok((blob, cid)) => {
                                    match crate::libp2p_beemesh::open_keystore() {
                                        Ok(ks) => {
                                            if let Err(e) = ks.put(&cid, &blob, Some(manifest_id)) {
                                                warn!("keystore put failed for cid {}: {:?}", cid, e);
                                            } else {
                                                warn!("keystore: stored self-send keyshare cid={} with manifest_id={}", cid, manifest_id);
                                                
                                                // Announce the provider (same as remote case)
                                                let (reply_tx_inner, _rx) = tokio::sync::mpsc::unbounded_channel();
                                                let ctrl = crate::libp2p_beemesh::control::Libp2pControl::AnnounceProvider { 
                                                    cid: cid.clone(), 
                                                    ttl_ms: 3000, 
                                                    reply_tx: reply_tx_inner 
                                                };
                                                crate::libp2p_beemesh::control::enqueue_control(ctrl);
                                            }
                                        }
                                        Err(e) => warn!("could not open keystore for self-send: {:?}", e),
                                    }
                                }
                                Err(e) => warn!("failed to encrypt share for keystore (self-send): {:?}", e),
                            }
                        }
                        Err(e) => warn!("failed to base64-decode self-send share: {:?}", e),
                    }
                } else {
                    warn!("self-send keyshare missing 'share' field");
                }
            }
            Err(e) => warn!("failed to serialize self-send payload: {:?}", e),
        }
        
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

    // Deliver the share payload using the dedicated keyshare request-response behaviour.
    let request_bytes = serde_json::to_vec(&share_payload).unwrap_or_default();
    warn!("libp2p: SendKeyShare payload json={} bytes_len={}", match serde_json::to_string(&share_payload) { Ok(s) => s, Err(_) => "<serialize-failed>".to_string() }, request_bytes.len());
    let request_id = swarm.behaviour_mut().keyshare_rr.send_request(&peer_id, request_bytes);
    info!("libp2p: sent keyshare to peer={} request_id={:?}", peer_id, request_id);

    let _ = reply_tx.send(Ok(()));
}

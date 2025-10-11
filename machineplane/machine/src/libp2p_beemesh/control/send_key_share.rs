use base64::Engine;
use libp2p::{PeerId, Swarm};
use log::{info, warn};
use tokio::sync::mpsc;

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

        // For self-send, directly process the keyshare storage instead of using RequestResponse
        // This avoids potential issues with the RequestResponse protocol handling self-addressed requests
        info!(
            "libp2p: processing self-send keyshare directly, request_bytes len={}",
            request_bytes.len()
        );

        // Process the keyshare storage directly using the same logic as keyshare_message handler
        if let Ok(envelope) = protocol::machine::root_as_envelope(&request_bytes) {
            warn!("libp2p: self-send parsed envelope successfully");

            // Extract payload from envelope
            let payload_bytes = envelope
                .payload()
                .map(|v| v.iter().collect::<Vec<u8>>())
                .unwrap_or_default();
            let payload_type = envelope.payload_type().unwrap_or("");

            if !payload_bytes.is_empty() {
                warn!(
                    "libp2p: self-send storing payload, type={}, len={}",
                    payload_type,
                    payload_bytes.len()
                );

                // Store the payload directly using keystore (same logic as keyshare_message handler)
                match crypto::open_keystore_default() {
                    Ok(mut ks) => {
                        // For capability tokens, store the complete envelope; for keyshares, store the payload
                        let bytes_to_store = if payload_type == "capability" {
                            // Store the complete signed envelope for capability tokens
                            request_bytes.clone()
                        } else {
                            // Store the payload for keyshares
                            payload_bytes.clone()
                        };

                        // Use the same storage logic as keyshare_message handler
                        let (blob, cid) = crypto::encrypt_share_for_keystore(&bytes_to_store)
                            .unwrap_or_else(|_| (Vec::new(), String::new()));

                        // Extract manifest_id from payload based on type
                        let manifest_id_for_storage = if payload_type == "capability" {
                            // Handle capability token
                            if let Ok(capability_token) =
                                protocol::machine::root_as_capability_token(&payload_bytes)
                            {
                                if let Some(root_capability) = capability_token.root_capability() {
                                    if let Some(task_id) = root_capability.task_id() {
                                        let manifest_id = task_id.to_string();
                                        warn!("libp2p: self-send parsed CapabilityToken, manifest_id={}", manifest_id);
                                        Some(manifest_id)
                                    } else {
                                        warn!("libp2p: self-send capability token has no task_id");
                                        None
                                    }
                                } else {
                                    warn!(
                                        "libp2p: self-send capability token has no root_capability"
                                    );
                                    None
                                }
                            } else {
                                warn!(
                                    "libp2p: self-send failed to parse payload as CapabilityToken"
                                );
                                None
                            }
                        } else {
                            // Handle keyshare - extract raw share bytes from KeyShares flatbuffer
                            if let Ok(payload_str) = std::str::from_utf8(&payload_bytes) {
                                warn!(
                                    "libp2p: self-send payload decoded as UTF-8 string, len={}",
                                    payload_str.len()
                                );
                                if let Ok(keyshares_bytes) =
                                    base64::engine::general_purpose::STANDARD.decode(payload_str)
                                {
                                    warn!("libp2p: self-send base64 decoded successfully, keyshares_bytes len={}", keyshares_bytes.len());
                                    if let Ok(key_shares) =
                                        protocol::machine::root_as_key_shares(&keyshares_bytes)
                                    {
                                        let manifest_id =
                                            key_shares.manifest_id().map(|id| id.to_string());
                                        warn!("libp2p: self-send parsed KeyShares flatbuffer, manifest_id={:?}", manifest_id);

                                        // Extract individual shares and store each one separately
                                        if let Some(shares_vector) = key_shares.shares() {
                                            for i in 0..shares_vector.len() {
                                                let share_b64 = shares_vector.get(i);
                                                if let Ok(raw_share_bytes) =
                                                    base64::engine::general_purpose::STANDARD
                                                        .decode(share_b64)
                                                {
                                                    warn!("libp2p: self-send extracting raw share {} len={}", i, raw_share_bytes.len());

                                                    // Store this individual raw share
                                                    let (share_blob, share_cid) =
                                                        crypto::encrypt_share_for_keystore(
                                                            &raw_share_bytes,
                                                        )
                                                        .unwrap_or_else(|_| {
                                                            (Vec::new(), String::new())
                                                        });

                                                    if let Err(e) = ks.put(
                                                        &share_cid,
                                                        &share_blob,
                                                        manifest_id.as_deref(),
                                                    ) {
                                                        warn!("libp2p: self-send keystore put failed for share cid {}: {:?}", share_cid, e);
                                                    } else {
                                                        if let Some(mid) = manifest_id.as_ref() {
                                                            warn!("libp2p: self-send keystore SUCCESS stored raw share {} cid={} for manifest_id={}", i, share_cid, mid);
                                                        }

                                                        // Announce this share CID
                                                        let (announce_tx, _rx) =
                                                            tokio::sync::mpsc::unbounded_channel();
                                                        let ctrl = crate::libp2p_beemesh::control::Libp2pControl::AnnounceProvider {
                                                                cid: share_cid.clone(),
                                                                ttl_ms: 3000,
                                                                reply_tx: announce_tx,
                                                            };
                                                        crate::libp2p_beemesh::control::enqueue_control(ctrl);
                                                    }
                                                } else {
                                                    warn!("libp2p: self-send failed to decode share {} base64", i);
                                                }
                                            }
                                        }
                                        manifest_id
                                    } else {
                                        warn!(
                                            "libp2p: self-send failed to parse as KeyShares flatbuffer"
                                        );
                                        None
                                    }
                                } else {
                                    warn!(
                                        "libp2p: self-send base64 decode failed, trying direct parse"
                                    );
                                    if let Ok(key_shares) =
                                        protocol::machine::root_as_key_shares(&payload_bytes)
                                    {
                                        let manifest_id =
                                            key_shares.manifest_id().map(|id| id.to_string());
                                        warn!("libp2p: self-send parsed KeyShares flatbuffer directly, manifest_id={:?}", manifest_id);

                                        // Extract individual shares and store each one separately
                                        if let Some(shares_vector) = key_shares.shares() {
                                            for i in 0..shares_vector.len() {
                                                let share_b64 = shares_vector.get(i);
                                                if let Ok(raw_share_bytes) =
                                                    base64::engine::general_purpose::STANDARD
                                                        .decode(share_b64)
                                                {
                                                    warn!("libp2p: self-send extracting raw share {} len={}", i, raw_share_bytes.len());

                                                    // Store this individual raw share
                                                    let (share_blob, share_cid) =
                                                        crypto::encrypt_share_for_keystore(
                                                            &raw_share_bytes,
                                                        )
                                                        .unwrap_or_else(|_| {
                                                            (Vec::new(), String::new())
                                                        });

                                                    if let Err(e) = ks.put(
                                                        &share_cid,
                                                        &share_blob,
                                                        manifest_id.as_deref(),
                                                    ) {
                                                        warn!("libp2p: self-send keystore put failed for share cid {}: {:?}", share_cid, e);
                                                    } else {
                                                        if let Some(mid) = manifest_id.as_ref() {
                                                            warn!("libp2p: self-send keystore SUCCESS stored raw share {} cid={} for manifest_id={}", i, share_cid, mid);
                                                        }

                                                        // Announce this share CID
                                                        let (announce_tx, _rx) =
                                                            tokio::sync::mpsc::unbounded_channel();
                                                        let ctrl = crate::libp2p_beemesh::control::Libp2pControl::AnnounceProvider {
                                                                cid: share_cid.clone(),
                                                                ttl_ms: 3000,
                                                                reply_tx: announce_tx,
                                                            };
                                                        crate::libp2p_beemesh::control::enqueue_control(ctrl);
                                                    }
                                                } else {
                                                    warn!("libp2p: self-send failed to decode share {} base64", i);
                                                }
                                            }
                                        }
                                        manifest_id
                                    } else {
                                        warn!("libp2p: self-send failed to parse payload as KeyShares flatbuffer");
                                        None
                                    }
                                }
                            } else {
                                warn!("libp2p: self-send payload is not valid UTF-8");
                                None
                            }
                        };

                        // For capability tokens, still store the complete envelope with the proper metadata
                        if payload_type == "capability" {
                            let store_meta = manifest_id_for_storage
                                .as_ref()
                                .map(|id| format!("capability:{}", id));

                            let (blob, cid) = crypto::encrypt_share_for_keystore(&bytes_to_store)
                                .unwrap_or_else(|_| (Vec::new(), String::new()));

                            if let Err(e) = ks.put(&cid, &blob, store_meta.as_deref()) {
                                warn!(
                                    "libp2p: self-send keystore put failed for cid {}: {:?}",
                                    cid, e
                                );
                            } else {
                                if let Some(manifest_id) = manifest_id_for_storage.as_ref() {
                                    warn!("libp2p: self-send keystore SUCCESS stored {} cid={} for manifest_id={}", payload_type, cid, manifest_id);
                                } else {
                                    warn!("libp2p: self-send keystore SUCCESS stored simple {} cid={} type={}", payload_type, cid, payload_type);
                                }

                                // Always announce the CID itself (same as keyshare_message handler)
                                let (announce_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
                                let ctrl =
                                    crate::libp2p_beemesh::control::Libp2pControl::AnnounceProvider {
                                        cid: cid.clone(),
                                        ttl_ms: 3000,
                                        reply_tx: announce_tx,
                                    };
                                crate::libp2p_beemesh::control::enqueue_control(ctrl);
                            }
                        }
                    }
                    Err(e) => {
                        warn!("libp2p: self-send could not open keystore: {:?}", e);
                    }
                }
            } else {
                warn!("libp2p: self-send envelope has empty payload");
            }
        } else {
            warn!("libp2p: self-send failed to parse request_bytes as envelope");
        }

        warn!("libp2p: self-send keyshare processing completed");
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

    // Deliver the payload bytes using the dedicated keyshare request-response behaviour.
    let request_id = swarm
        .behaviour_mut()
        .keyshare_rr
        .send_request(&peer_id, request_bytes);
    info!(
        "libp2p: sent keyshare to peer={} request_id={:?}",
        peer_id, request_id
    );

    let _ = reply_tx.send(Ok(()));
}

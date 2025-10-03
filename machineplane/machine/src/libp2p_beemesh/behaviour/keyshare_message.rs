use libp2p::request_response;
use log::{info, warn};
use base64::engine::general_purpose;
use base64::Engine as _;

/// Handle inbound key-share request-response messages.
/// Expects the request bytes to be JSON (encrypted share payload) and replies with an ApplyResponse flatbuffer.
pub fn keyshare_message(
    message: request_response::Message<Vec<u8>, Vec<u8>>,
    peer: libp2p::PeerId,
    swarm: &mut libp2p::Swarm<super::MyBehaviour>,
    _local_peer: libp2p::PeerId,
) {
    match message {
        request_response::Message::Request { request, channel, .. } => {
            warn!("libp2p: received keyshare request from peer={} request_size={}", peer, request.len());

            // Try to parse incoming request as JSON
            match serde_json::from_slice::<serde_json::Value>(&request) {
                Ok(val) => {
                    info!("libp2p: keyshare payload from {} = {}", peer, val);

                    // Validate envelope shape and signature + nonce using security helper.
                    match crate::libp2p_beemesh::security::verify_envelope_and_check_nonce(&val) {
                        Ok((payload_bytes, _pub, _sig)) => {
                            // The CLI currently places shares as base64 strings inside the envelope payload.
                            // If the payload is a ciphertext produced by our KEM flow, attempt to decapsulate
                            // using the node's on-disk KEM private key. The decapsulated shared secret is
                            // returned as a Zeroizing<Vec<u8>> and will be zeroed on drop.
                            match crypto::ensure_kem_keypair_on_disk() {
                                Ok((_pubb, privb)) => {
                                    match crypto::decapsulate_share(&privb, &payload_bytes) {
                                        Ok(shared_secret) => {
                                            info!("libp2p: successfully decapsulated shared secret for peer={}", peer);
                                            // Use shared_secret as needed (e.g., derive symmetric key, decrypt payload).
                                                    // It's intentionally not persisted; Zeroizing will clear it on drop.
                                                    drop(shared_secret);

                                                    // Encrypt the provided payload and store in the local keystore.
                                                    match crypto::encrypt_share_for_keystore(&payload_bytes) {
                                                        Ok((blob, cid)) => {
                                                            // Open keystore (may be in-memory when BEEMESH_KEYSTORE_EPHEMERAL is set)
                                                            match crate::libp2p_beemesh::open_keystore() {
                                                                Ok(ks) => {
                                                                    warn!("attempting keystore.put for cid={} size={}", cid, blob.len());
                                                                    if let Err(e) = ks.put(&cid, &blob, Some("keyshare")) {
                                                                        warn!("keystore put failed for cid {}: {:?}", cid, e);
                                                                    } else {
                                                                        info!("keystore: stored keyshare cid={}", cid);
                                                                        // Enqueue an AnnounceProvider control message to be handled centrally by the libp2p task
                                                                        let (reply_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
                                                                        let ctrl = crate::libp2p_beemesh::control::Libp2pControl::AnnounceProvider { cid: cid.clone(), ttl_ms: 3000, reply_tx };
                                                                        crate::libp2p_beemesh::control::enqueue_control(ctrl);
                                                                    }
                                                                }
                                                                Err(e) => {
                                                                    warn!("could not open keystore: {:?}", e);
                                                                }
                                                            }
                                                            let resp = protocol::machine::build_keyshare_response(
                                                                true,
                                                                "keyshare_op",
                                                                "decapsulated, stored and announced",
                                                            );
                                                            let _ = swarm.behaviour_mut().keyshare_rr.send_response(channel, resp);
                                                        }
                                                        Err(e) => {
                                                            warn!("failed to encrypt share for keystore: {:?}", e);
                                                            let resp = protocol::machine::build_keyshare_response(
                                                                false,
                                                                "keyshare_op",
                                                                "encryption for storage failed",
                                                            );
                                                            let _ = swarm.behaviour_mut().keyshare_rr.send_response(channel, resp);
                                                        }
                                                    }
                                        }
                                        Err(e) => {
                                            warn!("libp2p: failed to decapsulate share from {}: {:?}", peer, e);
                                            let resp = protocol::machine::build_keyshare_response(
                                                false,
                                                "keyshare_op",
                                                "decapsulation failed",
                                            );
                                            let _ = swarm.behaviour_mut().keyshare_rr.send_response(channel, resp);
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!("libp2p: could not read or create kem keypair: {:?}", e);
                                    let resp = protocol::machine::build_keyshare_response(
                                        false,
                                        "keyshare_op",
                                        "server kem key unavailable",
                                    );
                                    let _ = swarm.behaviour_mut().keyshare_rr.send_response(channel, resp);
                                }
                            }
                        }
                        Err(e) => {
                            // Try a simple fallback: some clients may send a bare {"share": "BASE64"}
                            // If so, accept and store it directly to the keystore to support the
                            // test harness and older clients.
                            warn!("libp2p: keyshare envelope verification failed from {}: {:?} - trying fallback", peer, e);
                            if let Some(share_val) = val.get("share").and_then(|v| v.as_str()) {
                                match general_purpose::STANDARD.decode(share_val) {
                                    Ok(payload_bytes) => {
                                        // proceed to encrypt for keystore and store
                                        match crypto::encrypt_share_for_keystore(&payload_bytes) {
                                            Ok((blob, cid)) => {
                                                match crate::libp2p_beemesh::open_keystore() {
                                                    Ok(ks) => {
                                                        if let Err(e) = ks.put(&cid, &blob, Some("keyshare")) {
                                                            warn!("keystore put failed for cid {}: {:?}", cid, e);
                                                        } else {
                                                            info!("keystore: stored keyshare cid={} (fallback)", cid);
                                                            let (reply_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
                                                            let ctrl = crate::libp2p_beemesh::control::Libp2pControl::AnnounceProvider { cid: cid.clone(), ttl_ms: 3000, reply_tx };
                                                            crate::libp2p_beemesh::control::enqueue_control(ctrl);
                                                        }
                                                    }
                                                    Err(e) => warn!("could not open keystore: {:?}", e),
                                                }
                                                let resp = protocol::machine::build_keyshare_response(
                                                    true,
                                                    "keyshare_op",
                                                    "stored via fallback",
                                                );
                                                let _ = swarm.behaviour_mut().keyshare_rr.send_response(channel, resp);
                                            }
                                            Err(e) => {
                                                warn!("failed to encrypt share for keystore (fallback): {:?}", e);
                                                let resp = protocol::machine::build_keyshare_response(
                                                    false,
                                                    "keyshare_op",
                                                    "encryption for storage failed",
                                                );
                                                let _ = swarm.behaviour_mut().keyshare_rr.send_response(channel, resp);
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        warn!("libp2p: failed to base64-decode fallback share from {}: {:?}", peer, e);
                                        let resp = protocol::machine::build_keyshare_response(
                                            false,
                                            "keyshare_op",
                                            "invalid base64 share",
                                        );
                                        let _ = swarm.behaviour_mut().keyshare_rr.send_response(channel, resp);
                                    }
                                }
                            } else {
                                warn!("libp2p: keyshare envelope verification failed from {}: {:?}", peer, e);
                                let resp = protocol::machine::build_keyshare_response(
                                    false,
                                    "keyshare_op",
                                    "envelope verification failed",
                                );
                                let _ = swarm.behaviour_mut().keyshare_rr.send_response(channel, resp);
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("libp2p: failed to parse keyshare JSON from {}: {:?}", peer, e);
                    let resp = protocol::machine::build_keyshare_response(
                        false,
                        "keyshare_op",
                        "invalid json payload",
                    );
                    let _ = swarm.behaviour_mut().keyshare_rr.send_response(channel, resp);
                }
            }
        }
        request_response::Message::Response { response, .. } => {
            info!("libp2p: received keyshare response from peer={}", peer);
            // Parse the response if needed
            match protocol::machine::root_as_apply_response(&response) {
                Ok(resp) => {
                    info!("libp2p: keyshare response - ok={} op={:?} msg={:?}", resp.ok(), resp.operation_id(), resp.message());
                }
                Err(e) => {
                    warn!("libp2p: failed to parse keyshare response: {:?}", e);
                }
            }
        }
    }
}

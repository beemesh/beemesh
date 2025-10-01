use libp2p::request_response;
use log::{info, warn};

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
            info!("libp2p: received keyshare request from peer={}", peer);

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
                                            let resp = protocol::machine::build_keyshare_response(
                                                true,
                                                "keyshare_op",
                                                "decapsulated and accepted",
                                            );
                                            let _ = swarm.behaviour_mut().keyshare_rr.send_response(channel, resp);
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

use log::{info, debug, error};
use tokio::time::Instant;
use std::time::Duration;
use libp2p::request_response;

pub fn handshake_request<F>(
    request: Vec<u8>,
    peer: libp2p::PeerId,
    send_response: F,
    handshake_states: &mut std::collections::HashMap<libp2p::PeerId, super::super::HandshakeState>,
) where
    F: FnOnce(Vec<u8>),
{
    log::info!("libp2p: received handshake request from peer={}", peer);

    // If the request might be an Envelope (JSON or FlatBuffer), try to verify and extract inner bytes
    let effective_request = match serde_json::from_slice::<serde_json::Value>(&request) {
        Ok(val) => {
            match crate::libp2p_beemesh::security::verify_envelope_and_check_nonce(&val) {
                Ok((payload_bytes, _pub, _sig)) => payload_bytes,
                Err(e) => {
                    if crate::libp2p_beemesh::security::require_signed_messages() {
                        log::error!("rejecting unsigned/invalid handshake request: {:?}", e);
                        // Send an empty error response
                        let error_response = protocol::machine::build_handshake(0, 0, "", "");
                        send_response(error_response);
                        return;
                    }
                    request.clone()
                }
            }
        }
        Err(_) => request.clone(),
    };

    // Parse the FlatBuffer handshake request
    match protocol::machine::root_as_handshake(&effective_request) {
        Ok(handshake_req) => {
            log::debug!("libp2p: handshake request - signature={:?}", handshake_req.signature());

            // Mark this peer as confirmed
            let state = handshake_states.entry(peer.clone()).or_insert(super::super::HandshakeState {
                attempts: 0,
                last_attempt: Instant::now() - Duration::from_secs(3),
                confirmed: false,
            });
            state.confirmed = true;

            // Create a response with our own signature
            let response = protocol::machine::build_handshake(0, 0, "TODO", "TODO");

            // Send the response back via closure
            send_response(response);
            log::info!("libp2p: sent handshake response to peer={}", peer);
        }
        Err(e) => {
            log::error!("libp2p: failed to parse handshake request: {:?}", e);
            // Send empty response on parse error
            let error_response = protocol::machine::build_handshake(0, 0, "TODO", "TODO");
            send_response(error_response);
        }
    }
}

pub fn handshake_response(
    response: Vec<u8>,
    peer: libp2p::PeerId,
    handshake_states: &mut std::collections::HashMap<libp2p::PeerId, super::super::HandshakeState>,
) {
    log::info!("libp2p: received handshake response from peer={}", peer);

    // Parse the response
    match protocol::machine::root_as_handshake(&response) {
        Ok(handshake_resp) => {
            log::debug!("libp2p: handshake response - signature={:?}", handshake_resp.signature());

            // Mark this peer as confirmed
            let state = handshake_states.entry(peer.clone()).or_insert(super::super::HandshakeState {
                attempts: 0,
                last_attempt: Instant::now() - Duration::from_secs(3),
                confirmed: false,
            });
            state.confirmed = true;
        }
        Err(e) => {
            log::error!("libp2p: failed to parse handshake response: {:?}", e);
        }
    }
}

/// Handle a full `request_response::Message` for the handshake protocol.
/// This centralizes request/response handling so callers only need to delegate the
/// `Event::Message` into this function.
pub fn handshake_message_event(
    message: request_response::Message<Vec<u8>, Vec<u8>>,
    peer: libp2p::PeerId,
    swarm: &mut libp2p::Swarm<super::MyBehaviour>,
    handshake_states: &mut std::collections::HashMap<libp2p::PeerId, super::super::HandshakeState>,
) {
    match message {
        request_response::Message::Request { request, channel, .. } => {
            // Use the existing helper to process requests and send the response via the swarm
            let req = request.clone();
            let peer_c = peer.clone();
            let swarm_ref = swarm;
            handshake_request(req, peer_c, |resp| {
                let _ = swarm_ref.behaviour_mut().handshake_rr.send_response(channel, resp);
            }, handshake_states);
        }
        request_response::Message::Response { response, .. } => {
            // Delegate to the existing response handler
            handshake_response(response.clone(), peer, handshake_states);
        }
    }
}

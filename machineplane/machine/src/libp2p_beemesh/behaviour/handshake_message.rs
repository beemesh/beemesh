use super::message_verifier::verify_signed_message;
use crate::libp2p_beemesh::envelope::{sign_with_node_keys, SignEnvelopeConfig};
use libp2p::request_response;
use std::time::Duration;
use tokio::time::Instant;

pub fn handshake_request<F>(
    request: Vec<u8>,
    peer: libp2p::PeerId,
    send_response: F,
    handshake_states: &mut std::collections::HashMap<libp2p::PeerId, super::super::HandshakeState>,
) where
    F: FnOnce(Vec<u8>),
{
    //log::info!("libp2p: received handshake request from peer={}", peer);

    // Handshakes should be wrapped in signed envelopes for consistency
    let verified = match verify_signed_message(&peer, &request, |err| {
        log::error!("rejecting invalid handshake request: {}", err);
    }) {
        Some(envelope) => envelope,
        None => {
            let error_response = protocol::machine::build_handshake(0, 0, "", "");
            send_response(error_response);
            return;
        }
    };
    let effective_request = verified.payload;

    // Parse the FlatBuffer handshake request
    match protocol::machine::root_as_handshake(&effective_request) {
        Ok(_handshake_req) => {
            // Mark this peer as confirmed
            let state =
                handshake_states
                    .entry(peer.clone())
                    .or_insert(super::super::HandshakeState {
                        attempts: 0,
                        last_attempt: Instant::now() - Duration::from_secs(3),
                        confirmed: false,
                    });
            state.confirmed = true;

            // Create a handshake response wrapped in a signed envelope
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let nonce = format!("handshake_resp_{}", rand::random::<u32>());

            // Build simple handshake response
            let handshake_response = protocol::machine::build_handshake(
                rand::random::<u32>(),
                timestamp,
                "beemesh/1.0",
                &peer.to_string(),
            );

            // Wrap in signed envelope
            let sign_cfg = SignEnvelopeConfig {
                nonce: Some(&nonce),
                timestamp: Some(timestamp),
                ..Default::default()
            };

            match sign_with_node_keys(&handshake_response, "handshake", sign_cfg) {
                Ok(signed) => send_response(signed.bytes),
                Err(e) => {
                    log::error!("failed to sign handshake response: {:?}", e);
                    let error_response = protocol::machine::build_handshake(0, 0, "", "");
                    send_response(error_response);
                }
            }
            //log::info!("libp2p: sent handshake response to peer={}", peer);
        }
        Err(e) => {
            log::error!("libp2p: failed to parse handshake request: {:?}", e);
            // Send empty response on parse error
            let error_response = protocol::machine::build_handshake(0, 0, "", "");
            send_response(error_response);
        }
    }
}

pub fn handshake_response(
    response: Vec<u8>,
    peer: libp2p::PeerId,
    handshake_states: &mut std::collections::HashMap<libp2p::PeerId, super::super::HandshakeState>,
) {
    //log::info!("libp2p: received handshake response from peer={}", peer);

    // Verify the signed envelope for handshake response
    let verified = match verify_signed_message(&peer, &response, |err| {
        log::error!("rejecting invalid handshake response: {}", err);
    }) {
        Some(envelope) => envelope,
        None => return,
    };
    let effective_response = verified.payload;

    // Parse the response
    match protocol::machine::root_as_handshake(&effective_response) {
        Ok(_handshake_resp) => {
            //log::debug!("libp2p: handshake response - signature={:?}", handshake_resp.signature());

            // Mark this peer as confirmed
            let state =
                handshake_states
                    .entry(peer.clone())
                    .or_insert(super::super::HandshakeState {
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
        request_response::Message::Request {
            request, channel, ..
        } => {
            // Use the existing helper to process requests and send the response via the swarm
            let req = request.clone();
            let peer_c = peer.clone();
            let swarm_ref = swarm;
            handshake_request(
                req,
                peer_c,
                |resp| {
                    let _ = swarm_ref
                        .behaviour_mut()
                        .handshake_rr
                        .send_response(channel, resp);
                },
                handshake_states,
            );
        }
        request_response::Message::Response { response, .. } => {
            // Delegate to the existing response handler
            handshake_response(response.clone(), peer, handshake_states);
        }
    }
}

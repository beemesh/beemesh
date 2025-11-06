use anyhow::Context;
use log::{debug, error};
use rand::Rng;
use std::collections::HashMap as StdHashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

use crate::libp2p_beemesh::envelope::{
    SignEnvelopeConfig, sign_with_existing_keypair, sign_with_node_keys,
};
use protocol::libp2p_constants::FREE_CAPACITY_PREFIX;

/// Lightweight helpers to centralize common envelope signing and broadcast logic.
pub fn make_nonce(prefix: Option<&str>) -> String {
    if let Some(p) = prefix {
        format!("{}_{}", p, rand::thread_rng().r#gen::<u32>())
    } else {
        format!("nonce_{}", rand::thread_rng().r#gen::<u32>())
    }
}

pub fn make_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0u64)
}

/// Sign payload using the node's on-disk keypair with a generated nonce/timestamp.
/// Returns the signed envelope bytes (ready to send).
pub fn sign_payload_default(
    payload: &[u8],
    payload_type: &str,
    nonce_prefix: Option<&str>,
) -> anyhow::Result<Vec<u8>> {
    let nonce = make_nonce(nonce_prefix);
    let timestamp = make_timestamp_ms();
    let sign_cfg = SignEnvelopeConfig {
        nonce: Some(&nonce),
        timestamp: Some(timestamp),
        ..Default::default()
    };
    let signed = sign_with_node_keys(payload, payload_type, sign_cfg)
        .context("failed to sign payload with node keys")?;
    Ok(signed.bytes)
}

/// Broadcast a signed request to all scheduler peers. This reuses the on-disk keypair and signs
/// a unique envelope per peer (nonce includes peer id) before sending using the scheduler request-response
/// behaviour. Returns the number of peers the request was sent to.
pub fn broadcast_signed_request_to_peers(
    swarm: &mut libp2p::Swarm<crate::libp2p_beemesh::behaviour::MyBehaviour>,
    payload: &[u8],
    payload_type: &str,
) -> anyhow::Result<usize> {
    // Load keypair once
    let (pub_bytes, sk_bytes) = crypto::ensure_keypair_on_disk().context("loading keypair")?;

    let timestamp = make_timestamp_ms();
    let mut sent = 0usize;

    // Collect peers from gossipsub peer list (same approach as existing code)
    let peers: Vec<libp2p::PeerId> = swarm
        .behaviour()
        .gossipsub
        .all_peers()
        .map(|(p, _)| p.clone())
        .collect();

    for peer in peers.iter() {
        let nonce = make_nonce(Some(&format!("{}_query", peer)));
        let sign_cfg = SignEnvelopeConfig {
            nonce: Some(&nonce),
            timestamp: Some(timestamp),
            ..Default::default()
        };

        match sign_with_existing_keypair(payload, payload_type, sign_cfg, &pub_bytes, &sk_bytes) {
            Ok(signed) => {
                let _req_id = swarm
                    .behaviour_mut()
                    .scheduler_rr
                    .send_request(peer, signed.bytes);
                debug!("sent signed request to peer {}", peer);
                sent += 1;
            }
            Err(e) => {
                error!("failed to sign request for peer {}: {:?}", peer, e);
            }
        }
    }

    Ok(sent)
}

/// Notify all pending capacity observers for the given request id with a lazily constructed payload.
pub fn notify_capacity_observers<F>(
    pending_queries: &mut StdHashMap<String, Vec<mpsc::UnboundedSender<String>>>,
    request_id: &str,
    mut build_payload: F,
) where
    F: FnMut() -> String,
{
    if let Some(senders) = pending_queries.get_mut(request_id) {
        let payload = build_payload();
        for tx in senders.iter() {
            let _ = tx.send(payload.clone());
        }
    }
}

/// Extract the manifest/task identifier encoded in a capacity request id.
/// Supports the current `prefix:manifest_id:uuid` format and falls back to the
/// legacy `prefix-manifest_id-uuid` pattern for compatibility.
pub fn extract_manifest_id_from_request_id(request_id: &str) -> Option<String> {
    let mut colon_parts = request_id.split(':');
    if let (Some(prefix), Some(manifest)) = (colon_parts.next(), colon_parts.next()) {
        if prefix == FREE_CAPACITY_PREFIX && !manifest.is_empty() {
            return Some(manifest.to_string());
        }
    }

    if request_id.starts_with(FREE_CAPACITY_PREFIX) {
        let parts: Vec<&str> = request_id.split('-').collect();
        if parts.len() >= 2 && !parts[1].is_empty() {
            return Some(parts[1].to_string());
        }
    }

    None
}

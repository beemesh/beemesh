use std::collections::HashMap as StdHashMap;
use tokio::sync::mpsc;

use crate::messages::constants::FREE_CAPACITY_PREFIX;
use libp2p::{PeerId, identity::PublicKey};
use multihash::Multihash;

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

/// Extract the manifest/tender identifier encoded in a capacity request id.
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

    // Fallback: treat the entire request id as the manifest identifier when no prefix is present.
    // Some callers send the manifest ID directly as the request id, so avoid dropping those
    // capacity requests.
    if !request_id.is_empty() {
        return Some(request_id.to_string());
    }

    None
}

/// Attempt to derive a libp2p [`PublicKey`] from the given [`PeerId`].
///
/// Ed25519 peer IDs are encoded as identity multihashes containing the protobuf-encoded
/// public key, which allows reconstructing the key for signature verification.
pub fn peer_id_to_public_key(peer_id: &PeerId) -> Option<PublicKey> {
    let multihash = Multihash::<64>::from_bytes(&peer_id.to_bytes()).ok()?;
    PublicKey::try_decode_protobuf(multihash.digest()).ok()
}

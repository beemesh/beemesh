use libp2p::{PeerId, identity::PublicKey};
use multihash::Multihash;

/// Attempt to derive a libp2p [`PublicKey`] from the given [`PeerId`].
///
/// Ed25519 peer IDs are encoded as identity multihashes containing the protobuf-encoded
/// public key, which allows reconstructing the key for signature verification.
pub fn peer_id_to_public_key(peer_id: &PeerId) -> Option<PublicKey> {
    let multihash = Multihash::from_bytes(&peer_id.to_bytes()).ok()?;
    PublicKey::try_decode_protobuf(multihash.digest()).ok()
}

use crate::libp2p_beemesh::envelope::{sign_with_node_keys, SignEnvelopeConfig, SignedEnvelope};
use base64::Engine;

/// Input parameters for building a capacity reply.
pub struct CapacityReplyParams<'a> {
    pub ok: bool,
    pub cpu_milli: u32,
    pub memory_bytes: u64,
    pub storage_bytes: u64,
    pub request_id: &'a str,
    pub responder_peer: &'a str,
    pub region: &'a str,
    pub capabilities: &'a [&'a str],
}

/// Flatbuffer capacity reply alongside optional encoded KEM public key.
pub struct CapacityReply {
    pub payload: Vec<u8>,
    pub kem_pub_b64: Option<String>,
}

/// Build the flatbuffer payload for a capacity reply and capture the associated KEM public key.
pub fn build_capacity_reply(params: CapacityReplyParams<'_>) -> CapacityReply {
    let kem_pub_b64 = crypto::ensure_kem_keypair_on_disk()
        .map(|(pubb, _)| base64::engine::general_purpose::STANDARD.encode(&pubb))
        .ok();

    let payload = protocol::machine::build_capacity_reply(
        params.ok,
        params.cpu_milli,
        params.memory_bytes,
        params.storage_bytes,
        params.request_id,
        params.responder_peer,
        params.region,
        kem_pub_b64.as_deref(),
        params.capabilities,
    );

    CapacityReply {
        payload,
        kem_pub_b64,
    }
}

/// Sign a capacity reply payload with the node's key material.
pub fn sign_capacity_reply(payload: &[u8]) -> anyhow::Result<SignedEnvelope> {
    sign_with_node_keys(payload, "capacity_reply", SignEnvelopeConfig::default())
}

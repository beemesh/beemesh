use crate::libp2p_beemesh::envelope::{SignEnvelopeConfig, SignedEnvelope, sign_with_node_keys};
use base64::Engine;
use log::warn;

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

pub const DEFAULT_CAPACITY_CPU_MILLI: u32 = 1000;
pub const DEFAULT_CAPACITY_MEMORY_BYTES: u64 = 1024 * 1024 * 512;
pub const DEFAULT_CAPACITY_STORAGE_BYTES: u64 = 1024 * 1024 * 1024;
pub const DEFAULT_CAPACITY_REGION: &str = "local";
static DEFAULT_CAPACITY_CAPABILITIES: [&str; 1] = ["default"];

/// Build baseline capacity parameters shared across request-response paths.
pub fn baseline_capacity_params<'a>(
    request_id: &'a str,
    responder_peer: &'a str,
) -> CapacityReplyParams<'a> {
    CapacityReplyParams {
        ok: true,
        cpu_milli: DEFAULT_CAPACITY_CPU_MILLI,
        memory_bytes: DEFAULT_CAPACITY_MEMORY_BYTES,
        storage_bytes: DEFAULT_CAPACITY_STORAGE_BYTES,
        request_id,
        responder_peer,
        region: DEFAULT_CAPACITY_REGION,
        capabilities: &DEFAULT_CAPACITY_CAPABILITIES,
    }
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

/// Convenience helper to build a capacity reply using baseline parameters with overrides.
pub fn build_capacity_reply_with<'a, F>(
    request_id: &'a str,
    responder_peer: &'a str,
    mut adjust: F,
) -> CapacityReply
where
    F: FnMut(&mut CapacityReplyParams<'a>),
{
    let mut params = baseline_capacity_params(request_id, responder_peer);
    adjust(&mut params);
    build_capacity_reply(params)
}

/// Sign a capacity reply payload with the node's key material.
pub fn sign_capacity_reply(payload: &[u8]) -> anyhow::Result<SignedEnvelope> {
    sign_with_node_keys(payload, "capacity_reply", SignEnvelopeConfig::default())
}

/// Emit a warning when the KEM public key is missing from a capacity reply payload.
pub fn warn_missing_kem(context: &str, peer_label: &str, kem_pub_b64: Option<&str>) {
    if kem_pub_b64.map_or(true, |value| value.is_empty()) {
        warn!(
            "libp2p: {} capacity reply missing KEM public key for {}",
            context, peer_label
        );
    }
}

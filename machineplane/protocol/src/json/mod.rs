use serde::{Deserialize, Serialize};
use serde_json::Value;
use base64::engine::general_purpose;
use base64::Engine as _;
use serde_json_canonicalizer::to_vec as jcs_to_vec;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SharesMeta {
    pub n: usize,
    pub k: usize,
    pub count: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Envelope {
    /// base64-encoded ciphertext payload (as produced by the CLI)
    pub payload: String,
    /// base64-encoded nonce string
    pub nonce: String,
    pub shares_meta: SharesMeta,

    /// Millisecond epoch timestamp
    #[serde(default)]
    pub ts: Option<u64>,

    /// Signature algorithm identifier, e.g. "ml-dsa-65"
    #[serde(default)]
    pub alg: Option<String>,

    /// Optional signature (may be absent for unsigned / already-decrypted payloads)
    /// For compatibility with existing code the sig may be prefixed with "ml-dsa-65:BASE64"
    #[serde(default)]
    pub sig: Option<String>,
    /// Optional public key (base64)
    #[serde(default)]
    pub pubkey: Option<String>,

    /// Optional peer id (libp2p)
    #[serde(default)]
    pub peer_id: Option<String>,
}

// #[derive(Debug, Serialize, Deserialize, Clone)]
// pub struct CapacityRequest {
//     pub cpu_milli: u32,
//     pub memory_bytes: u64,
//     pub storage_bytes: u64,
//     pub replicas: u32,
// }

// #[derive(Debug, Serialize, Deserialize, Clone)]
// pub struct ApplyPayload {
//     /// The manifest JSON as an embedded JSON object/string
//     pub manifest: serde_json::Value,
//     /// Scheduling/capacity request associated with this apply
//     pub capacity_request: CapacityRequest,
// }

// #[derive(Debug, Serialize, Deserialize, Clone)]
// pub struct SharesPayload {
//     /// Base64-encoded shares produced by Shamir split
//     pub shares: Vec<String>,
//     pub shares_meta: SharesMeta,
// }

impl Envelope {
    /// Decode the base64 payload into bytes.
    pub fn decode_payload(&self) -> anyhow::Result<Vec<u8>> {
        let decoded = general_purpose::STANDARD
            .decode(&self.payload)
            .map_err(|e| anyhow::anyhow!("base64 decode payload failed: {}", e))?;
        Ok(decoded)
    }

    /// Convert into a serde_json::Value (useful for existing verification helpers that expect Value)
    pub fn to_value(&self) -> anyhow::Result<Value> {
        let v = serde_json::to_value(self).map_err(|e| anyhow::anyhow!(e))?;
        Ok(v)
    }

    /// Prepare a JSON value for signature verification by removing signature/pubkey
    /// This mirrors the behavior of signing the payload without the sig fields.
    pub fn to_value_for_verification(&self) -> anyhow::Result<Value> {
        let mut v = self.to_value()?;
        if let Value::Object(ref mut map) = v {
            map.remove("sig");
            map.remove("pubkey");
        }
        Ok(v)
    }
}

// /// Produce a canonical JSON representation with deterministic key ordering.
// /// This implements a simple canonicalization by recursively sorting object keys
// /// and serializing without extra whitespace. It's not a full JCS implementation
// /// but sufficient for deterministic signing within this project.
// pub fn canonicalize_json_value(v: &Value) -> anyhow::Result<Vec<u8>> {
//     let bytes = jcs_to_vec(v).map_err(|e| anyhow::anyhow!("canonicalize failed: {}", e))?;
//     Ok(bytes)
// }

// // -- Task / REST API types -------------------------------------------------

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TaskCreateRequest {
    pub manifest: serde_json::Value,
    #[serde(default)]
    pub replicas: Option<u32>,
    #[serde(default)]
    pub manifest_id: Option<String>,
    #[serde(default)]
    pub operation_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TaskCreateResponse {
    pub ok: bool,
    pub task_id: String,
    pub manifest_id: String,
    pub selection_window_ms: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DistributeSharesRequest {
    #[serde(default)]
    pub shares_envelope: Option<serde_json::Value>,
    // per-target payloads (peer id + optional payload)
    pub targets: Vec<ShareTarget>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DistributeSharesResponse {
    pub ok: bool,
    pub results: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AssignRequest {
    #[serde(default)]
    pub chosen_peers: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AssignResponse {
    pub ok: bool,
    pub task_id: String,
    pub assigned_peers: Vec<String>,
    pub per_peer: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TaskStatusResponse {
    pub task_id: String,
    pub state: String,
    pub assigned_peers: Vec<String>,
    pub shares_distributed: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ShareTarget {
    pub peer_id: String,
    /// Arbitrary JSON payload (the CLI should have encrypted the share for the recipient)
    pub payload: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DistributeCapabilitiesRequest {
    // per-target capability payloads (peer id + capability envelope)
    pub targets: Vec<CapabilityTarget>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CapabilityTarget {
    pub peer_id: String,
    /// Signed capability envelope (JSON object with sig, pubkey, etc.)
    pub payload: serde_json::Value,
}


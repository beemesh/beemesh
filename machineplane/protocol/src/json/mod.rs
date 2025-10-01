use serde::{Deserialize, Serialize};
use serde_json::Value;
use base64::engine::general_purpose;
use base64::Engine as _;

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

    /// Optional signature (may be absent for unsigned / already-decrypted payloads)
    #[serde(default)]
    pub sig: Option<String>,
    /// Optional public key (base64)
    #[serde(default)]
    pub pubkey: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CapacityRequest {
    pub cpu_milli: u32,
    pub memory_bytes: u64,
    pub storage_bytes: u64,
    pub replicas: u32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ApplyPayload {
    /// The manifest JSON as an embedded JSON object/string
    pub manifest: serde_json::Value,
    /// Scheduling/capacity request associated with this apply
    pub capacity_request: CapacityRequest,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SharesPayload {
    /// Base64-encoded shares produced by Shamir split
    pub shares: Vec<String>,
    pub shares_meta: SharesMeta,
}

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
}

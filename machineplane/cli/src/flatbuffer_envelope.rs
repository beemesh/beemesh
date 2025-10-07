use anyhow::Result;
use base64::Engine;
use protocol::machine::{build_envelope_canonical, build_envelope_signed, root_as_envelope};
use serde_json::Value as JsonValue;
use crypto::sign_envelope;

/// Helper for building flatbuffer Envelopes instead of JSON
pub struct FlatbufferEnvelopeBuilder;

impl FlatbufferEnvelopeBuilder {
    pub fn new() -> Self {
        Self
    }

    /// Build a manifest envelope containing encrypted manifest payload
    pub fn build_manifest_envelope(
        &mut self,
        ciphertext: &[u8],
        nonce_bytes: &[u8],
        n: usize,
        k: usize,
        count: usize,
    ) -> Result<Vec<u8>> {
        // Create a simple payload containing the encrypted data and metadata
        let payload_json = serde_json::json!({
            "payload": base64::engine::general_purpose::STANDARD.encode(ciphertext),
            "nonce": base64::engine::general_purpose::STANDARD.encode(nonce_bytes),
            "shares_meta": { "n": n, "k": k, "count": count },
        });
        let payload_bytes = serde_json::to_vec(&payload_json)?;

        let envelope_nonce: [u8; 16] = rand::random();
        let nonce_str = base64::engine::general_purpose::STANDARD.encode(&envelope_nonce);
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        Ok(build_envelope_canonical(
            &payload_bytes,
            "manifest",
            &nonce_str,
            ts,
            "ml-dsa-65",
        ))
    }

    /// Build a simple envelope with just payload and type
    pub fn build_simple_envelope(
        &mut self,
        payload_type: &str,
        payload_content: &str,
    ) -> Result<Vec<u8>> {
        let payload_bytes = payload_content.as_bytes();

        let envelope_nonce: [u8; 16] = rand::random();
        let nonce_str = base64::engine::general_purpose::STANDARD.encode(&envelope_nonce);
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        Ok(build_envelope_canonical(
            payload_bytes,
            payload_type,
            &nonce_str,
            ts,
            "ml-dsa-65",
        ))
    }

    /// Build a shares envelope containing encrypted share data
    pub fn build_shares_envelope(
        &mut self,
        shares_b64: &[String],
        n: usize,
        k: usize,
        count: usize,
    ) -> Result<Vec<u8>> {
        let shares_payload = serde_json::json!({
            "shares": shares_b64,
            "shares_meta": { "n": n, "k": k, "count": count }
        });
        let payload_bytes = serde_json::to_vec(&shares_payload)?;

        let envelope_nonce: [u8; 16] = rand::random();
        let nonce_str = base64::engine::general_purpose::STANDARD.encode(&envelope_nonce);
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        Ok(build_envelope_canonical(
            &payload_bytes,
            "shares",
            &nonce_str,
            ts,
            "ml-dsa-65",
        ))
    }

    /// Build a capability token envelope - creates JSON format compatible with existing verification
    pub fn build_capability_envelope(
        &mut self,
        manifest_id: &str,
        peer_id: &str,
        ts: u64,
    ) -> Result<Vec<u8>> {
        // Create capability token payload (this goes inside the envelope payload field)
        let token_obj = serde_json::json!({
            "task_id": manifest_id,
            "issuer": "cli",
            "required_quorum": 1,
            "caveats": { "authorized_peer": peer_id },
            "ts": ts,
        });
        let token_bytes = serde_json::to_vec(&token_obj)?;

        let envelope_nonce: [u8; 16] = rand::random();
        let nonce_str = base64::engine::general_purpose::STANDARD.encode(&envelope_nonce);

        // Create envelope in the exact format expected by security verification
        // This matches what send_apply_request.rs creates (lines 144-146)
        let mut envelope = serde_json::Map::new();
        envelope.insert("payload".to_string(), serde_json::Value::String(
            base64::engine::general_purpose::STANDARD.encode(&token_bytes)
        ));
        envelope.insert("manifest_id".to_string(), serde_json::Value::String(manifest_id.to_string()));
        envelope.insert("type".to_string(), serde_json::Value::String("capability".to_string()));
        envelope.insert("nonce".to_string(), serde_json::Value::String(nonce_str));
        envelope.insert("ts".to_string(), serde_json::Value::Number(ts.into()));
        envelope.insert("alg".to_string(), serde_json::Value::String("ml-dsa-65".to_string()));
        
        let envelope_value = serde_json::Value::Object(envelope);
        let envelope_bytes = serde_json::to_vec(&envelope_value)?;

        Ok(envelope_bytes)
    }

    /// Add signature and pubkey to an existing envelope
    pub fn sign_envelope(
        envelope_bytes: &[u8],
        sk_bytes: &[u8],
        pk_bytes: &[u8],
    ) -> Result<Vec<u8>> {
        let (sig_b64, pub_b64) = sign_envelope(sk_bytes, pk_bytes, envelope_bytes)?;
        
        // Try to parse as JSON first (for capability envelopes)
        if let Ok(mut envelope_json) = serde_json::from_slice::<serde_json::Value>(envelope_bytes) {
            if let Some(obj) = envelope_json.as_object_mut() {
                obj.insert("sig".to_string(), serde_json::Value::String(format!("ml-dsa-65:{}", sig_b64)));
                obj.insert("pubkey".to_string(), serde_json::Value::String(pub_b64));
                return Ok(serde_json::to_vec(&envelope_json)?);
            }
        }
        
        // Fall back to flatbuffer handling
        let envelope = root_as_envelope(envelope_bytes)
            .map_err(|e| anyhow::anyhow!("Failed to parse envelope: {}", e))?;

        // Build signed envelope using protocol helper
        let payload_bytes = envelope.payload()
            .map(|v| v.bytes())
            .unwrap_or(&[]);
        
        Ok(build_envelope_signed(
            payload_bytes,
            envelope.payload_type().unwrap_or(""),
            envelope.nonce().unwrap_or(""),
            envelope.ts(),
            "ml-dsa-65",
            "ml-dsa-65",
            &sig_b64,
            &pub_b64,
        ))
    }
}

/// Convert flatbuffer Envelope or JSON envelope to JSON for compatibility with existing code
pub fn envelope_to_json(envelope_bytes: &[u8]) -> Result<JsonValue> {
    // Try JSON parsing first - if this works, it's already JSON
    if let Ok(json_value) = serde_json::from_slice::<JsonValue>(envelope_bytes) {
        return Ok(json_value);
    }
    
    // Fall back to flatbuffer parsing
    let envelope = root_as_envelope(envelope_bytes)
        .map_err(|e| anyhow::anyhow!("Failed to parse envelope: {}", e))?;

    let mut json_obj = serde_json::Map::new();
    
    if let Some(payload) = envelope.payload() {
        json_obj.insert("payload".to_string(), serde_json::Value::String(
            base64::engine::general_purpose::STANDARD.encode(payload.bytes())
        ));
    }
    
    if let Some(payload_type) = envelope.payload_type() {
        json_obj.insert("payload_type".to_string(), serde_json::Value::String(payload_type.to_string()));
    }
    
    if let Some(nonce) = envelope.nonce() {
        json_obj.insert("nonce".to_string(), serde_json::Value::String(nonce.to_string()));
    }
    
    json_obj.insert("ts".to_string(), serde_json::Value::Number(envelope.ts().into()));
    
    if let Some(alg) = envelope.alg() {
        json_obj.insert("alg".to_string(), serde_json::Value::String(alg.to_string()));
    }
    
    if let Some(sig) = envelope.sig() {
        json_obj.insert("sig".to_string(), serde_json::Value::String(sig.to_string()));
    }
    
    if let Some(pubkey) = envelope.pubkey() {
        json_obj.insert("pubkey".to_string(), serde_json::Value::String(pubkey.to_string()));
    }
    
    if let Some(peer_id) = envelope.peer_id() {
        json_obj.insert("peer_id".to_string(), serde_json::Value::String(peer_id.to_string()));
    }

    Ok(serde_json::Value::Object(json_obj))
}
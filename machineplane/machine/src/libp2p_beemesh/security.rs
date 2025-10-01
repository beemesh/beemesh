use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Context;
use base64::engine::general_purpose;
use base64::Engine as _;
use serde_json::Value;

use crypto;

/// Keep nonces for a short window to mitigate replay attacks.
/// Simple in-memory map: nonce_str -> Instant inserted_at.
///
/// This is intentionally small and simple. In production you may want an
/// LRU cache, sharded maps, or a persistent store for high-throughput nodes.
static NONCE_STORE: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();

fn nonce_store() -> &'static Mutex<HashMap<String, Instant>> {
    NONCE_STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Remove expired entries older than `max_age`
fn prune_nonces(max_age: Duration) {
    let mut store = nonce_store().lock().unwrap();
    let now = Instant::now();
    store.retain(|_, &mut t| now.duration_since(t) <= max_age);
}

/// Verify an incoming JSON envelope and check nonce uniqueness.
///
/// Expected envelope JSON shape (example produced by CLI):
/// {
///   "payload": "<base64-ciphertext>",
///   "nonce": "<base64-nonce>",
///   "shares_meta": { ... },
///   // server gets "sig" and "pubkey" fields appended by the CLI:
///   "sig": "ml-dsa-65:BASE64_SIG",
///   "pubkey": "BASE64_PUBKEY"
/// }
///
/// This function:
/// - extracts and removes `sig` and `pubkey`
/// - reserializes the remaining envelope (deterministic insertion/order assumption)
/// - base64-decodes `pubkey` and `sig`
/// - calls `crypto::verify_envelope(pub_bytes, envelope_bytes, sig_bytes)`
/// - verifies `nonce` is not already seen in the last `nonce_window`
/// - returns the decoded payload bytes (ciphertext) on success
pub fn verify_envelope_and_check_nonce(
    envelope: &Value,
) -> anyhow::Result<(Vec<u8>, String, String)> {
    // Must be an object
    let obj = envelope
        .as_object()
        .context("envelope is not a JSON object")?;

    // Extract sig and pubkey fields
    let sig_value = obj.get("sig").context("envelope missing 'sig' field")?;
    let pub_value = obj.get("pubkey").context("envelope missing 'pubkey' field")?;

    let sig_s = sig_value.as_str().context("'sig' is not a string")?;
    let pub_b64 = pub_value.as_str().context("'pubkey' is not a string")?;

    // Signature string may have a scheme prefix like "ml-dsa-65:BASE64", handle that
    let sig_b64 = sig_s
        .splitn(2, ':')
        .nth(if sig_s.contains(':') { 1 } else { 0 })
        .unwrap_or(sig_s);

    let sig_bytes = general_purpose::STANDARD
        .decode(sig_b64)
        .context("failed to base64-decode signature")?;
    let pub_bytes = general_purpose::STANDARD
        .decode(pub_b64)
        .context("failed to base64-decode pubkey")?;

    // Build a new Value that excludes sig and pubkey for verification (the CLI signed this envelope)
    let mut envelope_no_sig = serde_json::Map::new();
    for (k, v) in obj.iter() {
        if k == "sig" || k == "pubkey" {
            continue;
        }
        envelope_no_sig.insert(k.clone(), v.clone());
    }
    let envelope_no_sig_value = Value::Object(envelope_no_sig);
    let envelope_bytes = serde_json::to_vec(&envelope_no_sig_value)
        .context("failed to serialize envelope for verification")?;

    // Verify signature using crypto helper
    crypto::verify_envelope(&pub_bytes, &envelope_bytes, &sig_bytes)
        .context("signature verification failed")?;

    // Extract nonce and check replay
    let nonce_val = envelope
        .get("nonce")
        .context("envelope missing 'nonce' field for replay protection")?;
    let nonce_str = if nonce_val.is_string() {
        nonce_val.as_str().unwrap().to_string()
    } else {
        // If it's base64 bytes as array, accept string or number forms—require string for simplicity
        return Err(anyhow::anyhow!("envelope 'nonce' must be a string"));
    };

    // Nonce window
    let nonce_window = Duration::from_secs(300); // 5 minutes

    {
        let mut store = nonce_store().lock().unwrap();

        // Prune expired entries opportunistically
        let now = Instant::now();
        store.retain(|_, &mut t| now.duration_since(t) <= nonce_window);

        if store.contains_key(&nonce_str) {
            return Err(anyhow::anyhow!("replay detected: nonce already seen"));
        }

        store.insert(nonce_str, Instant::now());
    }

    // Return the decoded payload bytes (ciphertext) so the caller can decrypt as needed.
    let payload_val = envelope
        .get("payload")
        .context("envelope missing 'payload' field")?;
    let payload_b64 = payload_val
        .as_str()
        .context("'payload' field is not a base64 string")?;
    let payload_bytes = general_purpose::STANDARD
        .decode(payload_b64)
        .context("failed to base64-decode payload")?;

    // Return payload bytes plus original pubkey and signature strings so callers can
    // propagate them into AppliedManifest records.
    Ok((payload_bytes, pub_b64.to_string(), sig_s.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crypto::{ensure_pqc_init, ensure_keypair_ephemeral, sign_envelope};
    use serde_json::json;

    #[test]
    fn test_verify_and_replay() {
        ensure_pqc_init().expect("pqc init");
        let (pubb, privb) = ensure_keypair_ephemeral().expect("keygen");

        let payload = b"{\"a\":1}".to_vec();
        let envelope = json!({
            "payload": base64::engine::general_purpose::STANDARD.encode(&payload),
            "nonce": "n-abc",
        });
        let envelope_bytes = serde_json::to_vec(&envelope).unwrap();
        let (sig_b64, pub_b64) = sign_envelope(&privb, &pubb, &envelope_bytes).expect("sign");

        let mut env_with_sig = envelope.clone();
        env_with_sig["sig"] = json!(format!("ml-dsa-65:{}", sig_b64));
        env_with_sig["pubkey"] = json!(pub_b64);

        let (decoded, _pub, _sig) = verify_envelope_and_check_nonce(&env_with_sig).expect("verify");
        assert_eq!(decoded, payload);

        // Replay should be rejected
        assert!(verify_envelope_and_check_nonce(&env_with_sig).is_err());
    }
}

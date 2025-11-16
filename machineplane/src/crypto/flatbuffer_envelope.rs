use anyhow::Context;
use base64::Engine;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

static NONCE_STORE: OnceLock<Mutex<HashMap<String, HashMap<String, Instant>>>> = OnceLock::new();

fn nonce_store() -> &'static Mutex<HashMap<String, HashMap<String, Instant>>> {
    NONCE_STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(test)]
pub(crate) fn reset_nonce_store() {
    if let Some(store) = NONCE_STORE.get() {
        store.lock().unwrap().clear();
    }
}

/// Accept a signature string potentially prefixed (e.g. "ml-dsa-65:<b64>") and return decoded bytes.
/// Keeps prefix-handling logic centralized.
pub fn normalize_and_decode_signature(sig_opt: Option<&str>) -> anyhow::Result<Vec<u8>> {
    let sig_str = sig_opt.unwrap_or("");
    let b64_part = if let Some(idx) = sig_str.find(':') {
        &sig_str[idx + 1..]
    } else {
        sig_str
    };
    base64::engine::general_purpose::STANDARD
        .decode(b64_part)
        .context("failed to base64-decode signature")
}

/// Check replay protection: ensure nonce is not seen in `nonce_window` and insert it.
/// Returns Err if duplicate or invalid.
pub fn check_and_insert_nonce(nonce_str: &str, nonce_window: Duration) -> anyhow::Result<()> {
    check_and_insert_nonce_for_peer(nonce_str, nonce_window, "global")
}

/// Check replay protection for a specific peer: ensure nonce is not seen in `nonce_window` and insert it.
/// Returns Err if duplicate or invalid.
pub fn check_and_insert_nonce_for_peer(
    nonce_str: &str,
    nonce_window: Duration,
    peer_id: &str,
) -> anyhow::Result<()> {
    if nonce_str.is_empty() {
        return Err(anyhow::anyhow!("nonce cannot be empty"));
    }

    let now = Instant::now();
    let mut store = nonce_store().lock().unwrap();

    // Get or create peer-specific nonce store
    let peer_store = store
        .entry(peer_id.to_string())
        .or_insert_with(HashMap::new);

    // prune old nonces for this peer
    peer_store.retain(|_, &mut t| now.duration_since(t) <= nonce_window);

    if peer_store.contains_key(nonce_str) {
        return Err(anyhow::anyhow!("replay detected: nonce already seen"));
    }
    peer_store.insert(nonce_str.to_string(), now);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_nonce_replay_protection() {
        reset_nonce_store();
        let nonce = "test-nonce-unique";
        let window = Duration::from_secs(60);

        // First use should succeed
        assert!(check_and_insert_nonce(nonce, window).is_ok());

        // Second use should fail (replay)
        assert!(check_and_insert_nonce(nonce, window).is_err());
    }

    #[test]
    fn test_signature_prefix_handling() {
        reset_nonce_store();
        // Test with prefix
        let sig_with_prefix = "ml-dsa-65:dGVzdA=="; // "test" in base64
        let decoded = normalize_and_decode_signature(Some(sig_with_prefix)).unwrap();
        assert_eq!(decoded, b"test");

        // Test without prefix
        let sig_without_prefix = "dGVzdA==";
        let decoded = normalize_and_decode_signature(Some(sig_without_prefix)).unwrap();
        assert_eq!(decoded, b"test");
    }
}

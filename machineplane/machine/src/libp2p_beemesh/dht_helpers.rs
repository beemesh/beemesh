use libp2p::PeerId;
use protocol::machine::{build_applied_manifest, SignatureScheme, OperationType};
use serde_json::Value;
use base64::Engine;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use tokio::sync::mpsc;
use log::{info, warn};

use crate::libp2p_beemesh::control::Libp2pControl;

/// Helper function to create and store an AppliedManifest in the DHT after successful deployment
pub async fn store_manifest_in_dht(
    control_tx: &mpsc::UnboundedSender<Libp2pControl>,
    tenant: String,
    operation_id: String,
    local_peer_id: PeerId,
    manifest: Value,
    manifest_kind: String,
) -> Result<String, String> {
    // Generate a content-addressable ID for the manifest
    let manifest_json = serde_json::to_string(&manifest)
        .map_err(|e| format!("Failed to serialize manifest: {}", e))?;

    // Create a stable ID based on tenant, operation_id, and content hash
    let mut hasher = DefaultHasher::new();
    tenant.hash(&mut hasher);
    operation_id.hash(&mut hasher);
    manifest_json.hash(&mut hasher);
    let manifest_id = format!("{:x}", hasher.finish());

    // Create content hash
    let mut content_hasher = DefaultHasher::new();
    manifest_json.hash(&mut content_hasher);
    let content_hash = format!("{:x}", content_hasher.finish());

    // Get current timestamp
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("Failed to get timestamp: {}", e))?
        .as_millis() as u64;

    // Create labels with basic metadata
    let labels = vec![
        ("deployed-by".to_string(), "beemesh-node".to_string()),
        ("kind".to_string(), manifest_kind.clone()),
        ("tenant".to_string(), tenant.clone()),
    ];

    // Build the AppliedManifest FlatBuffer
    // Attempt to extract owner public key and signature from the incoming manifest envelope
    // (the CLI places `pubkey` and `sig` in the JSON envelope). If present, decode base64
    // and embed the raw bytes into the AppliedManifest so other nodes can verify the owner.
    let owner_pubkey_bytes: Vec<u8> = manifest
        .get("pubkey")
        .and_then(|v| v.as_str())
        .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok())
        .unwrap_or_default();

    let owner_sig_str = manifest.get("sig").and_then(|v| v.as_str());
    let owner_sig_bytes = crate::libp2p_beemesh::envelope::normalize_and_decode_signature(
        owner_sig_str,
    ).map_err(|e| format!("failed to decode signature: {:?}", e))?;
    // owner_sig_bytes is now the decoded signature bytes

    let signature_scheme = SignatureScheme::NONE; // CLI uses PQ scheme not represented in enum yet

    let manifest_data = build_applied_manifest(
        &manifest_id,
        &tenant,
        &operation_id,
        &local_peer_id.to_string(),
        &owner_pubkey_bytes,
        signature_scheme,
        &owner_sig_bytes,
        &manifest_json,
        &manifest_kind,
        labels,
        timestamp,
        OperationType::APPLY,
        3600, // 1 hour TTL
        &content_hash,
    );

    // Send the manifest to be stored in the DHT
    let (reply_tx, mut reply_rx) = mpsc::unbounded_channel();
    let store_msg = Libp2pControl::StoreAppliedManifest {
        manifest_data,
        reply_tx,
    };

    control_tx.send(store_msg)
        .map_err(|e| format!("Failed to send DHT store request: {}", e))?;

    // Wait for the result
    match reply_rx.recv().await {
        Some(Ok(())) => {
            info!("Successfully stored manifest {} in DHT", manifest_id);
            Ok(manifest_id)
        }
        Some(Err(e)) => Err(format!("Failed to store manifest in DHT: {}", e)),
        None => Err("DHT store operation was cancelled".to_string()),
    }
}

/// Helper function to retrieve a manifest from the DHT
pub async fn get_manifest_from_dht(
    control_tx: &mpsc::UnboundedSender<Libp2pControl>,
    manifest_id: String,
) -> Result<Option<Vec<u8>>, String> {
    let (reply_tx, mut reply_rx) = mpsc::unbounded_channel();
    let get_msg = Libp2pControl::GetManifestFromDht {
        manifest_id: manifest_id.clone(),
        reply_tx,
    };

    control_tx.send(get_msg)
        .map_err(|e| format!("Failed to send DHT get request: {}", e))?;

    // Wait for the result
    match reply_rx.recv().await {
        Some(Ok(result)) => {
            if result.is_some() {
                info!("Successfully retrieved manifest {} from DHT", manifest_id);
            } else {
                warn!("Manifest {} not found in DHT", manifest_id);
            }
            Ok(result)
        }
        Some(Err(e)) => Err(format!("Failed to get manifest from DHT: {}", e)),
        None => Err("DHT get operation was cancelled".to_string()),
    }
}

/// Helper function to bootstrap the DHT
pub async fn bootstrap_dht(
    control_tx: &mpsc::UnboundedSender<Libp2pControl>,
) -> Result<(), String> {
    let (reply_tx, mut reply_rx) = mpsc::unbounded_channel();
    let bootstrap_msg = Libp2pControl::BootstrapDht { reply_tx };

    control_tx.send(bootstrap_msg)
        .map_err(|e| format!("Failed to send DHT bootstrap request: {}", e))?;

    // Wait for the result
    match reply_rx.recv().await {
        Some(Ok(())) => {
            info!("Successfully bootstrapped DHT");
            Ok(())
        }
        Some(Err(e)) => Err(format!("Failed to bootstrap DHT: {}", e)),
        None => Err("DHT bootstrap operation was cancelled".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::machine::root_as_applied_manifest;

    #[test]
    fn test_build_applied_manifest_includes_owner() {
        let tenant = "t".to_string();
        let operation_id = "op".to_string();
        let local_peer_id = libp2p::PeerId::random();
        let manifest = serde_json::json!({
            "pubkey": base64::engine::general_purpose::STANDARD.encode(&vec![1u8,2u8,3u8]),
            // Use the standardized signature prefix for test fixtures
            "sig": format!("ml-dsa-65:{}", base64::engine::general_purpose::STANDARD.encode(&vec![9u8,8u8,7u8])),
        });

        // Call store_manifest_in_dht connects to control channel; instead, replicate the AppliedManifest builder logic here
        let manifest_json = serde_json::to_string(&manifest).unwrap();
        let mut hasher = DefaultHasher::new();
        tenant.hash(&mut hasher);
        operation_id.hash(&mut hasher);
        manifest_json.hash(&mut hasher);
        let manifest_id = format!("{:x}", hasher.finish());

        let mut content_hasher = DefaultHasher::new();
        manifest_json.hash(&mut content_hasher);
        let content_hash = format!("{:x}", content_hasher.finish());

        let timestamp = 0u64;

        let labels = vec![("k".to_string(), "v".to_string())];

        let owner_pubkey_bytes: Vec<u8> = manifest
            .get("pubkey")
            .and_then(|v| v.as_str())
            .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok())
            .unwrap_or_default();

        let owner_sig_str = manifest.get("sig").and_then(|v| v.as_str());
        let owner_sig_bytes = crate::libp2p_beemesh::envelope::normalize_and_decode_signature(
            owner_sig_str,
        ).unwrap();
        // owner_sig_bytes is now the decoded signature bytes

        let buf = build_applied_manifest(
            &manifest_id,
            &tenant,
            &operation_id,
            &local_peer_id.to_string(),
            &owner_pubkey_bytes,
            SignatureScheme::NONE,
            &owner_sig_bytes,
            &manifest_json,
            "Test",
            labels,
            timestamp,
            OperationType::APPLY,
            3600,
            &content_hash,
        );

        let parsed = root_as_applied_manifest(&buf).expect("parse");
        assert_eq!(parsed.owner_pubkey().unwrap().len(), 3);
        assert_eq!(parsed.signature().unwrap().len(), 3);
    }
}
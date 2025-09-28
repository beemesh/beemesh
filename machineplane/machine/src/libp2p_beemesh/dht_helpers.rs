use libp2p::PeerId;
use protocol::machine::{build_applied_manifest, SignatureScheme, OperationType};
use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use tokio::sync::mpsc;

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
    // Note: In production, you should sign this with your node's private key
    let empty_pubkey = vec![];
    let empty_signature = vec![];

    let manifest_data = build_applied_manifest(
        &manifest_id,
        &tenant,
        &operation_id,
        &local_peer_id.to_string(),
        &empty_pubkey,
        SignatureScheme::NONE,
        &empty_signature,
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
            println!("Successfully stored manifest {} in DHT", manifest_id);
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
                println!("Successfully retrieved manifest {} from DHT", manifest_id);
            } else {
                println!("Manifest {} not found in DHT", manifest_id);
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
            println!("Successfully bootstrapped DHT");
            Ok(())
        }
        Some(Err(e)) => Err(format!("Failed to bootstrap DHT: {}", e)),
        None => Err("DHT bootstrap operation was cancelled".to_string()),
    }
}
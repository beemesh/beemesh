use base64::Engine;
use crypto::{encrypt_payload_for_recipient, ensure_keypair_on_disk};
use log::debug;
use log::error;
use log::info;

use serde_json::Value as JsonValue;
use serde_yaml;
use std::env;
use std::path::PathBuf;

mod flatbuffers;
use flatbuffers::FlatbufferClient;

mod flatbuffer_envelope;
use flatbuffer_envelope::FlatbufferEnvelopeBuilder;

pub async fn apply_file(path: PathBuf) -> anyhow::Result<String> {
    debug!("apply_file called for path: {:?}", path);

    if !path.exists() {
        error!("apply_file: file not found: {}", path.display());
        anyhow::bail!("file not found: {}", path.display());
    }

    let contents = tokio::fs::read_to_string(&path).await?;
    debug!(
        "apply_file: file contents read successfully, length: {}",
        contents.len()
    );
    info!(
        "File contents read successfully, length: {}",
        contents.len()
    );

    // Parse manifest to JSON if possible, else wrap raw
    let manifest_json: JsonValue = match serde_yaml::from_str(&contents) {
        Ok(v) => v,
        Err(_) => serde_json::json!({"raw": contents}),
    };
    debug!("apply_file: manifest parsed successfully");

    // Ensure CLI keypair
    let (pk_bytes, sk_bytes) = ensure_keypair_on_disk()?;
    let pk_b64 = base64::engine::general_purpose::STANDARD.encode(&pk_bytes);

    // Hardcoded tenant for now
    let tenant = "00000000-0000-0000-0000-000000000000";

    // Compute stable manifest_id from manifest content (like Kubernetes)
    let manifest_id = protocol::machine::compute_manifest_id_from_content(&manifest_json, tenant)
        .ok_or_else(|| anyhow::anyhow!("Failed to extract name from manifest"))?;
    debug!("Computed manifest_id: {}", manifest_id);

    // API base URL can be overridden with BEEMESH_API env var
    let base = env::var("BEEMESH_API").unwrap_or_else(|_| "http://127.0.0.1:3000".to_string());
    debug!("Creating FlatbufferClient with base URL: {}", base);
    let mut fb_client = FlatbufferClient::new(base)?;

    // Fetch machine's public key for encrypted communication
    debug!("Fetching machine's public key...");
    fb_client.fetch_machine_public_key().await?;
    debug!("Successfully fetched machine's public key");

    // 1) Get candidates for node selection
    debug!("About to call get_candidates...");
    let peers = fb_client.get_candidates(tenant, &manifest_id).await?;
    debug!(
        "apply_file: get_candidates completed successfully, found {} peers",
        peers.len()
    );

    if peers.is_empty() {
        anyhow::bail!("No candidate nodes available for scheduling");
    }

    // 2) Parse peer ID and public key from candidates response
    // Expected format: "peer_id:pubkey_b64"
    let (winning_node_id, winning_node_pubkey) = {
        let first_peer = &peers[0];
        if let Some(colon_pos) = first_peer.find(':') {
            let peer_id = &first_peer[..colon_pos];
            let pubkey_b64 = &first_peer[colon_pos + 1..];
            (peer_id.to_string(), pubkey_b64.to_string())
        } else {
            anyhow::bail!(
                "Invalid candidate format: expected 'peer_id:pubkey_b64', got '{}'",
                first_peer
            );
        }
    };

    info!("Selected winning node: {}", winning_node_id);
    debug!("Winning node public key: {}", winning_node_pubkey);

    // 3) Encrypt manifest directly for the winning node using its public key
    let manifest_json_str = serde_json::to_string(&manifest_json)?;
    let winning_node_pubkey_bytes = base64::engine::general_purpose::STANDARD
        .decode(&winning_node_pubkey)
        .map_err(|e| anyhow::anyhow!("Failed to decode winning node public key: {}", e))?;

    let encrypted_blob =
        encrypt_payload_for_recipient(&winning_node_pubkey_bytes, manifest_json_str.as_bytes())?;

    // The encrypted_blob already contains the encrypted data, nonce, and KEM ciphertext
    let payload_b64 = base64::engine::general_purpose::STANDARD.encode(&encrypted_blob);

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let encrypted_manifest_bytes = protocol::machine::build_encrypted_manifest(
        "",           // No separate nonce needed - it's in the blob
        &payload_b64, // The complete encrypted blob
        "ml-kem-512", // Using ML-KEM for asymmetric encryption
        1,            // Single recipient
        1,            // Single recipient
        Some("kubernetes"),
        &[],
        ts,
        Some(&winning_node_id), // Include target node info
    );

    // 4) Create task with encrypted manifest
    debug!("Creating task with directly encrypted manifest...");
    let create_resp = fb_client
        .create_task(
            tenant,
            &encrypted_manifest_bytes,
            Some(manifest_id.clone()),
            None,
        )
        .await?;
    debug!("Task created successfully: {:?}", create_resp);

    // 5) Send manifest directly to winning node
    debug!(
        "Sending encrypted manifest directly to winning node: {}",
        winning_node_id
    );

    // Create envelope for direct manifest delivery
    let envelope_builder =
        FlatbufferEnvelopeBuilder::with_keys("cli-client".to_string(), pk_b64.clone());
    let envelope_nonce: [u8; 16] = rand::random();
    let nonce_str = base64::engine::general_purpose::STANDARD.encode(&envelope_nonce);

    let manifest_envelope_bytes = protocol::machine::build_envelope_canonical(
        &encrypted_manifest_bytes,
        "manifest",
        &nonce_str,
        ts,
        "ml-dsa-65",
        None,
    );

    // Sign the manifest envelope
    let signed_manifest_envelope =
        envelope_builder.sign_envelope(&manifest_envelope_bytes, &sk_bytes, &pk_bytes)?;
    let manifest_envelope_b64 =
        base64::engine::general_purpose::STANDARD.encode(&signed_manifest_envelope);

    // Send manifest directly to winning node
    let manifest_dist_resp = fb_client
        .distribute_manifests(
            tenant,
            &manifest_id,
            &manifest_envelope_b64,
            &vec![winning_node_id.clone()],
        )
        .await?;
    debug!(
        "Direct manifest distribution response: {:?}",
        manifest_dist_resp
    );

    // 6) Assign task to winning node
    // For now, assign to the winning node (in the future, this could be multiple nodes for replicas)
    let chosen_peers = vec![winning_node_id.clone()];

    debug!(
        "About to call assign_task with chosen_peers: {:?}",
        chosen_peers
    );
    info!(
        "Assigning task to winning node: tenant={}, manifest_id={}, node={}",
        tenant, &manifest_id, winning_node_id
    );

    let assign_resp = fb_client
        .assign_task(tenant, &manifest_id, chosen_peers)
        .await?;
    debug!("Assign task response: {:?}", assign_resp);

    let ok = assign_resp
        .get("ok")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !ok {
        anyhow::bail!("assign failed");
    }

    debug!("apply_file completed successfully");
    info!(
        "Apply completed successfully for manifest_id {} on winning node {}",
        manifest_id, winning_node_id
    );

    Ok(manifest_id)
}

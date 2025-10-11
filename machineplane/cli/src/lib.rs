use base64::Engine;
use crypto::{encrypt_manifest, ensure_keypair_on_disk, split_symmetric_key, KEY_DIR};
use dirs::home_dir;
use log::debug;
use log::error;
use log::info;
use scheduler::{DistributionConfig, DistributionPlan, SecurityLevel};
use serde_json::Value as JsonValue;
use serde_yaml;
use std::env;
use std::fs;
use std::path::PathBuf;

mod flatbuffers;
use flatbuffers::{CapabilityTarget, FlatbufferClient};

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

    // ensure keypair
    let (pk_bytes, sk_bytes) = ensure_keypair_on_disk()?;

    // encrypt manifest
    let (ciphertext, nonce_bytes, sym, _nonce) = encrypt_manifest(&manifest_json)?;

    // Calculate optimal distribution with default parameters first
    // We'll get actual candidates later and may adjust if needed
    let default_config = DistributionConfig {
        min_threshold: 2,
        max_fault_tolerance: 3,
        security_level: SecurityLevel::Standard,
    };

    // For now, use default 5 candidates assumption (will be refined after getting actual candidates)
    let initial_plan = DistributionPlan::calculate_optimal_distribution(5, &default_config)
        .map_err(|e| anyhow::anyhow!("Failed to calculate initial distribution plan: {}", e))?;

    // Use calculated optimal values for shares
    let n = initial_plan.keyshare_count;
    let k = initial_plan.reconstruction_threshold;

    debug!(
        "apply_file: splitting symmetric key into {} shares with threshold {} (initial plan)",
        n, k
    );
    let shares_vec = split_symmetric_key(&sym, n, k);
    debug!(
        "apply_file: symmetric key split into {} shares",
        shares_vec.len()
    );

    // store shares under ~/.beemesh/shares
    let home = home_dir().ok_or_else(|| anyhow::anyhow!("could not determine home dir"))?;
    let shares_dir = home.join(KEY_DIR).join("shares");
    if !shares_dir.exists() {
        fs::create_dir_all(&shares_dir)?;
    }
    for (i, share) in shares_vec.iter().enumerate() {
        let fname = shares_dir.join(format!("share-{}.bin", i + 1));
        fs::write(&fname, share)?;
    }

    // build encrypted manifest flatbuffer
    let pk_b64 = base64::engine::general_purpose::STANDARD.encode(&pk_bytes);
    let mut envelope_builder =
        FlatbufferEnvelopeBuilder::with_keys("cli-client".to_string(), pk_b64.clone());

    // hardcoded tenant for now
    let tenant = "00000000-0000-0000-0000-000000000000";

    // Generate operation_id for the server
    let operation_id = uuid::Uuid::new_v4().to_string();

    // Create raw EncryptedManifest
    let nonce_b64 = base64::engine::general_purpose::STANDARD.encode(&nonce_bytes);
    let payload_b64 = base64::engine::general_purpose::STANDARD.encode(&ciphertext);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let encrypted_manifest_bytes = protocol::machine::build_encrypted_manifest(
        &nonce_b64,
        &payload_b64,
        "aes-256-gcm",
        k as u32,
        n as u32,
        Some("kubernetes"),
        &[],
        ts,
        None,
    );

    // build shares envelope using flatbuffers (contains the base64 shares for peers)
    let shares_b64: Vec<String> = shares_vec
        .iter()
        .map(|s| base64::engine::general_purpose::STANDARD.encode(s))
        .collect();

    // Compute stable manifest_id from manifest content (like Kubernetes)
    let manifest_id = protocol::machine::compute_manifest_id_from_content(&manifest_json, tenant)
        .ok_or_else(|| anyhow::anyhow!("Failed to extract name from manifest"))?;
    debug!("Computed manifest_id: {}", manifest_id);

    // API base URL can be overridden with BEEMESH_API env var
    let base = env::var("BEEMESH_API").unwrap_or_else(|_| "http://127.0.0.1:3000".to_string());
    debug!(
        "apply_file: creating FlatbufferClient with base URL: {}",
        base
    );
    debug!("Creating FlatbufferClient with base URL: {}", base);
    let mut fb_client = FlatbufferClient::new(base)?;
    debug!("apply_file: FlatbufferClient created");

    // Fetch machine's public key for encrypted communication
    debug!("Fetching machine's public key...");
    fb_client.fetch_machine_public_key().await?;
    debug!("Successfully fetched machine's public key");

    // TODO: Get machine's public key from somewhere (e.g., discovery, config file, etc.)
    // For now, communication will work but won't be encrypted without the machine's pubkey

    // 1) Create task using flatbuffer client (store manifest in task store)
    debug!("About to call create_task...");
    let create_resp = fb_client
        .create_task(
            tenant,
            &encrypted_manifest_bytes,
            Some(manifest_id.clone()), // Use computed manifest_id
            Some(operation_id.clone()),
        )
        .await?;
    debug!("create_task completed successfully");
    debug!("create_task response: {:?}", create_resp);
    println!("CLI: create_task response: {:?}", create_resp);
    let returned_manifest_id = create_resp
        .get("manifest_id")
        .and_then(|v| v.as_str())
        .unwrap_or(&manifest_id) // Fallback to computed manifest_id
        .to_string();
    let returned_task_id = create_resp
        .get("task_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    debug!(
        "create_task returned: manifest_id={}, task_id={}",
        returned_manifest_id, returned_task_id
    );
    println!(
        "CLI: create_task returned: manifest_id={}, task_id={}",
        returned_manifest_id, returned_task_id
    );
    info!("Created task with manifest_id {}", returned_manifest_id);

    // Now rebuild the shares envelope with the correct manifest_id
    let shares_envelope_bytes = envelope_builder.build_shares_envelope(
        &shares_b64,
        n,
        k,
        shares_vec.len(),
        &returned_manifest_id,
    )?;

    // Rebuild signed shares envelope
    let shares_envelope_signed_bytes =
        envelope_builder.sign_envelope(&shares_envelope_bytes, &sk_bytes, &pk_bytes)?;

    // Convert to base64 again
    let shares_envelope_signed_b64 =
        base64::engine::general_purpose::STANDARD.encode(&shares_envelope_signed_bytes);

    // 2) Query candidates using flatbuffer client
    debug!("About to call get_candidates...");
    debug!("get_candidates: using manifest_id={}", returned_manifest_id);
    let peers = fb_client
        .get_candidates(tenant, &returned_manifest_id)
        .await?;
    debug!(
        "apply_file: get_candidates completed successfully, found {} peers",
        peers.len()
    );

    // Recalculate optimal distribution with actual candidate count
    let final_distribution_plan =
        DistributionPlan::calculate_optimal_distribution(peers.len(), &default_config)
            .map_err(|e| anyhow::anyhow!("Failed to calculate final distribution plan: {}", e))?;

    info!(
        "Final distribution plan: keyshares={}, threshold={}, manifests={}, byzantine_ft={} (for {} candidates)",
        final_distribution_plan.keyshare_count,
        final_distribution_plan.reconstruction_threshold,
        final_distribution_plan.manifest_count,
        final_distribution_plan.byzantine_fault_tolerance,
        peers.len()
    );

    // Validate our initial plan was sufficient, warn if not
    if final_distribution_plan.keyshare_count != n
        || final_distribution_plan.reconstruction_threshold != k
    {
        info!(
            "Distribution plan updated from initial: keyshares {}→{}, threshold {}→{}",
            n,
            final_distribution_plan.keyshare_count,
            k,
            final_distribution_plan.reconstruction_threshold
        );
    }
    debug!("get_candidates completed successfully");
    debug!("Candidates: {:?}", peers);

    // 3) Distribute shares to the responders using flatbuffers
    let mut target_flatbuffers: Vec<Vec<u8>> = Vec::new();
    let mut target_peer_ids: Vec<String> = Vec::new();

    // Only send shares to the first n responders (matching the number of shares created)
    for (i, peer) in peers.iter().take(n).enumerate() {
        // read local share file created earlier
        let share_path = home
            .join(KEY_DIR)
            .join("shares")
            .join(format!("share-{}.bin", i + 1));
        if share_path.exists() {
            let share_bytes = tokio::fs::read(&share_path).await?;

            // Create a KeyShares flatbuffer containing just this one share with manifest_id
            let share_b64 = base64::engine::general_purpose::STANDARD.encode(&share_bytes);
            let individual_keyshares_fb = protocol::machine::build_key_shares(
                &[share_b64], // Just this one share
                n as u32,
                k as u32,
                1, // count = 1 for individual share
                &returned_manifest_id,
            );

            // Create a flatbuffer envelope containing the KeyShares data
            let mut envelope_builder =
                FlatbufferEnvelopeBuilder::with_keys("cli-client".to_string(), pk_b64.clone());
            let share_envelope_bytes = envelope_builder.build_simple_envelope(
                "keyshare",
                &base64::engine::general_purpose::STANDARD.encode(&individual_keyshares_fb),
            )?;

            // Sign the share envelope
            let signed_share_envelope =
                envelope_builder.sign_envelope(&share_envelope_bytes, &sk_bytes, &pk_bytes)?;

            target_flatbuffers.push(signed_share_envelope);
            target_peer_ids.push(peer.clone());
        } else {
            log::warn!("missing local share file: {}", share_path.display());
        }
    }

    debug!("About to call distribute_shares_flatbuffer...");
    let dist_resp = fb_client
        .distribute_shares_flatbuffer(
            tenant,
            &returned_manifest_id,
            &shares_envelope_signed_b64,
            &target_peer_ids,
            &target_flatbuffers,
        )
        .await?;
    debug!("Distribute shares response: {:?}", dist_resp);

    // 4) Distribute manifests to the same nodes that received shares
    debug!("About to call distribute_manifests...");

    // Create manifest envelope with the encrypted manifest (use raw bytes, not base64)
    // We need to use the same envelope format as create_task to ensure consistency
    let envelope_nonce: [u8; 16] = rand::random();
    let nonce_str = base64::engine::general_purpose::STANDARD.encode(&envelope_nonce);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
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

    // Create manifest target peers based on final distribution plan
    let manifest_target_peers: Vec<String> = peers
        .iter()
        .take(final_distribution_plan.manifest_count)
        .cloned()
        .collect();

    let manifest_dist_resp = fb_client
        .distribute_manifests(
            tenant,
            &returned_manifest_id,
            &manifest_envelope_b64,
            &manifest_target_peers,
        )
        .await?;
    debug!("Distribute manifests response: {:?}", manifest_dist_resp);

    // 5) Distribute capability tokens using flatbuffers
    // Create capability tokens for keyshare access (for the first n peers that received shares)
    let keyshare_capability_peers: Vec<String> = peers.iter().take(n).cloned().collect();
    debug!(
        "Creating keyshare capability tokens for {} peers: {:?}",
        keyshare_capability_peers.len(),
        keyshare_capability_peers
    );
    let keyshare_capability_targets = create_keyshare_capability_tokens(
        &keyshare_capability_peers,
        &returned_manifest_id,
        &pk_bytes,
        &sk_bytes,
    )
    .await?;
    debug!("About to call distribute_capabilities for keyshare access...");
    let keyshare_capability_resp = fb_client
        .distribute_capabilities(tenant, &returned_manifest_id, &keyshare_capability_targets)
        .await?;
    debug!(
        "Distribute keyshare capabilities response: {:?}",
        keyshare_capability_resp
    );

    debug!(
        "Creating manifest capability tokens for {} peers (plan specifies {} manifests): {:?}",
        manifest_target_peers.len(),
        final_distribution_plan.manifest_count,
        manifest_target_peers
    );
    let manifest_capability_targets = create_manifest_capability_tokens(
        &manifest_target_peers, // Nodes based on distribution plan
        &returned_manifest_id,
        &pk_bytes,
        &sk_bytes,
    )
    .await?;
    debug!("About to call distribute_capabilities for manifest access...");
    let manifest_capability_resp = fb_client
        .distribute_capabilities(tenant, &returned_manifest_id, &manifest_capability_targets)
        .await?;
    debug!(
        "Distribute manifest capabilities response: {:?}",
        manifest_capability_resp
    );

    // 6) Assign task to replicas using flatbuffers
    // Determine replicas desired from manifest
    let replicas = manifest_json
        .get("replicas")
        .and_then(|v| v.as_u64())
        .or_else(|| {
            manifest_json
                .get("spec")
                .and_then(|s| s.get("replicas").and_then(|r| r.as_u64()))
        })
        .unwrap_or(1) as usize;
    let chosen_peers: Vec<String> = peers.into_iter().take(replicas).collect();
    debug!(
        "apply_file: about to call assign_task with {} chosen_peers",
        chosen_peers.len()
    );
    info!(
        "About to call assign_task with tenant={}, manifest_id={}, chosen_peers={:?}",
        tenant, &returned_manifest_id, &chosen_peers
    );

    let assign_resp = fb_client
        .assign_task(tenant, &returned_manifest_id, chosen_peers)
        .await?;
    debug!("Assign response: {:?}", assign_resp);

    let ok = assign_resp
        .get("ok")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !ok {
        anyhow::bail!("assign failed");
    }

    info!("Apply succeeded");

    Ok(returned_manifest_id)
}

/// Create capability tokens for peers using flatbuffers
async fn create_keyshare_capability_tokens(
    peers: &[String],
    manifest_id: &str,
    pk_bytes: &[u8],
    sk_bytes: &[u8],
) -> anyhow::Result<Vec<CapabilityTarget>> {
    // Get current timestamp
    let ts_millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    // Create keyshare capability tokens for each peer
    let mut targets: Vec<CapabilityTarget> = Vec::new();

    for peer_id in peers {
        // Create keyshare capability envelope using flatbuffers
        let pk_b64 = base64::engine::general_purpose::STANDARD.encode(pk_bytes);
        let mut envelope_builder =
            FlatbufferEnvelopeBuilder::with_keys("cli-client".to_string(), pk_b64.clone());
        let capability_envelope_bytes =
            envelope_builder.build_keyshare_capability_envelope(manifest_id, peer_id, ts_millis)?;

        // Sign capability envelope
        let capability_envelope_signed_bytes =
            envelope_builder.sign_envelope(&capability_envelope_bytes, sk_bytes, pk_bytes)?;

        let target = CapabilityTarget {
            peer_id: peer_id.clone(),
            payload: capability_envelope_signed_bytes,
        };
        targets.push(target);
    }

    Ok(targets)
}

async fn create_manifest_capability_tokens(
    peers: &[String],
    manifest_id: &str,
    pk_bytes: &[u8],
    sk_bytes: &[u8],
) -> anyhow::Result<Vec<CapabilityTarget>> {
    // Get current timestamp
    let ts_millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    // Create manifest capability tokens for each peer
    let mut targets: Vec<CapabilityTarget> = Vec::new();

    for peer_id in peers {
        // Create manifest capability envelope using flatbuffers
        let pk_b64 = base64::engine::general_purpose::STANDARD.encode(pk_bytes);
        let mut envelope_builder =
            FlatbufferEnvelopeBuilder::with_keys("cli-client".to_string(), pk_b64.clone());
        let capability_envelope_bytes =
            envelope_builder.build_manifest_capability_envelope(manifest_id, peer_id, ts_millis)?;

        // Sign capability envelope
        let capability_envelope_signed_bytes =
            envelope_builder.sign_envelope(&capability_envelope_bytes, sk_bytes, pk_bytes)?;

        let target = CapabilityTarget {
            peer_id: peer_id.clone(),
            payload: capability_envelope_signed_bytes,
        };
        targets.push(target);
    }

    Ok(targets)
}

/// Create a manifest access capability token
fn create_manifest_access_capability_token(
    manifest_id: String,
    allowed_peer_id: String,
    expires_at: u64,
    issuer_privkey: &[u8],
    issuer_pubkey: Vec<u8>,
) -> anyhow::Result<serde_json::Value> {
    // Create permissions for the token
    let permissions = serde_json::json!({
        "can_read": true,
        "can_execute": true,
        "specific_version": null
    });

    // Create the token data to be signed
    let token_data = format!(
        "{}:{}:{}:{}:{}:{}",
        manifest_id,
        allowed_peer_id,
        expires_at,
        true, // can_read
        true, // can_execute
        0     // specific_version (0 means any)
    );

    // Simple signature (in production, use proper crypto)
    let signature = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        token_data.hash(&mut hasher);
        issuer_privkey.hash(&mut hasher);
        let hash = hasher.finish();
        hash.to_be_bytes().to_vec()
    };

    let token = serde_json::json!({
        "manifest_id": manifest_id,
        "allowed_peer_id": allowed_peer_id,
        "expires_at": expires_at,
        "permissions": permissions,
        "issuer_pubkey": base64::engine::general_purpose::STANDARD.encode(&issuer_pubkey),
        "signature": base64::engine::general_purpose::STANDARD.encode(&signature)
    });

    Ok(token)
}

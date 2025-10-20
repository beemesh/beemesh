use base64::Engine;
use crypto::{encrypt_payload_for_recipient, ensure_keypair_on_disk};
use log::debug;
use log::error;
use log::info;
use scheduler::{NodeCandidate, NodeCapabilities, Scheduler, SchedulerConfig, SchedulingStrategy};
use uuid::Uuid;

use serde_json::Value as JsonValue;
use serde_yaml;
use std::collections::HashMap;
use std::env;
use std::path::PathBuf;

mod flatbuffers;
use flatbuffers::FlatbufferClient;

mod flatbuffer_envelope;

// Helper function to extract manifest name from JSON
fn extract_manifest_name_from_json(manifest_json: &serde_json::Value) -> Option<String> {
    manifest_json
        .get("metadata")?
        .get("name")?
        .as_str()
        .map(|s| s.to_string())
}



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

    // Extract replicas count from manifest (check spec.replicas or top-level replicas, default to 1)
    let replicas = manifest_json
        .get("spec")
        .and_then(|s| s.get("replicas"))
        .and_then(|r| r.as_u64())
        .or_else(|| manifest_json.get("replicas").and_then(|r| r.as_u64()))
        .unwrap_or(1) as usize;

    info!("Manifest requires {} replicas", replicas);

    // Ensure CLI keypair - always use persistent keypairs for consistency
    let (pk_bytes, _sk_bytes) = ensure_keypair_on_disk()?;

    // Hardcoded tenant for now
    let tenant = "00000000-0000-0000-0000-000000000000";

    // Compute stable manifest_id from owning public key and manifest name only
    // This allows manifest content to be updated while keeping the same ID for overwriting
    let manifest_id = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        
        // Hash owning public key for security
        pk_bytes.hash(&mut hasher);
        
        // Extract and hash manifest name - this is the stable identifier
        if let Some(name) = extract_manifest_name_from_json(&manifest_json) {
            debug!("CLI: Using manifest name '{}' for manifest_id", name);
            name.hash(&mut hasher);
        } else {
            return Err(anyhow::anyhow!("Manifest must have a name field in metadata for deployment"));
        }
        
        format!("{:016x}", hasher.finish())[..16].to_string()
    };
    debug!("CLI: Computed manifest_id: {} with pubkey: {:02x?}", manifest_id, &pk_bytes[..8]);

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

    // Ensure we have enough peers for the requested replicas
    if peers.len() < replicas {
        anyhow::bail!(
            "Not enough candidate nodes available: need {}, got {}",
            replicas,
            peers.len()
        );
    }

    // 2) Parse peer IDs and public keys from candidates response and use scheduler
    // Expected format: "peer_id:pubkey_b64"
    let mut node_candidates = Vec::new();
    let mut peer_info_map = HashMap::new();

    for peer in &peers {
        if let Some(colon_pos) = peer.find(':') {
            let peer_id = &peer[..colon_pos];
            let pubkey_b64 = &peer[colon_pos + 1..];

            // Store peer info for later lookup
            peer_info_map.insert(peer_id.to_string(), pubkey_b64.to_string());

            // Create NodeCandidate for scheduler
            let candidate = NodeCandidate {
                node_id: peer_id.to_string(),
                load_factor: 0.0, // We don't have load info, assume available
                available: true,
                capabilities: NodeCapabilities::default(),
            };
            node_candidates.push(candidate);
        } else {
            anyhow::bail!(
                "Invalid candidate format: expected 'peer_id:pubkey_b64', got '{}'",
                peer
            );
        }
    }

    debug!(
        "Parsed {} node candidates from peers",
        node_candidates.len()
    );

    // 3) Use scheduler to select candidates with round-robin strategy
    let scheduler_config = SchedulerConfig {
        strategy: SchedulingStrategy::RoundRobin,
        max_candidates: None,
        enable_load_balancing: false,
    };
    let scheduler = Scheduler::new(scheduler_config);

    debug!(
        "Using scheduler with round-robin strategy to select {} replicas from {} candidates",
        replicas,
        node_candidates.len()
    );
    let scheduling_plan = scheduler.schedule_workload(&node_candidates, replicas)?;
    debug!(
        "Scheduler selected candidate indices: {:?}",
        scheduling_plan.selected_candidates
    );

    // 4) Build selected_nodes list from scheduling plan
    let mut selected_nodes: Vec<(String, String)> = Vec::new();
    for candidate_idx in scheduling_plan.selected_candidates {
        let candidate = &node_candidates[candidate_idx];
        let node_id = &candidate.node_id;
        let pubkey_b64 = peer_info_map
            .get(node_id)
            .ok_or_else(|| anyhow::anyhow!("Missing pubkey for node: {}", node_id))?;
        selected_nodes.push((node_id.clone(), pubkey_b64.clone()));
    }

    info!(
        "Scheduled {} nodes for {} replicas using {:?} strategy: {:?}",
        selected_nodes.len(),
        replicas,
        scheduling_plan.strategy_used,
        selected_nodes.iter().map(|(id, _)| id).collect::<Vec<_>>()
    );
    debug!("Selected nodes with pubkeys: {:?}", selected_nodes);

    // 5) Create encrypted tasks for each node sequentially with same manifest_id
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let mut created_task_ids = Vec::new();

    // Create and assign tasks for each node sequentially to avoid store conflicts
    for (node_id, node_pubkey) in &selected_nodes {
        debug!("Creating encrypted task for node: {}", node_id);

        // Create a modified manifest with replicas=1 for this specific node
        let mut node_manifest = manifest_json.clone();
        if let Some(spec) = node_manifest.get_mut("spec") {
            if let Some(spec_obj) = spec.as_object_mut() {
                spec_obj.insert(
                    "replicas".to_string(),
                    serde_json::Value::Number(serde_json::Number::from(1)),
                );
            }
        } else if manifest_json.get("replicas").is_some() {
            // Handle top-level replicas field
            if let Some(manifest_obj) = node_manifest.as_object_mut() {
                manifest_obj.insert(
                    "replicas".to_string(),
                    serde_json::Value::Number(serde_json::Number::from(1)),
                );
            }
        }

        let node_manifest_str = serde_json::to_string(&node_manifest)?;
        debug!("Node {} will receive manifest with replicas=1", node_id);

        let node_pubkey_bytes = base64::engine::general_purpose::STANDARD
            .decode(node_pubkey)
            .map_err(|e| {
                anyhow::anyhow!("Failed to decode node public key for {}: {}", node_id, e)
            })?;

        // Encrypt the manifest for the recipient using KEM
        let encrypted_blob =
            encrypt_payload_for_recipient(&node_pubkey_bytes, node_manifest_str.as_bytes())?;

        // Create a simple envelope containing the encrypted payload
        let encrypted_manifest_bytes = protocol::machine::build_envelope_canonical_with_peer(
            &encrypted_blob,
            "manifest", // payload_type
            "",         // nonce (empty for now)
            ts,         // timestamp
            "ml-kem-512", // algorithm
            "", // peer_id (empty for now)
            None, // pubkey
        );

        // 4) Send ApplyRequest directly to the target peer via the forwarding endpoint
        debug!(
            "Sending ApplyRequest directly to node {} with manifest_id {}",
            node_id, manifest_id
        );
        
        // Build ApplyRequest with the encrypted manifest
        let operation_id = Uuid::new_v4().to_string();
        let manifest_json_b64 = base64::engine::general_purpose::STANDARD.encode(&encrypted_manifest_bytes);
        
        let apply_request_bytes = protocol::machine::build_apply_request(
            1, // replicas (1 per target peer)
            tenant,
            &operation_id,
            &manifest_json_b64,
            "", // origin_peer (CLI doesn't have peer ID)
            &manifest_id,
        );
        
        // Send via the apply_direct endpoint which forwards to the peer
        let url = format!(
            "{}/tenant/{}/apply_direct/{}",
            fb_client.base_url.trim_end_matches('/'),
            tenant,
            node_id
        );
        
        let response_bytes = fb_client
            .send_encrypted_request(&url, &apply_request_bytes, "apply_request")
            .await?;
            
        // Parse response as ApplyResponse
        let apply_response = protocol::machine::root_as_apply_response(&response_bytes)
            .map_err(|e| anyhow::anyhow!("Failed to parse ApplyResponse: {}", e))?;
            
        if !apply_response.ok() {
            anyhow::bail!("Direct apply failed for node {}: {}", node_id, apply_response.message().unwrap_or("unknown error"));
        }
        
        debug!("Direct apply successful for node {}", node_id);

        created_task_ids.push(manifest_id.clone());

        // Add a small delay to avoid task store conflicts
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    debug!(
        "Successfully created and assigned tasks to {} nodes",
        created_task_ids.len()
    );

    info!(
        "Apply completed for manifest_id {} distributed to {} nodes (all will announce same ID to DHT)",
        manifest_id,
        selected_nodes.len()
    );

    Ok(manifest_id)
}

pub async fn delete_file(path: PathBuf, force: bool) -> anyhow::Result<String> {
    debug!("delete_file called for path: {:?}, force: {}", path, force);

    if !path.exists() {
        error!("delete_file: file not found: {}", path.display());
        anyhow::bail!("file not found: {}", path.display());
    }

    let contents = tokio::fs::read_to_string(&path).await?;
    debug!(
        "delete_file: file contents read successfully, length: {}",
        contents.len()
    );
    info!(
        "File contents read successfully, length: {}",
        contents.len()
    );

    // Parse manifest to JSON if possible
    let manifest_json: JsonValue = match serde_yaml::from_str(&contents) {
        Ok(v) => v,
        Err(_) => serde_json::json!({"raw": contents}),
    };
    debug!("delete_file: manifest parsed successfully");

    // Ensure CLI keypair - always use persistent keypairs for consistency
    let (pk_bytes, _sk_bytes) = ensure_keypair_on_disk()?;

    // Hardcoded tenant for now
    let tenant = "00000000-0000-0000-0000-000000000000";

    // Compute stable manifest_id from owning public key and manifest name only
    // This matches the same logic as apply_file for consistency
    let manifest_id = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        
        // Hash owning public key for security
        pk_bytes.hash(&mut hasher);
        
        // Extract and hash manifest name - this is the stable identifier
        if let Some(name) = extract_manifest_name_from_json(&manifest_json) {
            debug!("CLI: Using manifest name '{}' for manifest_id", name);
            name.hash(&mut hasher);
        } else {
            return Err(anyhow::anyhow!("Manifest must have a name field in metadata for deletion"));
        }
        
        format!("{:016x}", hasher.finish())[..16].to_string()
    };
    debug!("CLI: Computed manifest_id for deletion: {} with pubkey: {:02x?}", manifest_id, &pk_bytes[..8]);

    // API base URL can be overridden with BEEMESH_API env var
    let base = env::var("BEEMESH_API").unwrap_or_else(|_| "http://127.0.0.1:3000".to_string());
    debug!("Creating FlatbufferClient with base URL: {}", base);
    let mut fb_client = FlatbufferClient::new(base)?;

    // Fetch machine's public key for encrypted communication
    debug!("Fetching machine's public key...");
    fb_client.fetch_machine_public_key().await?;
    debug!("Successfully fetched machine's public key");

    // Build delete request
    let operation_id = Uuid::new_v4().to_string();
    let delete_request_bytes = protocol::machine::build_delete_request(
        &manifest_id,
        tenant,
        &operation_id,
        "", // origin_peer (CLI doesn't have peer ID)
        force,
    );

    // Send delete request via REST API
    let url = format!(
        "{}/tenant/{}/tasks/{}",
        fb_client.base_url.trim_end_matches('/'),
        tenant,
        manifest_id
    );

    debug!("Sending delete request to: {}", url);
    
    let body = if force { "force=true" } else { "" };
    let response_bytes = fb_client
        .send_delete_request(&url, body.as_bytes())
        .await?;
    
    // Parse response as DeleteResponse
    let delete_response = protocol::machine::root_as_delete_response(&response_bytes)
        .map_err(|e| anyhow::anyhow!("Failed to parse DeleteResponse: {}", e))?;
    
    if !delete_response.ok() {
        anyhow::bail!("Delete failed: {}", delete_response.message().unwrap_or("unknown error"));
    }
    
    info!(
        "Delete completed for manifest_id {}: {}",
        manifest_id,
        delete_response.message().unwrap_or("success")
    );

    Ok(manifest_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_candidate_parsing_and_scheduling() {
        // Test data in the format returned by get_candidates
        let peers = vec![
            "peer1:dGVzdF9wdWJrZXlfMQ==".to_string(),
            "peer2:dGVzdF9wdWJrZXlfMg==".to_string(),
            "peer3:dGVzdF9wdWJrZXlfMw==".to_string(),
            "peer4:dGVzdF9wdWJrZXlfNA==".to_string(),
        ];

        // Parse candidates
        let mut node_candidates = Vec::new();
        let mut peer_info_map = HashMap::new();

        for peer in &peers {
            if let Some(colon_pos) = peer.find(':') {
                let peer_id = &peer[..colon_pos];
                let pubkey_b64 = &peer[colon_pos + 1..];

                peer_info_map.insert(peer_id.to_string(), pubkey_b64.to_string());

                let candidate = NodeCandidate {
                    node_id: peer_id.to_string(),
                    load_factor: 0.0,
                    available: true,
                    capabilities: NodeCapabilities::default(),
                };
                node_candidates.push(candidate);
            }
        }

        // Test scheduler with round-robin
        let scheduler_config = SchedulerConfig {
            strategy: SchedulingStrategy::RoundRobin,
            max_candidates: None,
            enable_load_balancing: false,
        };
        let scheduler = Scheduler::new(scheduler_config);

        // Schedule 2 replicas
        let replicas = 2;
        let scheduling_plan = scheduler
            .schedule_workload(&node_candidates, replicas)
            .unwrap();

        assert_eq!(scheduling_plan.selected_candidates.len(), replicas);
        assert_eq!(scheduling_plan.total_candidates, peers.len());
        assert_eq!(
            scheduling_plan.strategy_used,
            SchedulingStrategy::RoundRobin
        );

        // Verify selected nodes can be mapped back to peer info
        for candidate_idx in &scheduling_plan.selected_candidates {
            let candidate = &node_candidates[*candidate_idx];
            assert!(peer_info_map.contains_key(&candidate.node_id));
        }
    }

    #[test]
    fn test_invalid_peer_format() {
        let peers = vec!["invalid_peer_format".to_string()];

        // This should result in a format error when parsing
        let mut node_candidates = Vec::new();
        let mut has_error = false;

        for peer in &peers {
            if let Some(colon_pos) = peer.find(':') {
                let peer_id = &peer[..colon_pos];
                let _pubkey_b64 = &peer[colon_pos + 1..];

                let candidate = NodeCandidate {
                    node_id: peer_id.to_string(),
                    load_factor: 0.0,
                    available: true,
                    capabilities: NodeCapabilities::default(),
                };
                node_candidates.push(candidate);
            } else {
                has_error = true;
            }
        }

        assert!(has_error);
        assert!(node_candidates.is_empty());
    }
}

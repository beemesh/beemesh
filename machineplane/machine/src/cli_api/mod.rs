use base64::Engine;
use crypto::{encrypt_payload_for_recipient, ensure_keypair_on_disk};
use log::{debug, error, info};
use scheduler::{NodeCandidate, NodeCapabilities, Scheduler, SchedulerConfig, SchedulingStrategy};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use uuid::Uuid;

mod flatbuffers;
pub use flatbuffers::FlatbufferClient;

mod flatbuffer_envelope;

fn extract_manifest_name_from_json(manifest_json: &serde_json::Value) -> Option<String> {
    manifest_json
        .get("metadata")?
        .get("name")?
        .as_str()
        .map(|s| s.to_string())
}

pub async fn apply_file(path: PathBuf, api_base: Option<&str>) -> anyhow::Result<String> {
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

    let manifest_json: JsonValue = match serde_yaml::from_str(&contents) {
        Ok(v) => v,
        Err(_) => serde_json::json!({"raw": contents}),
    };
    debug!("apply_file: manifest parsed successfully");

    let replicas = manifest_json
        .get("spec")
        .and_then(|s| s.get("replicas"))
        .and_then(|r| r.as_u64())
        .or_else(|| manifest_json.get("replicas").and_then(|r| r.as_u64()))
        .unwrap_or(1) as usize;

    info!("Manifest requires {} replicas", replicas);

    let (pk_bytes, _sk_bytes) = ensure_keypair_on_disk()?;

    let manifest_id = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();

        pk_bytes.hash(&mut hasher);

        if let Some(name) = extract_manifest_name_from_json(&manifest_json) {
            debug!("CLI: Using manifest name '{}' for manifest_id", name);
            name.hash(&mut hasher);
        } else {
            return Err(anyhow::anyhow!(
                "Manifest must have a name field in metadata for deployment"
            ));
        }

        format!("{:016x}", hasher.finish())[..16].to_string()
    };
    debug!(
        "CLI: Computed manifest_id: {} with pubkey: {:02x?}",
        manifest_id,
        &pk_bytes[..8]
    );

    let base = api_base
        .map(|s| s.to_string())
        .or_else(|| env::var("BEEMESH_API").ok())
        .unwrap_or_else(|| "http://127.0.0.1:3000".to_string());
    debug!("Creating FlatbufferClient with base URL: {}", base);
    let mut fb_client = FlatbufferClient::new(base)?;

    debug!("Fetching machine's public key...");
    fb_client.fetch_machine_public_key().await?;
    debug!("Successfully fetched machine's public key");

    debug!("About to call get_candidates...");
    let peers = fb_client.get_candidates(&manifest_id).await?;
    debug!(
        "apply_file: get_candidates completed successfully, found {} peers",
        peers.len()
    );

    if peers.is_empty() {
        anyhow::bail!("No candidate nodes available for scheduling");
    }

    if peers.len() < replicas {
        anyhow::bail!(
            "Not enough candidate nodes available: need {}, got {}",
            replicas,
            peers.len()
        );
    }

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

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let mut created_task_ids = Vec::new();

    let original_manifest_str = serde_json::to_string(&manifest_json)?;

    for (node_id, node_pubkey) in &selected_nodes {
        debug!("Creating encrypted task for node: {}", node_id);

        let node_pubkey_bytes = base64::engine::general_purpose::STANDARD
            .decode(node_pubkey)
            .map_err(|e| {
                anyhow::anyhow!("Failed to decode node public key for {}: {}", node_id, e)
            })?;

        let encrypted_blob =
            encrypt_payload_for_recipient(&node_pubkey_bytes, original_manifest_str.as_bytes())?;

        let encrypted_manifest_bytes = protocol::machine::build_envelope_canonical_with_peer(
            &encrypted_blob,
            "manifest",
            "",
            ts,
            "ml-kem-512",
            "",
            None,
        );

        debug!(
            "Sending ApplyRequest directly to node {} with manifest_id {}",
            node_id, manifest_id
        );

        let operation_id = Uuid::new_v4().to_string();
        let manifest_json_b64 =
            base64::engine::general_purpose::STANDARD.encode(&encrypted_manifest_bytes);

        let apply_request_bytes = protocol::machine::build_apply_request(
            1,
            &operation_id,
            &manifest_json_b64,
            "",
            &manifest_id,
        );

        let url = format!(
            "{}/apply_direct/{}",
            fb_client.base_url.trim_end_matches('/'),
            node_id
        );

        let response_bytes = fb_client
            .send_encrypted_request(&url, &apply_request_bytes, "apply_request")
            .await?;

        let apply_response = protocol::machine::root_as_apply_response(&response_bytes)
            .map_err(|e| anyhow::anyhow!("Failed to parse ApplyResponse: {}", e))?;

        if !apply_response.ok() {
            anyhow::bail!(
                "Direct apply failed for node {}: {}",
                node_id,
                apply_response.message().unwrap_or("unknown error")
            );
        }

        debug!("Direct apply successful for node {}", node_id);

        created_task_ids.push(manifest_id.clone());

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

pub async fn delete_file(
    path: PathBuf,
    force: bool,
    api_base: Option<&str>,
) -> anyhow::Result<String> {
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

    let manifest_json: JsonValue = match serde_yaml::from_str(&contents) {
        Ok(v) => v,
        Err(_) => serde_json::json!({"raw": contents}),
    };
    debug!("delete_file: manifest parsed successfully");

    let (pk_bytes, _sk_bytes) = ensure_keypair_on_disk()?;

    let manifest_id = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();

        pk_bytes.hash(&mut hasher);

        if let Some(name) = extract_manifest_name_from_json(&manifest_json) {
            debug!("CLI: Using manifest name '{}' for manifest_id", name);
            name.hash(&mut hasher);
        } else {
            return Err(anyhow::anyhow!(
                "Manifest must have a name field in metadata for deletion"
            ));
        }

        format!("{:016x}", hasher.finish())[..16].to_string()
    };
    debug!(
        "CLI: Computed manifest_id for deletion: {} with pubkey: {:02x?}",
        manifest_id,
        &pk_bytes[..8]
    );

    let base = api_base
        .map(|s| s.to_string())
        .or_else(|| env::var("BEEMESH_API").ok())
        .unwrap_or_else(|| "http://127.0.0.1:3000".to_string());
    debug!("Creating FlatbufferClient with base URL: {}", base);
    let mut fb_client = FlatbufferClient::new(base)?;

    debug!("Fetching machine's public key...");
    fb_client.fetch_machine_public_key().await?;
    debug!("Successfully fetched machine's public key");

    let operation_id = Uuid::new_v4().to_string();
    let delete_request_bytes =
        protocol::machine::build_delete_request(&manifest_id, &operation_id, "", force);

    let url = format!(
        "{}/tasks/{}",
        fb_client.base_url.trim_end_matches('/'),
        manifest_id
    );

    debug!("Sending delete request to: {}", url);

    let response_bytes = fb_client
        .send_delete_request(&url, &delete_request_bytes)
        .await?;

    let delete_response = protocol::machine::root_as_delete_response(&response_bytes)
        .map_err(|e| anyhow::anyhow!("Failed to parse DeleteResponse: {}", e))?;

    if !delete_response.ok() {
        anyhow::bail!(
            "Delete failed: {}",
            delete_response.message().unwrap_or("unknown error")
        );
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
        let peers = vec![
            "peer1:dGVzdF9wdWJrZXlfMQ==".to_string(),
            "peer2:dGVzdF9wdWJrZXlfMg==".to_string(),
            "peer3:dGVzdF9wdWJrZXlfMw==".to_string(),
            "peer4:dGVzdF9wdWJrZXlfNA==".to_string(),
        ];

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

        let scheduler_config = SchedulerConfig {
            strategy: SchedulingStrategy::RoundRobin,
            max_candidates: None,
            enable_load_balancing: false,
        };
        let scheduler = Scheduler::new(scheduler_config);

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

        for candidate_idx in &scheduling_plan.selected_candidates {
            let candidate = &node_candidates[*candidate_idx];
            assert!(peer_info_map.contains_key(&candidate.node_id));
        }
    }

    #[test]
    fn test_invalid_peer_format() {
        let peers = vec!["invalid_peer_format".to_string()];

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

use anyhow::Result;
use base64::Engine;
use std::collections::HashMap;
use tokio::sync::mpsc;

use machine::libp2p_beemesh::control::Libp2pControl;
use machine::restapi::TaskRecord;
use protocol::machine::{
    build_distribute_manifests_request, root_as_distribute_manifests_response,
};

#[tokio::test]
async fn test_distribute_manifests_request_parsing() -> Result<()> {
    // Test building and parsing distribute manifests request
    let manifest_id = "test-manifest-123";
    let manifest_envelope_json =
        base64::engine::general_purpose::STANDARD.encode(b"test manifest envelope data");

    let targets = vec![
        ("12D3KooWTestPeer1".to_string(), "payload1".to_string()),
        ("12D3KooWTestPeer2".to_string(), "payload2".to_string()),
    ];

    let request_data =
        build_distribute_manifests_request(manifest_id, &manifest_envelope_json, &targets);

    // Parse the request back
    let parsed_request = protocol::machine::root_as_distribute_manifests_request(&request_data)?;

    assert_eq!(parsed_request.manifest_id().unwrap(), manifest_id);
    assert_eq!(
        parsed_request.manifest_envelope_json().unwrap(),
        manifest_envelope_json
    );

    let parsed_targets = parsed_request.targets().unwrap();
    assert_eq!(parsed_targets.len(), 2);

    assert_eq!(
        parsed_targets.get(0).peer_id().unwrap(),
        "12D3KooWTestPeer1"
    );
    assert_eq!(parsed_targets.get(0).payload_json().unwrap(), "payload1");
    assert_eq!(
        parsed_targets.get(1).peer_id().unwrap(),
        "12D3KooWTestPeer2"
    );
    assert_eq!(parsed_targets.get(1).payload_json().unwrap(), "payload2");

    Ok(())
}

#[tokio::test]
async fn test_distribute_manifests_response_building() -> Result<()> {
    // Test building distribute manifests response
    let results = vec![
        ("12D3KooWTestPeer1".to_string(), "delivered".to_string()),
        ("12D3KooWTestPeer2".to_string(), "timeout".to_string()),
        (
            "12D3KooWTestPeer3".to_string(),
            "error: connection failed".to_string(),
        ),
    ];

    let response_data = protocol::machine::build_distribute_manifests_response(true, &results);

    // Parse the response back
    let parsed_response = root_as_distribute_manifests_response(&response_data)?;

    assert_eq!(parsed_response.ok(), true);

    // The response encodes results as JSON string for now
    let results_json = parsed_response.results_json().unwrap();
    let parsed_results: HashMap<String, String> = serde_json::from_str(results_json)?;

    assert_eq!(parsed_results.len(), 3);
    assert_eq!(
        parsed_results.get("12D3KooWTestPeer1").unwrap(),
        "delivered"
    );
    assert_eq!(parsed_results.get("12D3KooWTestPeer2").unwrap(), "timeout");
    assert_eq!(
        parsed_results.get("12D3KooWTestPeer3").unwrap(),
        "error: connection failed"
    );

    Ok(())
}

#[tokio::test]
async fn test_distribute_manifests_task_record_updates() -> Result<()> {
    use std::sync::Arc;
    use tokio::sync::RwLock;

    // Create a mock task store with a test task
    let task_store = Arc::new(RwLock::new(HashMap::new()));
    let task_id = "test-task-123".to_string();

    let task_record = TaskRecord {
        manifest_bytes: vec![1, 2, 3, 4],
        created_at: std::time::SystemTime::now(),
        shares_distributed: HashMap::new(),
        manifests_distributed: HashMap::new(),
        assigned_peers: None,
        manifest_cid: Some(task_id.clone()),
        last_operation_id: None,
        version: 1,
    };

    task_store
        .write()
        .await
        .insert(task_id.clone(), task_record);

    // Simulate manifest distribution updates
    {
        let mut tasks = task_store.write().await;
        if let Some(record) = tasks.get_mut(&task_id) {
            record.manifests_distributed.insert(
                "12D3KooWTestPeer1".to_string(),
                "manifest_payload_1".to_string(),
            );
            record.manifests_distributed.insert(
                "12D3KooWTestPeer2".to_string(),
                "manifest_payload_2".to_string(),
            );
        }
    }

    // Verify the updates
    let tasks = task_store.read().await;
    let updated_record = tasks.get(&task_id).unwrap();

    assert_eq!(updated_record.manifests_distributed.len(), 2);
    assert_eq!(
        updated_record
            .manifests_distributed
            .get("12D3KooWTestPeer1")
            .unwrap(),
        "manifest_payload_1"
    );
    assert_eq!(
        updated_record
            .manifests_distributed
            .get("12D3KooWTestPeer2")
            .unwrap(),
        "manifest_payload_2"
    );

    Ok(())
}

#[tokio::test]
async fn test_send_manifest_control_message() -> Result<()> {
    // Test the SendManifest control message structure
    let peer_id =
        "12D3KooWNvSZnPi3RrhrTwEY4LuuBeB6K6facKUCJci6FRDhbe91".parse::<libp2p::PeerId>()?;
    let manifest_id = "test-manifest-456".to_string();
    let manifest_payload = vec![1, 2, 3, 4, 5];

    let (reply_tx, _reply_rx) = mpsc::unbounded_channel::<Result<(), String>>();

    let control_message = Libp2pControl::SendManifest {
        peer_id,
        manifest_id: manifest_id.clone(),
        manifest_payload: manifest_payload.clone(),
        reply_tx,
    };

    // Verify the control message structure
    match control_message {
        Libp2pControl::SendManifest {
            peer_id: msg_peer_id,
            manifest_id: msg_manifest_id,
            manifest_payload: msg_payload,
            reply_tx: _,
        } => {
            assert_eq!(msg_peer_id, peer_id);
            assert_eq!(msg_manifest_id, manifest_id);
            assert_eq!(msg_payload, manifest_payload);
        }
        _ => panic!("Expected SendManifest control message"),
    }

    Ok(())
}

#[tokio::test]
async fn test_manifest_envelope_base64_encoding() -> Result<()> {
    // Test manifest envelope encoding/decoding
    let test_envelope_data = b"test manifest envelope flatbuffer data";
    let encoded = base64::engine::general_purpose::STANDARD.encode(test_envelope_data);

    // Verify we can decode it back
    let decoded = base64::engine::general_purpose::STANDARD.decode(&encoded)?;
    assert_eq!(decoded, test_envelope_data);

    // Test invalid base64 handling
    let invalid_b64 = "invalid!base64@data#";
    let decode_result = base64::engine::general_purpose::STANDARD.decode(invalid_b64);
    assert!(decode_result.is_err());

    Ok(())
}

#[tokio::test]
async fn test_peer_id_parsing() -> Result<()> {
    // Test valid peer ID parsing
    let valid_peer_id_str = "12D3KooWNvSZnPi3RrhrTwEY4LuuBeB6K6facKUCJci6FRDhbe91";
    let peer_id = valid_peer_id_str.parse::<libp2p::PeerId>()?;
    assert_eq!(peer_id.to_string(), valid_peer_id_str);

    // Test invalid peer ID handling
    let invalid_peer_id_str = "invalid-peer-id";
    let parse_result = invalid_peer_id_str.parse::<libp2p::PeerId>();
    assert!(parse_result.is_err());

    Ok(())
}

#[tokio::test]
async fn test_distribute_manifests_error_responses() -> Result<()> {
    // Test various error response scenarios
    let error_results = vec![
        ("12D3KooWPeer1".to_string(), "timeout".to_string()),
        (
            "12D3KooWPeer2".to_string(),
            "decode_error: invalid base64".to_string(),
        ),
        (
            "invalid-peer".to_string(),
            "invalid_peer_id: invalid format".to_string(),
        ),
    ];

    let response_data =
        protocol::machine::build_distribute_manifests_response(false, &error_results);
    let parsed_response = root_as_distribute_manifests_response(&response_data)?;

    assert_eq!(parsed_response.ok(), false);

    let results_json = parsed_response.results_json().unwrap();
    let parsed_results: HashMap<String, String> = serde_json::from_str(results_json)?;

    assert!(parsed_results
        .get("12D3KooWPeer1")
        .unwrap()
        .contains("timeout"));
    assert!(parsed_results
        .get("12D3KooWPeer2")
        .unwrap()
        .contains("decode_error"));
    assert!(parsed_results
        .get("invalid-peer")
        .unwrap()
        .contains("invalid_peer_id"));

    Ok(())
}

#[tokio::test]
async fn test_manifest_storage_format() -> Result<()> {
    // Test the manifest storage format used in libp2p sends
    let manifest_id = "test-manifest-789";
    let manifest_data = vec![0x12, 0x34, 0x56, 0x78];

    let payload_with_id = format!(
        "{}:{}",
        manifest_id,
        base64::engine::general_purpose::STANDARD.encode(&manifest_data)
    );

    // Parse it back
    if let Some((parsed_id, encoded_data)) = payload_with_id.split_once(':') {
        assert_eq!(parsed_id, manifest_id);

        let decoded_data = base64::engine::general_purpose::STANDARD.decode(encoded_data)?;
        assert_eq!(decoded_data, manifest_data);
    } else {
        panic!("Failed to parse manifest storage format");
    }

    Ok(())
}

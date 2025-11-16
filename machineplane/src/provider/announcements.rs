//! Provider announcement utilities
//!
//! This module contains utilities for creating and managing provider announcements
//! in the distributed hash table (DHT).

use libp2p::kad::RecordKey;
use libp2p::{PeerId, Swarm};
use log::{debug, error, info, warn};
use std::collections::HashMap;
use std::time::SystemTime;

use super::{ProviderError, ProviderResult};
use crate::libp2p_beemesh::behaviour::MyBehaviour;

/// Announcement data for a manifest provider
#[derive(Debug, Clone)]
pub struct ProviderAnnouncement {
    /// The peer ID making the announcement
    pub peer_id: PeerId,
    /// The manifest ID being provided
    pub manifest_id: String,
    /// When the announcement was created
    pub timestamp: SystemTime,
    /// TTL for this announcement (in seconds)
    pub ttl_seconds: u64,
    /// Additional metadata about the provider
    pub metadata: HashMap<String, String>,
    /// Signature of the announcement (for verification)
    pub signature: Option<Vec<u8>>,
}

impl ProviderAnnouncement {
    /// Create a new provider announcement
    pub fn new(
        peer_id: PeerId,
        manifest_id: String,
        ttl_seconds: u64,
        metadata: HashMap<String, String>,
    ) -> Self {
        Self {
            peer_id,
            manifest_id,
            timestamp: SystemTime::now(),
            ttl_seconds,
            metadata,
            signature: None,
        }
    }

    /// Create a provider announcement with signature
    pub fn new_signed(
        peer_id: PeerId,
        manifest_id: String,
        ttl_seconds: u64,
        metadata: HashMap<String, String>,
        signature: Vec<u8>,
    ) -> Self {
        Self {
            peer_id,
            manifest_id,
            timestamp: SystemTime::now(),
            ttl_seconds,
            metadata,
            signature: Some(signature),
        }
    }

    /// Check if this announcement has expired
    pub fn is_expired(&self) -> bool {
        if let Ok(elapsed) = self.timestamp.elapsed() {
            elapsed.as_secs() > self.ttl_seconds
        } else {
            true
        }
    }

    /// Get the DHT key for this announcement
    pub fn get_dht_key(&self) -> String {
        format!("provider:{}", self.manifest_id)
    }

    /// Serialize announcement to bytes for DHT storage
    pub fn to_bytes(&self) -> Vec<u8> {
        // Create a simple serialization format
        // In production, you might want to use a more robust format like FlatBuffers
        let mut data = Vec::new();

        // Add peer ID
        let peer_id_bytes = self.peer_id.to_bytes();
        data.extend_from_slice(&(peer_id_bytes.len() as u32).to_be_bytes());
        data.extend_from_slice(&peer_id_bytes);

        // Add manifest ID
        let manifest_id_bytes = self.manifest_id.as_bytes();
        data.extend_from_slice(&(manifest_id_bytes.len() as u32).to_be_bytes());
        data.extend_from_slice(manifest_id_bytes);

        // Add timestamp
        let timestamp_secs = self
            .timestamp
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        data.extend_from_slice(&timestamp_secs.to_be_bytes());

        // Add TTL
        data.extend_from_slice(&self.ttl_seconds.to_be_bytes());

        // Add metadata
        let metadata_json = serde_json::to_string(&self.metadata).unwrap_or_default();
        let metadata_bytes = metadata_json.as_bytes();
        data.extend_from_slice(&(metadata_bytes.len() as u32).to_be_bytes());
        data.extend_from_slice(metadata_bytes);

        // Add signature if present
        if let Some(ref signature) = self.signature {
            data.push(1); // Has signature flag
            data.extend_from_slice(&(signature.len() as u32).to_be_bytes());
            data.extend_from_slice(signature);
        } else {
            data.push(0); // No signature flag
        }

        data
    }

    /// Deserialize announcement from bytes
    pub fn from_bytes(data: &[u8]) -> Result<Self, Box<dyn std::error::Error>> {
        let mut offset = 0;

        // Read peer ID
        if data.len() < offset + 4 {
            return Err("Invalid data: missing peer ID length".into());
        }
        let peer_id_len = u32::from_be_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]) as usize;
        offset += 4;

        if data.len() < offset + peer_id_len {
            return Err("Invalid data: peer ID too short".into());
        }
        let peer_id = PeerId::from_bytes(&data[offset..offset + peer_id_len])?;
        offset += peer_id_len;

        // Read manifest ID
        if data.len() < offset + 4 {
            return Err("Invalid data: missing manifest ID length".into());
        }
        let manifest_id_len = u32::from_be_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]) as usize;
        offset += 4;

        if data.len() < offset + manifest_id_len {
            return Err("Invalid data: manifest ID too short".into());
        }
        let manifest_id = String::from_utf8(data[offset..offset + manifest_id_len].to_vec())?;
        offset += manifest_id_len;

        // Read timestamp
        if data.len() < offset + 8 {
            return Err("Invalid data: missing timestamp".into());
        }
        let timestamp_secs = u64::from_be_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
            data[offset + 4],
            data[offset + 5],
            data[offset + 6],
            data[offset + 7],
        ]);
        let timestamp = std::time::UNIX_EPOCH + std::time::Duration::from_secs(timestamp_secs);
        offset += 8;

        // Read TTL
        if data.len() < offset + 8 {
            return Err("Invalid data: missing TTL".into());
        }
        let ttl_seconds = u64::from_be_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
            data[offset + 4],
            data[offset + 5],
            data[offset + 6],
            data[offset + 7],
        ]);
        offset += 8;

        // Read metadata
        if data.len() < offset + 4 {
            return Err("Invalid data: missing metadata length".into());
        }
        let metadata_len = u32::from_be_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]) as usize;
        offset += 4;

        if data.len() < offset + metadata_len {
            return Err("Invalid data: metadata too short".into());
        }
        let metadata_json = String::from_utf8(data[offset..offset + metadata_len].to_vec())?;
        let metadata: HashMap<String, String> = serde_json::from_str(&metadata_json)?;
        offset += metadata_len;

        // Read signature flag
        if data.len() < offset + 1 {
            return Err("Invalid data: missing signature flag".into());
        }
        let has_signature = data[offset] == 1;
        offset += 1;

        let signature = if has_signature {
            if data.len() < offset + 4 {
                return Err("Invalid data: missing signature length".into());
            }
            let sig_len = u32::from_be_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]) as usize;
            offset += 4;

            if data.len() < offset + sig_len {
                return Err("Invalid data: signature too short".into());
            }
            Some(data[offset..offset + sig_len].to_vec())
        } else {
            None
        };

        Ok(Self {
            peer_id,
            manifest_id,
            timestamp,
            ttl_seconds,
            metadata,
            signature,
        })
    }
}

/// Utility functions for managing provider announcements
pub struct AnnouncementManager;

impl AnnouncementManager {
    /// Announce a provider in the DHT
    pub fn announce_provider(
        swarm: &mut Swarm<MyBehaviour>,
        manifest_id: &str,
        ttl_seconds: u64,
        metadata: HashMap<String, String>,
    ) -> ProviderResult<()> {
        let local_peer_id = *swarm.local_peer_id();

        info!(
            "Announcing provider for manifest: {} (peer: {}, ttl: {}s)",
            manifest_id, local_peer_id, ttl_seconds
        );

        // Create announcement
        let announcement = ProviderAnnouncement::new(
            local_peer_id,
            manifest_id.to_string(),
            ttl_seconds,
            metadata,
        );

        // Store announcement in DHT
        let record_key = RecordKey::new(&announcement.get_dht_key());
        let record = libp2p::kad::Record {
            key: record_key.clone(),
            value: announcement.to_bytes(),
            publisher: Some(local_peer_id),
            expires: None,
        };

        match swarm
            .behaviour_mut()
            .kademlia
            .put_record(record, libp2p::kad::Quorum::One)
        {
            Ok(query_id) => {
                debug!(
                    "DHT put_record started for provider announcement (query_id: {:?})",
                    query_id
                );
            }
            Err(e) => {
                error!("Failed to store provider announcement in DHT: {}", e);
                return Err(ProviderError::DhtError(format!(
                    "Failed to put record: {}",
                    e
                )));
            }
        }

        // Also start providing the key via Kademlia
        match swarm.behaviour_mut().kademlia.start_providing(record_key) {
            Ok(query_id) => {
                debug!(
                    "Started providing key for manifest {} (query_id: {:?})",
                    manifest_id, query_id
                );
                Ok(())
            }
            Err(e) => {
                error!(
                    "Failed to start providing key for manifest {}: {}",
                    manifest_id, e
                );
                Err(ProviderError::DhtError(format!(
                    "Failed to start providing: {}",
                    e
                )))
            }
        }
    }

    /// Stop announcing a provider
    pub fn stop_announcing(
        swarm: &mut Swarm<MyBehaviour>,
        manifest_id: &str,
    ) -> ProviderResult<()> {
        let provider_key = format!("provider:{}", manifest_id);
        let record_key = RecordKey::new(&provider_key);

        info!(
            "Stopping provider announcement for manifest: {}",
            manifest_id
        );

        // Stop providing the key
        swarm.behaviour_mut().kademlia.stop_providing(&record_key);

        debug!("Stopped providing key for manifest: {}", manifest_id);

        Ok(())
    }

    /// Create metadata for a provider announcement
    pub fn create_metadata(
        node_capabilities: &[String],
        resource_info: Option<&HashMap<String, String>>,
        custom_fields: &HashMap<String, String>,
    ) -> HashMap<String, String> {
        let mut metadata = HashMap::new();

        // Add node capabilities
        if !node_capabilities.is_empty() {
            metadata.insert("capabilities".to_string(), node_capabilities.join(","));
        }

        // Add resource information
        if let Some(resources) = resource_info {
            for (key, value) in resources {
                metadata.insert(format!("resource.{}", key), value.clone());
            }
        }

        // Add custom fields
        for (key, value) in custom_fields {
            metadata.insert(key.clone(), value.clone());
        }

        // Add announcement timestamp
        metadata.insert(
            "announced_at".to_string(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                .to_string(),
        );

        metadata
    }

    /// Validate a provider announcement
    pub fn validate_announcement(announcement: &ProviderAnnouncement) -> bool {
        // Check if expired
        if announcement.is_expired() {
            warn!(
                "Provider announcement expired for manifest: {}",
                announcement.manifest_id
            );
            return false;
        }

        // Check required fields
        if announcement.manifest_id.is_empty() {
            warn!("Provider announcement has empty manifest ID");
            return false;
        }

        // Additional validation could be added here
        // - Signature verification
        // - Peer ID validation
        // - Metadata validation

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libp2p::PeerId;

    #[test]
    fn test_provider_announcement_creation() {
        let peer_id = PeerId::random();
        let manifest_id = "test-manifest".to_string();
        let metadata = HashMap::new();

        let announcement = ProviderAnnouncement::new(peer_id, manifest_id.clone(), 3600, metadata);

        assert_eq!(announcement.peer_id, peer_id);
        assert_eq!(announcement.manifest_id, manifest_id);
        assert_eq!(announcement.ttl_seconds, 3600);
        assert!(!announcement.is_expired());
        assert_eq!(announcement.get_dht_key(), "provider:test-manifest");
    }

    #[test]
    fn test_provider_announcement_serialization() {
        let peer_id = PeerId::random();
        let manifest_id = "test-manifest".to_string();
        let mut metadata = HashMap::new();
        metadata.insert("key1".to_string(), "value1".to_string());
        metadata.insert("key2".to_string(), "value2".to_string());

        let original = ProviderAnnouncement::new(peer_id, manifest_id, 7200, metadata);

        let bytes = original.to_bytes();
        let deserialized = ProviderAnnouncement::from_bytes(&bytes).unwrap();

        assert_eq!(original.peer_id, deserialized.peer_id);
        assert_eq!(original.manifest_id, deserialized.manifest_id);
        assert_eq!(original.ttl_seconds, deserialized.ttl_seconds);
        assert_eq!(original.metadata, deserialized.metadata);
        assert_eq!(original.signature, deserialized.signature);
    }

    #[test]
    fn test_provider_announcement_with_signature() {
        let peer_id = PeerId::random();
        let manifest_id = "test-manifest".to_string();
        let metadata = HashMap::new();
        let signature = vec![1, 2, 3, 4, 5];

        let announcement = ProviderAnnouncement::new_signed(
            peer_id,
            manifest_id,
            3600,
            metadata,
            signature.clone(),
        );

        assert_eq!(announcement.signature, Some(signature));

        // Test serialization with signature
        let bytes = announcement.to_bytes();
        let deserialized = ProviderAnnouncement::from_bytes(&bytes).unwrap();
        assert_eq!(announcement.signature, deserialized.signature);
    }

    #[test]
    fn test_announcement_expiration() {
        let peer_id = PeerId::random();
        let manifest_id = "test-manifest".to_string();
        let metadata = HashMap::new();

        let mut announcement = ProviderAnnouncement::new(
            peer_id,
            manifest_id,
            1, // 1 second TTL
            metadata,
        );

        // Should not be expired initially
        assert!(!announcement.is_expired());

        // Manually set timestamp to past
        announcement.timestamp = SystemTime::now() - std::time::Duration::from_secs(2);

        // Should be expired now
        assert!(announcement.is_expired());
    }

    #[test]
    fn test_create_metadata() {
        let capabilities = vec!["podman".to_string(), "docker".to_string()];
        let mut resources = HashMap::new();
        resources.insert("cpu".to_string(), "4".to_string());
        resources.insert("memory".to_string(), "8GB".to_string());

        let mut custom = HashMap::new();
        custom.insert("region".to_string(), "us-west".to_string());

        let metadata =
            AnnouncementManager::create_metadata(&capabilities, Some(&resources), &custom);

        assert_eq!(
            metadata.get("capabilities"),
            Some(&"podman,docker".to_string())
        );
        assert_eq!(metadata.get("resource.cpu"), Some(&"4".to_string()));
        assert_eq!(metadata.get("resource.memory"), Some(&"8GB".to_string()));
        assert_eq!(metadata.get("region"), Some(&"us-west".to_string()));
        assert!(metadata.contains_key("announced_at"));
    }

    #[test]
    fn test_validate_announcement() {
        let peer_id = PeerId::random();
        let manifest_id = "test-manifest".to_string();
        let metadata = HashMap::new();

        // Valid announcement
        let valid_announcement =
            ProviderAnnouncement::new(peer_id, manifest_id, 3600, metadata.clone());
        assert!(AnnouncementManager::validate_announcement(
            &valid_announcement
        ));

        // Expired announcement
        let mut expired_announcement =
            ProviderAnnouncement::new(peer_id, "test-manifest-2".to_string(), 1, metadata.clone());
        expired_announcement.timestamp = SystemTime::now() - std::time::Duration::from_secs(2);
        assert!(!AnnouncementManager::validate_announcement(
            &expired_announcement
        ));

        // Empty manifest ID
        let invalid_announcement =
            ProviderAnnouncement::new(peer_id, String::new(), 3600, metadata);
        assert!(!AnnouncementManager::validate_announcement(
            &invalid_announcement
        ));
    }
}

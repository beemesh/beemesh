use libp2p::{
    request_response::{self, OutboundRequestId, ResponseChannel},
    PeerId,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// Announcement that a peer holds a specific manifest version
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestAnnouncement {
    pub manifest_id: String,
    pub version: u64,
    pub holder_peer_id: String,
    pub timestamp: u64,
    pub signature: Vec<u8>,
}

/// Query to find holders of a specific manifest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestHolderQuery {
    pub manifest_id: String,
    pub version: Option<u64>, // None means find latest version
}

/// Response containing list of peers that hold the manifest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestHolderResponse {
    pub holders: Vec<ManifestHolderInfo>,
    pub latest_version: Option<u64>,
}

/// Information about a peer that holds a manifest
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ManifestHolderInfo {
    pub peer_id: String,
    pub version: u64,
    pub timestamp: u64,
}

/// Events emitted by the manifest announcement behaviour
#[derive(Debug)]
pub enum ManifestAnnouncementEvent {
    HolderQuery {
        peer: PeerId,
        query: ManifestHolderQuery,
        channel: ResponseChannel<Vec<u8>>,
    },
    HolderResponse {
        peer: PeerId,
        response: ManifestHolderResponse,
    },
    OutboundFailure {
        peer: PeerId,
        request_id: OutboundRequestId,
        error: request_response::OutboundFailure,
    },
}

/// Storage for manifest holder information
#[derive(Debug, Default)]
pub struct ManifestHolderStore {
    manifest_holders: HashMap<String, Vec<ManifestHolderInfo>>,
}

impl ManifestHolderStore {
    /// Create a new manifest holder store
    pub fn new() -> Self {
        Self {
            manifest_holders: HashMap::new(),
        }
    }

    /// Announce that this node holds a manifest
    pub fn announce_manifest_holder(&mut self, manifest_id: String, version: u64) {
        let holder_info = ManifestHolderInfo {
            peer_id: "local".to_string(), // Will be filled with actual peer ID
            version,
            timestamp: current_timestamp(),
        };

        self.manifest_holders
            .entry(manifest_id)
            .or_insert_with(Vec::new)
            .push(holder_info);
    }

    /// Get known holders for a manifest
    pub fn get_manifest_holders(&self, manifest_id: &str) -> Vec<ManifestHolderInfo> {
        self.manifest_holders
            .get(manifest_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Update holder information from network announcements
    pub fn update_holder_info(&mut self, manifest_id: String, holder: ManifestHolderInfo) {
        let holders = self
            .manifest_holders
            .entry(manifest_id)
            .or_insert_with(Vec::new);

        // Remove old entries for this peer
        holders.retain(|h| h.peer_id != holder.peer_id);

        // Add new entry
        holders.push(holder);

        // Keep only the most recent entries (limit to prevent memory growth)
        if holders.len() > 100 {
            holders.sort_by_key(|h| h.timestamp);
            holders.truncate(50);
        }
    }

    /// Process an incoming query and generate response
    pub fn handle_holder_query(&self, query: &ManifestHolderQuery) -> ManifestHolderResponse {
        let holders = self.get_manifest_holders(&query.manifest_id);

        let filtered_holders = if let Some(version) = query.version {
            // Filter by specific version
            holders
                .into_iter()
                .filter(|h| h.version == version)
                .collect()
        } else {
            // Return all versions
            holders
        };

        let latest_version = filtered_holders.iter().map(|h| h.version).max();

        ManifestHolderResponse {
            holders: filtered_holders,
            latest_version,
        }
    }
}

/// Helper function to get current timestamp
fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest_announcement_creation() {
        let announcement = ManifestAnnouncement {
            manifest_id: "test-manifest".to_string(),
            version: 1,
            holder_peer_id: "peer123".to_string(),
            timestamp: current_timestamp(),
            signature: vec![1, 2, 3, 4],
        };

        assert_eq!(announcement.manifest_id, "test-manifest");
        assert_eq!(announcement.version, 1);
    }

    #[test]
    fn test_holder_query() {
        let query = ManifestHolderQuery {
            manifest_id: "test-manifest".to_string(),
            version: Some(1),
        };

        assert_eq!(query.manifest_id, "test-manifest");
        assert_eq!(query.version, Some(1));
    }

    #[test]
    fn test_holder_store_management() {
        let mut store = ManifestHolderStore::new();

        // Announce that we hold a manifest
        store.announce_manifest_holder("test-manifest".to_string(), 1);

        // Check that we can retrieve the holder info
        let holders = store.get_manifest_holders("test-manifest");
        assert_eq!(holders.len(), 1);
        assert_eq!(holders[0].version, 1);
    }

    #[test]
    fn test_holder_info_update() {
        let mut store = ManifestHolderStore::new();

        let holder1 = ManifestHolderInfo {
            peer_id: "peer1".to_string(),
            version: 1,
            timestamp: current_timestamp(),
        };

        let holder2 = ManifestHolderInfo {
            peer_id: "peer1".to_string(), // Same peer, different version
            version: 2,
            timestamp: current_timestamp() + 1,
        };

        store.update_holder_info("test-manifest".to_string(), holder1);
        store.update_holder_info("test-manifest".to_string(), holder2);

        let holders = store.get_manifest_holders("test-manifest");
        assert_eq!(holders.len(), 1); // Should only have one entry per peer
        assert_eq!(holders[0].version, 2); // Should have the latest version
    }
}

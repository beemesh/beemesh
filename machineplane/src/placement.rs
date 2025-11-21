//! Placement system for workload announcements
//!
//! This module provides functionality for nodes to announce their placements
//! of specific workloads.

use libp2p::kad::RecordKey;
use libp2p::{PeerId, Swarm};
use log::{debug, error, info, warn};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use crate::libp2p_beemesh::behaviour::MyBehaviour;

/// Errors that can occur during placement operations
#[derive(Debug, thiserror::Error)]
pub enum PlacementError {
    #[error("DHT error: {0}")]
    DhtError(String),

    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("Placement not found: {0}")]
    PlacementNotFound(String),
}

/// Result type for placement operations
pub type PlacementResult<T> = Result<T, PlacementError>;

/// Information about a manifest placement
#[derive(Debug, Clone)]
pub struct PlacementInfo {
    /// The peer ID of the provider
    pub peer_id: PeerId,
    /// The manifest ID being provided
    pub manifest_id: String,
    /// When this placement was first announced
    pub announced_at: SystemTime,
    /// When this placement was last seen
    pub last_seen: SystemTime,
    /// TTL for this placement announcement (in seconds)
    pub ttl_seconds: u64,
    /// Additional metadata about the placement
    pub metadata: HashMap<String, String>,
}

impl PlacementInfo {
    /// Check if this placement announcement has expired
    pub fn is_expired(&self) -> bool {
        if let Ok(elapsed) = self.announced_at.elapsed() {
            elapsed.as_secs() > self.ttl_seconds
        } else {
            true
        }
    }

    /// Update the last seen timestamp
    pub fn update_last_seen(&mut self) {
        self.last_seen = SystemTime::now();
    }
}

/// Manager for placement announcements
pub struct PlacementManager {
    /// Local placements (manifests this node is hosting)
    local_placements: Arc<Mutex<HashMap<String, PlacementInfo>>>,
    /// Configuration
    config: PlacementConfig,
}

/// Configuration for the placement manager
#[derive(Debug, Clone)]
pub struct PlacementConfig {
    /// Default TTL for placement announcements (in seconds)
    pub default_ttl_seconds: u64,
    /// How often to re-announce local placements (in seconds)
    pub reannounce_interval_seconds: u64,
}

impl Default for PlacementConfig {
    fn default() -> Self {
        Self {
            default_ttl_seconds: 3600,         // 1 hour
            reannounce_interval_seconds: 1800, // 30 minutes
        }
    }
}

impl PlacementManager {
    /// Create a new placement manager
    pub fn new(config: PlacementConfig) -> Self {
        Self {
            local_placements: Arc::new(Mutex::new(HashMap::new())),
            config,
        }
    }

    /// Create a placement manager with default configuration
    pub fn new_default() -> Self {
        Self::new(PlacementConfig::default())
    }

    /// Announce that this node is providing a manifest
    pub fn announce_placement(
        &self,
        swarm: &mut Swarm<MyBehaviour>,
        manifest_id: &str,
        metadata: HashMap<String, String>,
    ) -> PlacementResult<()> {
        let local_peer_id = *swarm.local_peer_id();

        info!(
            "Announcing placement for manifest: {} from peer: {}",
            manifest_id, local_peer_id
        );

        // Create placement info
        let placement_info = PlacementInfo {
            peer_id: local_peer_id,
            manifest_id: manifest_id.to_string(),
            announced_at: SystemTime::now(),
            last_seen: SystemTime::now(),
            ttl_seconds: self.config.default_ttl_seconds,
            metadata,
        };

        // Store locally
        {
            let mut local_placements = self.local_placements.lock().unwrap();
            local_placements.insert(manifest_id.to_string(), placement_info);
        }

        // Announce via DHT
        self.announce_via_dht(swarm, manifest_id)?;

        debug!(
            "Successfully announced placement for manifest: {}",
            manifest_id
        );
        Ok(())
    }

    /// Stop providing a manifest
    pub fn stop_providing(&self, manifest_id: &str) -> PlacementResult<()> {
        info!(
            "Stopping placement announcement for manifest: {}",
            manifest_id
        );

        let mut local_placements = self.local_placements.lock().unwrap();
        if local_placements.remove(manifest_id).is_some() {
            debug!("Removed local placement for manifest: {}", manifest_id);
            Ok(())
        } else {
            warn!(
                "Attempted to stop providing manifest {} but it wasn't being provided",
                manifest_id
            );
            Err(PlacementError::PlacementNotFound(manifest_id.to_string()))
        }
    }

    /// Get all manifests this node is providing
    pub fn get_local_placements(&self) -> Vec<PlacementInfo> {
        let local_placements = self.local_placements.lock().unwrap();
        local_placements.values().cloned().collect()
    }

    /// Re-announce all local placements
    pub fn reannounce_local_placements(&self, swarm: &mut Swarm<MyBehaviour>) {
        debug!("Re-announcing local placements");

        let manifest_ids: Vec<String> = {
            let local_placements = self.local_placements.lock().unwrap();
            local_placements.keys().cloned().collect()
        };

        let manifest_count = manifest_ids.len();

        for manifest_id in manifest_ids {
            if let Err(e) = self.announce_via_dht(swarm, &manifest_id) {
                warn!(
                    "Failed to re-announce placement for manifest {}: {}",
                    manifest_id, e
                );
            }
        }

        debug!("Re-announced {} local placements", manifest_count);
    }

    /// Start background tasks for placement management
    pub fn start_background_tasks(&self, _swarm: &mut Swarm<MyBehaviour>) {
        info!("Started placement manager background tasks");
    }

    /// Internal method to announce via DHT
    fn announce_via_dht(
        &self,
        swarm: &mut Swarm<MyBehaviour>,
        manifest_id: &str,
    ) -> PlacementResult<()> {
        // Create a provider record key
        let provider_key = format!("provider:{}", manifest_id);
        let record_key = RecordKey::new(&provider_key);

        // Start provider announcement
        match swarm.behaviour_mut().kademlia.start_providing(record_key) {
            Ok(query_id) => {
                debug!(
                    "Started DHT placement announcement for manifest {} (query_id: {:?})",
                    manifest_id, query_id
                );
                Ok(())
            }
            Err(e) => {
                error!(
                    "Failed to start DHT placement announcement for manifest {}: {}",
                    manifest_id, e
                );
                Err(PlacementError::DhtError(format!(
                    "Failed to start providing: {}",
                    e
                )))
            }
        }
    }

    /// Get statistics about the placement manager
    pub fn get_stats(&self) -> PlacementStats {
        let local_count = self.local_placements.lock().unwrap().len();
        PlacementStats {
            local_placements: local_count,
        }
    }
}

/// Statistics about the placement manager
#[derive(Debug, Clone)]
pub struct PlacementStats {
    /// Number of manifests this node is providing
    pub local_placements: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use libp2p::PeerId;

    #[test]
    fn test_placement_info_expiration() {
        let mut placement = PlacementInfo {
            peer_id: PeerId::random(),
            manifest_id: "test".to_string(),
            announced_at: SystemTime::now() - Duration::from_secs(3700), // 1 hour 1 minute ago
            last_seen: SystemTime::now(),
            ttl_seconds: 3600, // 1 hour TTL
            metadata: HashMap::new(),
        };

        assert!(placement.is_expired());

        // Update to recent announcement
        placement.announced_at = SystemTime::now();
        assert!(!placement.is_expired());
    }

    #[test]
    fn test_placement_manager_creation() {
        let manager = PlacementManager::new_default();
        let stats = manager.get_stats();

        assert_eq!(stats.local_placements, 0);
    }

    #[test]
    fn test_local_placement_management() {
        let manager = PlacementManager::new_default();

        // Initially no placements
        assert!(manager.get_local_placements().is_empty());

        // Test stop providing non-existent manifest
        assert!(manager.stop_providing("non-existent").is_err());
    }

    #[test]
    fn test_placement_stats() {
        let manager = PlacementManager::new_default();
        let stats = manager.get_stats();

        assert_eq!(stats.local_placements, 0);
    }
}

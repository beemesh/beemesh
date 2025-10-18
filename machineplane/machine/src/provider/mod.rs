//! Provider announcement system for manifest hosting
//!
//! This module provides functionality for nodes to announce themselves as providers
//! of specific manifests and for other nodes to discover which nodes are hosting
//! which manifests. This is more efficient than using gossipsub for discovery.

use libp2p::kad::RecordKey;
use libp2p::{PeerId, Swarm};
use log::{debug, error, info, warn};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc;

use crate::libp2p_beemesh::behaviour::MyBehaviour;

pub mod announcements;
pub mod discovery;

/// Errors that can occur during provider operations
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("DHT error: {0}")]
    DhtError(String),

    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("Provider not found: {0}")]
    ProviderNotFound(String),

    #[error("Timeout waiting for providers")]
    Timeout,
}

/// Result type for provider operations
pub type ProviderResult<T> = Result<T, ProviderError>;

/// Information about a manifest provider
#[derive(Debug, Clone)]
pub struct ProviderInfo {
    /// The peer ID of the provider
    pub peer_id: PeerId,
    /// The manifest ID being provided
    pub manifest_id: String,
    /// When this provider was first announced
    pub announced_at: SystemTime,
    /// When this provider was last seen
    pub last_seen: SystemTime,
    /// TTL for this provider announcement (in seconds)
    pub ttl_seconds: u64,
    /// Additional metadata about the provider
    pub metadata: HashMap<String, String>,
}

impl ProviderInfo {
    /// Check if this provider announcement has expired
    pub fn is_expired(&self) -> bool {
        if let Ok(elapsed) = self.announced_at.elapsed() {
            elapsed.as_secs() > self.ttl_seconds
        } else {
            true // If we can't determine elapsed time, consider expired
        }
    }

    /// Update the last seen timestamp
    pub fn update_last_seen(&mut self) {
        self.last_seen = SystemTime::now();
    }
}

/// Manager for provider announcements and discovery
pub struct ProviderManager {
    /// Local providers (manifests this node is hosting)
    local_providers: Arc<Mutex<HashMap<String, ProviderInfo>>>,
    /// Remote providers (manifests hosted by other nodes)
    remote_providers: Arc<Mutex<HashMap<String, Vec<ProviderInfo>>>>,
    /// Pending provider queries
    pending_queries: Arc<Mutex<HashMap<String, Vec<mpsc::UnboundedSender<Vec<ProviderInfo>>>>>>,
    /// Configuration
    config: ProviderConfig,
}

/// Configuration for the provider manager
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    /// Default TTL for provider announcements (in seconds)
    pub default_ttl_seconds: u64,
    /// How often to re-announce local providers (in seconds)
    pub reannounce_interval_seconds: u64,
    /// How often to clean up expired providers (in seconds)
    pub cleanup_interval_seconds: u64,
    /// Maximum number of providers to track per manifest
    pub max_providers_per_manifest: usize,
    /// Timeout for provider discovery queries (in seconds)
    pub discovery_timeout_seconds: u64,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            default_ttl_seconds: 3600,         // 1 hour
            reannounce_interval_seconds: 1800, // 30 minutes
            cleanup_interval_seconds: 300,     // 5 minutes
            max_providers_per_manifest: 50,
            discovery_timeout_seconds: 30,
        }
    }
}

impl ProviderManager {
    /// Create a new provider manager
    pub fn new(config: ProviderConfig) -> Self {
        Self {
            local_providers: Arc::new(Mutex::new(HashMap::new())),
            remote_providers: Arc::new(Mutex::new(HashMap::new())),
            pending_queries: Arc::new(Mutex::new(HashMap::new())),
            config,
        }
    }

    /// Create a provider manager with default configuration
    pub fn new_default() -> Self {
        Self::new(ProviderConfig::default())
    }

    /// Announce that this node is providing a manifest
    pub fn announce_provider(
        &self,
        swarm: &mut Swarm<MyBehaviour>,
        manifest_id: &str,
        metadata: HashMap<String, String>,
    ) -> ProviderResult<()> {
        let local_peer_id = *swarm.local_peer_id();

        info!(
            "Announcing provider for manifest: {} from peer: {}",
            manifest_id, local_peer_id
        );

        // Create provider info
        let provider_info = ProviderInfo {
            peer_id: local_peer_id,
            manifest_id: manifest_id.to_string(),
            announced_at: SystemTime::now(),
            last_seen: SystemTime::now(),
            ttl_seconds: self.config.default_ttl_seconds,
            metadata,
        };

        // Store locally
        {
            let mut local_providers = self.local_providers.lock().unwrap();
            local_providers.insert(manifest_id.to_string(), provider_info);
        }

        // Announce via DHT
        self.announce_via_dht(swarm, manifest_id)?;

        debug!(
            "Successfully announced provider for manifest: {}",
            manifest_id
        );
        Ok(())
    }

    /// Stop providing a manifest
    pub fn stop_providing(&self, manifest_id: &str) -> ProviderResult<()> {
        info!(
            "Stopping provider announcement for manifest: {}",
            manifest_id
        );

        let mut local_providers = self.local_providers.lock().unwrap();
        if local_providers.remove(manifest_id).is_some() {
            debug!("Removed local provider for manifest: {}", manifest_id);
            Ok(())
        } else {
            warn!(
                "Attempted to stop providing manifest {} but it wasn't being provided",
                manifest_id
            );
            Err(ProviderError::ProviderNotFound(manifest_id.to_string()))
        }
    }

    /// Discover providers for a manifest
    pub async fn discover_providers(
        &self,
        swarm: &mut Swarm<MyBehaviour>,
        manifest_id: &str,
    ) -> ProviderResult<Vec<ProviderInfo>> {
        debug!("Discovering providers for manifest: {}", manifest_id);

        // Check if we have cached providers
        {
            let remote_providers = self.remote_providers.lock().unwrap();
            if let Some(providers) = remote_providers.get(manifest_id) {
                // Filter out expired providers
                let valid_providers: Vec<ProviderInfo> = providers
                    .iter()
                    .filter(|p| !p.is_expired())
                    .cloned()
                    .collect();

                if !valid_providers.is_empty() {
                    debug!(
                        "Found {} cached providers for manifest: {}",
                        valid_providers.len(),
                        manifest_id
                    );
                    return Ok(valid_providers);
                }
            }
        }

        // Query DHT for providers
        self.query_dht_providers(swarm, manifest_id).await
    }

    /// Get all manifests this node is providing
    pub fn get_local_providers(&self) -> Vec<ProviderInfo> {
        let local_providers = self.local_providers.lock().unwrap();
        local_providers.values().cloned().collect()
    }

    /// Get providers for a specific manifest (including expired ones)
    pub fn get_providers_for_manifest(&self, manifest_id: &str) -> Vec<ProviderInfo> {
        let remote_providers = self.remote_providers.lock().unwrap();
        remote_providers
            .get(manifest_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Clean up expired providers
    pub fn cleanup_expired_providers(&self) {
        debug!("Cleaning up expired providers");

        let mut removed_count = 0;
        {
            let mut remote_providers = self.remote_providers.lock().unwrap();
            for (_manifest_id, providers) in remote_providers.iter_mut() {
                let original_len = providers.len();
                providers.retain(|p| !p.is_expired());
                removed_count += original_len - providers.len();
            }

            // Remove manifests with no providers
            remote_providers.retain(|_, providers| !providers.is_empty());
        }

        if removed_count > 0 {
            debug!("Cleaned up {} expired providers", removed_count);
        }
    }

    /// Re-announce all local providers
    pub fn reannounce_local_providers(&self, swarm: &mut Swarm<MyBehaviour>) {
        debug!("Re-announcing local providers");

        let manifest_ids: Vec<String> = {
            let local_providers = self.local_providers.lock().unwrap();
            local_providers.keys().cloned().collect()
        };

        let manifest_count = manifest_ids.len();

        for manifest_id in manifest_ids {
            if let Err(e) = self.announce_via_dht(swarm, &manifest_id) {
                warn!(
                    "Failed to re-announce provider for manifest {}: {}",
                    manifest_id, e
                );
            }
        }

        debug!("Re-announced {} local providers", manifest_count);
    }

    /// Start background tasks for provider management
    pub fn start_background_tasks(&self, _swarm: &mut Swarm<MyBehaviour>) {
        let _local_providers = Arc::clone(&self.local_providers);
        let remote_providers = Arc::clone(&self.remote_providers);
        let config = self.config.clone();

        // Start cleanup task
        {
            let remote_providers_clone = Arc::clone(&remote_providers);
            let cleanup_interval = Duration::from_secs(config.cleanup_interval_seconds);

            tokio::spawn(async move {
                let mut interval = tokio::time::interval(cleanup_interval);
                loop {
                    interval.tick().await;

                    // Clean up expired providers
                    let mut removed_count = 0;
                    {
                        let mut providers = remote_providers_clone.lock().unwrap();
                        for providers_list in providers.values_mut() {
                            let original_len = providers_list.len();
                            providers_list.retain(|p| !p.is_expired());
                            removed_count += original_len - providers_list.len();
                        }
                        providers.retain(|_, providers_list| !providers_list.is_empty());
                    }

                    if removed_count > 0 {
                        debug!(
                            "Background cleanup: removed {} expired providers",
                            removed_count
                        );
                    }
                }
            });
        }

        info!("Started provider manager background tasks");
    }

    /// Internal method to announce via DHT
    fn announce_via_dht(
        &self,
        swarm: &mut Swarm<MyBehaviour>,
        manifest_id: &str,
    ) -> ProviderResult<()> {
        // Create a provider record key
        let provider_key = format!("provider:{}", manifest_id);
        let record_key = RecordKey::new(&provider_key);

        // Start provider announcement
        match swarm.behaviour_mut().kademlia.start_providing(record_key) {
            Ok(query_id) => {
                debug!(
                    "Started DHT provider announcement for manifest {} (query_id: {:?})",
                    manifest_id, query_id
                );
                Ok(())
            }
            Err(e) => {
                error!(
                    "Failed to start DHT provider announcement for manifest {}: {}",
                    manifest_id, e
                );
                Err(ProviderError::DhtError(format!(
                    "Failed to start providing: {}",
                    e
                )))
            }
        }
    }

    /// Internal method to query DHT for providers
    async fn query_dht_providers(
        &self,
        swarm: &mut Swarm<MyBehaviour>,
        manifest_id: &str,
    ) -> ProviderResult<Vec<ProviderInfo>> {
        let provider_key = format!("provider:{}", manifest_id);
        let record_key = RecordKey::new(&provider_key);

        // Create a channel to receive results
        let (tx, mut rx) = mpsc::unbounded_channel();

        // Store the sender for this query
        {
            let mut pending_queries = self.pending_queries.lock().unwrap();
            pending_queries
                .entry(manifest_id.to_string())
                .or_insert_with(Vec::new)
                .push(tx);
        }

        // Start the DHT query
        let query_id = swarm.behaviour_mut().kademlia.get_providers(record_key);
        debug!(
            "Started DHT provider query for manifest {} (query_id: {:?})",
            manifest_id, query_id
        );

        // Wait for results with timeout
        let timeout = Duration::from_secs(self.config.discovery_timeout_seconds);
        match tokio::time::timeout(timeout, rx.recv()).await {
            Ok(Some(providers)) => {
                debug!(
                    "Found {} providers for manifest: {}",
                    providers.len(),
                    manifest_id
                );

                // Cache the results
                {
                    let mut remote_providers = self.remote_providers.lock().unwrap();
                    remote_providers.insert(manifest_id.to_string(), providers.clone());
                }

                Ok(providers)
            }
            Ok(None) => {
                warn!(
                    "Provider query channel closed for manifest: {}",
                    manifest_id
                );
                Ok(Vec::new())
            }
            Err(_) => {
                warn!(
                    "Timeout waiting for providers for manifest: {}",
                    manifest_id
                );
                Err(ProviderError::Timeout)
            }
        }
    }

    /// Handle DHT provider query results (called from libp2p event handler)
    pub fn handle_provider_found(&self, manifest_id: &str, provider_peer: PeerId) {
        debug!(
            "DHT provider found for manifest {}: {}",
            manifest_id, provider_peer
        );

        let provider_info = ProviderInfo {
            peer_id: provider_peer,
            manifest_id: manifest_id.to_string(),
            announced_at: SystemTime::now(),
            last_seen: SystemTime::now(),
            ttl_seconds: self.config.default_ttl_seconds,
            metadata: HashMap::new(),
        };

        // Add to remote providers
        {
            let mut remote_providers = self.remote_providers.lock().unwrap();
            let providers = remote_providers
                .entry(manifest_id.to_string())
                .or_insert_with(Vec::new);

            // Check if we already have this provider
            if let Some(existing) = providers.iter_mut().find(|p| p.peer_id == provider_peer) {
                existing.update_last_seen();
            } else {
                // Limit the number of providers per manifest
                if providers.len() >= self.config.max_providers_per_manifest {
                    // Remove the oldest provider
                    if let Some(oldest_idx) = providers
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, p)| p.last_seen)
                        .map(|(idx, _)| idx)
                    {
                        providers.remove(oldest_idx);
                    }
                }
                providers.push(provider_info);
            }
        }

        // Notify any pending queries
        {
            let mut pending_queries = self.pending_queries.lock().unwrap();
            if let Some(senders) = pending_queries.remove(manifest_id) {
                let providers = self.get_providers_for_manifest(manifest_id);
                for sender in senders {
                    let _ = sender.send(providers.clone());
                }
            }
        }
    }

    /// Get statistics about the provider manager
    pub fn get_stats(&self) -> ProviderStats {
        let local_count = self.local_providers.lock().unwrap().len();
        let (remote_manifests, total_remote_providers) = {
            let remote_providers = self.remote_providers.lock().unwrap();
            let manifests = remote_providers.len();
            let total_providers = remote_providers.values().map(|v| v.len()).sum();
            (manifests, total_providers)
        };
        let pending_queries = self.pending_queries.lock().unwrap().len();

        ProviderStats {
            local_providers: local_count,
            remote_manifests,
            total_remote_providers,
            pending_queries,
        }
    }
}

/// Statistics about the provider manager
#[derive(Debug, Clone)]
pub struct ProviderStats {
    /// Number of manifests this node is providing
    pub local_providers: usize,
    /// Number of remote manifests we know providers for
    pub remote_manifests: usize,
    /// Total number of remote providers tracked
    pub total_remote_providers: usize,
    /// Number of pending discovery queries
    pub pending_queries: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use libp2p::PeerId;

    #[test]
    fn test_provider_info_expiration() {
        let mut provider = ProviderInfo {
            peer_id: PeerId::random(),
            manifest_id: "test".to_string(),
            announced_at: SystemTime::now() - Duration::from_secs(3700), // 1 hour 1 minute ago
            last_seen: SystemTime::now(),
            ttl_seconds: 3600, // 1 hour TTL
            metadata: HashMap::new(),
        };

        assert!(provider.is_expired());

        // Update to recent announcement
        provider.announced_at = SystemTime::now();
        assert!(!provider.is_expired());
    }

    #[test]
    fn test_provider_manager_creation() {
        let manager = ProviderManager::new_default();
        let stats = manager.get_stats();

        assert_eq!(stats.local_providers, 0);
        assert_eq!(stats.remote_manifests, 0);
        assert_eq!(stats.total_remote_providers, 0);
        assert_eq!(stats.pending_queries, 0);
    }

    #[test]
    fn test_local_provider_management() {
        let manager = ProviderManager::new_default();

        // Initially no providers
        assert!(manager.get_local_providers().is_empty());

        // Test stop providing non-existent manifest
        assert!(manager.stop_providing("non-existent").is_err());
    }

    #[test]
    fn test_provider_stats() {
        let manager = ProviderManager::new_default();
        let stats = manager.get_stats();

        assert_eq!(stats.local_providers, 0);
        assert_eq!(stats.remote_manifests, 0);
        assert_eq!(stats.total_remote_providers, 0);
    }

    #[test]
    fn test_cleanup_expired_providers() {
        let manager = ProviderManager::new_default();

        // Add an expired provider
        {
            let mut remote_providers = manager.remote_providers.lock().unwrap();
            let expired_provider = ProviderInfo {
                peer_id: PeerId::random(),
                manifest_id: "test".to_string(),
                announced_at: SystemTime::now() - Duration::from_secs(7200), // 2 hours ago
                last_seen: SystemTime::now(),
                ttl_seconds: 3600, // 1 hour TTL
                metadata: HashMap::new(),
            };
            remote_providers.insert("test".to_string(), vec![expired_provider]);
        }

        // Stats should show 1 remote provider
        assert_eq!(manager.get_stats().total_remote_providers, 1);

        // Cleanup should remove it
        manager.cleanup_expired_providers();
        assert_eq!(manager.get_stats().total_remote_providers, 0);
    }
}

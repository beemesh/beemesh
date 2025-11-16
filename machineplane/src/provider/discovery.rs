//! Provider discovery utilities
//!
//! This module contains utilities for discovering providers of manifests
//! in the distributed hash table (DHT) and managing provider queries.

use libp2p::kad::RecordKey;
use libp2p::{PeerId, Swarm};
use log::{debug, error, info, warn};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc;

use super::{ProviderError, ProviderInfo, ProviderResult};
use crate::libp2p_beemesh::behaviour::MyBehaviour;

/// A discovery query for manifest providers
#[derive(Debug, Clone)]
pub struct DiscoveryQuery {
    /// The manifest ID being queried
    pub manifest_id: String,
    /// When the query was started
    pub started_at: SystemTime,
    /// Maximum time to wait for results
    pub timeout: Duration,
    /// Minimum number of providers to find
    pub min_providers: usize,
    /// Maximum number of providers to return
    pub max_providers: usize,
    /// Whether to include expired providers
    pub include_expired: bool,
}

impl DiscoveryQuery {
    /// Create a new discovery query with default settings
    pub fn new(manifest_id: String) -> Self {
        Self {
            manifest_id,
            started_at: SystemTime::now(),
            timeout: Duration::from_secs(30),
            min_providers: 1,
            max_providers: 10,
            include_expired: false,
        }
    }

    /// Set the timeout for this query
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set the minimum number of providers to find
    pub fn with_min_providers(mut self, min_providers: usize) -> Self {
        self.min_providers = min_providers;
        self
    }

    /// Set the maximum number of providers to return
    pub fn with_max_providers(mut self, max_providers: usize) -> Self {
        self.max_providers = max_providers;
        self
    }

    /// Include expired providers in results
    pub fn include_expired(mut self, include_expired: bool) -> Self {
        self.include_expired = include_expired;
        self
    }

    /// Check if this query has timed out
    pub fn is_timed_out(&self) -> bool {
        self.started_at.elapsed().unwrap_or_default() > self.timeout
    }

    /// Get the DHT key for this query
    pub fn get_dht_key(&self) -> String {
        format!("provider:{}", self.manifest_id)
    }
}

/// Results from a provider discovery query
#[derive(Debug, Clone)]
pub struct DiscoveryResult {
    /// The manifest ID that was queried
    pub manifest_id: String,
    /// List of discovered providers
    pub providers: Vec<ProviderInfo>,
    /// Whether the query completed successfully
    pub success: bool,
    /// Any error that occurred during discovery
    pub error: Option<String>,
    /// How long the discovery took
    pub duration: Duration,
    /// Whether the query timed out
    pub timed_out: bool,
}

/// Manager for discovery operations
pub struct DiscoveryManager {
    /// Active discovery queries
    active_queries: HashMap<String, DiscoveryQuery>,
    /// Channels waiting for query results
    pending_channels: HashMap<String, Vec<mpsc::UnboundedSender<DiscoveryResult>>>,
    /// Cache of recently discovered providers
    provider_cache: HashMap<String, (Vec<ProviderInfo>, SystemTime)>,
    /// Configuration
    config: DiscoveryConfig,
}

/// Configuration for the discovery manager
#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    /// How long to cache discovery results (in seconds)
    pub cache_ttl_seconds: u64,
    /// Maximum number of concurrent discovery queries
    pub max_concurrent_queries: usize,
    /// Default timeout for discovery queries
    pub default_timeout_seconds: u64,
    /// How often to clean up expired cache entries
    pub cache_cleanup_interval_seconds: u64,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            cache_ttl_seconds: 300, // 5 minutes
            max_concurrent_queries: 20,
            default_timeout_seconds: 30,
            cache_cleanup_interval_seconds: 60, // 1 minute
        }
    }
}

impl DiscoveryManager {
    /// Create a new discovery manager
    pub fn new(config: DiscoveryConfig) -> Self {
        Self {
            active_queries: HashMap::new(),
            pending_channels: HashMap::new(),
            provider_cache: HashMap::new(),
            config,
        }
    }

    /// Create a discovery manager with default configuration
    pub fn new_default() -> Self {
        Self::new(DiscoveryConfig::default())
    }

    /// Discover providers for a manifest
    pub async fn discover_providers(
        &mut self,
        swarm: &mut Swarm<MyBehaviour>,
        query: DiscoveryQuery,
    ) -> ProviderResult<DiscoveryResult> {
        let manifest_id = query.manifest_id.clone();

        debug!("Starting provider discovery for manifest: {}", manifest_id);

        // Check cache first
        if let Some(cached_result) = self.get_cached_providers(&manifest_id) {
            if !cached_result.providers.is_empty() {
                debug!(
                    "Found {} cached providers for manifest: {}",
                    cached_result.providers.len(),
                    manifest_id
                );
                return Ok(cached_result);
            }
        }

        // Check if we already have an active query for this manifest
        if self.active_queries.contains_key(&manifest_id) {
            debug!(
                "Discovery query already active for manifest: {}",
                manifest_id
            );
            return self.wait_for_existing_query(&manifest_id).await;
        }

        // Check concurrent query limit
        if self.active_queries.len() >= self.config.max_concurrent_queries {
            warn!(
                "Maximum concurrent queries reached ({}), rejecting discovery request",
                self.config.max_concurrent_queries
            );
            return Err(ProviderError::NetworkError(
                "Too many concurrent queries".to_string(),
            ));
        }

        // Start new discovery query
        self.start_discovery_query(swarm, query).await
    }

    /// Start a new discovery query in the DHT
    async fn start_discovery_query(
        &mut self,
        swarm: &mut Swarm<MyBehaviour>,
        query: DiscoveryQuery,
    ) -> ProviderResult<DiscoveryResult> {
        let manifest_id = query.manifest_id.clone();
        let provider_key = query.get_dht_key();
        let record_key = RecordKey::new(&provider_key);

        // Store the query
        self.active_queries
            .insert(manifest_id.clone(), query.clone());

        // Create result channel
        let (tx, mut rx) = mpsc::unbounded_channel();
        self.pending_channels
            .entry(manifest_id.clone())
            .or_insert_with(Vec::new)
            .push(tx);

        // Start DHT query
        let query_id = swarm.behaviour_mut().kademlia.get_providers(record_key);
        info!(
            "Started DHT provider query for manifest {} (query_id: {:?})",
            manifest_id, query_id
        );

        // Wait for results with timeout
        let timeout = Duration::from_secs(self.config.default_timeout_seconds);
        match tokio::time::timeout(timeout, rx.recv()).await {
            Ok(Some(result)) => {
                debug!(
                    "Discovery completed for manifest {}: {} providers found",
                    manifest_id,
                    result.providers.len()
                );

                // Cache the result
                if result.success && !result.providers.is_empty() {
                    self.cache_providers(&manifest_id, &result.providers);
                }

                Ok(result)
            }
            Ok(None) => {
                warn!("Discovery channel closed for manifest: {}", manifest_id);
                self.cleanup_query(&manifest_id);
                Ok(DiscoveryResult {
                    manifest_id,
                    providers: Vec::new(),
                    success: false,
                    error: Some("Discovery channel closed".to_string()),
                    duration: timeout,
                    timed_out: false,
                })
            }
            Err(_) => {
                warn!("Discovery timeout for manifest: {}", manifest_id);
                self.cleanup_query(&manifest_id);
                Ok(DiscoveryResult {
                    manifest_id,
                    providers: Vec::new(),
                    success: false,
                    error: Some("Discovery timeout".to_string()),
                    duration: timeout,
                    timed_out: true,
                })
            }
        }
    }

    /// Wait for an existing query to complete
    async fn wait_for_existing_query(
        &mut self,
        manifest_id: &str,
    ) -> ProviderResult<DiscoveryResult> {
        let (tx, mut rx) = mpsc::unbounded_channel();
        self.pending_channels
            .entry(manifest_id.to_string())
            .or_insert_with(Vec::new)
            .push(tx);

        let timeout = Duration::from_secs(self.config.default_timeout_seconds);
        match tokio::time::timeout(timeout, rx.recv()).await {
            Ok(Some(result)) => Ok(result),
            Ok(None) => Err(ProviderError::NetworkError(
                "Query channel closed".to_string(),
            )),
            Err(_) => Err(ProviderError::Timeout),
        }
    }

    /// Handle provider discovery results from DHT
    pub fn handle_providers_found(
        &mut self,
        manifest_id: &str,
        providers: Vec<PeerId>,
        query_duration: Duration,
    ) {
        debug!(
            "DHT providers found for manifest {}: {} providers",
            manifest_id,
            providers.len()
        );

        let query = match self.active_queries.get(manifest_id) {
            Some(q) => q.clone(),
            None => {
                warn!("Received providers for unknown query: {}", manifest_id);
                return;
            }
        };

        // Convert PeerIds to ProviderInfo
        let provider_infos: Vec<ProviderInfo> = providers
            .into_iter()
            .map(|peer_id| ProviderInfo {
                peer_id,
                manifest_id: manifest_id.to_string(),
                announced_at: SystemTime::now(),
                last_seen: SystemTime::now(),
                ttl_seconds: 3600, // Default TTL
                metadata: HashMap::new(),
            })
            .collect();

        // Filter providers based on query criteria
        let filtered_providers = self.filter_providers(&query, provider_infos);

        // Create result
        let result = DiscoveryResult {
            manifest_id: manifest_id.to_string(),
            providers: filtered_providers.clone(),
            success: true,
            error: None,
            duration: query_duration,
            timed_out: false,
        };

        // Cache the result
        if !filtered_providers.is_empty() {
            self.cache_providers(manifest_id, &filtered_providers);
        }

        // Notify waiting channels
        if let Some(channels) = self.pending_channels.remove(manifest_id) {
            for channel in channels {
                let _ = channel.send(result.clone());
            }
        }

        // Clean up the query
        self.cleanup_query(manifest_id);
    }

    /// Handle provider discovery errors
    pub fn handle_discovery_error(&mut self, manifest_id: &str, error: String) {
        error!(
            "Provider discovery error for manifest {}: {}",
            manifest_id, error
        );

        let query_duration = self
            .active_queries
            .get(manifest_id)
            .map(|q| q.started_at.elapsed().unwrap_or_default())
            .unwrap_or_default();

        let result = DiscoveryResult {
            manifest_id: manifest_id.to_string(),
            providers: Vec::new(),
            success: false,
            error: Some(error),
            duration: query_duration,
            timed_out: false,
        };

        // Notify waiting channels
        if let Some(channels) = self.pending_channels.remove(manifest_id) {
            for channel in channels {
                let _ = channel.send(result.clone());
            }
        }

        // Clean up the query
        self.cleanup_query(manifest_id);
    }

    /// Filter providers based on query criteria
    fn filter_providers(
        &self,
        query: &DiscoveryQuery,
        mut providers: Vec<ProviderInfo>,
    ) -> Vec<ProviderInfo> {
        // Filter expired providers if requested
        if !query.include_expired {
            providers.retain(|p| !p.is_expired());
        }

        // Remove duplicates
        let mut seen_peers = HashSet::new();
        providers.retain(|p| seen_peers.insert(p.peer_id));

        // Sort by last seen (most recent first)
        providers.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));

        // Limit to max_providers
        if providers.len() > query.max_providers {
            providers.truncate(query.max_providers);
        }

        debug!(
            "Filtered {} providers for manifest: {}",
            providers.len(),
            query.manifest_id
        );

        providers
    }

    /// Get cached providers for a manifest
    fn get_cached_providers(&self, manifest_id: &str) -> Option<DiscoveryResult> {
        if let Some((providers, cached_at)) = self.provider_cache.get(manifest_id) {
            let cache_age = cached_at.elapsed().unwrap_or_default();
            let cache_ttl = Duration::from_secs(self.config.cache_ttl_seconds);

            if cache_age < cache_ttl {
                return Some(DiscoveryResult {
                    manifest_id: manifest_id.to_string(),
                    providers: providers.clone(),
                    success: true,
                    error: None,
                    duration: Duration::from_millis(0), // Cached result
                    timed_out: false,
                });
            }
        }
        None
    }

    /// Cache providers for a manifest
    fn cache_providers(&mut self, manifest_id: &str, providers: &[ProviderInfo]) {
        self.provider_cache.insert(
            manifest_id.to_string(),
            (providers.to_vec(), SystemTime::now()),
        );
        debug!(
            "Cached {} providers for manifest: {}",
            providers.len(),
            manifest_id
        );
    }

    /// Clean up a completed query
    fn cleanup_query(&mut self, manifest_id: &str) {
        self.active_queries.remove(manifest_id);
        debug!("Cleaned up discovery query for manifest: {}", manifest_id);
    }

    /// Clean up expired cache entries
    pub fn cleanup_expired_cache(&mut self) {
        let cache_ttl = Duration::from_secs(self.config.cache_ttl_seconds);
        let mut removed_count = 0;

        self.provider_cache.retain(|_, (_, cached_at)| {
            let keep = cached_at.elapsed().unwrap_or_default() < cache_ttl;
            if !keep {
                removed_count += 1;
            }
            keep
        });

        if removed_count > 0 {
            debug!("Cleaned up {} expired cache entries", removed_count);
        }
    }

    /// Get statistics about the discovery manager
    pub fn get_stats(&self) -> DiscoveryStats {
        DiscoveryStats {
            active_queries: self.active_queries.len(),
            cached_manifests: self.provider_cache.len(),
            pending_channels: self.pending_channels.len(),
        }
    }

    /// Start background cleanup task
    pub fn start_background_cleanup(&self) {
        let cleanup_interval = Duration::from_secs(self.config.cache_cleanup_interval_seconds);

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(cleanup_interval);
            loop {
                interval.tick().await;
                // Note: In a real implementation, you'd need a way to access the
                // DiscoveryManager instance from this background task
                debug!("Background cache cleanup would run here");
            }
        });
    }
}

/// Statistics about the discovery manager
#[derive(Debug, Clone)]
pub struct DiscoveryStats {
    /// Number of active discovery queries
    pub active_queries: usize,
    /// Number of cached manifest results
    pub cached_manifests: usize,
    /// Number of channels waiting for results
    pub pending_channels: usize,
}

/// Utility functions for provider discovery
pub struct DiscoveryUtils;

impl DiscoveryUtils {
    /// Create a basic discovery query
    pub fn create_query(manifest_id: &str) -> DiscoveryQuery {
        DiscoveryQuery::new(manifest_id.to_string())
    }

    /// Create a discovery query with custom settings
    pub fn create_custom_query(
        manifest_id: &str,
        timeout_seconds: u64,
        min_providers: usize,
        max_providers: usize,
    ) -> DiscoveryQuery {
        DiscoveryQuery::new(manifest_id.to_string())
            .with_timeout(Duration::from_secs(timeout_seconds))
            .with_min_providers(min_providers)
            .with_max_providers(max_providers)
    }

    /// Extract unique peer IDs from provider list
    pub fn extract_peer_ids(providers: &[ProviderInfo]) -> Vec<PeerId> {
        let mut peer_ids: Vec<PeerId> = providers.iter().map(|p| p.peer_id).collect();
        peer_ids.sort();
        peer_ids.dedup();
        peer_ids
    }

    /// Group providers by manifest ID
    pub fn group_by_manifest(providers: &[ProviderInfo]) -> HashMap<String, Vec<ProviderInfo>> {
        let mut grouped = HashMap::new();
        for provider in providers {
            grouped
                .entry(provider.manifest_id.clone())
                .or_insert_with(Vec::new)
                .push(provider.clone());
        }
        grouped
    }

    /// Filter providers by metadata criteria
    pub fn filter_by_metadata(
        providers: &[ProviderInfo],
        criteria: &HashMap<String, String>,
    ) -> Vec<ProviderInfo> {
        providers
            .iter()
            .filter(|provider| {
                criteria.iter().all(|(key, value)| {
                    provider
                        .metadata
                        .get(key)
                        .map(|v| v == value)
                        .unwrap_or(false)
                })
            })
            .cloned()
            .collect()
    }

    /// Sort providers by preference (e.g., by last seen, metadata, etc.)
    pub fn sort_by_preference(mut providers: Vec<ProviderInfo>) -> Vec<ProviderInfo> {
        providers.sort_by(|a, b| {
            // Sort by last seen (most recent first)
            b.last_seen
                .cmp(&a.last_seen)
                // Then by announced time (most recent first)
                .then_with(|| b.announced_at.cmp(&a.announced_at))
                // Then by peer ID for deterministic ordering
                .then_with(|| a.peer_id.cmp(&b.peer_id))
        });
        providers
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libp2p::PeerId;

    #[test]
    fn test_discovery_query_creation() {
        let manifest_id = "test-manifest".to_string();
        let query = DiscoveryQuery::new(manifest_id.clone());

        assert_eq!(query.manifest_id, manifest_id);
        assert_eq!(query.timeout, Duration::from_secs(30));
        assert_eq!(query.min_providers, 1);
        assert_eq!(query.max_providers, 10);
        assert!(!query.include_expired);
        assert!(!query.is_timed_out());
        assert_eq!(query.get_dht_key(), "provider:test-manifest");
    }

    #[test]
    fn test_discovery_query_configuration() {
        let query = DiscoveryQuery::new("test".to_string())
            .with_timeout(Duration::from_secs(60))
            .with_min_providers(2)
            .with_max_providers(5)
            .include_expired(true);

        assert_eq!(query.timeout, Duration::from_secs(60));
        assert_eq!(query.min_providers, 2);
        assert_eq!(query.max_providers, 5);
        assert!(query.include_expired);
    }

    #[test]
    fn test_discovery_manager_creation() {
        let manager = DiscoveryManager::new_default();
        let stats = manager.get_stats();

        assert_eq!(stats.active_queries, 0);
        assert_eq!(stats.cached_manifests, 0);
        assert_eq!(stats.pending_channels, 0);
    }

    #[test]
    fn test_discovery_utils_extract_peer_ids() {
        let providers = vec![
            ProviderInfo {
                peer_id: PeerId::random(),
                manifest_id: "test1".to_string(),
                announced_at: SystemTime::now(),
                last_seen: SystemTime::now(),
                ttl_seconds: 3600,
                metadata: HashMap::new(),
            },
            ProviderInfo {
                peer_id: PeerId::random(),
                manifest_id: "test2".to_string(),
                announced_at: SystemTime::now(),
                last_seen: SystemTime::now(),
                ttl_seconds: 3600,
                metadata: HashMap::new(),
            },
        ];

        let peer_ids = DiscoveryUtils::extract_peer_ids(&providers);
        assert_eq!(peer_ids.len(), 2);
    }

    #[test]
    fn test_discovery_utils_group_by_manifest() {
        let peer_id1 = PeerId::random();
        let peer_id2 = PeerId::random();

        let providers = vec![
            ProviderInfo {
                peer_id: peer_id1,
                manifest_id: "test1".to_string(),
                announced_at: SystemTime::now(),
                last_seen: SystemTime::now(),
                ttl_seconds: 3600,
                metadata: HashMap::new(),
            },
            ProviderInfo {
                peer_id: peer_id2,
                manifest_id: "test1".to_string(),
                announced_at: SystemTime::now(),
                last_seen: SystemTime::now(),
                ttl_seconds: 3600,
                metadata: HashMap::new(),
            },
            ProviderInfo {
                peer_id: peer_id1,
                manifest_id: "test2".to_string(),
                announced_at: SystemTime::now(),
                last_seen: SystemTime::now(),
                ttl_seconds: 3600,
                metadata: HashMap::new(),
            },
        ];

        let grouped = DiscoveryUtils::group_by_manifest(&providers);
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped.get("test1").unwrap().len(), 2);
        assert_eq!(grouped.get("test2").unwrap().len(), 1);
    }

    #[test]
    fn test_discovery_utils_filter_by_metadata() {
        let mut metadata1 = HashMap::new();
        metadata1.insert("region".to_string(), "us-west".to_string());
        metadata1.insert("type".to_string(), "podman".to_string());

        let mut metadata2 = HashMap::new();
        metadata2.insert("region".to_string(), "us-east".to_string());
        metadata2.insert("type".to_string(), "podman".to_string());

        let providers = vec![
            ProviderInfo {
                peer_id: PeerId::random(),
                manifest_id: "test1".to_string(),
                announced_at: SystemTime::now(),
                last_seen: SystemTime::now(),
                ttl_seconds: 3600,
                metadata: metadata1,
            },
            ProviderInfo {
                peer_id: PeerId::random(),
                manifest_id: "test2".to_string(),
                announced_at: SystemTime::now(),
                last_seen: SystemTime::now(),
                ttl_seconds: 3600,
                metadata: metadata2,
            },
        ];

        let mut criteria = HashMap::new();
        criteria.insert("region".to_string(), "us-west".to_string());

        let filtered = DiscoveryUtils::filter_by_metadata(&providers, &criteria);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].manifest_id, "test1");
    }

    #[test]
    fn test_discovery_result() {
        let result = DiscoveryResult {
            manifest_id: "test".to_string(),
            providers: Vec::new(),
            success: true,
            error: None,
            duration: Duration::from_millis(100),
            timed_out: false,
        };

        assert_eq!(result.manifest_id, "test");
        assert!(result.success);
        assert!(result.error.is_none());
        assert!(!result.timed_out);
    }
}

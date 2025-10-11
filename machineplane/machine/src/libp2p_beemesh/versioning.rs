use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// Represents a pointer to a manifest with versioning information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestPointer {
    pub manifest_id: String,
    pub owner_pubkey: Vec<u8>,
    pub latest_version: u64,
    pub version_history: Vec<VersionEntry>,
    pub created_at: u64,
    pub updated_at: u64,
}

/// Represents a specific version of a manifest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionEntry {
    pub version: u64,
    pub content_hash: String,
    pub created_at: u64,
    pub changelog: Option<String>,
}

/// Storage for manifest versioning information
#[derive(Debug, Default)]
pub struct VersionStore {
    pointers: HashMap<String, ManifestPointer>,
}

impl VersionStore {
    /// Create a new version store
    pub fn new() -> Self {
        Self {
            pointers: HashMap::new(),
        }
    }

    /// Create a new manifest or update an existing one with a new version
    pub fn create_or_update_manifest(
        &mut self,
        manifest_id: String,
        content_hash: String,
        owner_pubkey: Vec<u8>,
        changelog: Option<String>,
    ) -> Result<u64, String> {
        let current_time = current_timestamp();

        match self.pointers.get_mut(&manifest_id) {
            Some(pointer) => {
                // Verify owner
                if pointer.owner_pubkey != owner_pubkey {
                    return Err("Only the manifest owner can update it".to_string());
                }

                // Create new version
                let new_version = pointer.latest_version + 1;
                let version_entry = VersionEntry {
                    version: new_version,
                    content_hash,
                    created_at: current_time,
                    changelog,
                };

                // Update pointer
                pointer.latest_version = new_version;
                pointer.updated_at = current_time;
                pointer.version_history.push(version_entry);

                // Keep only the last 100 versions to prevent unbounded growth
                if pointer.version_history.len() > 100 {
                    pointer
                        .version_history
                        .drain(0..pointer.version_history.len() - 100);
                }

                Ok(new_version)
            }
            None => {
                // Create new manifest pointer
                let version_entry = VersionEntry {
                    version: 1,
                    content_hash,
                    created_at: current_time,
                    changelog,
                };

                let pointer = ManifestPointer {
                    manifest_id: manifest_id.clone(),
                    owner_pubkey,
                    latest_version: 1,
                    version_history: vec![version_entry],
                    created_at: current_time,
                    updated_at: current_time,
                };

                self.pointers.insert(manifest_id, pointer);
                Ok(1)
            }
        }
    }

    /// Get the latest version number for a manifest
    pub fn get_latest_version(&self, manifest_id: &str) -> Option<u64> {
        self.pointers.get(manifest_id).map(|p| p.latest_version)
    }

    /// Verify that a given pubkey owns a manifest
    pub fn verify_owner(&self, manifest_id: &str, owner_pubkey: &[u8]) -> bool {
        self.pointers
            .get(manifest_id)
            .map(|p| p.owner_pubkey == owner_pubkey)
            .unwrap_or(false)
    }

    /// Get the manifest pointer for a given manifest ID
    pub fn get_manifest_pointer(&self, manifest_id: &str) -> Option<&ManifestPointer> {
        self.pointers.get(manifest_id)
    }

    /// Get a specific version entry
    pub fn get_version_entry(&self, manifest_id: &str, version: u64) -> Option<&VersionEntry> {
        self.pointers
            .get(manifest_id)?
            .version_history
            .iter()
            .find(|v| v.version == version)
    }

    /// Get all versions for a manifest
    pub fn get_all_versions(&self, manifest_id: &str) -> Vec<u64> {
        self.pointers
            .get(manifest_id)
            .map(|p| p.version_history.iter().map(|v| v.version).collect())
            .unwrap_or_default()
    }

    /// Get the content hash for a specific version
    pub fn get_content_hash(&self, manifest_id: &str, version: u64) -> Option<String> {
        self.get_version_entry(manifest_id, version)
            .map(|v| v.content_hash.clone())
    }

    /// Get the total number of manifests tracked
    pub fn len(&self) -> usize {
        self.pointers.len()
    }

    /// Check if the store is empty
    pub fn is_empty(&self) -> bool {
        self.pointers.is_empty()
    }

    /// Remove a manifest completely (all versions)
    pub fn remove_manifest(&mut self, manifest_id: &str) -> Option<ManifestPointer> {
        self.pointers.remove(manifest_id)
    }

    /// Get all manifest IDs
    pub fn get_all_manifest_ids(&self) -> Vec<String> {
        self.pointers.keys().cloned().collect()
    }
}

/// Helper function to get current timestamp
pub fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Generate a deterministic content hash from manifest data
pub fn generate_content_hash(data: &[u8]) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    data.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_store_basic_operations() {
        let mut store = VersionStore::new();
        assert!(store.is_empty());

        let owner_pubkey = vec![1, 2, 3, 4];
        let manifest_id = "test-manifest".to_string();
        let content_hash = "hash1".to_string();

        // Create new manifest
        let version = store
            .create_or_update_manifest(
                manifest_id.clone(),
                content_hash.clone(),
                owner_pubkey.clone(),
                Some("Initial version".to_string()),
            )
            .unwrap();

        assert_eq!(version, 1);
        assert_eq!(store.len(), 1);
        assert!(!store.is_empty());
        assert_eq!(store.get_latest_version(&manifest_id), Some(1));
    }

    #[test]
    fn test_version_updates() {
        let mut store = VersionStore::new();
        let owner_pubkey = vec![1, 2, 3, 4];
        let manifest_id = "test-manifest".to_string();

        // Create initial version
        let v1 = store
            .create_or_update_manifest(
                manifest_id.clone(),
                "hash1".to_string(),
                owner_pubkey.clone(),
                Some("Initial".to_string()),
            )
            .unwrap();
        assert_eq!(v1, 1);

        // Update to version 2
        let v2 = store
            .create_or_update_manifest(
                manifest_id.clone(),
                "hash2".to_string(),
                owner_pubkey.clone(),
                Some("Updated".to_string()),
            )
            .unwrap();
        assert_eq!(v2, 2);
        assert_eq!(store.get_latest_version(&manifest_id), Some(2));

        // Check version history
        let versions = store.get_all_versions(&manifest_id);
        assert_eq!(versions, vec![1, 2]);
    }

    #[test]
    fn test_owner_verification() {
        let mut store = VersionStore::new();
        let owner_pubkey = vec![1, 2, 3, 4];
        let other_pubkey = vec![5, 6, 7, 8];
        let manifest_id = "test-manifest".to_string();

        // Create manifest with owner
        store
            .create_or_update_manifest(
                manifest_id.clone(),
                "hash1".to_string(),
                owner_pubkey.clone(),
                None,
            )
            .unwrap();

        // Verify owner
        assert!(store.verify_owner(&manifest_id, &owner_pubkey));
        assert!(!store.verify_owner(&manifest_id, &other_pubkey));

        // Try to update with wrong owner
        let result = store.create_or_update_manifest(
            manifest_id.clone(),
            "hash2".to_string(),
            other_pubkey,
            None,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Only the manifest owner"));
    }

    #[test]
    fn test_content_hash_generation() {
        let data1 = b"test data 1";
        let data2 = b"test data 2";
        let data1_again = b"test data 1";

        let hash1 = generate_content_hash(data1);
        let hash2 = generate_content_hash(data2);
        let hash1_again = generate_content_hash(data1_again);

        assert_ne!(hash1, hash2);
        assert_eq!(hash1, hash1_again);
    }

    #[test]
    fn test_version_history_limit() {
        let mut store = VersionStore::new();
        let owner_pubkey = vec![1, 2, 3, 4];
        let manifest_id = "test-manifest".to_string();

        // Create 105 versions (should keep only last 100)
        for i in 1..=105 {
            store
                .create_or_update_manifest(
                    manifest_id.clone(),
                    format!("hash{}", i),
                    owner_pubkey.clone(),
                    Some(format!("Version {}", i)),
                )
                .unwrap();
        }

        let pointer = store.get_manifest_pointer(&manifest_id).unwrap();
        assert_eq!(pointer.version_history.len(), 100);
        assert_eq!(pointer.latest_version, 105);

        // Should have versions 6-105 (last 100)
        let versions = store.get_all_versions(&manifest_id);
        assert_eq!(versions.len(), 100);
        assert_eq!(versions[0], 6);
        assert_eq!(versions[99], 105);
    }
}

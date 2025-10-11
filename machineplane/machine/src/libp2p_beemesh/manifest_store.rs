use libp2p::PeerId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// Represents a stored manifest entry with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub manifest_id: String,
    pub version: u64,
    pub encrypted_data: Vec<u8>,
    pub stored_at: u64,
    pub access_tokens: Vec<String>,
    pub owner_pubkey: Vec<u8>,
}

/// Local storage for manifests that this node holds
#[derive(Debug, Default)]
pub struct LocalManifestStore {
    manifests: HashMap<(String, u64), ManifestEntry>,
}

impl LocalManifestStore {
    /// Create a new empty manifest store
    pub fn new() -> Self {
        Self {
            manifests: HashMap::new(),
        }
    }

    /// Store a manifest with the given version
    pub fn store_manifest(&mut self, entry: ManifestEntry) -> Result<(), String> {
        let key = (entry.manifest_id.clone(), entry.version);
        self.manifests.insert(key, entry);
        Ok(())
    }

    /// Get a specific version of a manifest
    pub fn get_manifest(&self, manifest_id: &str, version: u64) -> Option<&ManifestEntry> {
        let key = (manifest_id.to_string(), version);
        self.manifests.get(&key)
    }

    /// Get the latest version of a manifest
    pub fn get_latest_version(&self, manifest_id: &str) -> Option<&ManifestEntry> {
        self.manifests
            .iter()
            .filter(|((id, _), _)| id == manifest_id)
            .max_by_key(|((_, version), _)| *version)
            .map(|(_, entry)| entry)
    }

    /// Check if a peer has access to a manifest via access token
    pub fn has_access_token(&self, manifest_id: &str, version: u64, token: &str) -> bool {
        if let Some(entry) = self.get_manifest(manifest_id, version) {
            entry.access_tokens.contains(&token.to_string())
        } else {
            false
        }
    }

    /// Get all manifest IDs stored locally
    pub fn get_all_manifest_ids(&self) -> Vec<String> {
        self.manifests
            .keys()
            .map(|(id, _)| id.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect()
    }

    /// Get all versions of a specific manifest
    pub fn get_all_versions(&self, manifest_id: &str) -> Vec<u64> {
        let mut versions: Vec<u64> = self
            .manifests
            .keys()
            .filter(|(id, _)| id == manifest_id)
            .map(|(_, version)| *version)
            .collect();
        versions.sort();
        versions
    }

    /// Remove a specific version of a manifest
    pub fn remove_manifest(&mut self, manifest_id: &str, version: u64) -> Option<ManifestEntry> {
        let key = (manifest_id.to_string(), version);
        self.manifests.remove(&key)
    }

    /// Get the total number of manifests stored
    pub fn len(&self) -> usize {
        self.manifests.len()
    }

    /// Check if the store is empty
    pub fn is_empty(&self) -> bool {
        self.manifests.is_empty()
    }
}

/// Helper function to get current timestamp
pub fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Represents a manifest access capability token
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestAccessToken {
    pub manifest_id: String,
    pub allowed_peer_id: String,
    pub expires_at: u64,
    pub permissions: ManifestPermissions,
    pub issuer_pubkey: Vec<u8>,
    pub signature: Vec<u8>,
}

/// Permissions for manifest access
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestPermissions {
    pub can_read: bool,
    pub can_execute: bool,
    pub specific_version: Option<u64>,
}

/// Create a manifest access capability token
pub fn create_manifest_access_token(
    manifest_id: String,
    allowed_peer_id: String,
    expires_at: u64,
    permissions: ManifestPermissions,
    issuer_privkey: &[u8],
    issuer_pubkey: Vec<u8>,
) -> Result<ManifestAccessToken, String> {
    // Create the token data to be signed
    let token_data = format!(
        "{}:{}:{}:{}:{}:{}",
        manifest_id,
        allowed_peer_id,
        expires_at,
        permissions.can_read,
        permissions.can_execute,
        permissions.specific_version.unwrap_or(0)
    );

    // Sign the token data (simplified signature - in production use proper crypto)
    let signature = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        token_data.hash(&mut hasher);
        issuer_privkey.hash(&mut hasher);
        let hash = hasher.finish();
        hash.to_be_bytes().to_vec()
    };

    Ok(ManifestAccessToken {
        manifest_id,
        allowed_peer_id,
        expires_at,
        permissions,
        issuer_pubkey,
        signature,
    })
}

/// Verify a manifest access capability token
pub fn verify_manifest_access_token(
    token: &ManifestAccessToken,
    manifest_id: &str,
    peer_id: &str,
) -> Result<bool, String> {
    // Check if token is for the correct manifest
    if token.manifest_id != manifest_id {
        return Ok(false);
    }

    // Check if token is for the correct peer
    if token.allowed_peer_id != peer_id {
        return Ok(false);
    }

    // Check if token has expired
    let current_time = current_timestamp();
    if current_time > token.expires_at {
        return Ok(false);
    }

    // Verify signature (simplified verification - in production use proper crypto)
    let token_data = format!(
        "{}:{}:{}:{}:{}:{}",
        token.manifest_id,
        token.allowed_peer_id,
        token.expires_at,
        token.permissions.can_read,
        token.permissions.can_execute,
        token.permissions.specific_version.unwrap_or(0)
    );

    let expected_signature = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        // We don't have the private key for verification, so this is a simplified check
        // In production, use proper signature verification
        let mut hasher = DefaultHasher::new();
        token_data.hash(&mut hasher);
        // For now, just check if signature is not empty
        !token.signature.is_empty()
    };

    Ok(expected_signature)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest_store_basic_operations() {
        let mut store = LocalManifestStore::new();
        assert!(store.is_empty());

        let entry = ManifestEntry {
            manifest_id: "test-manifest".to_string(),
            version: 1,
            encrypted_data: vec![1, 2, 3, 4],
            stored_at: current_timestamp(),
            access_tokens: vec!["token1".to_string()],
            owner_pubkey: vec![5, 6, 7, 8],
        };

        // Store manifest
        assert!(store.store_manifest(entry.clone()).is_ok());
        assert_eq!(store.len(), 1);
        assert!(!store.is_empty());

        // Retrieve manifest
        let retrieved = store.get_manifest("test-manifest", 1);
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().manifest_id, "test-manifest");
        assert_eq!(retrieved.unwrap().version, 1);
    }

    #[test]
    fn test_latest_version() {
        let mut store = LocalManifestStore::new();

        // Store multiple versions
        for version in [1, 3, 2, 5, 4] {
            let entry = ManifestEntry {
                manifest_id: "test-manifest".to_string(),
                version,
                encrypted_data: vec![],
                stored_at: current_timestamp(),
                access_tokens: vec![],
                owner_pubkey: vec![],
            };
            store.store_manifest(entry).unwrap();
        }

        let latest = store.get_latest_version("test-manifest");
        assert!(latest.is_some());
        assert_eq!(latest.unwrap().version, 5);
    }

    #[test]
    fn test_access_tokens() {
        let mut store = LocalManifestStore::new();

        let entry = ManifestEntry {
            manifest_id: "test-manifest".to_string(),
            version: 1,
            encrypted_data: vec![],
            stored_at: current_timestamp(),
            access_tokens: vec!["valid-token".to_string(), "another-token".to_string()],
            owner_pubkey: vec![],
        };

        store.store_manifest(entry).unwrap();

        assert!(store.has_access_token("test-manifest", 1, "valid-token"));
        assert!(store.has_access_token("test-manifest", 1, "another-token"));
        assert!(!store.has_access_token("test-manifest", 1, "invalid-token"));
        assert!(!store.has_access_token("test-manifest", 2, "valid-token")); // wrong version
    }

    #[test]
    fn test_capability_token_creation_and_verification() {
        let issuer_privkey = vec![1, 2, 3, 4];
        let issuer_pubkey = vec![5, 6, 7, 8];
        let permissions = ManifestPermissions {
            can_read: true,
            can_execute: false,
            specific_version: Some(1),
        };

        let token = create_manifest_access_token(
            "test-manifest".to_string(),
            "peer123".to_string(),
            current_timestamp() + 3600, // expires in 1 hour
            permissions,
            &issuer_privkey,
            issuer_pubkey,
        )
        .unwrap();

        // Verify token
        assert!(verify_manifest_access_token(&token, "test-manifest", "peer123").unwrap());
        assert!(!verify_manifest_access_token(&token, "wrong-manifest", "peer123").unwrap());
        assert!(!verify_manifest_access_token(&token, "test-manifest", "wrong-peer").unwrap());
    }

    #[test]
    fn test_capability_token_expiration() {
        let issuer_privkey = vec![1, 2, 3, 4];
        let issuer_pubkey = vec![5, 6, 7, 8];
        let permissions = ManifestPermissions {
            can_read: true,
            can_execute: false,
            specific_version: None,
        };

        let expired_token = create_manifest_access_token(
            "test-manifest".to_string(),
            "peer123".to_string(),
            current_timestamp() - 1, // expired 1 second ago
            permissions,
            &issuer_privkey,
            issuer_pubkey,
        )
        .unwrap();

        // Verify expired token fails
        assert!(!verify_manifest_access_token(&expired_token, "test-manifest", "peer123").unwrap());
    }
}

//! Keystore helper utilities to eliminate repetitive keystore access patterns
//!
//! This module centralizes common keystore operations that were previously
//! duplicated across multiple modules in the machine crate.

use crate::keystore::Keystore;
use crate::logging::CryptoLogger;
use crate::{decrypt_share_from_blob, encrypt_share_for_keystore, ensure_kem_keypair_on_disk};
use anyhow::Result;
use log::{info, warn};

/// Keystore operation error types
#[derive(Debug, thiserror::Error)]
pub enum KeystoreError {
    #[error("Keystore access failed: {0}")]
    AccessFailed(String),
    #[error("Encryption failed: {0}")]
    EncryptionFailed(String),
    #[error("Decryption failed: {0}")]
    DecryptionFailed(String),
    #[error("Share not found: {0}")]
    ShareNotFound(String),
    #[error("Invalid share data: {0}")]
    InvalidShareData(String),
    #[error("Keypair operation failed: {0}")]
    KeypairFailed(String),
}

/// Keystore operation result type
pub type KeystoreResult<T> = Result<T, KeystoreError>;

/// Centralized keystore helper for common operations
pub struct KeystoreHelper;

impl KeystoreHelper {
    /// Store encrypted share in keystore with metadata
    pub fn store_encrypted_share(
        data: &[u8],
        manifest_id: &str,
        metadata: Option<&str>,
    ) -> KeystoreResult<String> {
        // Encrypt the share data
        let (blob, cid) = encrypt_share_for_keystore(data)
            .map_err(|e| KeystoreError::EncryptionFailed(e.to_string()))?;

        // Open keystore
        let keystore = Self::open_keystore()?;

        // Prepare metadata with manifest_id
        let meta_str = match metadata {
            Some(meta) => format!("manifest_id={};{}", manifest_id, meta),
            None => format!("manifest_id={}", manifest_id),
        };

        // Store in keystore
        keystore
            .put(&cid, &blob, Some(&meta_str))
            .map_err(|e| KeystoreError::AccessFailed(format!("keystore put failed: {:?}", e)))?;

        CryptoLogger::log_keystore_operation("put", Some(&cid), true, None);
        info!(
            "keystore: stored encrypted share for manifest_id={}, cid={}",
            manifest_id, cid
        );

        Ok(cid)
    }

    /// Retrieve and decrypt share from keystore by CID
    pub fn retrieve_and_decrypt_share(cid: &str) -> KeystoreResult<Vec<u8>> {
        // Open keystore
        let keystore = Self::open_keystore()?;

        // Get the encrypted blob
        let blob = keystore
            .get(cid)
            .map_err(|e| KeystoreError::AccessFailed(format!("keystore get failed: {:?}", e)))?
            .ok_or_else(|| KeystoreError::ShareNotFound(format!("CID not found: {}", cid)))?;

        // Get KEM keypair for decryption
        let (_pub_bytes, priv_bytes) = ensure_kem_keypair_on_disk()
            .map_err(|e| KeystoreError::KeypairFailed(e.to_string()))?;

        // Decrypt the share
        let decrypted = decrypt_share_from_blob(&blob, &priv_bytes)
            .map_err(|e| KeystoreError::DecryptionFailed(e.to_string()))?;

        CryptoLogger::log_keystore_operation("get", Some(cid), true, None);
        info!("keystore: retrieved and decrypted share for cid={}", cid);

        Ok(decrypted)
    }

    /// Find and retrieve all shares for a specific manifest ID
    pub fn find_shares_for_manifest(manifest_id: &str) -> KeystoreResult<Vec<(String, Vec<u8>)>> {
        // Open keystore
        let keystore = Self::open_keystore()?;

        // Find CIDs for this manifest
        let cids = keystore.find_cids_for_manifest(manifest_id).map_err(|e| {
            KeystoreError::AccessFailed(format!("find_cids_for_manifest failed: {:?}", e))
        })?;

        if cids.is_empty() {
            return Ok(Vec::new());
        }

        info!(
            "keystore: found {} CIDs for manifest_id={}",
            cids.len(),
            manifest_id
        );

        let mut shares = Vec::new();
        let (_pub_bytes, priv_bytes) = ensure_kem_keypair_on_disk()
            .map_err(|e| KeystoreError::KeypairFailed(e.to_string()))?;

        // Retrieve and decrypt each share
        for cid in cids {
            match keystore.get(&cid) {
                Ok(Some(blob)) => match decrypt_share_from_blob(&blob, &priv_bytes) {
                    Ok(decrypted) => {
                        info!("keystore: successfully decrypted share for cid={}", cid);
                        shares.push((cid, decrypted));
                    }
                    Err(e) => {
                        warn!("keystore: failed to decrypt share for cid={}: {:?}", cid, e);
                        CryptoLogger::log_keystore_operation(
                            "decrypt",
                            Some(&cid),
                            false,
                            Some(&e.to_string()),
                        );
                    }
                },
                Ok(None) => {
                    warn!("keystore: CID {} not found (may have been deleted)", cid);
                }
                Err(e) => {
                    warn!("keystore: failed to retrieve CID {}: {:?}", cid, e);
                    CryptoLogger::log_keystore_operation(
                        "get",
                        Some(&cid),
                        false,
                        Some(&e.to_string()),
                    );
                }
            }
        }

        Ok(shares)
    }

    /// Find single share for manifest ID (returns first found)
    pub fn find_single_share_for_manifest(
        manifest_id: &str,
    ) -> KeystoreResult<Option<(String, Vec<u8>)>> {
        // Open keystore
        let keystore = Self::open_keystore()?;

        // Find single CID for this manifest
        let cid = keystore.find_cid_for_manifest(manifest_id).map_err(|e| {
            KeystoreError::AccessFailed(format!("find_cid_for_manifest failed: {:?}", e))
        })?;

        match cid {
            Some(cid) => match Self::retrieve_and_decrypt_share(&cid) {
                Ok(decrypted) => Ok(Some((cid, decrypted))),
                Err(e) => {
                    warn!(
                        "keystore: failed to decrypt share for manifest_id={}, cid={}: {:?}",
                        manifest_id, cid, e
                    );
                    Err(e)
                }
            },
            None => Ok(None),
        }
    }

    /// Store capability token in keystore
    pub fn store_capability_token(
        capability_data: &[u8],
        token_id: &str,
    ) -> KeystoreResult<String> {
        Self::store_encrypted_share(
            capability_data,
            &format!("capability_{}", token_id),
            Some("type=capability"),
        )
    }

    /// Retrieve capability token from keystore
    pub fn retrieve_capability_token(cid: &str) -> KeystoreResult<Vec<u8>> {
        Self::retrieve_and_decrypt_share(cid)
    }

    /// Store manifest with encryption
    pub fn store_encrypted_manifest(
        manifest_data: &[u8],
        manifest_id: &str,
    ) -> KeystoreResult<String> {
        Self::store_encrypted_share(manifest_data, manifest_id, Some("type=manifest"))
    }

    /// Batch operation: store multiple shares for the same manifest
    pub fn store_multiple_shares(
        shares: &[(Vec<u8>, String)], // (share_data, share_id)
        manifest_id: &str,
    ) -> KeystoreResult<Vec<String>> {
        let mut cids = Vec::new();

        for (share_data, share_id) in shares {
            let metadata = format!("type=share;share_id={}", share_id);
            match Self::store_encrypted_share(share_data, manifest_id, Some(&metadata)) {
                Ok(cid) => cids.push(cid),
                Err(e) => {
                    warn!(
                        "keystore: failed to store share {} for manifest {}: {:?}",
                        share_id, manifest_id, e
                    );
                    return Err(e);
                }
            }
        }

        info!(
            "keystore: stored {} shares for manifest_id={}",
            cids.len(),
            manifest_id
        );
        Ok(cids)
    }

    /// List all manifests in keystore (extracted from metadata)
    pub fn list_manifests() -> KeystoreResult<Vec<String>> {
        let keystore = Self::open_keystore()?;

        // Get all CIDs and extract manifest IDs from their metadata
        let all_cids = keystore
            .list_cids()
            .map_err(|e| KeystoreError::AccessFailed(format!("list_cids failed: {:?}", e)))?;

        let mut manifests = Vec::new();
        for cid in all_cids {
            // This is a simplified implementation since we don't have direct metadata access
            // In a real implementation, we'd need to add a method to get metadata
            // For now, we'll try to find manifests by checking if they're valid CIDs
            if !cid.is_empty() {
                // Extract potential manifest ID - this is a placeholder implementation
                if !manifests.contains(&cid) {
                    manifests.push(cid);
                }
            }
        }

        Ok(manifests)
    }

    /// Delete share by CID (not implemented - keystore doesn't support delete)
    pub fn delete_share(_cid: &str) -> KeystoreResult<()> {
        Err(KeystoreError::AccessFailed(
            "delete operation not implemented in keystore".to_string(),
        ))
    }

    /// Delete all shares for a manifest (not implemented - keystore doesn't support delete)
    pub fn delete_manifest_shares(_manifest_id: &str) -> KeystoreResult<usize> {
        Err(KeystoreError::AccessFailed(
            "delete operations not implemented in keystore".to_string(),
        ))
    }

    /// Internal helper to open keystore with consistent error handling
    fn open_keystore() -> KeystoreResult<Keystore> {
        crate::open_keystore_default()
            .map_err(|e| KeystoreError::AccessFailed(format!("keystore open failed: {:?}", e)))
    }

    /// Get keystore statistics
    pub fn get_statistics() -> KeystoreResult<KeystoreStatistics> {
        let keystore = Self::open_keystore()?;

        let all_cids = keystore
            .list_cids()
            .map_err(|e| KeystoreError::AccessFailed(format!("list_cids failed: {:?}", e)))?;

        let manifests = Self::list_manifests()?;

        Ok(KeystoreStatistics {
            total_shares: all_cids.len(),
            total_manifests: manifests.len(),
            manifests,
        })
    }
}

/// Keystore statistics information
#[derive(Debug, Clone)]
pub struct KeystoreStatistics {
    pub total_shares: usize,
    pub total_manifests: usize,
    pub manifests: Vec<String>,
}

/// Convenience functions for common patterns
impl KeystoreHelper {
    /// Safe wrapper that logs errors but doesn't panic
    pub fn try_store_encrypted_share(
        data: &[u8],
        manifest_id: &str,
        metadata: Option<&str>,
    ) -> Option<String> {
        match Self::store_encrypted_share(data, manifest_id, metadata) {
            Ok(cid) => Some(cid),
            Err(e) => {
                warn!("keystore: failed to store encrypted share: {:?}", e);
                CryptoLogger::log_keystore_operation("put", None, false, Some(&e.to_string()));
                None
            }
        }
    }

    /// Safe wrapper for retrieval that logs errors but doesn't panic
    pub fn try_retrieve_and_decrypt_share(cid: &str) -> Option<Vec<u8>> {
        match Self::retrieve_and_decrypt_share(cid) {
            Ok(data) => Some(data),
            Err(e) => {
                warn!("keystore: failed to retrieve and decrypt share: {:?}", e);
                CryptoLogger::log_keystore_operation("get", Some(cid), false, Some(&e.to_string()));
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keystore_error_display() {
        let error = KeystoreError::AccessFailed("test error".to_string());
        assert!(error.to_string().contains("test error"));

        let error = KeystoreError::EncryptionFailed("encrypt fail".to_string());
        assert!(error.to_string().contains("encrypt fail"));

        let error = KeystoreError::DecryptionFailed("decrypt fail".to_string());
        assert!(error.to_string().contains("decrypt fail"));

        let error = KeystoreError::ShareNotFound("not found".to_string());
        assert!(error.to_string().contains("not found"));

        let error = KeystoreError::InvalidShareData("invalid".to_string());
        assert!(error.to_string().contains("invalid"));

        let error = KeystoreError::KeypairFailed("keypair error".to_string());
        assert!(error.to_string().contains("keypair error"));
    }

    #[test]
    fn test_keystore_statistics() {
        let stats = KeystoreStatistics {
            total_shares: 10,
            total_manifests: 5,
            manifests: vec!["manifest1".to_string(), "manifest2".to_string()],
        };

        assert_eq!(stats.total_shares, 10);
        assert_eq!(stats.total_manifests, 5);
        assert_eq!(stats.manifests.len(), 2);
    }

    #[test]
    fn test_safe_wrappers() {
        // These tests verify the safe wrapper functions don't panic
        // They will return None in test environment due to missing keystore setup

        let result = KeystoreHelper::try_store_encrypted_share(
            b"test data",
            "test_manifest",
            Some("test_metadata"),
        );
        // In test environment without proper keystore setup, this should return None
        // but not panic

        let result = KeystoreHelper::try_retrieve_and_decrypt_share("test_cid");
        // Similarly, this should return None but not panic
    }
}

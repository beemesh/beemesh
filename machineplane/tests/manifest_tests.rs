//! Manifest Tests
//!
//! This module tests manifest parsing and validation utilities.
//! It verifies helper functions handle missing fields gracefully and
//! that content hashing produces consistent, deterministic digests.

use sha2::{Digest, Sha256};

/// Helper to extract metadata.name from a K8s manifest JSON.
fn extract_manifest_name(manifest_data: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(manifest_data).ok()?;
    value
        .get("metadata")
        .and_then(|m| m.get("name"))
        .and_then(|n| n.as_str())
        .map(|s| s.to_string())
}

/// Helper to compute SHA256 hex digest of manifest content.
/// Used for integrity verification of manifest payloads.
fn compute_content_hash(manifest_data: &[u8]) -> String {
    hex::encode(Sha256::digest(manifest_data))
}

/// Verifies that `extract_manifest_name` returns `None` for invalid or incomplete JSON.
#[test]
fn manifest_name_extraction_handles_missing_fields() {
    assert!(extract_manifest_name(br"{}").is_none());
    assert!(extract_manifest_name(br#"{"metadata":{}} "#).is_none());
}

/// Verifies that `extract_manifest_name` correctly extracts the name from valid JSON.
#[test]
fn manifest_name_extraction_returns_name_when_present() {
    let manifest = br#"{"metadata":{"name":"demo","namespace":"ns"}}"#;
    assert_eq!(extract_manifest_name(manifest).as_deref(), Some("demo"));
}

/// Verifies that content hash computation produces consistent, deterministic digests.
#[test]
fn content_hashing_is_deterministic() {
    let manifest = br#"{"apiVersion":"v1","kind":"Pod","metadata":{"name":"demo"}}"#;
    let hash = compute_content_hash(manifest);

    assert!(!hash.is_empty());
    assert_eq!(hash, compute_content_hash(manifest));

    let different_manifest = br#"{"apiVersion":"v1","kind":"Pod","metadata":{"name":"other"}}"#;
    assert_ne!(
        hash,
        compute_content_hash(different_manifest)
    );
}

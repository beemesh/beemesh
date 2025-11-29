//! Manifest Tests
//!
//! This module tests manifest parsing, validation, and ID generation utilities.
//! It verifies helper functions handle missing fields gracefully and generate
//! consistent, deterministic manifest identifiers.

use machineplane::messages::machine::{compute_manifest_id_from_content, extract_manifest_name};

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

/// Verifies that `compute_manifest_id_from_content` produces consistent hashes.
#[test]
fn manifest_id_hashing_is_deterministic() {
    let manifest = br#"{"apiVersion":"v1","kind":"Pod","metadata":{"name":"demo"}}"#;
    let manifest_id = compute_manifest_id_from_content(manifest);

    assert!(!manifest_id.is_empty());
    assert_eq!(manifest_id, compute_manifest_id_from_content(manifest));

    let different_manifest = br#"{"apiVersion":"v1","kind":"Pod","metadata":{"name":"other"}}"#;
    assert_ne!(
        manifest_id,
        compute_manifest_id_from_content(different_manifest)
    );
}

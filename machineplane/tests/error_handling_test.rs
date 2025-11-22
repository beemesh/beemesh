//! Tests for error handling and manifest processing utilities.
//!
//! This module verifies helper functions used for parsing and identifying manifests,
//! ensuring they handle missing fields and generate consistent IDs.

use machineplane::messages::machine::{
    compute_manifest_id, compute_manifest_id_from_content, extract_manifest_name,
};

/// Verifies that `extract_manifest_name` returns `None` for invalid or incomplete JSON.
#[test]
fn extract_manifest_name_handles_missing_fields() {
    assert!(extract_manifest_name(br"{}").is_none());
    assert!(extract_manifest_name(br#"{"metadata":{}} "#).is_none());
}

/// Verifies that `extract_manifest_name` correctly extracts the name from valid JSON.
#[test]
fn extract_manifest_name_returns_name_when_present() {
    let manifest = br#"{"metadata":{"name":"demo","namespace":"ns"}}"#;
    assert_eq!(extract_manifest_name(manifest).as_deref(), Some("demo"));
}

/// Verifies that `compute_manifest_id_from_content` produces consistent hashes.
#[test]
fn compute_manifest_id_hashes_content() {
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

/// Verifies that `compute_manifest_id` includes a version suffix to distinguish updates.
#[test]
fn compute_manifest_id_includes_version_suffix() {
    let name = "demo";
    let version_one = compute_manifest_id(name, 1);
    let version_two = compute_manifest_id(name, 2);

    assert!(version_one.starts_with(name));
    assert!(version_two.starts_with(name));
    assert_ne!(version_one, version_two);
}

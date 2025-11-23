// Consolidated libp2p-related constants for topics, message prefixes, protocol names, etc.

/// Ident topic used for gossipsub in the beemesh fabric.
pub const BEEMESH_FABRIC: &str = "beemesh-fabric";

/// Default selection window in milliseconds for scheduler operations
pub const DEFAULT_SELECTION_WINDOW_MS: u64 = 250;

// === MANIFEST FIELDS ===

/// JSON field name used for replica count in manifests (top-level `replicas`).
pub const REPLICAS_FIELD: &str = "replicas";

/// JSON path field used for replica count in manifests under `spec.replicas`.
pub const SPEC_REPLICAS_FIELD: &str = "spec";

// === RESOURCE MANAGEMENT CONSTANTS ===

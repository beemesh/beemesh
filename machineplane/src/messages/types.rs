//! Message type definitions for BeeMesh network communication
//!
//! All messages use Bincode for serialization, replacing the previous FlatBuffers implementation.
//! These types are used for peer-to-peer communication via libp2p request-response protocols.

use serde::{Deserialize, Serialize};

// ============================================================================
// Core Request/Response Messages
// ============================================================================

/// Request to apply/deploy a Kubernetes manifest
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApplyRequest {
    pub replicas: u32,
    pub operation_id: String,
    pub manifest_json: String,
    pub origin_peer: String,
    pub manifest_id: String,
}

/// Response to an apply request
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApplyResponse {
    pub ok: bool,
    pub operation_id: String,
    pub message: String,
}

/// Request to delete a deployed manifest
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeleteRequest {
    pub manifest_id: String,
    pub operation_id: String,
    pub origin_peer: String,
    /// Force deletion even if verification fails
    pub force: bool,
}

/// Response to a delete request
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeleteResponse {
    pub ok: bool,
    pub operation_id: String,
    pub message: String,
    pub manifest_id: String,
    /// List of workload IDs that were removed
    pub removed_workloads: Vec<String>,
}

// ============================================================================
// Handshake and Health
// ============================================================================

/// Peer handshake message
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Handshake {
    pub nonce: u32,
    pub signature: String,
    pub protocol_version: String,
    pub timestamp: u64,
}

/// Health check message
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Health {
    pub ok: bool,
    pub status: String,
}

// ============================================================================
// Scheduler Messages
// ============================================================================

/// Task definition for scheduling
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Task {
    pub id: String,
    /// URL or Content Hash
    pub manifest_ref: String,
    /// Inline manifest (optional)
    pub manifest_json: String,
    pub requirements_cpu_cores: u32,
    pub requirements_memory_mb: u32,
    /// "stateless" | "stateful"
    pub workload_type: String,
    pub duplicate_tolerant: bool,
    pub max_parallel_duplicates: u32,
    pub placement_token: String,
    pub qos_preemptible: bool,
    pub timestamp: u64,
}

/// Bidding message for task assignment
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Bid {
    pub task_id: String,
    pub node_id: String,
    pub score: f64,
    pub resource_fit_score: f64,
    pub network_locality_score: f64,
    pub timestamp: u64,
    /// Signature of the bid content
    pub signature: Vec<u8>,
}

/// Scheduler event types
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[repr(u8)]
pub enum EventType {
    Deployed = 0,
    Failed = 1,
    Preempted = 2,
    Cancelled = 3,
}

/// Scheduler event notification
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SchedulerEvent {
    pub task_id: String,
    pub node_id: String,
    pub event_type: EventType,
    pub reason: String,
    pub timestamp: u64,
}

/// Lease hint for task assignment
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LeaseHint {
    pub task_id: String,
    pub node_id: String,
    pub score: f64,
    pub ttl_ms: u32,
    pub renew_nonce: u64,
    pub timestamp: u64,
    pub signature: Vec<u8>,
}

// ============================================================================
// Capacity Messages
// ============================================================================

/// Request for node capacity information
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CapacityRequest {
    pub request_id: String,
    pub cpu_milli: u32,
    pub memory_bytes: u64,
    pub storage_bytes: u64,
    pub replicas: u32,
}

/// Response with node capacity information
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CapacityReply {
    pub request_id: String,
    pub ok: bool,
    pub node_id: String,
    pub region: String,
    /// Optional base64-encoded KEM public key for node
    pub kem_pubkey: String,
    pub capabilities: Vec<String>,
    pub cpu_available_milli: u32,
    pub memory_available_bytes: u64,
    pub storage_available_bytes: u64,
}

// ============================================================================
// Task Response Messages
// ============================================================================

/// Response to task creation request
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskCreateResponse {
    pub ok: bool,
    pub task_id: String,
    pub manifest_ref: String,
    pub selection_window_ms: u64,
    pub message: String,
}

/// Response to task status query
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskStatusResponse {
    pub task_id: String,
    pub state: String,
    pub assigned_peers: Vec<String>,
    pub manifest_cid: String,
}

// ============================================================================
// Node and Candidate Messages
// ============================================================================

/// Candidate node with public key for encrypted key share distribution
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CandidateNode {
    pub peer_id: String,
    /// Base64-encoded KEM public key for encrypting key shares
    pub public_key: String,
}

/// Response with list of candidate nodes
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CandidatesResponse {
    pub ok: bool,
    /// List of candidate nodes with their public keys
    pub candidates: Vec<CandidateNode>,
}

/// Response with list of peer nodes
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodesResponse {
    pub peers: Vec<String>,
}

// ============================================================================
// Assignment Messages
// ============================================================================

/// Request to assign a task (schema not in .fbs files, inferred from generated code)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AssignRequest {
    pub task_id: String,
}

/// Response to assignment request
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AssignResponse {
    pub ok: bool,
    pub task_id: String,
    pub assigned_peers: Vec<String>,
    pub per_peer_results_json: String,
}

// ============================================================================
// Applied Manifest (DHT Storage)
// ============================================================================

/// Signature scheme used to sign the manifest record
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[repr(u8)]
pub enum SignatureScheme {
    None = 0,
    Ed25519 = 1,
    RsaPss = 2,
}

/// Operation performed with this manifest record
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[repr(u8)]
pub enum OperationType {
    Apply = 0,
    Update = 1,
    Delete = 2,
}

/// Simple key/value pair for labeling or small indexes
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KeyValue {
    pub key: String,
    pub value: String,
}

/// AppliedManifest is the object stored in the DHT representing an applied
/// Kubernetes manifest. It is designed to be verifiable (signature + pubkey),
/// multi-tenant friendly (tenant + owner_pubkey) and retrievable via a stable
/// identifier (`id` / `content_hash`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppliedManifest {
    /// A canonical identifier for this record. Recommended: hex-encoded SHA-256
    /// over canonicalized payload (SHA256(owner_pubkey + ':' + operation_id + ':' + canonical_manifest))
    pub id: String,

    /// Operation id supplied by the originator (correlates with ApplyRequest)
    pub operation_id: String,

    /// libp2p peer id of the origin that applied the manifest
    pub origin_peer: String,

    /// Raw public key bytes of the owner (for signature verification)
    pub owner_pubkey: Vec<u8>,

    /// Algorithm/scheme used for the signature
    pub signature_scheme: SignatureScheme,

    /// Binary signature over the canonicalized data
    pub signature: Vec<u8>,

    /// The manifest itself as JSON string
    pub manifest_json: String,

    /// Optional convenience field for indexing
    pub manifest_kind: String,

    /// Optional labels for indexing and quick queries
    pub labels: Vec<KeyValue>,

    /// Unix timestamp in milliseconds when manifest was applied/signed
    pub timestamp: u64,

    /// What kind of operation this record represents
    pub operation: OperationType,

    /// Optional TTL (in seconds) to help nodes expire records from DHT
    pub ttl_secs: u32,

    /// Hex-encoded SHA-256 of the canonicalized manifest_json (content id)
    pub content_hash: String,
}

// ============================================================================
// Default Implementations
// ============================================================================

impl Default for ApplyRequest {
    fn default() -> Self {
        Self {
            replicas: 1,
            operation_id: String::new(),
            manifest_json: String::new(),
            origin_peer: String::new(),
            manifest_id: String::new(),
        }
    }
}

impl Default for DeleteRequest {
    fn default() -> Self {
        Self {
            manifest_id: String::new(),
            operation_id: String::new(),
            origin_peer: String::new(),
            force: false,
        }
    }
}

impl Default for CapacityRequest {
    fn default() -> Self {
        Self {
            request_id: String::new(),
            cpu_milli: 0,
            memory_bytes: 0,
            storage_bytes: 0,
            replicas: 1,
        }
    }
}

impl Default for Task {
    fn default() -> Self {
        Self {
            id: String::new(),
            manifest_ref: String::new(),
            manifest_json: String::new(),
            requirements_cpu_cores: 0,
            requirements_memory_mb: 0,
            workload_type: "stateless".to_string(),
            duplicate_tolerant: true,
            max_parallel_duplicates: 1,
            placement_token: String::new(),
            qos_preemptible: false,
            timestamp: 0,
        }
    }
}

impl Default for AppliedManifest {
    fn default() -> Self {
        Self {
            id: String::new(),
            operation_id: String::new(),
            origin_peer: String::new(),
            owner_pubkey: Vec::new(),
            signature_scheme: SignatureScheme::None,
            signature: Vec::new(),
            manifest_json: String::new(),
            manifest_kind: String::new(),
            labels: Vec::new(),
            timestamp: 0,
            operation: OperationType::Apply,
            ttl_secs: 0,
            content_hash: String::new(),
        }
    }
}

//! # RPC Streams – Inter-Replica Request/Response Protocol
//!
//! This module defines the RPC request/response types and handler registration mechanism
//! for inter-replica communication. It enables workloads to define custom RPC methods
//! that can be invoked across the libp2p network.
//!
//! ## Protocol Overview
//!
//! ```text
//! ┌─────────────┐                         ┌─────────────┐
//! │  Replica A  │                         │  Replica B  │
//! │             │  RPCRequest{method,body}│             │
//! │   Client    │ ──────────────────────► │   Handler   │
//! │             │                         │             │
//! │             │  RPCResponse{ok,body}   │             │
//! │             │ ◄────────────────────── │             │
//! └─────────────┘                         └─────────────┘
//! ```
//!
//! ## Built-in Methods
//!
//! The workload module registers handlers for these standard methods:
//!
//! | Method    | Description                                    | Leader Only |
//! |-----------|------------------------------------------------|-------------|
//! | `healthz` | Health check (returns `{ok: true}`)            | No          |
//! | `leader`  | Query current leader (stateful workloads)      | No          |
//! | Custom    | Application-defined methods                    | Configurable|
//!
//! ## Leader-Only Routing (Stateful Workloads)
//!
//! When `leader_only: true` is set in the request, follower replicas automatically
//! proxy the request to the current Raft leader. This ensures consistency for
//! stateful operations (see workplane-spec.md §8.1).
//!
//! ## Handler Registration
//!
//! Handlers are registered via [`register_stream_handler`] with a boxed async function.
//! Only one handler can be active at a time (last registration wins).

use std::sync::{Arc, LazyLock, Mutex};

use crate::discovery::ServiceRecord;

/// An RPC request sent between workload replicas.
///
/// The request is serialized as JSON and sent via libp2p's request/response protocol.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct RPCRequest {
    /// Method name (e.g., "healthz", "leader", or custom application method).
    pub method: String,

    /// Request body as a JSON object. Method-specific parameters.
    #[serde(default)]
    pub body: serde_json::Map<String, serde_json::Value>,

    /// If `true`, this request should be handled by the leader only.
    ///
    /// For stateful workloads, followers receiving a `leader_only` request will
    /// proxy it to the current Raft leader rather than handling it locally.
    #[serde(default)]
    pub leader_only: bool,
}

/// An RPC response returned from a workload replica.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct RPCResponse {
    /// `true` if the request was handled successfully.
    pub ok: bool,

    /// Error message if `ok` is `false`.
    #[serde(default)]
    pub error: Option<String>,

    /// Response body as a JSON object. Method-specific response data.
    #[serde(default)]
    pub body: serde_json::Map<String, serde_json::Value>,
}

use futures::future::BoxFuture;

/// Type alias for an RPC handler function.
///
/// Handlers receive the requesting peer's [`ServiceRecord`] (for context) and the
/// [`RPCRequest`], and return a future that resolves to an [`RPCResponse`].
type Handler =
    Arc<dyn Fn(&ServiceRecord, RPCRequest) -> BoxFuture<'static, RPCResponse> + Send + Sync>;

/// Global handler storage. Protected by mutex for thread-safe registration.
static HANDLER: LazyLock<Mutex<Option<Handler>>> = LazyLock::new(|| Mutex::new(None));

/// Register the global RPC stream handler.
///
/// The handler is called for every incoming RPC request. Only one handler can be
/// registered at a time; subsequent calls replace the previous handler.
///
/// # Arguments
///
/// * `handler` - Async function that processes requests and returns responses
///
/// # Example
///
/// ```ignore
/// use workplane::streams::{register_stream_handler, RPCRequest, RPCResponse};
/// use std::sync::Arc;
///
/// register_stream_handler(Arc::new(|peer, request| {
///     Box::pin(async move {
///         match request.method.as_str() {
///             "ping" => RPCResponse { ok: true, ..Default::default() },
///             _ => RPCResponse {
///                 ok: false,
///                 error: Some("unknown method".to_string()),
///                 ..Default::default()
///             },
///         }
///     })
/// }));
/// ```
pub fn register_stream_handler(handler: Handler) {
    *HANDLER.lock().expect("handler lock") = Some(handler);
}

/// Handle an incoming RPC request by invoking the registered handler.
///
/// If no handler is registered, returns an error response.
///
/// # Arguments
///
/// * `peer_id` - The libp2p peer ID of the requesting replica
/// * `request` - The incoming RPC request
///
/// # Returns
///
/// The handler's response, or an error response if no handler is registered.
pub async fn handle_request(peer_id: &str, request: RPCRequest) -> RPCResponse {
    let handler = HANDLER.lock().expect("handler lock").clone();
    if let Some(h) = handler {
        // Create a minimal ServiceRecord for the handler context
        let record = ServiceRecord {
            peer_id: peer_id.to_string(),
            ..Default::default()
        };
        h(&record, request).await
    } else {
        RPCResponse {
            ok: false,
            error: Some("no handler registered".to_string()),
            body: Default::default(),
        }
    }
}

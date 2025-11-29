//! # Workplane – Per-Workload Service Discovery & Self-Healing Agent
//!
//! Workplane is the per-workload sidecar agent in the beemesh architecture, responsible for
//! service discovery, health monitoring, and self-healing within a workload's replica set.
//!
//! ## Architecture Overview
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                          Workplane Agent                                 │
//! │  ┌──────────────┐   ┌──────────────┐   ┌──────────────┐                 │
//! │  │  SelfHealer  │   │ RaftManager  │   │   Network    │                 │
//! │  │  (replicas)  │   │  (leader)    │   │  (libp2p)    │                 │
//! │  └──────┬───────┘   └──────┬───────┘   └──────┬───────┘                 │
//! │         │                  │                  │                         │
//! │         └──────────────────┴──────────────────┘                         │
//! │                            │                                             │
//! │                    ┌───────▼───────┐                                     │
//! │                    │   Workload    │                                     │
//! │                    │  (orchestr.)  │                                     │
//! │                    └───────┬───────┘                                     │
//! └────────────────────────────┼────────────────────────────────────────────┘
//!                              │
//!                    ┌─────────▼─────────┐
//!                    │  Workload DHT     │
//!                    │  (Kademlia+RPC)   │
//!                    └───────────────────┘
//! ```
//!
//! ## Key Concepts
//!
//! ### Workload DHT (WDHT)
//! Per-workload service discovery using Kademlia DHT. Each replica publishes a
//! [`discovery::ServiceRecord`] containing its endpoint, health status, and metadata.
//! Records have a TTL and are refreshed periodically to ensure stale entries expire.
//!
//! ### Raft Consensus (Stateful Workloads)
//! Stateful workloads use simplified Raft for leader election. Only the leader handles
//! client requests; followers proxy to the leader. See [`raft::RaftManager`] for details.
//!
//! ### Self-Healing
//! The [`selfheal::SelfHealer`] monitors replica health and count, automatically:
//! - Publishing clone tenders to scale up when replicas are missing
//! - Requesting removal of unhealthy or excess replicas
//!
//! ### RPC Streams
//! Inter-replica communication via libp2p request/response protocol. The [`streams`]
//! module defines [`streams::RPCRequest`] and [`streams::RPCResponse`] types used for
//! health checks, leader proxying, and custom workload-specific methods.
//!
//! ## Modules
//!
//! - [`config`] – Agent configuration with environment variable overrides
//! - [`discovery`] – In-memory WDHT cache with TTL-based expiration
//! - [`network`] – libp2p networking: Kademlia DHT + request/response RPC
//! - [`raft`] – Simplified Raft for leader election (no log replication)
//! - [`selfheal`] – Replica count monitoring and self-healing automation
//! - [`streams`] – RPC request/response types and handler registration
//! - [`workload`] – Main workload agent orchestrating all subsystems
//!
//! ## Configuration
//!
//! The agent is configured via [`config::Config`], populated from environment variables
//! prefixed with `BEE_`. Key settings include:
//!
//! | Environment Variable | Description                           |
//! |---------------------|---------------------------------------|
//! | `BEE_NAMESPACE`     | Kubernetes-style namespace            |
//! | `BEE_WORKLOAD_NAME` | Workload name (e.g., "nginx")         |
//! | `BEE_WORKLOAD_KIND` | "Deployment" or "StatefulSet"         |
//! | `BEE_REPLICAS`      | Desired replica count                 |
//! | `BEE_LIVENESS_URL`  | HTTP liveness probe URL               |
//! | `BEE_READINESS_URL` | HTTP readiness probe URL              |
//! | `BEE_BEEMESH_API`   | Machineplane API URL                  |
//!
//! ## Wire Protocol
//!
//! Communication uses libp2p with:
//! - **Kademlia DHT** – Service record storage and peer discovery
//! - **Request/Response** – RPC for health checks, leader queries, proxying
//! - **QUIC Transport** – Encrypted, multiplexed connections
//!
//! ## Example Usage
//!
//! ```ignore
//! use workplane::{Config, Workload};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let mut config = Config {
//!         namespace: "default".to_string(),
//!         workload_name: "nginx".to_string(),
//!         workload_kind: "Deployment".to_string(),
//!         liveness_url: "http://127.0.0.1:8080/health".to_string(),
//!         readiness_url: "http://127.0.0.1:8080/ready".to_string(),
//!         ..Default::default()
//!     };
//!     config.apply_defaults();
//!     let mut workload = Workload::new(config)?;
//!     workload.start();
//!     // ... run until shutdown
//!     workload.close().await;
//!     Ok(())
//! }
//! ```
//!
//! For the full specification, see `workplane-spec.md`.

pub mod config;
pub mod discovery;
pub mod raft;
pub mod network;
pub mod selfheal;
pub mod rpc;
pub mod workload;

// Internal metrics module – provides `increment_counter!` and `gauge!` macros
mod metrics;

pub use config::Config;
pub use workload::Workload;

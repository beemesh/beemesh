//! # Machineplane - BeeMesh Distributed Workload Scheduler
//!
//! The machineplane is the core runtime component of BeeMesh, implementing a
//! decentralized workload scheduler using a Tender/Bid/Award model over libp2p.
//!
//! ## Architecture
//!
//! The machineplane consists of several key modules:
//!
//! - **network**: libp2p networking layer (gossipsub + Kademlia + request-response)
//! - **scheduler**: Tender/Bid/Award scheduling logic with replay protection
//! - **messages**: Type definitions and cryptographic signing for scheduler messages
//! - **runtimes**: Container runtime adapters (Podman)
//! - **api**: REST API with Kubernetes compatibility
//!
//! ## Spec Reference
//!
//! See `machineplane-spec.md` for the full specification including:
//!
//! - §3: MDHT Discovery using Kademlia DHT
//! - §4-5: Tender/Bid/Award scheduling protocol
//! - §7: Ed25519 message signing
//! - §8: Replay protection and timestamp validation
//!
//! ## Quick Start
//!
//! ```no_run
//! use machineplane::{DaemonConfig, start_machineplane};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let config = DaemonConfig::default();
//!     let handles = start_machineplane(config).await?;
//!     // Handles include libp2p and REST API tasks
//!     Ok(())
//! }
//! ```

use clap::Parser;
use env_logger::Env;

pub mod api;
pub mod messages;
pub mod network;
pub mod runtimes;
pub mod scheduler;

// ============================================================================
// CLI Configuration
// ============================================================================

/// BeeMesh Host Agent - Distributed workload scheduler.
///
/// The machineplane daemon participates in the BeeMesh fabric, handling
/// workload scheduling via the Tender/Bid/Award protocol.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// Host address for REST API
    #[arg(long, default_value = "0.0.0.0")]
    pub rest_api_host: String,

    /// Port for REST API
    #[arg(long, default_value = "3000")]
    pub rest_api_port: u16,

    /// Custom node name (optional)
    #[arg(long)]
    pub node_name: Option<String>,

    /// Override the Podman socket URL used by runtime engines (defaults to CONTAINER_HOST env)
    #[arg(long, env = "CONTAINER_HOST")]
    pub podman_socket: Option<String>,

    /// Use ephemeral signing keys instead of writing to disk
    #[arg(long, default_value_t = false)]
    pub signing_ephemeral: bool,

    /// Use ephemeral KEM keys instead of writing to disk
    #[arg(long, default_value_t = false)]
    pub kem_ephemeral: bool,

    /// Enable fully ephemeral key handling for all keypair operations
    #[arg(long, default_value_t = false)]
    pub ephemeral_keys: bool,

    /// Bootstrap peer addresses for explicit peer discovery (can be specified multiple times).
    ///
    /// Use a full multiaddr that includes the transport and peer ID, e.g.
    /// `/ip4/127.0.0.1/udp/55493/quic-v1/p2p/<peer_id>`.
    #[arg(long)]
    pub bootstrap_peer: Vec<String>,

    /// Port for libp2p UDP/QUIC transport (default: 0 for auto-assignment)
    #[arg(long, default_value = "0")]
    pub libp2p_quic_port: u16,

    /// Host address for libp2p listeners (IPv4 or IPv6 literal, default: 0.0.0.0)
    #[arg(long, default_value = "0.0.0.0")]
    pub libp2p_host: String,

    /// Interval in seconds for DHT presence refresh and random walk discovery (default: 60)
    #[arg(long, default_value = "60")]
    pub dht_refresh_interval_secs: u64,
}

/// Type alias for daemon configuration to decouple test terminology from CLI parsing.
pub type DaemonConfig = Cli;

impl Default for Cli {
    fn default() -> Self {
        Self {
            rest_api_host: "127.0.0.1".to_string(),
            rest_api_port: 3000,
            node_name: None,
            podman_socket: None,
            signing_ephemeral: false,
            kem_ephemeral: false,
            ephemeral_keys: false,
            bootstrap_peer: Vec::new(),
            libp2p_quic_port: 0,
            libp2p_host: "0.0.0.0".to_string(),
            dht_refresh_interval_secs: 60,
        }
    }
}

// ============================================================================
// Podman Socket Configuration
// ============================================================================

/// Resolves and configures the Podman socket for container runtime operations.
///
/// This function:
/// 1. Uses the CLI-provided socket if available
/// 2. Falls back to auto-detection via `PodmanEngine::detect_podman_socket()`
/// 3. Normalizes the socket path (adds `unix://` prefix if needed)
/// 4. Sets the `CONTAINER_HOST` environment variable for child processes
///
/// # Returns
///
/// The normalized socket URL if a socket was found, or `None` if no socket is available.
fn resolve_and_configure_podman_socket(
    cli_socket: Option<String>,
) -> anyhow::Result<Option<String>> {
    let podman_socket = cli_socket
        .filter(|value| !value.trim().is_empty())
        .or_else(runtimes::podman::PodmanEngine::detect_podman_socket);

    let Some(podman_socket) = podman_socket else {
        log::warn!(
            "Podman socket not detected; runtime-backed apply will be disabled. Provide --podman-socket or set CONTAINER_HOST to enable."
        );
        // Explicitly clear any overrides so the Podman runtime stays disabled.
        runtimes::configure_podman_runtime(None);
        return Ok(None);
    };

    let normalized = runtimes::podman::PodmanEngine::normalize_socket(&podman_socket);
    // SAFETY: Setting process environment variables at startup is required so all runtime
    // consumers (including child processes) consistently use the configured Podman socket.
    unsafe {
        std::env::set_var("CONTAINER_HOST", &normalized);
    }

    runtimes::configure_podman_runtime(Some(normalized.clone()));

    Ok(Some(normalized))
}

// ============================================================================
// Daemon Entry Point
// ============================================================================

/// Starts the machineplane runtime.
///
/// This is the main entry point for the machineplane daemon. It:
///
/// 1. Initializes logging
/// 2. Configures the Podman runtime socket
/// 3. Generates an ephemeral Ed25519 keypair for this node
/// 4. Sets up the libp2p swarm (QUIC transport, gossipsub, Kademlia)
/// 5. Dials bootstrap peers if configured
/// 6. Initializes the runtime registry for container deployment
/// 7. Starts the REST API server
///
/// # Arguments
///
/// * `cli` - The daemon configuration (typically from CLI parsing)
///
/// # Returns
///
/// A vector of `JoinHandle`s for the spawned background tasks:
/// - libp2p event loop task
/// - REST API server task
///
/// # Example
///
/// ```no_run
/// use machineplane::{DaemonConfig, start_machineplane};
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     let config = DaemonConfig {
///         rest_api_port: 8080,
///         ..Default::default()
///     };
///     let handles = start_machineplane(config).await?;
///     // Wait for tasks to complete
///     for handle in handles {
///         let _ = handle.await;
///     }
///     Ok(())
/// }
/// ```
pub async fn start_machineplane(
    cli: DaemonConfig,
) -> anyhow::Result<Vec<tokio::task::JoinHandle<()>>> {
    // Initialize logger with info-level default so startup messages are visible
    let _ = env_logger::Builder::from_env(Env::default().default_filter_or("info")).try_init();

    // Configure Podman runtime socket
    let podman_socket = resolve_and_configure_podman_socket(cli.podman_socket.clone())?;
    if let Some(socket) = &podman_socket {
        log::info!("Using Podman socket: {}", socket);
    }

    // Generate an ephemeral libp2p keypair for this run
    // Machineplane intentionally treats peer identities as transient
    let keypair = libp2p::identity::Keypair::generate_ed25519();

    // Surface the local peer identity immediately for operators
    let local_peer_id = keypair.public().to_peer_id();
    log::info!("Local PeerId: {}", local_peer_id);

    // Store keypair bytes for network module (needed for message signing)
    let keypair_bytes = keypair.to_protobuf_encoding()?;
    let public_bytes = local_peer_id.to_bytes();
    network::set_node_keypair(Some((public_bytes.clone(), keypair_bytes.clone())));

    // Set up the libp2p swarm with gossipsub, Kademlia, and request-response
    let (mut swarm, topic, peer_rx, peer_tx) =
        network::setup_libp2p_node(cli.libp2p_quic_port, &cli.libp2p_host, &public_bytes)?;

    // Dial bootstrap peers if provided (for explicit peer discovery)
    for addr in &cli.bootstrap_peer {
        match addr.parse::<libp2p::multiaddr::Multiaddr>() {
            Ok(ma) => match swarm.dial(ma) {
                Ok(_) => log::debug!("Dialing bootstrap peer: {}", addr),
                Err(e) => log::warn!("Failed to dial bootstrap peer {}: {}", addr, e),
            },
            Err(e) => log::warn!(
                "Invalid bootstrap peer multiaddr '{}': {}. Example: /ip4/127.0.0.1/udp/55493/quic-v1/p2p/<peer_id>",
                addr,
                e
            ),
        }
    }

    // Create control channel for libp2p (allows REST handlers to send commands)
    let (control_tx, control_rx) =
        tokio::sync::mpsc::unbounded_channel::<network::control::Libp2pControl>();

    // Set the global control sender for distributed operations
    network::set_control_sender(control_tx.clone());

    // Spawn libp2p event loop task
    let control_tx_for_libp2p = control_tx.clone();
    let dht_refresh_interval = cli.dht_refresh_interval_secs;
    let libp2p_handle = tokio::spawn(async move {
        // Hold sender to keep channel alive
        let _keeper = control_tx_for_libp2p;
        if let Err(e) = network::start_libp2p_node(swarm, topic, peer_tx, control_rx, dht_refresh_interval).await {
            log::error!("libp2p node error: {}", e);
        }
    });

    // Initialize runtime registry for container deployment
    log::info!("Initializing runtime registry and provider manager...");
    if let Err(e) = scheduler::initialize_podman_manager().await {
        log::warn!(
            "Failed to initialize runtime registry: {}. Will use legacy deployment only.",
            e
        );
    } else {
        log::info!("Runtime registry and provider manager initialized successfully");
    }

    let mut handles = Vec::new();

    // Build REST API router
    let app = api::build_router(peer_rx, control_tx.clone(), public_bytes.clone());

    // Start REST API server
    let bind_addr = format!("{}:{}", cli.rest_api_host, cli.rest_api_port);
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    log::info!("rest api listening on {}", listener.local_addr().unwrap());
    handles.push(tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app.clone().into_make_service()).await {
            log::error!("axum server error: {}", e);
        }
    }));

    // Return all task handles (libp2p first)
    let mut all = vec![libp2p_handle];
    all.extend(handles);
    Ok(all)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::resolve_and_configure_podman_socket;
    use serial_test::serial;

    fn restore_env_var(key: &str, value: Option<String>) {
        // SAFETY: Tests reset the environment for isolation within a single-threaded context.
        unsafe {
            if let Some(original) = value {
                std::env::set_var(key, original);
            } else {
                std::env::remove_var(key);
            }
        }
    }

    #[test]
    #[serial]
    fn configure_socket_normalizes_and_exports_env() {
        let original_container = std::env::var("CONTAINER_HOST").ok();

        let normalized =
            resolve_and_configure_podman_socket(Some("/tmp/test-podman.sock".to_string()))
                .unwrap()
                .unwrap();

        assert_eq!(normalized, "unix:///tmp/test-podman.sock");
        assert_eq!(
            std::env::var("CONTAINER_HOST").unwrap(),
            "unix:///tmp/test-podman.sock"
        );

        crate::runtimes::configure_podman_runtime(None);
        restore_env_var("CONTAINER_HOST", original_container);
    }

    #[test]
    #[serial]
    fn podman_socket_absence_is_non_fatal() {
        let original_container = std::env::var("CONTAINER_HOST").ok();

        // Ensure no environment socket is provided
        unsafe {
            std::env::remove_var("CONTAINER_HOST");
        }

        let detected = resolve_and_configure_podman_socket(None).unwrap();
        assert!(detected.is_none());

        // Cleanup
        restore_env_var("CONTAINER_HOST", original_container);
    }
}

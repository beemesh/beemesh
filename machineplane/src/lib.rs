// logging macros are used in submodules; keep root lean
use clap::Parser;
use env_logger::Env;

pub mod api;
pub mod messages;
pub mod network;
pub mod runtimes;
pub mod scheduler;

/// beemesh Host Agent
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

    /// Bootstrap peer addresses for explicit peer discovery (can be specified multiple times)
    #[arg(long)]
    pub bootstrap_peer: Vec<String>,

    /// Port for libp2p UDP/QUIC transport (default: 0 for auto-assignment)
    #[arg(long, default_value = "0")]
    pub libp2p_quic_port: u16,

    /// Host address for libp2p listeners (IPv4 or IPv6 literal, default: 0.0.0.0)
    #[arg(long, default_value = "0.0.0.0")]
    pub libp2p_host: String,
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
        }
    }
}

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

/// Start the machineplane runtime using the provided CLI configuration.
/// Returns a Vec of JoinHandles for spawned background tasks (libp2p, servers, etc.).
pub async fn start_machineplane(
    cli: DaemonConfig,
) -> anyhow::Result<Vec<tokio::task::JoinHandle<()>>> {
    // initialize logger but don't panic if already initialized
    // Default to info-level logging so the local PeerId and other startup messages are visible
    // without requiring RUST_LOG to be set explicitly.
    let _ = env_logger::Builder::from_env(Env::default().default_filter_or("info")).try_init();

    let podman_socket = resolve_and_configure_podman_socket(cli.podman_socket.clone())?;
    if let Some(socket) = &podman_socket {
        log::info!("Using Podman socket: {}", socket);
    }

    // Generate an ephemeral libp2p keypair for this run. Machineplane intentionally
    // treats peer identities as transient and does not persist them between restarts.
    let keypair = libp2p::identity::Keypair::generate_ed25519();

    // Store keypair bytes for network module
    let keypair_bytes = keypair.to_protobuf_encoding()?;
    let public_bytes = keypair.public().to_peer_id().to_bytes();
    network::set_node_keypair(Some((public_bytes.clone(), keypair_bytes.clone())));

    let (mut swarm, topic, peer_rx, peer_tx) =
        network::setup_libp2p_node(cli.libp2p_quic_port, &cli.libp2p_host)?;

    // If bootstrap peers are provided, dial them explicitly (for in-process tests)
    for addr in &cli.bootstrap_peer {
        match addr.parse::<libp2p::multiaddr::Multiaddr>() {
            Ok(ma) => match swarm.dial(ma) {
                Ok(_) => log::debug!("Dialing bootstrap peer: {}", addr),
                Err(e) => log::warn!("Failed to dial bootstrap peer {}: {}", addr, e),
            },
            Err(e) => log::warn!("Invalid bootstrap peer address {}: {}", addr, e),
        }
    }

    // control channel for libp2p (from REST handlers to libp2p task)
    let (control_tx, control_rx) =
        tokio::sync::mpsc::unbounded_channel::<network::control::Libp2pControl>();

    // Set the global control sender for distributed operations
    network::set_control_sender(control_tx.clone());

    // Keep the sender side alive by moving one clone into the libp2p task.
    // If we don't keep a sender alive outside this function, the receiver will see
    // 'None' and the libp2p loop will exit immediately when there are no API servers.
    let control_tx_for_libp2p = control_tx.clone();
    let libp2p_handle = tokio::spawn(async move {
        // hold on to the sender for the lifetime of this task
        let _keeper = control_tx_for_libp2p;
        if let Err(e) = network::start_libp2p_node(swarm, topic, peer_tx, control_rx).await {
            log::error!("libp2p node error: {}", e);
        }
    });

    // Initialize runtime registry and provider manager for manifest deployment
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

    let app = api::build_router(peer_rx, control_tx.clone());

    // Public TCP server
    let bind_addr = format!("{}:{}", cli.rest_api_host, cli.rest_api_port);
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    log::info!("rest api listening on {}", listener.local_addr().unwrap());
    handles.push(tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app.clone().into_make_service()).await {
            log::error!("axum server error: {}", e);
        }
    }));

    // Prepend libp2p handle so caller can decide how to await
    let mut all = vec![libp2p_handle];
    all.extend(handles);
    Ok(all)
}

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

// logging macros are used in submodules; keep root lean
use clap::Parser;
use env_logger::Env;
use std::io::Write;

pub mod api;
pub mod messages;
pub mod network;
pub mod runtimes;
pub mod scheduler;

/// beemesh Host Agent
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// Store keypair in memory only (ephemeral node)
    #[arg(long, default_value_t = false)]
    pub ephemeral: bool,
    /// Host address for REST API
    #[arg(long, default_value = "0.0.0.0")]
    pub rest_api_host: String,

    /// Port for REST API
    #[arg(long, default_value = "3000")]
    pub rest_api_port: u16,

    /// Custom node name (optional)
    #[arg(long)]
    pub node_name: Option<String>,

    /// Override the Podman socket URL used by runtime engines (defaults to PODMAN_HOST env)
    #[arg(long, env = "PODMAN_HOST")]
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

    /// Directory to store machineplane keypair (default: /etc/beemesh/machineplane)
    #[arg(long, default_value = "/etc/beemesh/machineplane")]
    pub key_dir: String,

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
            ephemeral: false,
            rest_api_host: "127.0.0.1".to_string(),
            rest_api_port: 3000,
            node_name: None,
            podman_socket: None,
            signing_ephemeral: false,
            kem_ephemeral: false,
            ephemeral_keys: false,
            key_dir: "/etc/beemesh/machineplane".to_string(),
            bootstrap_peer: Vec::new(),
            libp2p_quic_port: 0,
            libp2p_host: "0.0.0.0".to_string(),
        }
    }
}

/// Start the machineplane runtime using the provided CLI configuration.
/// Returns a Vec of JoinHandles for spawned background tasks (libp2p, servers, etc.).
pub async fn start_machineplane(
    cli: DaemonConfig,
) -> anyhow::Result<Vec<tokio::task::JoinHandle<()>>> {
    // initialize logger but don't panic if already initialized
    let _ = env_logger::Builder::from_env(Env::default().default_filter_or("warn")).try_init();

    runtimes::configure_podman_runtime(cli.podman_socket.clone());

    // Load or generate libp2p keypair
    let keypair = if cli.ephemeral {
        log::info!("Using ephemeral keypair (not persisted to disk)");
        libp2p::identity::Keypair::generate_ed25519()
    } else {
        // Store keypair in configured key_dir (default /etc/beemesh/machineplane)
        use std::fs::OpenOptions;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let mut key_path = std::path::PathBuf::from(&cli.key_dir);

        // Helper to create directory with secure permissions
        let ensure_dir = |p: &std::path::Path| -> std::io::Result<()> {
            if !p.exists() {
                std::fs::create_dir_all(p)?;
            }
            let perms = std::fs::Permissions::from_mode(0o700);
            std::fs::set_permissions(p, perms)?;
            Ok(())
        };

        // Try to create key directory, fall back to $HOME/.beemesh on failure
        if let Err(e) = ensure_dir(&key_path) {
            log::warn!(
                "could not create/set permissions for {}: {}. Falling back to $HOME/.beemesh",
                key_path.display(),
                e
            );
            if let Some(home) = dirs::home_dir() {
                key_path = home.join(".beemesh");
                ensure_dir(&key_path)?;
            } else {
                return Err(anyhow::anyhow!("failed to determine home dir: {}", e));
            }
        }

        let keypair_path = key_path.join("libp2p_keypair.bin");

        // Load existing keypair or generate new one
        if keypair_path.exists() {
            log::info!("Loading keypair from {}", keypair_path.display());
            let bytes = std::fs::read(&keypair_path)?;
            libp2p::identity::Keypair::from_protobuf_encoding(&bytes)?
        } else {
            log::info!(
                "Generating new keypair and saving to {}",
                keypair_path.display()
            );
            let keypair = libp2p::identity::Keypair::generate_ed25519();
            let bytes = keypair.to_protobuf_encoding()?;

            // Write with secure permissions
            let mut opts = OpenOptions::new();
            opts.write(true).create(true).truncate(true).mode(0o600);
            let mut file = opts.open(&keypair_path)?;
            file.write_all(&bytes)?;

            keypair
        }
    };

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

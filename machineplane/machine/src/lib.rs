// logging macros are used in submodules; keep root lean
use base64::Engine;
use clap::Parser;
use env_logger::Env;
use std::io::Write;

mod hostapi;
pub mod libp2p_beemesh;
mod pod_communication;
pub mod provider;
pub mod restapi;
pub mod runtime;
pub mod workload_integration;
pub mod workload_manager;

/// beemesh Host Agent
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// Store keypair in memory only (ephemeral node)
    #[arg(long, default_value_t = false)]
    pub ephemeral: bool,
    /// Host address for REST API
    #[arg(long, default_value = "127.0.0.1")]
    pub rest_api_host: String,

    /// Port for REST API
    #[arg(long, default_value = "3000")]
    pub rest_api_port: u16,

    /// Disable REST API server
    #[arg(long, default_value_t = false)]
    pub disable_rest_api: bool,

    /// Disable REST API server
    #[arg(long, default_value_t = false)]
    pub disable_machine_api: bool,

    /// Custom node name (optional)
    #[arg(long)]
    pub node_name: Option<String>,

    #[arg(long, default_value = "/run/beemesh/host.sock")]
    pub api_socket: Option<String>,

    /// Directory to store machine keypair (default: /etc/beemesh/machine)
    #[arg(long, default_value = "/etc/beemesh/machine")]
    pub key_dir: String,

    /// Bootstrap peer addresses for explicit peer discovery (can be specified multiple times)
    #[arg(long)]
    pub bootstrap_peer: Vec<String>,

    /// Port for libp2p TCP transport (default: 0 for auto-assignment)
    #[arg(long, default_value = "0")]
    pub libp2p_tcp_port: u16,

    /// Port for libp2p UDP/QUIC transport (default: 0 for auto-assignment)
    #[arg(long, default_value = "0")]
    pub libp2p_quic_port: u16,

    /// Host address for libp2p listeners (default: 0.0.0.0)
    #[arg(long, default_value = "0.0.0.0")]
    pub libp2p_host: String,
}

/// Start the machine runtime using the provided CLI configuration.
/// Returns a Vec of JoinHandles for spawned background tasks (libp2p, servers, etc.).
pub async fn start_machine(cli: Cli) -> anyhow::Result<Vec<tokio::task::JoinHandle<()>>> {
    // initialize logger but don't panic if already initialized
    let _ = env_logger::Builder::from_env(Env::default().default_filter_or("warn")).try_init();

    // initialize PQC once at process startup; fail early if it doesn't initialize
    crypto::ensure_pqc_init().map_err(|e| anyhow::anyhow!("pqc initialization failed: {}", e))?;

    // Propagate ephemeral mode to subcomponents via env var so lower-level
    // handlers (e.g., keyshare processing) can open in-memory keystore without
    // requiring global config plumbing.
    if cli.ephemeral {
        std::env::set_var("BEEMESH_KEYSTORE_EPHEMERAL", "1");
        // In ephemeral mode, create a per-node shared keystore so all components
        // within this node process share the same keystore instance
        let shared_name = format!("node_{}", cli.rest_api_port);
        std::env::set_var("BEEMESH_KEYSTORE_SHARED_NAME", &shared_name);
        // Also set in libp2p module for use by keyshare processing
        libp2p_beemesh::set_keystore_shared_name(Some(shared_name));
    } else {
        libp2p_beemesh::set_keystore_shared_name(None);
    }

    let _keypair = if cli.ephemeral {
        let kp = crypto::ensure_keypair_ephemeral().ok();
        // also set global
        libp2p_beemesh::set_node_keypair(kp.clone());
        kp
    } else {
        // Store keypair in configured key_dir (default /etc/beemesh/machine)
        use std::fs::OpenOptions;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let mut key_path = std::path::PathBuf::from(&cli.key_dir);
        // try to create and set secure perms; on failure, fall back to $HOME/.beemesh
        let ensure_dir = |p: &std::path::Path| -> std::io::Result<()> {
            if !p.exists() {
                std::fs::create_dir_all(p)?;
            }
            let perms = std::fs::Permissions::from_mode(0o700);
            std::fs::set_permissions(p, perms)?;
            Ok(())
        };

        if let Err(e) = ensure_dir(&key_path) {
            log::warn!(
                "could not create/set permissions for {}: {}. Falling back to $HOME/.beemesh",
                key_path.display(),
                e
            );
            if let Some(home) = dirs::home_dir() {
                key_path = home.join(crypto::KEY_DIR);
                ensure_dir(&key_path)?;
            } else {
                return Err(anyhow::anyhow!("failed to determine home dir: {}", e));
            }
        }

        // Use similar logic as ensure_keypair_on_disk but with secure file creation
        let pubpath = key_path.join(crypto::PUBKEY_FILE);
        let privpath = key_path.join(crypto::PRIVKEY_FILE);

        if pubpath.exists() && privpath.exists() {
            let pk = std::fs::read(&pubpath)?;
            let sk = std::fs::read(&privpath)?;
            let kp = Some((pk, sk));
            libp2p_beemesh::set_node_keypair(kp.clone());
            kp
        } else {
            let dsa = saorsa_pqc::api::sig::ml_dsa_65();
            let (pubk, privk) = dsa
                .generate_keypair()
                .map_err(|e| anyhow::anyhow!("dsa generate_keypair failed: {:?}", e))?;
            let pk_bytes = pubk.to_bytes();
            let sk_bytes = privk.to_bytes();

            // Write files with secure mode 0o600
            let mut o = OpenOptions::new();
            o.write(true).create(true).truncate(true).mode(0o600);
            let mut f1 = o.open(&pubpath)?;
            f1.write_all(&pk_bytes)?;

            let mut o2 = OpenOptions::new();
            o2.write(true).create(true).truncate(true).mode(0o600);
            let mut f2 = o2.open(&privpath)?;
            f2.write_all(&sk_bytes)?;

            let kp = Some((pk_bytes, sk_bytes));
            libp2p_beemesh::set_node_keypair(kp.clone());
            kp
        }
    };
    let (mut swarm, topic, peer_rx, peer_tx) = libp2p_beemesh::setup_libp2p_node(
        cli.libp2p_tcp_port,
        cli.libp2p_quic_port,
        &cli.libp2p_host,
    )?;

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

    // Initialize envelope handler for encrypted communication
    // Use KEM private key for decryption and signing public key for serving via legacy endpoint
    let envelope_handler = {
        // Get KEM keypair for decryption of encrypted requests
        let (_kem_pk_bytes, kem_sk_bytes) = crypto::ensure_kem_keypair_on_disk()
            .map_err(|e| anyhow::anyhow!("Failed to get KEM keypair: {}", e))?;

        // Get signing keypair for legacy /api/v1/pubkey endpoint compatibility
        let (signing_pk_bytes, _signing_sk_bytes) = crypto::ensure_keypair_on_disk()
            .map_err(|e| anyhow::anyhow!("Failed to get signing keypair: {}", e))?;
        let signing_public_key_b64 =
            base64::engine::general_purpose::STANDARD.encode(&signing_pk_bytes);

        std::sync::Arc::new(restapi::envelope_handler::EnvelopeHandler::new(
            kem_sk_bytes,           // KEM private key for decryption
            signing_public_key_b64, // Signing public key for legacy endpoint
        ))
    };

    // control channel for libp2p (from REST handlers to libp2p task)
    let (control_tx, control_rx) =
        tokio::sync::mpsc::unbounded_channel::<libp2p_beemesh::control::Libp2pControl>();

    // Set the global control sender for distributed operations
    libp2p_beemesh::set_control_sender(control_tx.clone());

    // Keep the sender side alive by moving one clone into the libp2p task.
    // If we don't keep a sender alive outside this function, the receiver will see
    // 'None' and the libp2p loop will exit immediately when there are no API servers.
    let control_tx_for_libp2p = control_tx.clone();
    let libp2p_handle = tokio::spawn(async move {
        // hold on to the sender for the lifetime of this task
        let _keeper = control_tx_for_libp2p;
        let keystore_shared_name = if cli.ephemeral {
            Some(format!("node_{}", cli.rest_api_port))
        } else {
            None
        };
        if let Err(e) = libp2p_beemesh::start_libp2p_node(
            swarm,
            topic,
            peer_tx,
            control_rx,
            keystore_shared_name,
        )
        .await
        {
            log::error!("libp2p node error: {}", e);
        }
    });

    // Initialize runtime registry and provider manager for manifest deployment
    log::info!("Initializing runtime registry and provider manager...");
    if let Err(e) = workload_integration::initialize_workload_manager().await {
        log::warn!(
            "Failed to initialize runtime registry: {}. Will use legacy deployment only.",
            e
        );
    } else {
        log::info!("Runtime registry and provider manager initialized successfully");
    }

    let mut handles = Vec::new();

    // rest api server
    if !cli.disable_rest_api {
        let shared_name = if cli.ephemeral {
            Some(format!("node_{}", cli.rest_api_port))
        } else {
            None
        };
        let app = restapi::build_router(
            peer_rx,
            control_tx.clone(),
            shared_name,
            envelope_handler.clone(),
        );

        // Public TCP server
        let bind_addr = format!("{}:{}", cli.rest_api_host, cli.rest_api_port);
        let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
        log::info!("rest api listening on {}", listener.local_addr().unwrap());
        handles.push(tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app.clone().into_make_service()).await {
                log::error!("axum server error: {}", e);
            }
        }));
    } else {
        log::info!("REST API disabled");
    }

    // host api server - for gateway to host accesss
    if !cli.disable_machine_api {
        if let Some(socket_path) = cli.api_socket.clone() {
            // Ensure parent directory exists for the UDS socket. If we cannot create it,
            // log and skip starting the host API rather than causing the whole process to exit.
            let socket = socket_path.clone();
            let mut start_uds = true;
            if let Some(parent) = std::path::Path::new(&socket).parent() {
                if !parent.exists() {
                    match std::fs::create_dir_all(parent) {
                        Ok(_) => {
                            // set permissive dir perms if possible
                            #[cfg(unix)]
                            {
                                use std::os::unix::fs::PermissionsExt;
                                let _ = std::fs::set_permissions(
                                    parent,
                                    std::fs::Permissions::from_mode(0o755),
                                );
                            }
                        }
                        Err(e) => {
                            log::warn!(
                                "could not create parent dir for UDS {}: {}. Skipping host API.",
                                parent.display(),
                                e
                            );
                            start_uds = false;
                        }
                    }
                }
            }

            if start_uds {
                // remove stale socket file if present
                let _ = std::fs::remove_file(&socket);
                let app2 = hostapi::build_router(); //.merge(podman::build_router());
                let socket_clone = socket.clone();
                handles.push(tokio::spawn(async move {
                    // bind a unix domain socket and serve the axum app on it
                    match tokio::net::UnixListener::bind(&socket_clone) {
                        Ok(listener) => {
                            if let Err(e) = axum::serve(listener, app2.into_make_service()).await {
                                log::error!("axum UDS server error: {}", e);
                            }
                        }
                        Err(e) => log::error!("failed to bind UDS {}: {}", socket_clone, e),
                    }
                }));
            }
        }
    }

    // Prepend libp2p handle so caller can decide how to await
    let mut all = vec![libp2p_handle];
    all.extend(handles);
    Ok(all)
}

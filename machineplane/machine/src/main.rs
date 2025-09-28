use clap::Parser;

mod hostapi;
mod libp2p_beemesh;
mod pod_communication;
mod restapi;

/// beemesh Host Agent
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Host address for REST API
    #[arg(long, default_value = "127.0.0.1")]
    api_host: String,

    /// Port for REST API
    #[arg(long, default_value = "3000")]
    api_port: u16,

    /// Disable REST API server
    #[arg(long, default_value_t = false)]
    disable_rest_api: bool,

    /// Disable REST API server
    #[arg(long, default_value_t = false)]
    disable_machine_api: bool,

    /// Custom node name (optional)
    #[arg(long)]
    node_name: Option<String>,

    /// Libp2p base port (optional)
    #[arg(long)]
    libp2p_port: Option<u16>,

    #[arg(long, default_value = "/run/beemesh/host.sock")]
    api_socket: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    let (swarm, topic, peer_rx, peer_tx) = libp2p_beemesh::setup_libp2p_node()?;

    // control channel for libp2p (from REST handlers to libp2p task)
    let (control_tx, control_rx) = tokio::sync::mpsc::unbounded_channel::<libp2p_beemesh::control::Libp2pControl>();

    let libp2p_handle = tokio::spawn(async move {
        if let Err(e) = libp2p_beemesh::start_libp2p_node(swarm, topic, peer_tx, control_rx).await {
            eprintln!("libp2p node error: {}", e);
        }
    });

    let mut handles = Vec::new();

    // rest api server
    if !cli.disable_rest_api {
        let app = restapi::build_router(peer_rx, control_tx.clone());

        // Public TCP server
        let bind_addr = format!("{}:{}", cli.api_host, cli.api_port);
        let listener = tokio::net::TcpListener::bind(&bind_addr).await.unwrap();
        println!("listening on {}", listener.local_addr().unwrap());
        handles.push(tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app.clone().into_make_service()).await {
                eprintln!("axum server error: {}", e);
            }
        }));
    } else {
        println!("REST API disabled");
    }

    // host api server - for gateway to host accesss
    if !cli.disable_machine_api {
        if let Some(socket_path) = cli.api_socket.clone() {
            // remove stale
            let _ = std::fs::remove_file(&socket_path);
            let app2 = hostapi::build_router(); //.merge(podman::build_router());
            let socket = socket_path.clone();
            handles.push(tokio::spawn(async move {
                // bind a unix domain socket and serve the axum app on it
                match tokio::net::UnixListener::bind(&socket) {
                    Ok(listener) => {
                        if let Err(e) = axum::serve(listener, app2.into_make_service()).await {
                            eprintln!("axum UDS server error: {}", e);
                        }
                    }
                    Err(e) => eprintln!("failed to bind UDS {}: {}", socket, e),
                }
            }));
        }
    }

    // Wait for tasks
    if handles.is_empty() {
        let _ = libp2p_handle.await;
    } else {
        let mut all = vec![libp2p_handle];
        all.extend(handles);
        let _ = futures::future::join_all(all).await;
    }
    Ok(())
}

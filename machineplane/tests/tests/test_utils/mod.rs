use machine::{start_machine, Cli};
use std::sync::Once;
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;
use tokio::time::sleep;

static CLEANUP_HOOK_INIT: Once = Once::new();

#[cfg(test)]
#[allow(dead_code)]
pub struct NodeGuard {
    pub handles: Vec<JoinHandle<()>>, // spawned background tasks for in-process nodes
    pub processes: Vec<Child>,        // spawned processes for separate-process nodes
    cleaned_up: bool,                 // track if cleanup was already called
}

#[cfg(test)]
impl NodeGuard {
    #[allow(dead_code)]
    pub async fn cleanup(&mut self) {
        if self.cleaned_up {
            return; // Already cleaned up
        }
        
        // best-effort abort all handles
        for h in self.handles.drain(..) {
            let _ = h.abort();
        }

        // best-effort kill all processes
        for mut process in self.processes.drain(..) {
            let _ = process.kill().await;
        }
        
        self.cleaned_up = true;
    }
}

impl Drop for NodeGuard {
    fn drop(&mut self) {
        if self.cleaned_up {
            return; // Already cleaned up properly
        }
        
        eprintln!("NodeGuard::drop() - Running emergency cleanup");

        // Abort async handles (synchronous)
        for handle in self.handles.drain(..) {
            let _ = handle.abort();
        }

        // Kill processes synchronously (best effort)
        for mut process in self.processes.drain(..) {
            let _ = process.start_kill();
        }

        // System-level cleanup using shell commands
        global_cleanup();
    }
}

#[cfg(test)]
#[allow(dead_code)]
pub fn make_test_cli(
    rest_api_port: u16,
    disable_rest: bool,
    disable_machine: bool,
    api_socket: Option<String>,
    bootstrap_peers: Vec<String>,
    libp2p_tcp_port: u16,
    libp2p_quic_port: u16,
) -> Cli {
    Cli {
        ephemeral: true,
        rest_api_host: "127.0.0.1".to_string(),
        rest_api_port,
        disable_rest_api: disable_rest,
        disable_machine_api: disable_machine,
        node_name: None,
        api_socket,
        key_dir: String::from("/tmp/.beemesh_test_unused"),
        bootstrap_peer: bootstrap_peers,
        libp2p_tcp_port,
        libp2p_quic_port,
        libp2p_host: "0.0.0.0".to_string(),
    }
}

/// Start a list of nodes in separate processes. Returns a NodeGuard which will
/// kill the spawned processes on cleanup. `startup_delay` is awaited after
/// each node start to give it a moment to initialize before starting the next.
#[cfg(test)]
#[allow(dead_code)]
pub async fn start_nodes_as_processes(clis: Vec<Cli>, startup_delay: Duration) -> NodeGuard {
    let mut guard = NodeGuard {
        handles: Vec::new(),
        processes: Vec::new(),
        cleaned_up: false,
    };

    // Build the machine binary path - it should be available in the workspace root target/debug/
    let current_dir = std::env::current_dir().expect("failed to get current dir");
    let machine_binary = if current_dir.ends_with("tests") {
        // We're running from tests/ directory, go up to workspace root
        current_dir
            .parent()
            .expect("no parent dir")
            .join("target/debug/machine")
    } else {
        // We're running from workspace root
        current_dir.join("target/debug/machine")
    };

    if !machine_binary.exists() {
        panic!(
            "machine binary not found at {:?}. Run 'cargo build' first.",
            machine_binary
        );
    }

    for cli in clis {
        // Spawn machine process with CLI args
        let mut cmd = Command::new(&machine_binary);
        cmd.arg("--ephemeral")
            .arg("--rest-api-host")
            .arg(&cli.rest_api_host)
            .arg("--rest-api-port")
            .arg(&cli.rest_api_port.to_string())
            .arg("--libp2p-tcp-port")
            .arg(&cli.libp2p_tcp_port.to_string())
            .arg("--libp2p-quic-port")
            .arg(&cli.libp2p_quic_port.to_string())
            .arg("--libp2p-host")
            .arg(&cli.libp2p_host);

        if cli.disable_rest_api {
            cmd.arg("--disable-rest-api");
        }
        if cli.disable_machine_api {
            cmd.arg("--disable-machine-api");
        }

        for bootstrap in &cli.bootstrap_peer {
            cmd.arg("--bootstrap-peer").arg(bootstrap);
        }

        // Set environment variables for this process
        cmd.env("RUST_LOG", "info,libp2p=warn,quinn=warn")
            .env("BEEMESH_KEM_EPHEMERAL", "1")
            .env("BEEMESH_KEYSTORE_EPHEMERAL", "1");

        //println!("Starting machine process on port {}", cli.rest_api_port);
        match cmd.spawn() {
            Ok(child) => {
                guard.processes.push(child);
            }
            Err(e) => panic!("failed to start machine process: {:?}", e),
        }

        sleep(startup_delay).await;
    }

    guard
}

/// Start a list of nodes given their CLIs. Returns a NodeGuard which will abort
/// the spawned background tasks on cleanup. `startup_delay` is awaited after
/// each node start to give it a moment to initialize before starting the next.
#[cfg(test)]
#[allow(dead_code)]
pub async fn start_nodes(clis: Vec<Cli>, startup_delay: Duration) -> NodeGuard {
    let mut guard = NodeGuard {
        handles: Vec::new(),
        processes: Vec::new(),
        cleaned_up: false,
    };
    for cli in clis {
        match start_machine(cli).await {
            Ok(mut handles) => {
                guard.handles.append(&mut handles);
            }
            Err(e) => panic!("failed to start node: {:?}", e),
        }
        sleep(startup_delay).await;
    }
    guard
}

/// Setup global panic hook for test cleanup. Call this at the beginning of each test.
#[cfg(test)]
#[allow(dead_code)]
pub fn setup_cleanup_hook() {
    CLEANUP_HOOK_INIT.call_once(|| {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // Run cleanup before original panic handling
            eprintln!("Test panic detected, running global cleanup...");
            global_cleanup();
            default_hook(info);
        }));
    });
}

/// Global cleanup function that runs system commands to clean up test artifacts
pub fn global_cleanup() {
    eprintln!("Running global cleanup: pkill + rm commands");

    // Kill any remaining machine processes - be more specific about the pattern
    let pkill_result = std::process::Command::new("pkill")
        .args(["-f", "target/debug/machine"])
        .output();

    match pkill_result {
        Ok(output) => {
            if !output.stdout.is_empty() {
                eprintln!("pkill stdout: {}", String::from_utf8_lossy(&output.stdout));
            }
            if !output.stderr.is_empty() {
                eprintln!("pkill stderr: {}", String::from_utf8_lossy(&output.stderr));
            }
        }
        Err(e) => eprintln!("pkill command failed: {}", e),
    }

    // Remove keystore temporary files
    let rm_result = std::process::Command::new("sh")
        .args(["-c", "rm -f /tmp/beemesh_keystore_*"])
        .output();

    match rm_result {
        Ok(output) => {
            if !output.stderr.is_empty() {
                eprintln!("rm stderr: {}", String::from_utf8_lossy(&output.stderr));
            }
        }
        Err(e) => eprintln!("rm command failed: {}", e),
    }

    eprintln!("Global cleanup completed");
}

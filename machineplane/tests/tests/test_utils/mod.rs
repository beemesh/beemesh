use std::time::Duration;
use tokio::time::sleep;
use machine::{start_machine, Cli};
use tokio::task::JoinHandle;

pub struct NodeGuard {
    pub handles: Vec<JoinHandle<()>>, // spawned background tasks
}

impl NodeGuard {
    pub async fn cleanup(&mut self) {
        // best-effort abort all handles
        for h in self.handles.drain(..) {
            let _ = h.abort();
        }
    }
}

pub fn make_test_cli(api_port: u16, disable_rest: bool, disable_machine: bool, api_socket: Option<String>, bootstrap_peer: Option<String>) -> Cli {
    Cli {
        ephemeral: true,
        api_host: "127.0.0.1".to_string(),
        api_port,
        disable_rest_api: disable_rest,
        disable_machine_api: disable_machine,
        node_name: None,
        api_socket,
        key_dir: String::from("/tmp/.beemesh_test_unused"),
        bootstrap_peer,
    }
}

/// Start a list of nodes given their CLIs. Returns a NodeGuard which will abort
/// the spawned background tasks on cleanup. `startup_delay` is awaited after
/// each node start to give it a moment to initialize before starting the next.
pub async fn start_nodes(clis: Vec<Cli>, startup_delay: Duration) -> NodeGuard {
    let mut guard = NodeGuard { handles: Vec::new() };
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

//! Tests for CLI environment variable parsing.
//!
//! This module verifies that the CLI correctly picks up configuration from environment variables,
//! specifically `PODMAN_HOST`.

use clap::Parser;
use machineplane::Cli;

/// A RAII guard for setting and restoring environment variables.
struct EnvGuard {
    key: &'static str,
    previous: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var(key).ok();
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        if let Some(value) = &self.previous {
            unsafe {
                std::env::set_var(self.key, value);
            }
        } else {
            unsafe {
                std::env::remove_var(self.key);
            }
        }
    }
}

/// Verifies that `PODMAN_HOST` env var sets the `podman_socket` CLI arg.
#[test]
fn cli_pulls_podman_socket_from_env() {
    let _guard = EnvGuard::set("PODMAN_HOST", "unix:///tmp/env-podman.sock");

    let cli = Cli::parse_from(["beemesh-machineplane"]);

    assert_eq!(
        cli.podman_socket.as_deref(),
        Some("unix:///tmp/env-podman.sock")
    );
}

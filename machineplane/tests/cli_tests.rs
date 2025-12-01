//! CLI Tests
//!
//! This module tests CLI argument parsing and environment variable handling.
//! It verifies that the CLI correctly picks up configuration from environment
//! variables like `CONTAINER_HOST`.

use clap::Parser;
use machineplane::Cli;

/// A RAII guard for setting and restoring environment variables in tests.
///
/// # Safety
/// This type uses `unsafe` because [`std::env::set_var`] and [`std::env::remove_var`]
/// are unsafe in Rust 2024 due to potential data races in multi-threaded programs.
///
/// **Why this is safe here:**
/// - Tests using `EnvGuard` run with the `#[serial]` attribute
/// - This ensures single-threaded execution during env manipulation
/// - The RAII pattern guarantees cleanup even on test panic
struct EnvGuard {
    key: &'static str,
    previous: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var(key).ok();
        // SAFETY: Tests using EnvGuard run with #[serial], ensuring single-threaded execution.
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: Restore original environment. Still single-threaded via #[serial] on tests.
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

/// Verifies that `CONTAINER_HOST` env var sets the `podman_socket` CLI arg.
#[test]
fn container_host_env_sets_podman_socket() {
    let _guard = EnvGuard::set("CONTAINER_HOST", "unix:///tmp/env-podman.sock");

    let cli = Cli::parse_from(["beemesh-machineplane"]);

    assert_eq!(
        cli.podman_socket.as_deref(),
        Some("unix:///tmp/env-podman.sock")
    );
}

use clap::Parser;
use machine::Cli;

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

#[test]
fn cli_pulls_podman_socket_from_env() {
    let _guard = EnvGuard::set("PODMAN_HOST", "unix:///tmp/env-podman.sock");

    let cli = Cli::parse_from(["beemesh-machine"]);

    assert_eq!(
        cli.podman_socket.as_deref(),
        Some("unix:///tmp/env-podman.sock")
    );
}

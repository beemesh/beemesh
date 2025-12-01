#!/usr/bin/env bash
set -e

echo "Setting up Podman storage for rootless mode..."

# Create storage directories
mkdir -p /tmp/containers/{runroot,storage}
mkdir -p ~/.config/containers

# Configure storage to use fuse-overlayfs (required for rootless)
cat > ~/.config/containers/storage.conf <<'EOF'
[storage]
driver = "overlay"
runroot = "/tmp/containers/runroot"
graphroot = "/tmp/containers/storage"

[storage.options.overlay]
mount_program = "/usr/bin/fuse-overlayfs"
EOF

# Start Podman socket for REST API access (used by integration tests)
echo "Starting Podman socket..."
podman system service --time=0 unix:///tmp/podman.sock &

# Export CONTAINER_HOST for the machineplane to find the socket
echo "export CONTAINER_HOST=unix:///tmp/podman.sock" >> ~/.bashrc
echo "export CONTAINER_HOST=unix:///tmp/podman.sock" >> ~/.zshrc 2>/dev/null || true

echo "Podman setup complete. Socket available at unix:///tmp/podman.sock"

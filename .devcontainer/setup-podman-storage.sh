#!/usr/bin/env bash
set -e

mkdir -p /tmp/containers/{runroot,storage}
mkdir -p ~/.config/containers

cat > ~/.config/containers/storage.conf <<'EOF'
[storage]
driver = "overlay"
runroot = "/tmp/containers/runroot"
graphroot = "/tmp/containers/storage"

[storage.options.overlay]
mount_program = "/usr/bin/fuse-overlayfs"
EOF

FROM archlinux:latest

ENV RUSTUP_HOME=/usr/local/rustup
ENV CARGO_HOME=/usr/local/cargo
ENV PATH=/usr/local/cargo/bin:$PATH

# Install development dependencies:
# - rustup: Rust toolchain manager
# - git: Version control
# - gcc: C compiler for native dependencies
# - pkg-config: Library configuration
# - openssl: TLS support for reqwest
# - fuse-overlayfs: Overlay filesystem for rootless Podman
# - podman: Container runtime (required for integration tests)
# - musl: For static binary builds (optional, matches production)
RUN pacman -Syu --noconfirm \
        rustup \
        git \
        gcc \
        pkg-config \
        openssl \
        fuse-overlayfs \
        podman \
        musl \
    && pacman -Scc --noconfirm \
    && rm -rf /var/cache/pacman/pkg/*

# Setup Rust stable toolchain
RUN rustup default stable \
    && rustup target add x86_64-unknown-linux-musl \
    && chmod -R a+w "$RUSTUP_HOME" "$CARGO_HOME"

# Install cargo tools for development
RUN cargo install cargo-watch
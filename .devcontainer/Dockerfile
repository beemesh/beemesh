FROM archlinux:latest

ENV RUSTUP_HOME=/usr/local/rustup
ENV CARGO_HOME=/usr/local/cargo

RUN mkdir -p "$RUSTUP_HOME" "$CARGO_HOME" \
    && pacman -Syu --noconfirm rustup podman git \
    && pacman -Scc --noconfirm \
    && rm -rf /var/cache/pacman/pkg/* \
    && rustup default stable \
    && chmod -R a+w "$RUSTUP_HOME" "$CARGO_HOME"
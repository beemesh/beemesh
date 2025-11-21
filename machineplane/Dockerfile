# =====================
# 1. Build Rust binary
# =====================
FROM rust:latest AS builder

WORKDIR /app

# Install musl toolchain
RUN apt-get update \
    && apt-get install -y --no-install-recommends curl musl-tools pkg-config \
    && rm -rf /var/lib/apt/lists/*

RUN rustup target add x86_64-unknown-linux-musl

COPY . .

RUN cargo build --release --locked -p beemesh-machineplane --target x86_64-unknown-linux-musl
RUN strip /app/target/x86_64-unknown-linux-musl/release/machineplane

# ===============================
# 2. Fetch static Podman client
# ===============================
ARG PODMAN_REMOTE_VERSION=5.6.2

RUN curl -sSL "https://github.com/containers/podman/releases/download/v${PODMAN_REMOTE_VERSION}/podman-remote-static-linux_amd64.tar.gz" -o /tmp/podman.tar.gz \
    && mkdir -p /out/podman \
    && tar -xzf /tmp/podman.tar.gz -C /tmp \
    && install -m 0755 /tmp/bin/podman-remote-static-linux_amd64 /out/podman/podman \
    && strip /out/podman/podman \
    && rm -rf /tmp/podman.tar.gz /tmp/bin

# =====================
# 3. Minimal scratch image
# =====================
FROM scratch

# Add BusyBox binary for df
COPY --from=busybox:musl /bin/busybox /bin/busybox

# Symlink df explicitly
RUN ["/bin/busybox", "ln", "-s", "/bin/busybox", "/bin/df"]

# Minimal passwd/group for Podman
COPY --from=alpine:latest /etc/passwd /etc/passwd
COPY --from=alpine:latest /etc/group /etc/group

# CA certificates for HTTPS
COPY --from=alpine:latest /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/
# DNS config
#COPY --from=alpine:latest /etc/resolv.conf /etc/resolv.conf
COPY --from=alpine:latest /etc/nsswitch.conf /etc/nsswitch.conf
COPY --from=alpine:latest /etc/hosts /etc/hosts

# Copy binaries into /usr/local/bin
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/machineplane /usr/local/bin/machineplane
COPY --from=builder /out/podman/podman /usr/local/bin/podman

# Set PATH so /bin and /usr/local/bin are both available
ENV PATH=/usr/local/bin:/bin

ENTRYPOINT ["machineplane"]

FROM rust:latest AS builder

WORKDIR /app

# Install musl toolchain required for the musl target
RUN apt-get update \
	&& apt-get install -y --no-install-recommends curl musl-tools pkg-config \
	&& rm -rf /var/lib/apt/lists/*

RUN rustup target add x86_64-unknown-linux-musl

# Copy the workspace so we can build the machine binary
COPY . .

# Build the release binary for the machine crate using musl target
RUN cargo build --release --locked -p machine --target x86_64-unknown-linux-musl

ARG PODMAN_REMOTE_VERSION=5.0.3

# Fetch a static podman-remote client so the machine binary can drive host Podman
RUN curl -sSL "https://github.com/containers/podman/releases/download/v${PODMAN_REMOTE_VERSION}/podman-remote-static-linux_amd64.tar.gz" -o /tmp/podman-remote.tar.gz \
	&& mkdir -p /out/podman \
	&& tar -xzf /tmp/podman-remote.tar.gz -C /tmp \
	&& install -m 0755 /tmp/bin/podman-remote-static-linux_amd64 /out/podman/podman \
	&& rm -rf /tmp/podman-remote.tar.gz /tmp/bin

FROM alpine:latest

RUN apk add --no-cache ca-certificates

COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/machine /usr/local/bin/machine
COPY --from=builder /out/podman/podman /usr/local/bin/podman

ENTRYPOINT ["/usr/local/bin/machine"]

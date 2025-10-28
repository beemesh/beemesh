FROM rust:1.82-slim-bullseye AS builder

WORKDIR /app

# Copy the workspace so we can build the machine binary
COPY . .

# Build the release binary for the machine crate
RUN cargo build --release -p machine

FROM debian:bookworm-slim

RUN apt-get update \
	&& apt-get install -y --no-install-recommends ca-certificates \
	&& rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/machine /usr/local/bin/machine

ENTRYPOINT ["/usr/local/bin/machine"]

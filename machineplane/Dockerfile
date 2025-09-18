# Dockerfile.machine
FROM golang:1.24-alpine AS builder
WORKDIR /app
# Copy go mod files
COPY go.mod go.sum ./
RUN go mod download
# Copy source code
COPY . .
# Build the Machine Plane binary
RUN CGO_ENABLED=0 GOOS=linux go build -a -installsuffix cgo -o beemesh ./cmd/machine/main.go

# Final stage
FROM alpine:latest
RUN apk --no-cache add ca-certificates podman
WORKDIR /root/
# Copy the built binary
COPY --from=builder /app/beemesh .
# Expose ports for HTTP API and libp2p
EXPOSE 8080 4001
# Run as privileged for Podman socket access
CMD ["./beemesh"]
# Dockerfile.workload
FROM golang:1.24-alpine AS builder
WORKDIR /app
# Copy go mod files
COPY go.mod go.sum ./
RUN go mod download
# Copy Source Code
COPY . .
# Build the workload binary
RUN CGO_ENABLED=0 GOOS=linux go build -a -installsuffix cgo -o workload ./cmd/workload/main.go

# Final Stage
FROM alpine:latest
RUN apk --no-cache add ca-certificates
WORKDIR /root/
# Copy the built binary
COPY --from=builder /app/workload .
# Expose Ports
EXPOSE 8080 9090
# Start the workload infra container
CMD ["./workload"]
# Beemesh: Decentralized Container Orchestration

Beemesh is a lightweight, decentralized container orchestration system designed for infinite scalability. It surpasses Kubernetes' scaling limits (~5,000 nodes, ~150,000 pods) by eliminating the centralized control plane, leveraging loosely coupling, and implementing self-cloning for antifragility. The architecture strictly adheres to **Separation of Concerns** and aligns with **CAP** trade-offs: Availability and Partition Tolerance (A/P) for the Machine-Plane and stateless workloads, Consistency and Partition Tolerance (C/P) with eventual Availability for stateful workloads.

## Architecture

### Overview
Beemesh divides responsibilities into two planes:
- **Machine-Plane**: Handles node-level task execution, networking, and scheduling without application logic. It uses:
  - `libp2p` for decentralized networking (DHT for service discovery, Pub/Sub for task and metrics distribution).
  - Podman for container runtime.
  - A lightweight scheduler for resource-based task assignment.
- **Workload-Plane**: Manages application-specific logic via sidecars, including:
  - **Self-Cloning**: Sidecars monitor replicas for both stateless and stateful workloads and publish replacement tasks to recover from outages.
  - **Network Isolation**: Pods implement isolated private networks, with sidecars intercepting application sockets and proxying traffic via `libp2p` streams, removing the network perimeter for enhanced security.

### Key Features
- **Decentralized Orchestration**: No central control plane; DHT and Pub/Sub scale to thousands of nodes.
- **Self-Cloning Workloads**: Stateless sidecars maintain replica counts; stateful sidecars restore Raft quorum, ensuring resilience.
- **Network Isolation**: Pods use Podman’s `--network=private`, with sidecars as the sole entry/exit point, proxying traffic via `libp2p` streams.
- **CAP Alignment**:
  - **Machine-Plane**: A/P, operates in partitioned networks via `libp2p`.
  - **Stateless Workloads**: A/P, tolerates over-provisioning for availability.
  - **Stateful Workloads**: C/P, uses Raft for consistency, with cloning for eventual availability.
- **Lightweight**: Single binary (~50–80MB RAM) and pods (~20–40MB), ideal for edge devices.
- **Kubernetes Compatibility**: Accepts `Deployment` and `StatefulSet` manifests.

### Project Structure
```
beemesh/
├── main.go                     # Entry point, API server, metrics
├── pkg/
│   ├── types/                  # Shared data structures
│   │   └── types.go
│   ├── crypto/                 # Crypto utilities
│   │   └── crypto.go
│   ├── proxy/                  # libp2p-based proxy for sidecars
│   │   └── proxy.go
│   ├── machine/                # libp2p client for networking
│   │   └── client.go
│   ├── registry/               # DHT-based service and manifest registry
│   │   └── registry.go
│   ├── podman/                 # Podman runtime integration
│   │   └── podman.go
│   ├── scheduler/              # Decentralized scheduler
│   │   └── scheduler.go
├── cmd/
│   ├── sidecars/
│   │   ├── stateless/          # Stateless sidecar 
│   │   │   └── main.go
│   │   └── stateful/           # Stateful sidecar
│   │       └── main.go
├── manifests/
│   ├── my-stateless-app.yaml   # Example stateless workload
│   ├── my-raft-cluster.yaml    # Example stateful workload
├── go.mod                      # Go dependencies
└── README.md                   # Project documentation
```

## Prerequisites
- **Go**: 1.20+
- **Podman**: For container runtime
- **Dependencies**: Listed in `go.mod` (e.g., `libp2p`, `podman`, `gopsutil`, `k8s.io/api`)

## Setup
1. **Clone the Repository** (or use this script to create it).
2. **Build the Binary**:
   ```bash
   go build -o beemesh main.go
   ```
3. **Build Sidecar Images**:
   ```bash
   cd cmd/sidecars/stateless
   docker build -t your-repo/beemesh-stateless-sidecar:latest .
   docker push your-repo/beemesh-stateless-sidecar:latest
   cd ../stateful
   docker build -t your-repo/beemesh-stateful-sidecar:latest .
   docker push your-repo/beemesh-stateful-sidecar:latest
   ```
4. **Run Beemesh on Each Host**:
   ```bash
   export BEEMESH_NODE_ID="beemesh-node-$(uuidgen)"
   ./beemesh
   ```

## Usage
1. **Deploy Workloads**:
   - **Stateless**:
     ```bash
     curl -X POST -H "Content-Type: application/yaml" --data-binary @manifests/my-stateless-app.yaml http://localhost:8080/v1/workloads/stateless
     ```
   - **Stateful**:
     ```bash
     curl -X POST -H "Content-Type: application/yaml" --data-binary @manifests/my-raft-cluster.yaml http://localhost:8080/v1/workloads/stateful
     ```
2. **Verify Deployment**:
   - Check pods: `podman pod ls`, `podman ps`
   - Test services: `curl http://localhost:8080` (routed via sidecar and `libp2p`)
3. **Test Self-Cloning**:
   - Simulate outage: `podman pod rm -f my-stateless-app` or `podman pod rm -f my-raft-cluster-1`
   - Check logs: `Published clone task <task-id> for <workload>`
   - Verify pods: `podman pod ls`

## Security
- **Isolated Networking**: Pods use private network namespaces; sidecars intercept all traffic, removing the network perimeter.
- **libp2p Proxying**: Traffic is routed via encrypted `libp2p` streams (requires explicit Noise/TLS configuration for production).
- **Future Improvements**: Add `libp2p.Security`, TLS for API, and Podman socket access controls.

## Scalability
- **Decentralized**: `libp2p` DHT and Pub/Sub scale to thousands of nodes with low latency (~100–500ms).
- **Lightweight**: ~50–80MB RAM per node, ~20–40MB per pod.
- **Self-Cloning**: Sidecars handle recovery, offloading scheduler and enabling dynamic scaling.
- **No Central Bottlenecks**: Unlike Kubernetes’ `etcd` and `kube-apiserver`, Beemesh distributes state and scheduling.

## Limitations
- **Networking**: Cross-host `libp2p` routing assumes manual configuration; a CNI plugin (e.g., flannel) is recommended.
- **Security**: POC lacks explicit `libp2p` encryption and TLS; production requires these.
- **Ecosystem**: No CLI or Helm-like tooling yet.
- **Clone Deduplication**: Missing, may cause transient over-provisioning.

## Future Enhancements
- Store manifests in DHT for fully decentralized cloning (implemented in `pkg/registry/registry.go`).
- Add `libp2p.Security` for encrypted streams.
- Implement advanced scheduling (affinity, taints).
- Develop a CLI for workload management.
- Expose metrics for cloning and proxying in `/metrics`.

## Contributing
Contributions are welcome! Please submit pull requests or issues for bugs, features, or improvements.

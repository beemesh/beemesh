module beemesh

go 1.22  // Upgraded for improved performance and security features[3]

require (
    github.com/containers/podman/v5 v5.1.0  // Latest for better container isolation and networking[4]
    github.com/google/uuid v1.6.0  // Minor bump for stability
    github.com/hashicorp/raft v1.7.0  // Enhanced quorum handling for stateful workloads[5]
    github.com/libp2p/go-libp2p v0.36.0  // Major upgrade for faster peer discovery and encryption[1]
    github.com/libp2p/go-libp2p-kad-dht v0.25.0  // Aligned with libp2p for efficient DHT queries[1]
    github.com/multiformats/go-multiaddr v0.12.0  // Updated for broader address support
    github.com/prometheus/client_golang v1.19.0  // For advanced metrics in scheduling
    github.com/shirou/gopsutil v3.23.12+incompatible  // Better CPU/memory monitoring
    k8s.io/api v0.30.0  // Latest Kubernetes API compatibility for manifests[6]
    sigs.k8s.io/yaml v1.4.0  // Improved YAML parsing
)

require (
    github.com/beorn7/perks v1.0.1 // indirect
    github.com/cespare/xxhash/v2 v2.3.0 // indirect
    github.com/go-logr/logr v1.4.2 // indirect
    github.com/gogo/protobuf v1.3.2 // indirect
    github.com/golang/protobuf v1.5.4 // indirect
    github.com/libp2p/go-libp2p/core v0.44.0 // indirect
    github.com/prometheus/client_model v0.6.1 // indirect
    github.com/prometheus/common v0.55.0 // indirect
    github.com/prometheus/procfs v0.15.1 // indirect
    golang.org/x/sys v0.21.0 // indirect
    google.golang.org/protobuf v1.34.2 // indirect
)

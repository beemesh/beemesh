package types

type Task struct {
	TaskID        string
	Kind          string // StatelessWorkload or StatefulWorkload
	Name          string
	Spec          interface{}
	Destination   string // Target nodeID
	CPURequest    int64  // Millicores (deprecated, use PerCPURequest)
	MemoryRequest int64  // Bytes (deprecated, use PerMemRequest)
	CloneRequest  bool   // Indicates self-cloning task
	Replicas      int
	PerCPURequest int64  // Millicores per replica
	PerMemRequest int64  // Bytes per replica
	WorkloadKind  string // "stateless" or "stateful"
}

type Service struct {
	Name       string
	ProtocolID string
	IP         string
	Port       int
	Libp2pAddr string // libp2p multiaddr for stream endpoint
}

type HostMetrics struct {
	NodeID     string
	CPUFree    int64 // Millicores
	MemoryFree int64 // Bytes
	Timestamp  int64 // Unix seconds
}

type RBACPolicy struct {
	Name      string
	Verbs     []string // e.g., ["get", "list"]
	Resources []string // e.g., ["tasks", "services"]
	PeerIDs   []string // Bound peer IDs
}

type Proposal struct {
	TaskID       string
	NodeID       string
	MaxInstances int
}

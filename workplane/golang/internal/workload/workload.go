package workload

import (
	"context"
	"errors"
	"fmt"
	"time"

	"github.com/libp2p/go-libp2p/core/peer"
)

// Config contains everything the infra container needs to run in the workload plane.
type Config struct {
	// Identity
	PeerIDStr  string // string form of workload peer id (for logs; optional)
	PrivateKey []byte // marshaled libp2p private key (raw bytes)

	// Topology
	Namespace    string
	WorkloadName string
	PodName      string
	Replicas     int

	// Bootstrap
	BootstrapPeerStrings []string // multiaddrs with /p2p/… (string form)

	// Machine API (for publishing clone tasks if pubsub is not wired)
	BeemeshAPI string // e.g. "http://localhost:8080"

	// Timings
	ReplicaCheckInterval time.Duration
}

// Defaults fills zero values with sane defaults.
func (c *Config) Defaults() {
	if c.Namespace == "" {
		c.Namespace = "default"
	}
	if c.Replicas <= 0 {
		c.Replicas = 1
	}
	if c.BeemeshAPI == "" {
		c.BeemeshAPI = "http://localhost:8080"
	}
	if c.ReplicaCheckInterval == 0 {
		c.ReplicaCheckInterval = 30 * time.Second
	}
}

// Workload ties together network, streams, raft (if stateful), and self-healing.
type Workload struct {
	cfg Config

	Network    *Network
	Raft       *RaftManager // nil when stateless
	SelfHealer *SelfHealer
	isStateful bool
	manifest   []byte
}

// NewWorkload creates a Workload object but does not start background loops.
func NewWorkload(ctx context.Context, cfg Config, manifest []byte, isStateful bool) (*Workload, error) {
	if len(cfg.PrivateKey) == 0 {
		return nil, errors.New("config: PrivateKey is required")
	}
	if cfg.WorkloadName == "" || cfg.PodName == "" {
		return nil, errors.New("config: WorkloadName and PodName are required")
	}
	cfg.Defaults()

	net, err := NewNetwork(ctx, cfg)
	if err != nil {
		return nil, err
	}

	w := &Workload{
		cfg:        cfg,
		Network:    net,
		isStateful: isStateful,
		manifest:   manifest,
	}

	// Register service in Workload DHT
	if err := w.registerSelf(ctx); err != nil {
		_ = w.Close()
		return nil, fmt.Errorf("register: %w", err)
	}

	// Streams: simple health handler; replace with real RPC handlers
	RegisterStreamHandler(net.Host, func(remote peer.ID, req RPCRequest) RPCResponse {
		switch req.Method {
		case "healthz":
			return RPCResponse{OK: true, Body: map[string]interface{}{"remote": remote.String()}}
		default:
			return RPCResponse{OK: false, Error: "unknown method"}
		}
	})

	// Raft for stateful workloads
	if isStateful {
		rm, err := NewRaftManager(ctx, net.Host, cfg)
		if err != nil {
			_ = w.Close()
			return nil, err
		}
		w.Raft = rm

		// Discover peers and bootstrap/join
		infos, err := FindServicePeers(ctx, net.DHT, cfg.Namespace, cfg.WorkloadName)
		if err == nil {
			var ids []string
			for _, ai := range infos {
				ids = append(ids, ai.ID.String())
			}
			w.Raft.BootstrapIfNeeded(net.Host.ID().String(), ids)
		}
	}

	// Self-healing
	w.SelfHealer = NewSelfHealer(net, cfg, manifest)

	return w, nil
}

// Start launches background tasks (self-healing).
func (w *Workload) Start(ctx context.Context) {
	if w.SelfHealer != nil {
		w.SelfHealer.Start(ctx)
	}
}

// Close shuts down components.
func (w *Workload) Close() error {
	if w.SelfHealer != nil {
		w.SelfHealer.Stop()
	}
	// libp2p-raft has no explicit Close; GC on context cancel
	if w.Network != nil {
		return w.Network.Close()
	}
	return nil
}

func (w *Workload) registerSelf(ctx context.Context) error {
	if w.Network == nil {
		return errors.New("network not initialized")
	}
	ai := peer.AddrInfo{ID: w.Network.Host.ID(), Addrs: w.Network.Host.Addrs()}
	return RegisterService(ctx, w.Network.DHT, ai, w.cfg.Namespace, w.cfg.WorkloadName)
}

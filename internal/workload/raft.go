package workload

import (
	"context"
	"fmt"
	"os"
	"path/filepath"

	raftlib "github.com/libp2p/go-libp2p-raft"
	"github.com/libp2p/go-libp2p/core/host"
)

// raftFSM is a tiny demo FSM; replace with your application-specific FSM.
type raftFSM struct{}

func newFSM() *raftFSM { return &raftFSM{} }

// Apply would apply a log entry to state machine (stub).
func (f *raftFSM) Apply(b []byte) interface{} { return nil }

// Snapshot returns current state (stub).
func (f *raftFSM) Snapshot() (raftlib.FSMSnapshot, error) { return &raftSnapshot{}, nil }

// Restore loads state from a previous snapshot (stub).
func (f *raftFSM) Restore(rc *raftlib.Snapshot) error { return nil }

type raftSnapshot struct{}

// Persist writes snapshot to sink (stub).
func (s *raftSnapshot) Persist(sink raftlib.SnapshotSink) error { return sink.Close() }

// Release releases resources (stub).
func (s *raftSnapshot) Release() {}

// RaftManager owns the node for a StatefulSet workload.
type RaftManager struct {
	Node *raftlib.Raft
}

// NewRaftManager initializes a raft node for the workload pod.
func NewRaftManager(ctx context.Context, h host.Host, cfg Config) (*RaftManager, error) {
	dataDir := filepath.Join("/tmp/beemesh-raft", cfg.Namespace, cfg.WorkloadName, cfg.PodName)
	if err := os.MkdirAll(dataDir, 0o755); err != nil {
		return nil, fmt.Errorf("raft data dir: %w", err)
	}
	conf := &raftlib.Config{
		Protocol:      "/beemesh/workload/raft/1.0.0",
		HeartbeatTick: 1,
		ElectionTick:  3,
		StoragePath:   dataDir,
	}
	r, err := raftlib.NewRaft(ctx, h, newFSM(), conf)
	if err != nil {
		return nil, fmt.Errorf("new raft: %w", err)
	}
	return &RaftManager{Node: r}, nil
}

// BootstrapIfNeeded creates a single-node cluster if no peers available.
func (rm *RaftManager) BootstrapIfNeeded(selfID string, peerIDs []string) {
	if len(peerIDs) <= 1 {
		rm.Node.BootstrapCluster(raftlib.BootstrapConfig{
			ID:      selfID,
			Cluster: []string{selfID},
		})
		return
	}
	// otherwise join: filter out self
	var others []string
	for _, id := range peerIDs {
		if id != selfID {
			others = append(others, id)
		}
	}
	rm.Node.JoinCluster(raftlib.JoinConfig{
		ID:      selfID,
		Cluster: others,
	})
}

func (rm *RaftManager) LeaderID() string { return rm.Node.Leader() }

package scheduler

import (
	"context"
	"errors"
	"fmt"
	"sort"
	"time"

	"beemesh/pkg/types"
)

// PublishProposalFn is provided by the machine to broadcast a proposal over the mesh.
type PublishProposalFn func(ctx context.Context, p types.Proposal) error

// DelegateTaskFn tells a remote node to deploy (re-publish the task with Destination = nodeID).
type DelegateTaskFn func(ctx context.Context, nodeID string, t types.Task) error

// DeployLocalFn deploys the task locally (Podman etc).
type DeployLocalFn func(t types.Task) error

// MetricsProviderFn returns current free capacity.
type MetricsProviderFn func() types.HostMetrics

// ScoreFunc converts metrics to a comparable score.
type ScoreFunc func(m types.HostMetrics) float64

// Options config for the scheduler.
type Options struct {
	NodeID           string
	Window           time.Duration // proposal collection window
	PublishProposal  PublishProposalFn
	DelegateTask     DelegateTaskFn
	DeployLocal      DeployLocalFn
	MetricsProvider  MetricsProviderFn
	Score            ScoreFunc
	Now              func() time.Time // injectable clock for tests
	MaxBufferedProps int              // protects against bursty meshes
}

// Scheduler implements the distributed selection + assignment.
type Scheduler struct {
	opt Options
	// proposals intake is bounded to avoid unbounded growth under broadcast storms
	intake chan types.Proposal
}

// New creates a new scheduler instance.
func New(opt Options) (*Scheduler, error) {
	if opt.NodeID == "" {
		return nil, errors.New("scheduler: NodeID is required")
	}
	if opt.PublishProposal == nil || opt.DelegateTask == nil || opt.DeployLocal == nil {
		return nil, errors.New("scheduler: PublishProposal, DelegateTask, and DeployLocal are required")
	}
	if opt.MetricsProvider == nil || opt.Score == nil {
		return nil, errors.New("scheduler: MetricsProvider and Score are required")
	}
	if opt.Window <= 0 {
		opt.Window = 350 * time.Millisecond
	}
	if opt.Now == nil {
		opt.Now = time.Now
	}
	if opt.MaxBufferedProps <= 0 {
		opt.MaxBufferedProps = 256
	}
	return &Scheduler{
		opt:    opt,
		intake: make(chan types.Proposal, opt.MaxBufferedProps),
	}, nil
}

// Intake returns a send-only channel to push proposals observed on pubsub into the scheduler.
// The caller should forward only relevant proposals (same TaskID). Backpressure applies.
func (s *Scheduler) Intake() chan<- types.Proposal { return s.intake }

// DistributedSchedule runs one selection round for a task.
// It (1) computes and publishes our proposal, (2) collects proposals for s.opt.Window,
// (3) assigns replicas proportionally to scores, (4) deploys locally or delegates remotely.
func (s *Scheduler) DistributedSchedule(ctx context.Context, task types.Task) {
	// 0) sanity
	if task.Replicas <= 0 {
		task.Replicas = 1
	}

	// 1) publish our proposal
	m := s.opt.MetricsProvider()
	selfScore := s.opt.Score(m)
	selfProp := types.Proposal{
		TaskID:    task.TaskID,
		NodeID:    s.opt.NodeID,
		Score:     selfScore,
		Timestamp: s.opt.Now().UTC(),
	}
	_ = s.opt.PublishProposal(ctx, selfProp) // best-effort broadcast

	// 2) collect proposals for a short, configurable window
	props := s.collect(ctx, task.TaskID)

	// Ensure our own proposal is included even if we didn't hear ourselves back.
	props = upsertProposal(props, selfProp)

	if len(props) == 0 {
		// No proposals? deploy locally as a fallback.
		_ = s.opt.DeployLocal(task)
		return
	}

	// 3) sort deterministically (score desc, older timestamp first, then NodeID asc)
	sort.Slice(props, func(i, j int) bool {
		if props[i].Score != props[j].Score {
			return props[i].Score > props[j].Score
		}
		if !props[i].Timestamp.Equal(props[j].Timestamp) {
			return props[i].Timestamp.Before(props[j].Timestamp)
		}
		return props[i].NodeID < props[j].NodeID
	})

	// 4) proportional replica assignment (at least 1 for top scorers until we exhaust)
	assignments := s.assignReplicas(task.Replicas, props)

	// 5) execute assignments
	for nodeID, count := range assignments {
		if count <= 0 {
			continue
		}
		if nodeID == s.opt.NodeID {
			// deploy N replicas locally (naive: repeat same manifest N times)
			for i := 0; i < count; i++ {
				if err := s.opt.DeployLocal(task); err != nil {
					// best effort; in a real system you'd emit an event here
				}
			}
		} else {
			// delegate N replicas remotely; publish same task N times but targeted
			for i := 0; i < count; i++ {
				_ = s.opt.DelegateTask(ctx, nodeID, task)
			}
		}
	}
}

// collect drains proposals for the window or until ctx cancellation.
func (s *Scheduler) collect(ctx context.Context, taskID string) []types.Proposal {
	deadline := time.NewTimer(s.opt.Window)
	defer deadline.Stop()

	var out []types.Proposal
	for {
		select {
		case <-ctx.Done():
			return out
		case <-deadline.C:
			return out
		case p := <-s.intake:
			if p.TaskID == taskID {
				out = append(out, p)
			}
		}
	}
}

// assignReplicas distributes replicas across nodes in proportion to their scores,
// guaranteeing deterministic rounding and at least 1 to the top node while replicas remain.
func (s *Scheduler) assignReplicas(total int, props []types.Proposal) map[string]int {
	assign := make(map[string]int, len(props))
	if total <= 0 {
		return assign
	}
	// Sum scores
	var sum float64
	for _, p := range props {
		if p.Score > 0 {
			sum += p.Score
		}
	}
	if sum <= 0 {
		// no meaningful scores; give one to the top prop until we run out
		for i := 0; i < total && i < len(props); i++ {
			assign[props[i].NodeID]++
		}
		return assign
	}

	// First pass: floor of proportional share
	remaining := total
	type frac struct {
		nodeID string
		frac   float64
	}
	// keep track of fractional remainders to allocate leftovers deterministically
	var remainders []frac
	for _, p := range props {
		share := (p.Score / sum) * float64(total)
		base := int(share) // floor
		if base > 0 {
			assign[p.NodeID] += base
			remaining -= base
		}
		remainders = append(remainders, frac{nodeID: p.NodeID, frac: share - float64(base)})
	}

	// Distribute leftovers by largest remainder; tie-break by proposal order already deterministic
	sort.SliceStable(remainders, func(i, j int) bool { return remainders[i].frac > remainders[j].frac })
	for remaining > 0 {
		for _, r := range remainders {
			if remaining == 0 {
				break
			}
			assign[r.nodeID]++
			remaining--
		}
	}
	return assign
}

func upsertProposal(list []types.Proposal, p types.Proposal) []types.Proposal {
	found := false
	for i := range list {
		if list[i].NodeID == p.NodeID && list[i].TaskID == p.TaskID {
			list[i] = p
			found = true
			break
		}
	}
	if !found {
		list = append(list, p)
	}
	return list
}

// Utility for a sensible default score function (available mem + 1000*cpu + network)
func DefaultScore(m types.HostMetrics) float64 {
	return m.MemoryFreeMB + 1000*m.CPUCoresFree + m.NetworkScore
}

// Utility for pretty debug printing (optional)
func DebugAssignments(a map[string]int) string {
	type item struct {
		node string
		n    int
	}
	var v []item
	for k, n := range a {
		v = append(v, item{k, n})
	}
	sort.Slice(v, func(i, j int) bool {
		if v[i].n != v[j].n {
			return v[i].n > v[j].n
		}
		return v[i].node < v[j].node
	})
	out := "assignments:"
	for _, it := range v {
		out += fmt.Sprintf(" %s=%d", it.node, it.n)
	}
	return out
}

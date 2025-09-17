package machine

import (
	"beemesh/pkg/machine/node"
	"beemesh/pkg/machine/podman"
	"beemesh/pkg/types"
	"context"
	"log"
	"sort"
	"time"
)

// Scheduler is responsible for distributed scheduling decisions across the cluster.
// It listens for proposals from other nodes and decides how to distribute replicas.
type Scheduler struct {
	client       *node.Node
	podmanClient *podman.Client
}

// NewScheduler creates a new Scheduler instance.
func NewScheduler(client *node.Node) *Scheduler {
	return &Scheduler{
		client: client,
	}
}

// SetPodmanClient sets the Podman client for the scheduler to use for deployments.
func (s *Scheduler) SetPodmanClient(client *podman.Client) {
	s.podmanClient = client
}

// getCurrentMetrics returns the current node metrics.
func (s *Scheduler) getCurrentMetrics() types.HostMetrics {
	return metrics.GetCurrentMetrics(s.client.GetNodeID())
}

// DistributedSchedule implements a simple distributed scheduling algorithm.
// Steps:
//  1. Each node calculates a "score" representing its available capacity.
//  2. Nodes publish proposals with these scores.
//  3. Scheduler collects proposals for a fixed window (5 seconds).
//  4. Replicas are assigned starting with the node that has the highest score.
func (s *Scheduler) DistributedSchedule(ctx context.Context, task types.Task, proposalsChan <-chan types.Proposal) {
	if s.podmanClient == nil {
		log.Printf("[Scheduler] Error: Podman client not set, cannot deploy task %s", task.TaskID)
		return
	}

	nodeID := s.client.GetNodeID()

	// Step 1: Gather current metrics for this node.
	m := s.getCurrentMetrics()

	// Calculate a simple score: available memory (MB) + (CPU cores * 1000)
	// This is a naive heuristic. A production scheduler would use a more sophisticated formula.
	score := float64(m.MemoryFree/1024/1024) + float64(m.CPUCores*1000)

	// Step 2: Publish this node's proposal to the cluster.
	proposal := types.Proposal{
		TaskID:    task.TaskID,
		NodeID:    nodeID,
		Score:     score,
		Timestamp: time.Now().UTC(),
	}

	if err := s.client.PublishProposal(ctx, proposal); err != nil {
		log.Printf("[Scheduler] Failed to publish proposal for task %s: %v", task.TaskID, err)
		return
	}

	log.Printf("[Scheduler] Published proposal for task %s with score %.2f", task.TaskID, score)

	// Step 3: Collect proposals from all nodes for 5 seconds.
	var proposals []types.Proposal
	timer := time.NewTimer(5 * time.Second)
	defer timer.Stop()

collectLoop:
	for {
		select {
		case p := <-proposalsChan:
			if p.TaskID == task.TaskID {
				proposals = append(proposals, p)
			}
		case <-timer.C:
			break collectLoop
		case <-ctx.Done():
			log.Printf("[Scheduler] Context cancelled while collecting proposals for task %s", task.TaskID)
			return
		}
	}

	if len(proposals) == 0 {
		log.Printf("[Scheduler] No proposals received for task %s. Deploying locally.", task.TaskID)
		// If no other proposals, deploy locally.
		if err := s.podmanClient.DeployWorkloadFromTask(task); err != nil {
			log.Printf("[Scheduler] Failed to deploy task %s locally: %v", task.TaskID, err)
		} else {
			log.Printf("[Scheduler] Successfully deployed task %s locally", task.TaskID)
		}
		return
	}

	// Step 4: Sort proposals by score (descending). Highest score first.
	sort.Slice(proposals, func(i, j int) bool {
		return proposals[i].Score > proposals[j].Score
	})

	// Step 5: Assign replicas to nodes based on sorted scores.
	remaining := task.Replicas
	for _, p := range proposals {
		if remaining <= 0 {
			break
		}

		// In this basic implementation, each node gets only one replica at a time.
		assign := 1
		if assign > remaining {
			assign = remaining
		}

		if p.NodeID == nodeID {
			// Instead of deploying immediately, wait a short time to ensure we've collected most proposals.
			// This prevents the first responder from always getting the task.
			time.Sleep(100 * time.Millisecond)

			// Deploy directly on this node.
			if err := s.podmanClient.DeployWorkloadFromTask(task); err != nil {
				log.Printf("[Scheduler] Failed to deploy replica of task %s on node %s: %v", task.TaskID, nodeID, err)
			} else {
				log.Printf("[Scheduler] Deployed replica of task %s on node %s", task.TaskID, nodeID)
			}
		} else {
			log.Printf("[Scheduler] Replica of task %s assigned to node %s", task.TaskID, p.NodeID)
			// In a real system, you would send a direct message to that node's libp2p host
			// instructing it to deploy the task. For now, we log it.
			// TODO: Implement direct task delegation to remote nodes.
		}

		remaining -= assign
	}

	if remaining > 0 {
		log.Printf("[Scheduler] Insufficient resources or nodes to deploy all replicas for task %s. Remaining: %d", task.TaskID, remaining)
	}
}

const (
	TopicTasks     = "scheduler-tasks"
	TopicProposals = "scheduler-proposals"
	TopicWins      = "scheduler-wins"
)

type ResourceRequirements struct {
	CPU    float64 `json:"cpu"`
	Memory float64 `json:"memory"`
}

type Task struct {
	ID         string               `json:"id"`
	Name       string               `json:"name"`
	Resources  ResourceRequirements `json:"resources"`
	Priority   int                  `json:"priority"`
	Manifest   []byte               `json:"manifest"`
	DeadlineMs int64                `json:"deadline_ms,omitempty"`
}

type Bid struct {
	TaskID string  `json:"task_id"`
	NodeID string  `json:"node_id"`
	Score  float64 `json:"score"`
}

type Win struct {
	TaskID string `json:"task_id"`
	NodeID string `json:"node_id"`
}

type Scheduler struct {
	log    Logger
	ps     *common.PubSub
	dht    *MachineDHT
	mx     *Metrics
	nodeID string

	// internal queues
	wins <-chan common.Message // alias doesn't exist; adapt
}

// modify PubSub visibility
type pubsub interface {
	Publish(string, any) error
	Subscribe(string) <-chan common.Message
}

func NewScheduler(log Logger, ps *common.PubSub, dht *MachineDHT, mx *Metrics, nodeID string) *Scheduler {
	return &Scheduler{log: log, ps: ps, dht: dht, mx: mx, nodeID: nodeID}
}

func PublishTask(ctx context.Context, ps *common.PubSub, t Task) error {
	return ps.Publish(TopicTasks, t)
}

// EvaluateAndBid listens for tasks and submits bids based on local metrics.
func (s *Scheduler) EvaluateAndBid(ctx context.Context) error {
	taskCh := s.ps.Subscribe(TopicTasks)
	for {
		select {
		case <-ctx.Done():
			return ctx.Err()
		case msg, ok := <-taskCh:
			if !ok {
				return nil
			}
			task := common.Decode[Task](msg.Data)
			score := s.score(task)
			bid := Bid{TaskID: task.ID, NodeID: s.nodeID, Score: score}
			s.log.Infof("bid: task=%s node=%s score=%.3f", task.ID, s.nodeID, score)
			_ = s.ps.Publish(TopicProposals, bid)

			// Local selection window (100-500ms)
			go s.selectBestAfterWindow(task.ID, 200*time.Millisecond)
		}
	}
}

func (s *Scheduler) score(t Task) float64 {
	// naive score: higher is better
	availCPU := s.mx.AvailableCPU()
	availMem := s.mx.AvailableMemory()
	if availCPU <= 0 || availMem <= 0 {
		return 0
	}
	return (availCPU / (t.Resources.CPU + 0.01)) + (availMem / (t.Resources.Memory + 1))
}

func (s *Scheduler) selectBestAfterWindow(taskID string, window time.Duration) {
	proposals := make([]Bid, 0, 8)
	proCh := s.ps.Subscribe(TopicProposals)
	timer := time.NewTimer(window)
	for {
		select {
		case <-timer.C:
			if len(proposals) == 0 {
				return
			}
			sort.Slice(proposals, func(i, j int) bool { return proposals[i].Score > proposals[j].Score })
			win := Win{TaskID: taskID, NodeID: proposals[0].NodeID}
			s.log.Infof("win selected locally: task=%s node=%s", taskID, win.NodeID)
			_ = s.ps.Publish(TopicWins, win)
			return
		case msg := <-proCh:
			bid := common.Decode[Bid](msg.Data)
			if bid.TaskID == taskID {
				proposals = append(proposals, bid)
			}
		}
	}
}

// DeployLoop listens for wins and deploys the task if this node won.
func (s *Scheduler) DeployLoop(ctx context.Context) error {
	winCh := s.ps.Subscribe(TopicWins)
	for {
		select {
		case <-ctx.Done():
			return ctx.Err()
		case msg, ok := <-winCh:
			if !ok {
				return nil
			}
			win := common.Decode[Win](msg.Data)
			if win.NodeID == s.nodeID {
				s.log.Infof("deploying task=%s on node=%s (simulated)", win.TaskID, s.nodeID)
				// TODO: integrate with Podman
			}
		}
	}
}

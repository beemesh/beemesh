package workload

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"time"

	"github.com/google/uuid"
	"sigs.k8s.io/yaml"

	"beemesh/internal/common"
)

// SelfHealer monitors replicas via the Workload DHT and issues clone tasks if needed.
type SelfHealer struct {
	Network   *Network
	Config    Config
	HC        *http.Client // used to call machine-plane HTTP API when needed
	Manifest  []byte       // the original manifest for cloning
	stopCh    chan struct{}
	stoppedCh chan struct{}
}

// NewSelfHealer constructs a controller.
func NewSelfHealer(n *Network, cfg Config, manifest []byte) *SelfHealer {
	return &SelfHealer{
		Network:   n,
		Config:    cfg,
		HC:        &http.Client{Timeout: 5 * time.Second},
		Manifest:  manifest,
		stopCh:    make(chan struct{}),
		stoppedCh: make(chan struct{}),
	}
}

// Start begins the watch loop.
func (s *SelfHealer) Start(ctx context.Context) {
	go func() {
		defer close(s.stoppedCh)
		ticker := time.NewTicker(s.Config.ReplicaCheckInterval)
		defer ticker.Stop()

		for {
			select {
			case <-ctx.Done():
				return
			case <-s.stopCh:
				return
			case <-ticker.C:
				s.checkAndHeal(ctx)
			}
		}
	}()
}

func (s *SelfHealer) Stop() {
	close(s.stopCh)
	<-s.stoppedCh
}

func (s *SelfHealer) checkAndHeal(ctx context.Context) {
	// Count providers for this service
	providers, err := FindServicePeers(ctx, s.Network.DHT, s.Config.Namespace, s.Config.WorkloadName)
	if err != nil {
		return
	}
	if len(providers) >= s.Config.Replicas {
		return
	}
	deficit := s.Config.Replicas - len(providers)
	for i := 0; i < deficit; i++ {
		_ = s.publishCloneTask()
	}
}

// publishCloneTask posts a clone request to the machine plane HTTP API.
func (s *SelfHealer) publishCloneTask() error {
	var kindHint struct {
		Kind string `yaml:"kind"`
	}
	if err := yaml.Unmarshal(s.Manifest, &kindHint); err != nil {
		return fmt.Errorf("parse manifest kind: %w", err)
	}

	var workloadKind string
	switch kindHint.Kind {
	case "Deployment":
		workloadKind = "StatelessWorkload"
	case "StatefulSet":
		workloadKind = "StatefulWorkload"
	default:
		return fmt.Errorf("unsupported manifest kind %q", kindHint.Kind)
	}

	name, err := common.ParseManifestName(s.Manifest, workloadKind)
	if err != nil {
		return fmt.Errorf("parse manifest name: %w", err)
	}

	task := common.Task{
		TaskID:       uuid.NewString(),
		Kind:         workloadKind,
		Name:         name,
		Manifest:     s.Manifest,
		Destination:  "scheduler",
		CloneRequest: true,
		Replicas:     1,
		Namespace:    s.Config.Namespace,
	}

	body, _ := json.Marshal(task)
	req, _ := http.NewRequest("POST", s.Config.BeemeshAPI+"/v1/publish_task", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")

	resp, err := s.HC.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode >= 300 {
		return fmt.Errorf("publish clone task: status %d", resp.StatusCode)
	}
	return nil
}

package common

import "time"

type Task struct {
	// Unique identifier for this task
	TaskID string `json:"taskID"`

	// Logical type of workload or task.
	// Examples:
	//   - "StatelessWorkload"
	//   - "StatefulWorkload"
	//   - "ControlPlaneTask"
	Kind string `json:"kind"`

	// Kubernetes workload/service name
	Name string `json:"name"`

	// Raw Kubernetes manifest associated with this task
	Manifest []byte `json:"manifest"`

	// Logical routing destination:
	//   - "scheduler" to send to the distributed scheduler
	//   - "" or "all" to broadcast
	//   - nodeID of a specific node to deploy directly
	Destination string `json:"destination"`

	// Indicates that this task is a request to clone an existing workload
	CloneRequest bool `json:"cloneRequest"`

	// Number of replicas requested for scaling/self-healing
	Replicas int `json:"replicas"`

	// Kubernetes namespace for the workload
	Namespace string `json:"namespace"`

	// Optional free-form specification for future extensibility
	// Use only JSON-friendly types (maps, slices, primitive values, or concrete structs)
	Spec any `json:"spec,omitempty"`
}

//
// Scheduler Types
//

// Proposal represents a distributed scheduling decision from a node
// participating in the cluster's scheduling process.
type Proposal struct {
	// Unique ID of the task being scheduled
	TaskID string `json:"taskID"`

	// The node making the proposal
	NodeID string `json:"nodeID"`

	// Score or weight for scheduling
	Score float64 `json:"score"`

	// Timestamp of when the proposal was made
	Timestamp time.Time `json:"timestamp"`
}

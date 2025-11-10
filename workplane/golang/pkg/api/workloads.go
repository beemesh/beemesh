package api

type WorkloadSpec struct {
	Name     string            `json:"name"`
	Image    string            `json:"image"`
	Replicas int               `json:"replicas"`
	Env      map[string]string `json:"env,omitempty"`
}

package api

type MachineInfo struct {
	ID       string  `json:"id"`
	CPUFree  float64 `json:"cpu_free"`
	MemFree  float64 `json:"mem_free"`
	Hostname string  `json:"hostname,omitempty"`
}

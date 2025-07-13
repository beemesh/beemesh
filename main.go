package main

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net/http"
	"os"
	"sort"
	"time"

	"beemesh/pkg/machine"
	"beemesh/pkg/podman"
	"beemesh/pkg/registry"
	"beemesh/pkg/scheduler"
	"beemesh/pkg/types"

	"github.com/gorilla/mux"
	"github.com/prometheus/client_golang/prometheus/promhttp"
	"github.com/shirou/gopsutil/cpu"
	"github.com/shirou/gopsutil/mem"
)

func main() {
	ctx := context.Background()
	nodeID := os.Getenv("BEEMESH_NODE_ID")
	client, err := machine.NewClient(ctx, nodeID)
	if err != nil {
		log.Fatalf("Failed to create Machine-Plane client: %v", err)
	}
	defer client.Close()

	reg, err := registry.NewRegistry(ctx, client, nodeID)
	if err != nil {
		log.Fatalf("Failed to create registry: %v", err)
	}
	defer reg.Close()

	podmanClient, err := podman.NewClient()
	if err != nil {
		log.Fatalf("Failed to create Podman client: %v", err)
	}

	sched := scheduler.NewScheduler(client)
	client.ReceiveTasks(ctx, func(task types.Task) {
		if task.Destination == "scheduler" {
			distributedSchedule(ctx, task, client, podmanClient)
		} else if task.Destination == "" || task.Destination == "all" || task.Destination == client.nodeID {
			if err := podmanClient.Deploy(task); err != nil {
				log.Printf("Failed to deploy task %s: %v", task.TaskID, err)
			}
		}
	})

	go func() {
		ticker := time.NewTicker(10 * time.Second)
		defer ticker.Stop()
		for {
			select {
			case <-ctx.Done():
				return
			case <-ticker.C:
				metrics := getCurrentMetrics(client.nodeID)
				if err := client.PublishMetrics(ctx, metrics); err != nil {
					log.Printf("Failed to publish metrics: %v", err)
				}
			}
		}
	}()

	router := mux.NewRouter()

	// K8s-like API paths
	router.HandleFunc("/api/v1/namespaces/{namespace}/pods", func(w http.ResponseWriter, r *http.Request) {
		vars := mux.Vars(r)
		namespace := vars["namespace"]
		// List pods: Query DHT for workloads in namespace
		// Placeholder: Implement actual listing
		fmt.Fprintf(w, "Listing pods in namespace %s", namespace)
	}).Methods("GET")

	router.HandleFunc("/apis/apps/v1/namespaces/{namespace}/deployments", func(w http.ResponseWriter, r *http.Request) {
		vars := mux.Vars(r)
		namespace := vars["namespace"]
		body, err := io.ReadAll(r.Body)
		if err != nil {
			http.Error(w, "Failed to read request body", http.StatusBadRequest)
			return
		}
		if err := sched.ProcessWorkload(ctx, body, "stateless", namespace); err != nil {
			http.Error(w, fmt.Sprintf("Failed to process deployment in namespace %s: %v", namespace, err), http.StatusInternalServerError)
			return
		}
		fmt.Fprintf(w, "Deployment processed in namespace %s", namespace)
	}).Methods("POST")

	router.HandleFunc("/apis/apps/v1/namespaces/{namespace}/statefulsets", func(w http.ResponseWriter, r *http.Request) {
		vars := mux.Vars(r)
		namespace := vars["namespace"]
		body, err := io.ReadAll(r.Body)
		if err != nil {
			http.Error(w, "Failed to read request body", http.StatusBadRequest)
			return
		}
		if err := sched.ProcessWorkload(ctx, body, "stateful", namespace); err != nil {
			http.Error(w, fmt.Sprintf("Failed to process statefulset in namespace %s: %v", namespace, err), http.StatusInternalServerError)
			return
		}
		fmt.Fprintf(w, "StatefulSet processed in namespace %s", namespace)
	}).Methods("POST")

	// RBAC endpoints
	router.HandleFunc("/apis/rbac.authorization.k8s.io/v1/namespaces/{namespace}/roles", func(w http.ResponseWriter, r *http.Request) {
		vars := mux.Vars(r)
		namespace := vars["namespace"]
		body, err := io.ReadAll(r.Body)
		if err != nil {
			http.Error(w, "Failed to read request body", http.StatusBadRequest)
			return
		}
		var policy types.RBACPolicy
		if err := json.Unmarshal(body, &policy); err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}
		policyData, _ := json.Marshal(policy)
		if err := reg.StoreRBACPolicy(ctx, namespace, policy.Name, policyData); err != nil {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}
		fmt.Fprintf(w, "RBAC Role %s created in namespace %s", policy.Name, namespace)
	}).Methods("POST")

	// ConfigMap endpoints
	router.HandleFunc("/api/v1/namespaces/{namespace}/configmaps", func(w http.ResponseWriter, r *http.Request) {
		vars := mux.Vars(r)
		namespace := vars["namespace"]
		body, err := io.ReadAll(r.Body)
		if err != nil {
			http.Error(w, "Failed to read request body", http.StatusBadRequest)
			return
		}
		var config struct {
			Name string            `json:"name"`
			Data map[string]string `json:"data"`
		}
		if err := json.Unmarshal(body, &config); err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}
		data, _ := json.Marshal(config.Data)
		if err := reg.StoreConfigMap(ctx, namespace, config.Name, data); err != nil {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}
		fmt.Fprintf(w, "ConfigMap %s created in namespace %s", config.Name, namespace)
	}).Methods("POST")

	// Existing endpoints with namespace support
	router.HandleFunc("/v1/workloads/stateless", func(w http.ResponseWriter, r *http.Request) {
		namespace := r.Header.Get("X-Namespace")
		if namespace == "" {
			namespace = "default"
		}
		body, err := io.ReadAll(r.Body)
		if err != nil {
			http.Error(w, "Failed to read request body", http.StatusBadRequest)
			return
		}
		if err := sched.ProcessWorkload(ctx, body, "stateless", namespace); err != nil {
			http.Error(w, fmt.Sprintf("Failed to process workload: %v", err), http.StatusInternalServerError)
			return
		}
		fmt.Fprintf(w, "Stateless workload processed in namespace %s", namespace)
	}).Methods("POST")

	router.HandleFunc("/v1/workloads/stateful", func(w http.ResponseWriter, r *http.Request) {
		namespace := r.Header.Get("X-Namespace")
		if namespace == "" {
			namespace = "default"
		}
		body, err := io.ReadAll(r.Body)
		if err != nil {
			http.Error(w, "Failed to read request body", http.StatusBadRequest)
			return
		}
		if err := sched.ProcessWorkload(ctx, body, "stateful", namespace); err != nil {
			http.Error(w, fmt.Sprintf("Failed to process workload: %v", err), http.StatusInternalServerError)
			return
		}
		fmt.Fprintf(w, "Stateful workload processed in namespace %s", namespace)
	}).Methods("POST")

	router.HandleFunc("/v1/register_service", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != "POST" {
			http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
			return
		}
		namespace := r.Header.Get("X-Namespace")
		if namespace == "" {
			namespace = "default"
		}
		var service types.Service
		if err := json.NewDecoder(r.Body).Decode(&service); err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}
		if err := reg.RegisterService(ctx, namespace, service); err != nil {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}
		fmt.Fprintf(w, "Service registered in namespace %s", namespace)
	}).Methods("POST")

	router.HandleFunc("/v1/resolve_service", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != "GET" {
			http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
			return
		}
		namespace := r.Header.Get("X-Namespace")
		if namespace == "" {
			namespace = "default"
		}
		protocolID := r.URL.Query().Get("protocol_id")
		if protocolID == "" {
			http.Error(w, "Missing protocol_id", http.StatusBadRequest)
			return
		}
		addresses, err := reg.ResolveService(ctx, namespace, protocolID)
		if err != nil {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}
		json.NewEncoder(w).Encode(addresses)
	}).Methods("GET")

	router.HandleFunc("/v1/store_manifest", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != "POST" {
			http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
			return
		}
		namespace := r.Header.Get("X-Namespace")
		if namespace == "" {
			namespace = "default"
		}
		workloadName := r.URL.Query().Get("workload_name")
		if workloadName == "" {
			http.Error(w, "Missing workload_name", http.StatusBadRequest)
			return
		}
		body, err := io.ReadAll(r.Body)
		if err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}
		if err := reg.StoreManifest(ctx, namespace, workloadName, body); err != nil {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}
		fmt.Fprintf(w, "Manifest stored in namespace %s", namespace)
	}).Methods("POST")

	router.HandleFunc("/v1/get_manifest", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != "GET" {
			http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
			return
		}
		namespace := r.Header.Get("X-Namespace")
		if namespace == "" {
			namespace = "default"
		}
		workloadName := r.URL.Query().Get("workload_name")
		if workloadName == "" {
			http.Error(w, "Missing workload_name", http.StatusBadRequest)
			return
		}
		manifest, err := reg.GetManifest(ctx, namespace, workloadName)
		if err != nil {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}
		w.Write(manifest)
	}).Methods("GET")

	router.HandleFunc("/v1/publish_task", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != "POST" {
			http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
			return
		}
		namespace := r.Header.Get("X-Namespace")
		if namespace == "" {
			namespace = "default"
		}
		var task types.Task
		if err := json.NewDecoder(r.Body).Decode(&task); err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}
		// Validate RBAC here
		if err := validateRBAC(ctx, reg, namespace, r.Header.Get("X-Peer-ID"), "publish", "tasks"); err != nil {
			http.Error(w, fmt.Sprintf("RBAC denied: %v", err), http.StatusForbidden)
			return
		}
		if err := client.PublishTask(ctx, task); err != nil {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}
		fmt.Fprintf(w, "Task published in namespace %s", namespace)
	}).Methods("POST")

	router.Handle("/metrics", promhttp.Handler())
	log.Fatal(http.ListenAndServe(":8080", router))
}

func distributedSchedule(ctx context.Context, task types.Task, client *machine.Client, podmanClient *podman.Client) {
	metrics := getCurrentMetrics(client.nodeID)
	perCPU := task.PerCPURequest
	perMem := task.PerMemRequest
	maxInst := int(1<<63 - 1) // large number
	if perCPU > 0 {
		maxInst = int(metrics.CPUFree / perCPU)
	}
	if perMem > 0 {
		maxInst = min(maxInst, int(metrics.MemoryFree/perMem))
	}
	if maxInst > 0 {
		if err := client.PublishProposal(ctx, types.Proposal{TaskID: task.TaskID, NodeID: client.nodeID, MaxInstances: maxInst}); err != nil {
			log.Printf("Failed to publish proposal for task %s: %v", task.TaskID, err)
		}
	}
	proposalsChan := client.ReceiveProposals(ctx)
	var proposals []types.Proposal
	timer := time.NewTimer(5 * time.Second)
	defer timer.Stop()
	for {
		select {
		case p := <-proposalsChan:
			if p.TaskID == task.TaskID {
				proposals = append(proposals, p)
			}
		case <-timer.C:
			sort.Slice(proposals, func(i, j int) bool {
				return proposals[i].NodeID < proposals[j].NodeID
			})
			remaining := task.Replicas
			currentOrdinal := 0
			isStateful := task.WorkloadKind == "stateful"
			for _, p := range proposals {
				if remaining <= 0 {
					break
				}
				assign := min(p.MaxInstances, remaining)
				if p.NodeID == client.nodeID {
					if err := podmanClient.DeployInstances(task, assign, currentOrdinal, isStateful); err != nil {
						log.Printf("Failed to deploy instances for task %s: %v", task.TaskID, err)
					} else {
						log.Printf("Deployed %d instances for task %s starting at ordinal %d", assign, task.TaskID, currentOrdinal)
					}
				}
				currentOrdinal += assign
				remaining -= assign
			}
			if remaining > 0 {
				log.Printf("Insufficient resources to deploy all %d replicas for task %s, %d remaining", task.Replicas, task.TaskID, remaining)
			}
			return
		}
	}
}

func getCurrentMetrics(nodeID string) types.HostMetrics {
	totalCPU, _ := cpu.Counts(true)
	cpuPercent, _ := cpu.Percent(0, false)
	cpuUsed := int64(float64(totalCPU) * 1000 * cpuPercent[0] / 100)
	vm, _ := mem.VirtualMemory()
	return types.HostMetrics{
		NodeID:     nodeID,
		CPUFree:    int64(totalCPU*1000) - cpuUsed,
		MemoryFree: int64(vm.Free),
		Timestamp:  time.Now().Unix(),
	}
}

func min(a, b int) int {
	if a < b {
		return a
	}
	return b
}

func validateRBAC(ctx context.Context, reg *registry.Registry, namespace, peerID, verb, resource string) error {
	// Fetch all roles in namespace
	// For simplicity, assume roles are stored as "role/<name>"
	// This is placeholder; in practice, query DHT for keys with prefix
	// Assume we fetch a policy
	policyBytes, err := reg.GetRBACPolicy(ctx, namespace, "default-role") // Example
	if err != nil {
		return err
	}
	var policy types.RBACPolicy
	if err := json.Unmarshal(policyBytes, &policy); err != nil {
		return err
	}
	if !contains(policy.PeerIDs, peerID) {
		return fmt.Errorf("peer %s not bound to policy", peerID)
	}
	if !contains(policy.Verbs, verb) || !contains(policy.Resources, resource) {
		return fmt.Errorf("action %s on %s not allowed", verb, resource)
	}
	return nil
}

func contains(slice []string, item string) bool {
	for _, s := range slice {
		if s == item {
			return true
		}
	}
	return false
}

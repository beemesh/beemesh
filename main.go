package main

import (
	"context"
	"fmt"
	"io"
	"log"
	"net/http"
	"os"
	"time"

	"beemesh/pkg/machine"
	"beemesh/pkg/podman"
	"beemesh/pkg/scheduler"
	"beemesh/pkg/types"
	"github.com/prometheus/client_golang/prometheus/promhttp"
	"github.com/shirou/gopsutil/cpu"
	"github.com/shirou/gopsutil/mem"
)

func main() {
	ctx := context.Background()
	client, err := machine.NewClient(ctx, os.Getenv("BEEMESH_NODE_ID"))
	if err != nil {
		log.Fatalf("Failed to create Machine-Plane client: %v", err)
	}
	defer client.Close()

	podmanClient, err := podman.NewClient()
	if err != nil {
		log.Fatalf("Failed to create Podman client: %v", err)
	}

	scheduler := scheduler.NewScheduler(client)
	client.ReceiveTasks(ctx, func(task types.Task) {
		if err := podmanClient.Deploy(task); err != nil {
			log.Printf("Failed to deploy task %s: %v", task.TaskID, err)
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
				totalCPU, _ := cpu.Counts(true)
				cpuPercent, _ := cpu.Percent(0, false)
				cpuUsed := int64(float64(totalCPU) * 1000 * cpuPercent[0] / 100)
				vm, _ := mem.VirtualMemory()
				metrics := types.HostMetrics{
					NodeID:     client.nodeID,
					CPUFree:    int64(totalCPU*1000) - cpuUsed,
					MemoryFree: int64(vm.Free),
					Timestamp:  time.Now().Unix(),
				}
				if err := client.PublishMetrics(ctx, metrics); err != nil {
					log.Printf("Failed to publish metrics: %v", err)
				}
			}
		}
	}()

	http.HandleFunc("/v1/workloads/stateless", func(w http.ResponseWriter, r *http.Request) {
		body, err := io.ReadAll(r.Body)
		if err != nil {
			http.Error(w, "Failed to read request body", http.StatusBadRequest)
			return
		}
		if err := scheduler.ProcessWorkload(ctx, body, "stateless"); err != nil {
			http.Error(w, fmt.Sprintf("Failed to process workload: %v", err), http.StatusInternalServerError)
			return
		}
		fmt.Fprintf(w, "Stateless workload processed")
	})

	http.HandleFunc("/v1/workloads/stateful", func(w http.ResponseWriter, r *http.Request) {
		body, err := io.ReadAll(r.Body)
		if err != nil {
			http.Error(w, "Failed to read request body", http.StatusBadRequest)
			return
		}
		if err := scheduler.ProcessWorkload(ctx, body, "stateful"); err != nil {
			http.Error(w, fmt.Sprintf("Failed to process workload: %v", err), http.StatusInternalServerError)
			return
		}
		fmt.Fprintf(w, "Stateful workload processed")
	})

	http.Handle("/metrics", promhttp.Handler())
	log.Fatal(http.ListenAndServe(":8080", nil))
}

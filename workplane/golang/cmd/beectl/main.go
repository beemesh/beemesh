// cmd/beectl/main.go
// A CLI tool for Beemesh with kubectl-like syntax.
package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net/http"
	"os"
	"os/exec"

	"github.com/google/uuid"
	"gopkg.in/yaml.v3"
)

const (
	// DefaultBeemeshAPI is the default endpoint for the Beemesh daemon.
	DefaultBeemeshAPI = "http://localhost:8080"
	// DefaultNamespace is used if none is specified.
	DefaultNamespace = "default"
)

// KubernetesResource is a minimal struct to parse the 'kind' and 'metadata.name' from a manifest.
type KubernetesResource struct {
	Kind     string `yaml:"kind" json:"kind"`
	Metadata struct {
		Name string `yaml:"name" json:"name"`
	} `yaml:"metadata" json:"metadata"`
}

func main() {
	if len(os.Args) < 2 {
		printUsage()
		os.Exit(1)
	}

	command := os.Args[1]

	switch command {
	case "create":
		handleCreate(os.Args[2:])
	case "get":
		handleGet(os.Args[2:])
	case "delete":
		handleDelete(os.Args[2:])
	default:
		fmt.Fprintf(os.Stderr, "Error: unknown command %q\n", command)
		printUsage()
		os.Exit(1)
	}
}

// handleCreate handles the 'beectl create -f <file>' command.
func handleCreate(args []string) {
	if len(args) < 2 || args[0] != "-f" {
		fmt.Fprintln(os.Stderr, "Error: 'create' requires '-f <filename>'")
		fmt.Fprintln(os.Stderr, "Example: beectl create -f my-app.yaml")
		os.Exit(1)
	}

	filename := args[1]
	manifestData, err := os.ReadFile(filename)
	if err != nil {
		log.Fatalf("Failed to read file %s: %v", filename, err)
	}

	// Determine the resource type (Deployment or StatefulSet) from the manifest.
	resource, err := parseResourceKind(manifestData)
	if err != nil {
		log.Fatalf("Failed to parse manifest: %v", err)
	}

	var endpoint string
	switch resource.Kind {
	case "Deployment":
		endpoint = "/apis/apps/v1/namespaces/default/deployments"
	case "StatefulSet":
		endpoint = "/apis/apps/v1/namespaces/default/statefulsets"
	default:
		log.Fatalf("Unsupported resource kind: %s. Only 'Deployment' and 'StatefulSet' are supported.", resource.Kind)
	}

	// Send POST request to Beemesh API.
	resp, err := sendHTTPRequest("POST", endpoint, bytes.NewReader(manifestData), "application/yaml")
	if err != nil {
		log.Fatalf("Failed to create resource: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		log.Fatalf("Server returned error: %s", string(body))
	}

	fmt.Printf("%s '%s' created\n", resource.Kind, resource.Metadata.Name)
}

// handleGet handles the 'beectl get <resource>' command.
func handleGet(args []string) {
	if len(args) == 0 {
		fmt.Fprintln(os.Stderr, "Error: resource type must be specified")
		fmt.Fprintln(os.Stderr, "Supported types: pods, deployments, statefulsets")
		os.Exit(1)
	}

	resourceType := args[0]
	switch resourceType {
	case "pods":
		// For now, we shell out to 'podman pod ls' as a practical way to list pods.
		// A true implementation would call a Beemesh API endpoint.
		cmd := exec.Command("podman", "pod", "ls")
		cmd.Stdout = os.Stdout
		cmd.Stderr = os.Stderr
		if err := cmd.Run(); err != nil {
			log.Fatalf("Failed to list pods: %v", err)
		}
	case "deployments", "statefulsets":
		fmt.Printf("NAME\n")
		fmt.Printf("(No %s found or not implemented in Beemesh API)\n", resourceType)
	default:
		fmt.Fprintf(os.Stderr, "Error: unsupported resource type %q\n", resourceType)
		fmt.Fprintln(os.Stderr, "Supported types: pods, deployments, statefulsets")
		os.Exit(1)
	}
}

// handleDelete handles the 'beectl delete <resource> <name>' command.
func handleDelete(args []string) {
	if len(args) < 2 {
		fmt.Fprintln(os.Stderr, "Error: resource type and name must be specified")
		fmt.Fprintln(os.Stderr, "Example: beectl delete pod my-pod-123456")
		os.Exit(1)
	}

	resourceType, name := args[0], args[1]

	switch resourceType {
	case "pod":
		// To delete a pod, we publish a task to the scheduler with a special flag or directly remove it via Podman.
		// Since the Beemesh API doesn't have a direct 'delete pod' endpoint, we'll use the task API.
		taskID := uuid.New().String()
		task := map[string]interface{}{
			"TaskID":      taskID,
			"Kind":        "DeletePod",
			"Name":        name,
			"Destination": "all", // or target specific node if known
			"Namespace":   DefaultNamespace,
		}

		taskData, err := json.Marshal(task)
		if err != nil {
			log.Fatalf("Failed to marshal delete task: %v", err)
		}

		resp, err := sendHTTPRequest("POST", "/v1/publish_task", bytes.NewReader(taskData), "application/json")
		if err != nil {
			log.Fatalf("Failed to publish delete task: %v", err)
		}
		defer resp.Body.Close()

		if resp.StatusCode != http.StatusOK {
			body, _ := io.ReadAll(resp.Body)
			log.Fatalf("Server returned error: %s", string(body))
		}

		fmt.Printf("pod '%s' deleted (task %s published)\n", name, taskID)
	case "deployment", "statefulset":
		fmt.Fprintf(os.Stderr, "Error: Deleting %s is not implemented in Beemesh API.\n", resourceType)
		os.Exit(1)
	default:
		fmt.Fprintf(os.Stderr, "Error: unsupported resource type %q for delete\n", resourceType)
		fmt.Fprintln(os.Stderr, "Supported types: pod, deployment, statefulset")
		os.Exit(1)
	}
}

// parseResourceKind parses the 'kind' and 'metadata.name' from a YAML or JSON manifest.
func parseResourceKind(data []byte) (*KubernetesResource, error) {
	resource := &KubernetesResource{}

	// Try YAML first.
	if err := yaml.Unmarshal(data, resource); err == nil && resource.Kind != "" {
		return resource, nil
	}

	// If YAML fails, try JSON.
	if err := json.Unmarshal(data, resource); err == nil && resource.Kind != "" {
		return resource, nil
	}

	return nil, fmt.Errorf("failed to parse manifest, 'kind' field not found or invalid format")
}

// sendHTTPRequest sends an HTTP request to the Beemesh API.
func sendHTTPRequest(method, endpoint string, body io.Reader, contentType string) (*http.Response, error) {
	url := DefaultBeemeshAPI + endpoint
	req, err := http.NewRequest(method, url, body)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Content-Type", contentType)
	req.Header.Set("X-Namespace", DefaultNamespace)

	client := &http.Client{}
	return client.Do(req)
}

// printUsage prints the CLI usage.
func printUsage() {
	fmt.Println("beectl - A CLI for Beemesh with kubectl-like syntax")
	fmt.Println()
	fmt.Println("Usage:")
	fmt.Println("  beectl [command]")
	fmt.Println()
	fmt.Println("Available Commands:")
	fmt.Println("  create -f <filename>   Create a resource from a file.")
	fmt.Println("  get <resource>         Display one or many resources.")
	fmt.Println("  delete <resource> <name> Delete a resource.")
	fmt.Println()
	fmt.Println("Examples:")
	fmt.Println("  beectl create -f my-deployment.yaml")
	fmt.Println("  beectl get pods")
	fmt.Println("  beectl delete pod my-pod-123456")
}

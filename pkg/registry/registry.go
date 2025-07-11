package registry

import (
	"context"
	"fmt"
	"log"
	"strings"

	"beemesh/pkg/types"
	"github.com/libp2p/go-libp2p/core/host"
	"github.com/libp2p/go-libp2p/core/peer"
	dht "github.com/libp2p/go-libp2p-kad-dht"
)

type Registry struct {
	client *Client
	dht    *dht.IpfsDHT
	nodeID string
}

func NewRegistry(ctx context.Context, client *Client, nodeID string) (*Registry, error) {
	dht, err := dht.New(ctx, client.Host())
	if err != nil {
		return nil, fmt.Errorf("failed to create DHT: %v", err)
	}
	if err := dht.Bootstrap(ctx); err != nil {
		return nil, fmt.Errorf("failed to bootstrap DHT: %v", err)
	}
	return &Registry{client: client, dht: dht, nodeID: nodeID}, nil
}

func (c *Client) Host() host.Host {
	return c.host
}

func (r *Registry) Client() *Client {
	return r.client
}

func (r *Registry) Close() error {
	return r.dht.Close()
}

func (r *Registry) RegisterService(ctx context.Context, namespace string, service types.Service) error {
	key := fmt.Sprintf("/ns/%s/%s/%s", namespace, service.ProtocolID, service.Name)
	value := []byte(service.Libp2pAddr)
	if err := r.dht.PutValue(ctx, key, value); err != nil {
		return fmt.Errorf("failed to register service %s in namespace %s: %v", service.Name, namespace, err)
	}
	log.Printf("Registered service %s at %s in namespace %s", service.Name, service.Libp2pAddr, namespace)
	return nil
}

func (r *Registry) ResolveService(ctx context.Context, namespace, protocolID string) ([]string, error) {
	records, err := r.dht.FindProviders(ctx, fmt.Sprintf("/ns/%s/%s", namespace, protocolID))
	if err != nil {
		return nil, fmt.Errorf("failed to resolve service %s in namespace %s: %v", protocolID, namespace, err)
	}
	addresses := make([]string, 0, len(records))
	for _, record := range records {
		for _, addr := range record.Addrs {
			addresses = append(addresses, addr.String())
		}
	}
	return addresses, nil
}

func (r *Registry) StoreManifest(ctx context.Context, namespace, workloadName string, manifest []byte) error {
	key := fmt.Sprintf("/ns/%s/beemesh/manifest/%s", namespace, workloadName)
	if err := r.dht.PutValue(ctx, key, manifest); err != nil {
		return fmt.Errorf("failed to store manifest %s in namespace %s: %v", workloadName, namespace, err)
	}
	log.Printf("Stored manifest for %s in DHT under namespace %s", workloadName, namespace)
	return nil
}

func (r *Registry) GetManifest(ctx context.Context, namespace, workloadName string) ([]byte, error) {
	key := fmt.Sprintf("/ns/%s/beemesh/manifest/%s", namespace, workloadName)
	value, err := r.dht.GetValue(ctx, key)
	if err != nil {
		return nil, fmt.Errorf("failed to get manifest %s in namespace %s: %v", workloadName, namespace, err)
	}
	return value, nil
}

func (r *Registry) StoreRBACPolicy(ctx context.Context, namespace, policyName string, policy []byte) error {
	key := fmt.Sprintf("/ns/%s/rbac/%s", namespace, policyName)
	if err := r.dht.PutValue(ctx, key, policy); err != nil {
		return fmt.Errorf("failed to store RBAC policy %s in namespace %s: %v", policyName, namespace, err)
	}
	log.Printf("Stored RBAC policy %s in DHT under namespace %s", policyName, namespace)
	return nil
}

func (r *Registry) GetRBACPolicy(ctx context.Context, namespace, policyName string) ([]byte, error) {
	key := fmt.Sprintf("/ns/%s/rbac/%s", namespace, policyName)
	value, err := r.dht.GetValue(ctx, key)
	if err != nil {
		return nil, fmt.Errorf("failed to get RBAC policy %s in namespace %s: %v", policyName, namespace, err)
	}
	return value, nil
}

func (r *Registry) StoreConfigMap(ctx context.Context, namespace, configName string, data []byte) error {
	key := fmt.Sprintf("/ns/%s/config/%s", namespace, configName)
	if err := r.dht.PutValue(ctx, key, data); err != nil {
		return fmt.Errorf("failed to store ConfigMap %s in namespace %s: %v", configName, namespace, err)
	}
	return nil
}

func (r *Registry) GetConfigMap(ctx context.Context, namespace, configName string) ([]byte, error) {
	key := fmt.Sprintf("/ns/%s/config/%s", namespace, configName)
	return r.dht.GetValue(ctx, key)
}

package machine

import (
	"context"
	"encoding/json"
	"fmt"
	"log"
	"time"

	"beemesh/pkg/types"
	"github.com/libp2p/go-libp2p"
	"github.com/libp2p/go-libp2p/core/host"
	"github.com/libp2p/go-libp2p/core/network"
	"github.com/libp2p/go-libp2p/core/peer"
	"github.com/libp2p/go-libp2p/p2p/protocol/ping"
	"github.com/multiformats/go-multiaddr"
)

type Client struct {
	host      host.Host
	nodeID    string
	resources chan types.HostMetrics
}

func NewClient(ctx context.Context, nodeID string) (*Client, error) {
	h, err := libp2p.New()
	if err != nil {
		return nil, fmt.Errorf("failed to create libp2p host: %v", err)
	}
	return &Client{host: h, nodeID: nodeID, resources: make(chan types.HostMetrics, 10)}, nil
}

func (c *Client) Host() host.Host {
	return c.host
}

func (c *Client) PublishTask(ctx context.Context, task types.Task) error {
	topic, err := c.host.PubSub().Join("scheduler-tasks")
	if err != nil {
		return fmt.Errorf("failed to join scheduler-tasks: %v", err)
	}
	data, err := json.Marshal(task)
	if err != nil {
		return fmt.Errorf("failed to marshal task: %v", err)
	}
	return topic.Publish(ctx, data)
}

func (c *Client) ReceiveTasks(ctx context.Context, handler func(types.Task)) {
	topic, err := c.host.PubSub().Join("scheduler-tasks")
	if err != nil {
		log.Fatalf("Failed to join scheduler-tasks: %v", err)
	}
	sub, err := topic.Subscribe()
	if err != nil {
		log.Fatalf("Failed to subscribe to scheduler-tasks: %v", err)
	}
	go func() {
		for {
			msg, err := sub.Next(ctx)
			if err != nil {
				log.Printf("Failed to read from scheduler-tasks: %v", err)
				continue
			}
			var task types.Task
			if err := json.Unmarshal(msg.Data, &task); err != nil {
				log.Printf("Failed to unmarshal task: %v", err)
				continue
			}
			handler(task)
		}
	}()
}

func (c *Client) PublishMetrics(ctx context.Context, metrics types.HostMetrics) error {
	topic, err := c.host.PubSub().Join("scheduler-metrics")
	if err != nil {
		return fmt.Errorf("failed to join scheduler-metrics: %v", err)
	}
	data, err := json.Marshal(metrics)
	if err != nil {
		return fmt.Errorf("failed to marshal metrics: %v", err)
	}
	return topic.Publish(ctx, data)
}

func (c *Client) ReceiveMetrics(ctx context.Context) chan types.HostMetrics {
	go func() {
		topic, err := c.host.PubSub().Join("scheduler-metrics")
		if err != nil {
			log.Fatalf("Failed to join scheduler-metrics: %v", err)
		}
		sub, err := topic.Subscribe()
		if err != nil {
			log.Fatalf("Failed to subscribe to scheduler-metrics: %v", err)
		}
		for {
			msg, err := sub.Next(ctx)
			if err != nil {
				log.Printf("Failed to read from scheduler-metrics: %v", err)
				continue
			}
			var metrics types.HostMetrics
			if err := json.Unmarshal(msg.Data, &metrics); err != nil {
				log.Printf("Failed to unmarshal metrics: %v", err)
				continue
			}
			c.resources <- metrics
		}
	}()
	return c.resources
}

func (c *Client) Close() error {
	return c.host.Close()
}

package main

import (
	"context"
	"fmt"
	"os"

	"github.com/libp2p/go-libp2p-core/host"
	pubsub "github.com/libp2p/go-libp2p-pubsub"
)

func pubsubInit(ctx context.Context, pod host.Host, appID string) {
	gossipSub, err := pubsub.NewGossipSub(ctx, pod)
	if err != nil {
		panic(err)
	}
	subscription, err := gossipSub.Subscribe(config.ProtocolID)
	if err != nil {
		panic(err)
	}
	for {
		msg, err := subscription.Next(ctx)
		if err != nil {
			fmt.Fprintln(os.Stderr, err)
			continue
		}
		logger.Info(msg.TopicIDs)
	}
}

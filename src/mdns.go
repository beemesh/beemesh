package main

import (
	"context"
	"time"

	"github.com/libp2p/go-libp2p-core/host"
	"github.com/libp2p/go-libp2p-core/network"
	"github.com/libp2p/go-libp2p-core/peer"
	"github.com/libp2p/go-libp2p/p2p/discovery"
)

type Notifee struct {
	pod host.Host
	ctx context.Context
}

func (notifee *Notifee) HandlePeerFound(addrInfo peer.AddrInfo) {
	if notifee.pod.Network().Connectedness(addrInfo.ID) != network.Connected {
		err := notifee.pod.Connect(notifee.ctx, addrInfo)
		if err != nil {
			logger.Warn("Connection failed: ", err)
		} else {
			logger.Debug("Connection established: ", addrInfo)
		}
	}
}

func mdnsInit(ctx context.Context, pod host.Host, appID string) (n *Notifee) {
	service, err := discovery.NewMdnsService(ctx, pod, time.Second*30, appID)
	if err != nil {
		panic(err)
	}
	notifee := &Notifee{pod: pod, ctx: ctx}
	service.RegisterNotifee(notifee)
	logger.Debug("Notifee registered ...")
	return notifee
}

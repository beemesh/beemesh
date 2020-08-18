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

		logger.Debug("Handle Peer Found", addrInfo)
		err := notifee.pod.Connect(notifee.ctx, addrInfo)
		if err != nil {
			logger.Debug("Peer failed: ", err)
			/*for _, conn := range notifee.pod.Network().ConnsToPeer(addrInfo.ID) {
				logger.Info("Close connection to peer ", addrInfo.ID)
				conn.Close()
			}
			notifee.pod.Network().ClosePeer(addrInfo.ID)
			notifee.pod.Peerstore().ClearAddrs(addrInfo.ID)*/
		} else {
			logger.Info("Peer connected: ", addrInfo)
			logger.Info("Conns / FindPeers connected: ", len(pod.Network().Conns()), len(peers))
			peers <- addrInfo
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

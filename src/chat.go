package main

import (
	"context"
	"flag"
	"fmt"
	"io"
	"net"
	"sync"

	"github.com/ipfs/go-log"
	"github.com/libp2p/go-libp2p"
	"github.com/libp2p/go-libp2p-core/network"
	"github.com/libp2p/go-libp2p-core/peer"
	"github.com/libp2p/go-libp2p-core/protocol"
	discovery "github.com/libp2p/go-libp2p-discovery"
	dht "github.com/libp2p/go-libp2p-kad-dht"
	multiaddr "github.com/multiformats/go-multiaddr"
)

var logger = log.Logger("beemesh")

func handleStream(stream network.Stream) {
	logger.Info("New incomming connection. Fowarding to server")
	conn, err := net.Dial("tcp", config.ServerConnectAddress)
	if err != nil {
		logger.Errorf("Error connecting to server: %v", err)
		if err := stream.Close(); err != nil {
			logger.Errorf("Cannot close stream: %v", err)
		}
		return
	}
	forward(stream, conn)
}

func forward(stream network.Stream, conn net.Conn) {
	go func() {
		if _, err := io.Copy(stream, conn); err != nil {
			logger.Errorf("Error forwarding data from connection to stream: %v", err)
		}
	}()
	go func() {
		if _, err := io.Copy(conn, stream); err != nil {
			logger.Errorf("Error sending data from stream to connection: %v", err)
		}
	}()
}

var config Config

func main() {
	log.SetLogLevel("beemesh", "info")
	help := flag.Bool("h", false, "Display Help")
	var err error
	config, err = ParseFlags()
	if err != nil {
		panic(err)
	}

	if *help {
		fmt.Println("This program demonstrates a simple p2p chat application using libp2p")
		fmt.Println()
		fmt.Println("Usage: Run './chat in two different terminals. Let them connect to the bootstrap nodes, announce themselves and connect to the peers")
		flag.PrintDefaults()
		return
	}

	ctx := context.Background()

	// libp2p.New constructs a new libp2p Host. Other options can be added
	// here.
	host, err := libp2p.New(ctx,
		libp2p.ListenAddrs([]multiaddr.Multiaddr(config.ListenAddresses)...),
		libp2p.NATPortMap(),
	)
	if err != nil {
		panic(err)
	}
	logger.Info("Host created. We are:", host.ID())
	logger.Info(host.Addrs())

	// Set a function as stream handler. This function is called when a peer
	// initiates a connection and starts a stream with this peer.
	host.SetStreamHandler(protocol.ID(config.ProtocolID), handleStream)

	// // Start a DHT, for use in peer discovery. We can't just make a new DHT
	// // client because we want each peer to maintain its own local copy of the
	// // DHT, so that the bootstrapping node of the DHT can go down without
	// // inhibiting future peer discovery.
	kademliaDHT, err := dht.New(ctx, host)
	if err != nil {
		panic(err)
	}

	// Bootstrap the DHT. In the default configuration, this spawns a Background
	// thread that will refresh the peer table every five minutes.
	logger.Debug("Bootstrapping the DHT")
	if err = kademliaDHT.Bootstrap(ctx); err != nil {
		panic(err)
	}

	// Let's connect to the bootstrap nodes first. They will tell us about the
	// other nodes in the network.
	var wg sync.WaitGroup
	//logger.Debug(config.BootstrapPeers)
	for _, peerAddr := range config.BootstrapPeers {
		peerinfo, _ := peer.AddrInfoFromP2pAddr(peerAddr)
		wg.Add(1)
		go func() {
			defer wg.Done()
			if err := host.Connect(ctx, *peerinfo); err != nil {
				logger.Warning(err)
			} else {
				logger.Info("Connection established with bootstrap node:", *peerinfo)
			}
		}()
	}
	wg.Wait()

	// We use a rendezvous point "meet me here" to announce our location.
	// This is like telling your friends to meet you at the Eiffel Tower.
	logger.Info("Announcing ourselves...")
	routingDiscovery := discovery.NewRoutingDiscovery(kademliaDHT)
	discovery.Advertise(ctx, routingDiscovery, config.RendezvousString)
	logger.Info("Successfully announced!")
	kademliaDHT.RoutingTable().Print()

	// Open listening port
	ln, err := net.Listen("tcp", config.ProxyListenAddress)
	if err != nil {
		panic(err)
	}
	for {
		conn, err := ln.Accept()
		if err != nil {
			// handle error
			continue
		}
		logger.Info("New incomming connection. Looking for peers to forward")

		// Now, look for others who have announced
		// This is like your friend telling you the location to meet you.
		logger.Debug("Searching for other peers...")
		peerChan, err := routingDiscovery.FindPeers(ctx, config.RendezvousString)
		if err != nil {
			panic(err)
		}
		var found bool
		for peer := range peerChan {
			if peer.ID == host.ID() {
				continue
			}

			logger.Debugf("Connecting to: %s", peer)
			stream, err := host.NewStream(ctx, peer.ID, protocol.ID(config.ProtocolID))
			if err != nil {
				logger.Warningf("Connection failed: %v", err)
				continue
			}
			logger.Infof("Forwarding to peer %v", peer.ID)
			forward(stream, conn)
			found = true
			break
		}
		if !found {
			logger.Info("No peers found. Disconnecting")
			if err := conn.Close(); err != nil {
				logger.Infof("error closing connection: %v", err)
			}
		}
	}
}

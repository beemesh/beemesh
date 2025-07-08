package proxy

import (
	"context"
	"fmt"
	"io"
	"log"
	"net"
	"strings"

	"beemesh/pkg/registry"
	"beemesh/pkg/types"
	"github.com/libp2p/go-libp2p/core/network"
	"github.com/libp2p/go-libp2p/core/peer"
	"github.com/multiformats/go-multiaddr"
)

type Proxy struct {
	registry       *registry.Registry
	parentKind     string
	parentName     string
	desiredReplicas string
	localPort      int
}

func NewProxy(reg *registry.Registry, parentKind, parentName, desiredReplicas string, localPort int) (*Proxy, error) {
	return &Proxy{
		registry:       reg,
		parentKind:     parentKind,
		parentName:     parentName,
		desiredReplicas: desiredReplicas,
		localPort:      localPort,
	}, nil
}

func (p *Proxy) ListenAndServe(proxyPort string) error {
	p.registry.Client().Host().SetStreamHandler("/beemesh/proxy/1.0", p.handleStream)
	listener, err := net.Listen("tcp", fmt.Sprintf(":%s", proxyPort))
	if err != nil {
		return fmt.Errorf("failed to listen on proxy port %s: %v", proxyPort, err)
	}
	defer listener.Close()
	log.Printf("Proxy listening on %s, intercepting traffic to localhost:%d", proxyPort, p.localPort)
	for {
		conn, err := listener.Accept()
		if err != nil {
			log.Printf("Failed to accept connection: %v", err)
			continue
		}
		go p.handleLocalConnection(conn)
	}
}

func (p *Proxy) handleLocalConnection(conn net.Conn) {
	defer conn.Close()
	localConn, err := net.Dial("tcp", fmt.Sprintf("localhost:%d", p.localPort))
	if err != nil {
		log.Printf("Failed to connect to local app at localhost:%d: %v", p.localPort, err)
		return
	}
	defer localConn.Close()
	go io.Copy(localConn, conn)
	io.Copy(conn, localConn)
}

func (p *Proxy) handleStream(s network.Stream) {
	defer s.Close()
	buf := make([]byte, 1024)
	n, err := s.Read(buf)
	if err != nil {
		log.Printf("Failed to read from stream: %v", err)
		return
	}
	destService := string(buf[:n])
	addresses, err := p.registry.ResolveService(context.Background(), fmt.Sprintf("/beemesh/sidecar/%s/1.0", p.parentKind))
	if err != nil {
		log.Printf("Failed to resolve service %s: %v", destService, err)
		return
	}
	var targetAddr string
	for _, addr := range addresses {
		if strings.Contains(addr, destService) {
			targetAddr = addr
			break
		}
	}
	if targetAddr == "" {
		log.Printf("Service %s not found", destService)
		return
	}
	peerAddr, err := multiaddr.NewMultiaddr(targetAddr)
	if err != nil {
		log.Printf("Invalid libp2p address %s: %v", targetAddr, err)
		return
	}
	peerID, err := peer.AddrInfoFromP2pAddr(peerAddr)
	if err != nil {
		log.Printf("Failed to parse peer address %s: %v", targetAddr, err)
		return
	}
	targetStream, err := p.registry.Client().Host().NewStream(context.Background(), peerID.ID, "/beemesh/proxy/1.0")
	if err != nil {
		log.Printf("Failed to open stream to %s: %v", peerID.ID, err)
		return
	}
	defer targetStream.Close()
	_, err = targetStream.Write([]byte(destService))
	if err != nil {
		log.Printf("Failed to write to target stream: %v", err)
		return
	}
	go io.Copy(targetStream, s)
	io.Copy(s, targetStream)
}

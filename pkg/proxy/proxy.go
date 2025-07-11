package proxy

import (
	"fmt"
	"log"
	"net"
)

type Proxy struct {
	parentKind     string
	parentName     string
	desiredReplicas string
	localPort      int
}

func NewProxy(parentKind, parentName, desiredReplicas string, localPort int) (*Proxy, error) {
	return &Proxy{
		parentKind     : parentKind,
		parentName     : parentName,
		desiredReplicas: desiredReplicas,
		localPort      : localPort,
	}, nil
}

func (p *Proxy) ListenAndServe(proxyPort string) error {
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

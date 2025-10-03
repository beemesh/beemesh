use libp2p::gossipsub;

pub fn mdns_discovered(
    list: Vec<(libp2p::PeerId, libp2p::Multiaddr)>,
    swarm: &mut libp2p::Swarm<super::MyBehaviour>,
    handshake_states: &mut std::collections::HashMap<libp2p::PeerId, super::super::HandshakeState>,
    peer_tx: &tokio::sync::watch::Sender<Vec<String>>,
) {
    use libp2p::multiaddr::{Multiaddr, Protocol};
    
    for (peer_id, multiaddr) in list {
        log::warn!("mDNS discovered peer {} at {}", peer_id, multiaddr);
        
        // Add the peer to gossipsub mesh
        swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
        
        // Add the original address to the address book
        swarm.add_peer_address(peer_id, multiaddr.clone());
        
        // For in-process testing: if this is an external IP, also add a localhost version
        let mut is_external_ip = false;
        let mut port = None;
        for proto in multiaddr.iter() {
            match proto {
                Protocol::Ip4(ip) if !ip.is_loopback() && !ip.is_unspecified() => {
                    is_external_ip = true;
                }
                Protocol::Tcp(p) => port = Some(p),
                _ => {}
            }
        }
        
        if is_external_ip && port.is_some() {
            let localhost_addr = format!("/ip4/127.0.0.1/tcp/{}/p2p/{}", port.unwrap(), peer_id);
            if let Ok(localhost_multiaddr) = localhost_addr.parse::<Multiaddr>() {
                log::warn!("Adding localhost address for in-process testing: {}", localhost_multiaddr);
                swarm.add_peer_address(peer_id, localhost_multiaddr);
            }
        }
        
        handshake_states.entry(peer_id.clone()).or_insert(super::super::HandshakeState {
            attempts: 0,
            last_attempt: tokio::time::Instant::now() - std::time::Duration::from_secs(3),
            confirmed: false,
        });
    }
    // Update peer list in channel
    let topic = gossipsub::IdentTopic::new(protocol::libp2p_constants::BEEMESH_CLUSTER);
    let peers: Vec<String> = swarm.behaviour().gossipsub.mesh_peers(&topic.hash()).map(|p| p.to_string()).collect();
    let _ = peer_tx.send(peers);
}

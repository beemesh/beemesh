use libp2p::gossipsub;

pub fn mdns_discovered(
    list: Vec<(libp2p::PeerId, libp2p::Multiaddr)>,
    swarm: &mut libp2p::Swarm<super::MyBehaviour>,
    handshake_states: &mut std::collections::HashMap<libp2p::PeerId, super::super::HandshakeState>,
    peer_tx: &tokio::sync::watch::Sender<Vec<String>>,
) {
    for (peer_id, _multiaddr) in list {
        swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
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

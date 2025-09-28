use libp2p::gossipsub;

pub fn gossipsub_subscribed(peer_id: libp2p::PeerId, topic: gossipsub::TopicHash) {
    println!("Peer {peer_id} subscribed to topic: {topic}");
}



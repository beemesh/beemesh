
use libp2p::gossipsub;

pub fn gossipsub_unsubscribed(peer_id: libp2p::PeerId, topic: gossipsub::TopicHash) {
    println!("Peer {peer_id} unsubscribed from topic: {topic}");
}



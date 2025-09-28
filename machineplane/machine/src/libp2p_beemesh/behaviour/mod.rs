pub mod apply_inbound_failure;
pub mod apply_outbound_failure;
pub mod apply_message;
pub mod gossipsub_message;
pub mod gossipsub_subscribed;
pub mod gossipsub_unsubscribed;
pub mod handshake_inbound_failure;
pub mod handshake_outbound_failure;
pub mod handshake_message;
pub mod mdns_discovered;
pub mod mdns_expired;

pub use apply_inbound_failure::apply_inbound_failure;
pub use apply_outbound_failure::apply_outbound_failure;
pub use apply_message::apply_message;
pub use gossipsub_message::gossipsub_message;
pub use gossipsub_subscribed::gossipsub_subscribed;
pub use gossipsub_unsubscribed::gossipsub_unsubscribed;
pub use handshake_inbound_failure::handshake_inbound_failure;
pub use handshake_outbound_failure::handshake_outbound_failure;
pub use handshake_message::handshake_message_event;
use libp2p::{gossipsub, kad, mdns, request_response, swarm::NetworkBehaviour};
pub use mdns_discovered::mdns_discovered;
pub use mdns_expired::mdns_expired;

use crate::libp2p_beemesh::{ApplyCodec, HandshakeCodec};

// Add DHT event handlers
pub mod kademlia_event;
pub use kademlia_event::kademlia_event;

#[derive(NetworkBehaviour)]
pub struct MyBehaviour {
    pub gossipsub: gossipsub::Behaviour,
    pub mdns: mdns::tokio::Behaviour,
    pub apply_rr: request_response::Behaviour<ApplyCodec>,
    pub handshake_rr: request_response::Behaviour<HandshakeCodec>,
    pub kademlia: kad::Behaviour<kad::store::MemoryStore>,
}
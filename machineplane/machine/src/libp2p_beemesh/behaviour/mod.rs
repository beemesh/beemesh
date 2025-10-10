pub mod apply_inbound_failure;
pub mod apply_message;
pub mod apply_outbound_failure;
pub mod failure_handlers;
pub mod gossipsub_message;
pub mod gossipsub_subscribed;
pub mod gossipsub_unsubscribed;
pub mod handshake_inbound_failure;
pub mod handshake_message;
pub mod handshake_outbound_failure;
pub mod keyshare_message;
pub mod mdns_discovered;
pub mod mdns_expired;
pub mod scheduler_message;

pub use apply_inbound_failure::apply_inbound_failure;
pub use apply_message::apply_message;
pub use apply_outbound_failure::apply_outbound_failure;
pub use gossipsub_message::gossipsub_message;
pub use gossipsub_subscribed::gossipsub_subscribed;
pub use gossipsub_unsubscribed::gossipsub_unsubscribed;
pub use handshake_inbound_failure::handshake_inbound_failure;
pub use handshake_message::handshake_message_event;
pub use handshake_outbound_failure::handshake_outbound_failure;
pub use keyshare_message::keyshare_message;
use libp2p::{gossipsub, kad, mdns, request_response, swarm::NetworkBehaviour};
pub use mdns_discovered::mdns_discovered;
pub use mdns_expired::mdns_expired;
pub use scheduler_message::scheduler_message;

use crate::libp2p_beemesh::request_response_codec::SchedulerCodec;
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
    pub keyshare_rr: request_response::Behaviour<ApplyCodec>,
    pub scheduler_rr: request_response::Behaviour<SchedulerCodec>,
    pub kademlia: kad::Behaviour<kad::store::MemoryStore>,
}

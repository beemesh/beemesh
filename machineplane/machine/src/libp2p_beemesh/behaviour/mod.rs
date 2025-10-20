pub mod apply_inbound_failure;
pub mod apply_outbound_failure;
pub mod delete_message;
pub mod failure_handlers;
pub mod gossipsub_message;
pub mod gossipsub_subscribed;
pub mod gossipsub_unsubscribed;
pub mod handshake_inbound_failure;
pub mod handshake_message;
pub mod handshake_outbound_failure;


pub mod scheduler_message;

pub use apply_inbound_failure::apply_inbound_failure;
pub use apply_outbound_failure::apply_outbound_failure;
pub use delete_message::{delete_message, delete_outbound_failure, delete_inbound_failure};
pub use gossipsub_message::gossipsub_message;
pub use gossipsub_subscribed::gossipsub_subscribed;
pub use gossipsub_unsubscribed::gossipsub_unsubscribed;
pub use handshake_inbound_failure::handshake_inbound_failure;
pub use handshake_message::handshake_message_event;
pub use handshake_outbound_failure::handshake_outbound_failure;
use libp2p::{gossipsub, kad, request_response, swarm::NetworkBehaviour};


pub use scheduler_message::scheduler_message;

use crate::libp2p_beemesh::request_response_codec::SchedulerCodec;
use crate::libp2p_beemesh::{ApplyCodec, HandshakeCodec, DeleteCodec};

// Add DHT event handlers
pub mod kademlia_event;
pub use kademlia_event::kademlia_event;

#[derive(NetworkBehaviour)]
pub struct MyBehaviour {
    pub gossipsub: gossipsub::Behaviour,
    pub apply_rr: request_response::Behaviour<ApplyCodec>,
    pub handshake_rr: request_response::Behaviour<HandshakeCodec>,
    pub scheduler_rr: request_response::Behaviour<SchedulerCodec>,
    pub delete_rr: request_response::Behaviour<DeleteCodec>,
    pub manifest_fetch_rr: request_response::Behaviour<ApplyCodec>,
    pub kademlia: kad::Behaviour<kad::store::MemoryStore>,
}

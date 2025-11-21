pub mod delete_message;
pub mod failure_handlers;
pub mod gossipsub_message;
pub mod gossipsub_subscribed;
pub mod gossipsub_unsubscribed;
pub mod handshake_message;

pub use delete_message::{delete_inbound_failure, delete_message, delete_outbound_failure};
pub use failure_handlers::{
    handle_apply_inbound_failure as apply_inbound_failure,
    handle_apply_outbound_failure as apply_outbound_failure,
    handle_handshake_inbound_failure as handshake_inbound_failure,
    handle_handshake_outbound_failure as handshake_outbound_failure,
};
pub use gossipsub_message::gossipsub_message;
pub use gossipsub_subscribed::gossipsub_subscribed;
pub use gossipsub_unsubscribed::gossipsub_unsubscribed;
pub use handshake_message::handshake_message_event;
use libp2p::{autonat, identify, relay};
use libp2p::{gossipsub, kad, request_response, swarm::NetworkBehaviour};

use crate::network::{ApplyCodec, DeleteCodec, HandshakeCodec};

// Add DHT event handlers
pub mod kademlia_event;
pub use kademlia_event::kademlia_event;

#[derive(NetworkBehaviour)]
pub struct MyBehaviour {
    pub gossipsub: gossipsub::Behaviour,
    pub apply_rr: request_response::Behaviour<ApplyCodec>,
    pub handshake_rr: request_response::Behaviour<HandshakeCodec>,
    pub delete_rr: request_response::Behaviour<DeleteCodec>,
    pub manifest_fetch_rr: request_response::Behaviour<ApplyCodec>,
    pub kademlia: kad::Behaviour<kad::store::MemoryStore>,
    pub relay: relay::Behaviour,
    pub autonat: autonat::Behaviour,
    pub identify: identify::Behaviour,
}

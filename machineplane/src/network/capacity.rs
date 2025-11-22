use anyhow::{Result, anyhow};
use libp2p::gossipsub;

use crate::network::reply::{self, CapacityReply, CapacityReplyParams};

/// Build a capacity reply, apply optional adjustments, and emit standard KEM warnings.
pub fn compose_capacity_reply<'a, F>(
    warn_context: &str,
    request_id: &'a str,
    responder_peer: &'a str,
    mut adjust: F,
) -> CapacityReply
where
    F: FnMut(&mut CapacityReplyParams<'a>),
{
    let reply = reply::build_capacity_reply_with(request_id, responder_peer, |params| {
        adjust(params);
    });
    reply::warn_missing_kem(warn_context, responder_peer, reply.kem_pub_b64.as_deref());
    reply
}

/// Sign and publish a capacity reply over GossipSub.
pub fn publish_gossipsub_capacity_reply(
    behaviour: &mut gossipsub::Behaviour,
    topic: &gossipsub::TopicHash,
    reply: &CapacityReply,
) -> Result<()> {
    behaviour
        .publish(topic.clone(), reply.payload.clone())
        .map(|_| ())
        .map_err(|err| anyhow!("failed to publish capacity reply: {:?}", err))
}

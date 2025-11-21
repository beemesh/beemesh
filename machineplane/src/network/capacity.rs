use anyhow::{Context, Result, anyhow};
use libp2p::{gossipsub, request_response};

use crate::network::reply::{self, CapacityReply, CapacityReplyParams};
use crate::network::request_response_codec::SchedulerCodec;

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

/// Build a baseline capacity reply without any field overrides.
pub fn compose_default_capacity_reply(
    warn_context: &str,
    request_id: &str,
    responder_peer: &str,
) -> CapacityReply {
    compose_capacity_reply(warn_context, request_id, responder_peer, |_| {})
}

/// Sign and publish a capacity reply over GossipSub.
pub fn publish_gossipsub_capacity_reply(
    behaviour: &mut gossipsub::Behaviour,
    topic: &gossipsub::TopicHash,
    reply: &CapacityReply,
) -> Result<()> {
    let signed_bytes = crate::network::utils::sign_payload_default(
        &reply.payload,
        "capacity_reply",
        Some("capreply"),
    )
    .context("signing capacity reply")?;

    behaviour
        .publish(topic.clone(), signed_bytes)
        .map(|_| ())
        .map_err(|err| anyhow!("failed to publish capacity reply: {:?}", err))
}

/// Send a capacity reply through the scheduler request-response channel.
pub fn send_scheduler_capacity_reply(
    behaviour: &mut request_response::Behaviour<SchedulerCodec>,
    channel: request_response::ResponseChannel<Vec<u8>>,
    reply: CapacityReply,
) -> Result<()> {
    behaviour
        .send_response(channel, reply.payload)
        .map_err(|err| anyhow!("failed to send capacity reply: {:?}", err))
}

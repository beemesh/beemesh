use libp2p::gossipsub;

pub fn gossipsub_message(
    peer_id: libp2p::PeerId,
    message: gossipsub::Message,
    topic: gossipsub::TopicHash,
    swarm: &mut libp2p::Swarm<super::MyBehaviour>,
    pending_queries: &mut std::collections::HashMap<String, Vec<tokio::sync::mpsc::UnboundedSender<String>>>,
) {
    println!("received message");
    // First try CapacityRequest
    if let Ok(cap_req) = protocol::machine::root_as_capacity_request(&message.data) {
        let orig_request_id = cap_req.request_id().unwrap_or("").to_string();
        println!("libp2p: received capreq id={} from peer={} payload_bytes={}", orig_request_id, peer_id, message.data.len());
        // Build a capacity reply and publish it (include request_id inside the reply)
        let mut fbb = flatbuffers::FlatBufferBuilder::new();
        let req_id_off = fbb.create_string(&orig_request_id);
        let node_id_off = fbb.create_string(&peer_id.to_string());
        let region_off = fbb.create_string("local");
        // capabilities vector
        let caps_vec = {
            let mut tmp: Vec<flatbuffers::WIPOffset<&str>> = Vec::new();
            tmp.push(fbb.create_string("default"));
            fbb.create_vector(&tmp)
        };
        let reply_args = protocol::machine::CapacityReplyArgs {
            request_id: Some(req_id_off),
            ok: true,
            node_id: Some(node_id_off),
            region: Some(region_off),
            capabilities: Some(caps_vec),
            cpu_available_milli: 1000u32,
            memory_available_bytes: 1024u64 * 1024 * 512,
            storage_available_bytes: 1024u64 * 1024 * 1024,
        };
        let reply_off = protocol::machine::CapacityReply::create(&mut fbb, &reply_args);
        protocol::machine::finish_capacity_reply_buffer(&mut fbb, reply_off);
        let finished = fbb.finished_data().to_vec();
        let _ = swarm.behaviour_mut().gossipsub.publish(topic.clone(), finished.as_slice());
        println!("libp2p: published capreply for id={} ({} bytes)", orig_request_id, finished.len());
        return;
    }

    // Then try CapacityReply
    if let Ok(cap_reply) = protocol::machine::root_as_capacity_reply(&message.data) {
        let request_part = cap_reply.request_id().unwrap_or("").to_string();
        println!("libp2p: received capreply for id={} from peer={}", request_part, peer_id);
        if let Some(senders) = pending_queries.get_mut(&request_part) {
            for tx in senders.iter() {
                let _ = tx.send(peer_id.to_string());
            }
        }
        return;
    }

    println!("Received non-savvy message ({} bytes) from peer {} — ignoring", message.data.len(), peer_id);
}

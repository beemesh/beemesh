//! Scheduler module for Decentralized Bidding
//!
//! This module implements the "Pull" model where nodes listen for Tasks,
//! evaluate their fit, submit Bids, and if they win, acquire a Lease.

use crate::messages::constants::{
    DEFAULT_SELECTION_WINDOW_MS, SCHEDULER_EVENTS, SCHEDULER_PROPOSALS, SCHEDULER_TENDERS,
};
use crate::messages::machine;
use crate::messages::types::{Bid, LeaseHint, SchedulerEvent, Task};
use crate::network::behaviour::MyBehaviour;
use libp2p::{Swarm, gossipsub};
use log::{debug, error, info, warn};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio::time::sleep;

/// Scheduler manages the bidding lifecycle
pub struct Scheduler {
    /// Local node ID (PeerId string)
    local_node_id: String,
    /// Active bids we are tracking: TaskID -> BidContext
    active_bids: Arc<Mutex<HashMap<String, BidContext>>>,
    /// Channel to send messages back to the network loop
    outbound_tx: mpsc::UnboundedSender<(String, Vec<u8>)>,
}

struct BidContext {
    task_id: String,
    my_bid_score: f64,
    highest_bid_score: f64,
    highest_bidder_id: String,
    selection_window_end: u64,
}

impl Scheduler {
    pub fn new(
        local_node_id: String,
        outbound_tx: mpsc::UnboundedSender<(String, Vec<u8>)>,
    ) -> Self {
        Self {
            local_node_id,
            active_bids: Arc::new(Mutex::new(HashMap::new())),
            outbound_tx,
        }
    }

    /// Handle incoming Gossipsub message
    pub async fn handle_message(
        &self,
        topic_hash: &gossipsub::TopicHash,
        message: &gossipsub::Message,
    ) {
        let tenders = gossipsub::IdentTopic::new(SCHEDULER_TENDERS).hash();
        let proposals = gossipsub::IdentTopic::new(SCHEDULER_PROPOSALS).hash();
        let events = gossipsub::IdentTopic::new(SCHEDULER_EVENTS).hash();

        if *topic_hash == tenders {
            self.handle_task(message).await;
        } else if *topic_hash == proposals {
            self.handle_bid(message).await;
        } else if *topic_hash == events {
            self.handle_event(message).await;
        }
    }

    /// Process a Task message: Evaluate -> Bid
    async fn handle_task(&self, message: &gossipsub::Message) {
        match machine::root_as_task(&message.data) {
            Ok(task) => {
                let task_id = task.id.clone();
                info!("Received Task: {}", task_id);

                // 1. Evaluate Fit (Capacity Check)
                // TODO: Connect to CapacityVerifier
                let capacity_score = 1.0; // Placeholder: assume perfect fit for now

                // 2. Calculate Bid Score
                // Score = (ResourceFit * 0.4) + (NetworkLocality * 0.3) + (Reputation * 0.3)
                // For now, just use random or static for testing
                let my_score = capacity_score * 0.8 + 0.2; // Simple formula

                // 3. Create BidContext
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64;
                let window_end = now + DEFAULT_SELECTION_WINDOW_MS;

                {
                    let mut bids = self.active_bids.lock().unwrap();
                    bids.insert(
                        task_id.to_string(),
                        BidContext {
                            task_id: task_id.to_string(),
                            my_bid_score: my_score,
                            highest_bid_score: my_score, // Start with our score
                            highest_bidder_id: self.local_node_id.clone(),
                            selection_window_end: window_end,
                        },
                    );
                }

                // 4. Publish Bid
                let bid_bytes = machine::build_bid(
                    &task_id,
                    &self.local_node_id,
                    my_score,
                    capacity_score,
                    0.5, // Network locality placeholder
                    now,
                    &[], // Signature placeholder
                );

                // 4. Publish Bid
                let bid_bytes = machine::build_bid(
                    &task_id,
                    &self.local_node_id,
                    my_score,
                    capacity_score,
                    0.5, // Network locality placeholder
                    now,
                    &[], // Signature placeholder
                );

                if let Err(e) = self
                    .outbound_tx
                    .send((SCHEDULER_PROPOSALS.to_string(), bid_bytes))
                {
                    error!("Failed to queue bid for task {}: {}", task_id, e);
                } else {
                    info!("Queued Bid for task {} with score {:.2}", task_id, my_score);
                }

                // 5. Spawn Selection Window Waiter
                let active_bids = self.active_bids.clone();
                let task_id_clone = task_id.to_string();
                let local_id = self.local_node_id.clone();
                // We need a way to trigger deployment, for now just log
                tokio::spawn(async move {
                    sleep(Duration::from_millis(DEFAULT_SELECTION_WINDOW_MS)).await;

                    let winner = {
                        let mut bids = active_bids.lock().unwrap();
                        if let Some(ctx) = bids.remove(&task_id_clone) {
                            if ctx.highest_bidder_id == local_id {
                                true
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    };

                    if winner {
                        info!("WON BID for task {}! Proceeding to Lease...", task_id_clone);

                        // TODO: Write LeaseHint to MDHT (dht_manager.put_record)
                        // For now, we just log it. Real implementation needs access to dht_manager
                        // or send a command to the main loop to do it.

                        // Publish LeaseHint (simulated via Event for now)
                        // In reality, we write to DHT and then start the workload.

                        // Trigger Deployment (simulated)
                        info!("Deploying workload for task {}", task_id_clone);

                        // TODO: Call run::deploy_task(...)
                    } else {
                        info!("Lost bid for task {}.", task_id_clone);
                    }
                });
            }
            Err(e) => error!("Failed to parse Task message: {}", e),
        }
    }

    /// Process a Bid message: Update highest bid
    async fn handle_bid(&self, message: &gossipsub::Message) {
        match machine::root_as_bid(&message.data) {
            Ok(bid) => {
                let task_id = bid.task_id.clone();
                let bidder_id = bid.node_id.clone();
                let score = bid.score;

                // Ignore our own bids (handled locally)
                if bidder_id == self.local_node_id {
                    return;
                }

                let mut bids = self.active_bids.lock().unwrap();
                if let Some(ctx) = bids.get_mut(&task_id) {
                    if score > ctx.highest_bid_score {
                        info!(
                            "Saw higher bid for task {}: {:.2} from {}",
                            task_id, score, bidder_id
                        );
                        ctx.highest_bid_score = score;
                        ctx.highest_bidder_id = bidder_id.to_string();
                    }
                }
            }
            Err(e) => error!("Failed to parse Bid message: {}", e),
        }
    }

    /// Process Scheduler Events (e.g. Cancelled, Preempted)
    async fn handle_event(&self, message: &gossipsub::Message) {
        match machine::root_as_scheduler_event(&message.data) {
            Ok(event) => {
                let task_id = event.task_id.clone();
                info!(
                    "Received Scheduler Event for task {}: {:?}",
                    task_id, event.event_type
                );
                // Handle cancellation etc.
            }
            Err(e) => error!("Failed to parse SchedulerEvent message: {}", e),
        }
    }
}

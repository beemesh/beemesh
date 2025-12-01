//! # Raft Consensus Module
//!
//! Simplified Raft implementation for leader election in stateful workloads.
//! This is NOT a full Raft log-replication implementation - it handles only
//! leader election which is sufficient for workload orchestration.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::{Duration, Instant};

/// Election timeout range (randomized to prevent split votes)
const ELECTION_TIMEOUT_MIN_MS: u64 = 150;
const ELECTION_TIMEOUT_MAX_MS: u64 = 300;

/// Heartbeat interval for leader
const HEARTBEAT_INTERVAL_MS: u64 = 50;

/// Raft node state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaftState {
    Follower,
    Candidate,
    Leader,
}

/// Role from the perspective of workload management
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[derive(Default)]
pub enum RaftRole {
    Leader,
    Follower,
    /// Not yet part of cluster or election in progress
    #[default]
    Detached,
}


/// Vote request message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoteRequest {
    pub term: u64,
    pub candidate_id: String,
    pub workload_id: String,
}

/// Vote response message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoteResponse {
    pub term: u64,
    pub vote_granted: bool,
    pub voter_id: String,
    pub workload_id: String,
}

/// Heartbeat from leader
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Heartbeat {
    pub term: u64,
    pub leader_id: String,
    pub workload_id: String,
}

/// Leadership update event
#[derive(Debug, Clone)]
pub struct LeadershipUpdate {
    pub workload_id: String,
    pub role: RaftRole,
    pub leader_id: Option<String>,
    pub term: u64,
}

/// Actions that RaftManager requests the caller to perform
#[derive(Debug, Clone)]
pub enum RaftAction {
    /// Send vote request to all peers
    RequestVotes(VoteRequest),
    /// Send heartbeat to all peers
    SendHeartbeat(Heartbeat),
    /// Leadership state changed
    LeadershipChanged(LeadershipUpdate),
}

/// Raft manager for a single workload
pub struct RaftManager {
    /// Our node ID (peer ID)
    node_id: String,
    /// Workload ID this raft manages
    workload_id: String,
    /// Current term
    term: u64,
    /// Current state
    state: RaftState,
    /// Who we voted for in current term
    voted_for: Option<String>,
    /// Votes received (when candidate)
    votes_received: HashSet<String>,
    /// Known peers in this workload's cluster
    peers: HashSet<String>,
    /// Current leader (if known)
    leader_id: Option<String>,
    /// Last heartbeat/activity time
    last_heartbeat: Instant,
    /// Election timeout (randomized)
    election_timeout: Duration,
    /// Last known role (for change detection)
    last_role: RaftRole,
}

impl RaftManager {
    /// Create a new Raft manager for a workload
    pub fn new(node_id: String, workload_id: String) -> Self {
        Self {
            node_id,
            workload_id,
            term: 0,
            state: RaftState::Follower,
            voted_for: None,
            votes_received: HashSet::new(),
            peers: HashSet::new(),
            leader_id: None,
            last_heartbeat: Instant::now(),
            election_timeout: random_election_timeout(),
            last_role: RaftRole::Detached,
        }
    }

    /// Get current role
    pub fn role(&self) -> RaftRole {
        match self.state {
            RaftState::Leader => RaftRole::Leader,
            RaftState::Follower if self.leader_id.is_some() => RaftRole::Follower,
            _ => RaftRole::Detached,
        }
    }

    /// Get current term
    pub fn term(&self) -> u64 {
        self.term
    }

    /// Get current leader ID
    pub fn leader_id(&self) -> Option<&String> {
        self.leader_id.as_ref()
    }

    /// Get our node ID
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// Check if we are the leader
    pub fn is_leader(&self) -> bool {
        self.state == RaftState::Leader
    }

    /// Update known peers
    pub fn update_peers(&mut self, peers: HashSet<String>) {
        self.peers = peers;
        self.peers.remove(&self.node_id); // Don't include ourselves
    }

    /// Bootstrap: if alone, become leader immediately
    pub fn bootstrap_if_needed(&mut self) -> Vec<RaftAction> {
        let mut actions = Vec::new();

        if self.peers.is_empty() && self.state != RaftState::Leader {
            // Single node - become leader immediately
            self.state = RaftState::Leader;
            self.leader_id = Some(self.node_id.clone());
            self.term = 1;

            actions.push(RaftAction::LeadershipChanged(LeadershipUpdate {
                workload_id: self.workload_id.clone(),
                role: RaftRole::Leader,
                leader_id: Some(self.node_id.clone()),
                term: self.term,
            }));

            self.last_role = RaftRole::Leader;
        }

        actions
    }

    /// Periodic tick - check timeouts, send heartbeats
    pub fn tick(&mut self) -> Vec<RaftAction> {
        let mut actions = Vec::new();
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_heartbeat);

        match self.state {
            RaftState::Leader => {
                // Send heartbeat if interval passed
                if elapsed >= Duration::from_millis(HEARTBEAT_INTERVAL_MS) {
                    self.last_heartbeat = now;
                    actions.push(RaftAction::SendHeartbeat(Heartbeat {
                        term: self.term,
                        leader_id: self.node_id.clone(),
                        workload_id: self.workload_id.clone(),
                    }));
                }
            }
            RaftState::Follower | RaftState::Candidate => {
                // Check election timeout
                if elapsed >= self.election_timeout {
                    self.start_election(&mut actions);
                }
            }
        }

        // Check for role changes
        let current_role = self.role();
        if current_role != self.last_role {
            actions.push(RaftAction::LeadershipChanged(LeadershipUpdate {
                workload_id: self.workload_id.clone(),
                role: current_role,
                leader_id: self.leader_id.clone(),
                term: self.term,
            }));
            self.last_role = current_role;
        }

        actions
    }

    /// Start an election
    fn start_election(&mut self, actions: &mut Vec<RaftAction>) {
        self.term += 1;
        self.state = RaftState::Candidate;
        self.voted_for = Some(self.node_id.clone());
        self.votes_received.clear();
        self.votes_received.insert(self.node_id.clone()); // Vote for ourselves
        self.last_heartbeat = Instant::now();
        self.election_timeout = random_election_timeout();

        // If no peers, we win immediately
        if self.peers.is_empty() {
            self.become_leader(actions);
            return;
        }

        actions.push(RaftAction::RequestVotes(VoteRequest {
            term: self.term,
            candidate_id: self.node_id.clone(),
            workload_id: self.workload_id.clone(),
        }));
    }

    /// Become leader
    fn become_leader(&mut self, actions: &mut Vec<RaftAction>) {
        self.state = RaftState::Leader;
        self.leader_id = Some(self.node_id.clone());

        actions.push(RaftAction::LeadershipChanged(LeadershipUpdate {
            workload_id: self.workload_id.clone(),
            role: RaftRole::Leader,
            leader_id: Some(self.node_id.clone()),
            term: self.term,
        }));

        // Send immediate heartbeat
        actions.push(RaftAction::SendHeartbeat(Heartbeat {
            term: self.term,
            leader_id: self.node_id.clone(),
            workload_id: self.workload_id.clone(),
        }));

        self.last_role = RaftRole::Leader;
    }

    /// Handle incoming vote request
    pub fn handle_vote_request(&mut self, request: VoteRequest) -> VoteResponse {
        // If request term is greater, update our term and become follower
        if request.term > self.term {
            self.term = request.term;
            self.state = RaftState::Follower;
            self.voted_for = None;
            self.leader_id = None;
        }

        let vote_granted = if request.term < self.term {
            // Reject votes from old terms
            false
        } else if self.voted_for.is_none() || self.voted_for.as_ref() == Some(&request.candidate_id)
        {
            // Grant vote if we haven't voted or already voted for this candidate
            self.voted_for = Some(request.candidate_id.clone());
            self.last_heartbeat = Instant::now(); // Reset timeout
            true
        } else {
            false
        };

        VoteResponse {
            term: self.term,
            vote_granted,
            voter_id: self.node_id.clone(),
            workload_id: self.workload_id.clone(),
        }
    }

    /// Handle incoming vote response
    pub fn handle_vote_response(&mut self, response: VoteResponse) -> Vec<RaftAction> {
        let mut actions = Vec::new();

        // Ignore if we're not a candidate or term changed
        if self.state != RaftState::Candidate || response.term != self.term {
            return actions;
        }

        if response.vote_granted {
            self.votes_received.insert(response.voter_id);

            // Check if we have majority
            let total_nodes = self.peers.len() + 1; // peers + ourselves
            let majority = (total_nodes / 2) + 1;

            if self.votes_received.len() >= majority {
                self.become_leader(&mut actions);
            }
        } else if response.term > self.term {
            // Someone has higher term, step down
            self.term = response.term;
            self.state = RaftState::Follower;
            self.voted_for = None;
            self.leader_id = None;
        }

        actions
    }

    /// Handle incoming heartbeat from leader
    pub fn handle_heartbeat(&mut self, heartbeat: Heartbeat) -> Vec<RaftAction> {
        let mut actions = Vec::new();

        if heartbeat.term >= self.term {
            // Valid heartbeat from current or newer leader
            self.term = heartbeat.term;
            self.state = RaftState::Follower;
            self.leader_id = Some(heartbeat.leader_id.clone());
            self.last_heartbeat = Instant::now();
            self.voted_for = None; // Clear vote for next election

            // Check for role change
            let current_role = self.role();
            if current_role != self.last_role {
                actions.push(RaftAction::LeadershipChanged(LeadershipUpdate {
                    workload_id: self.workload_id.clone(),
                    role: current_role,
                    leader_id: self.leader_id.clone(),
                    term: self.term,
                }));
                self.last_role = current_role;
            }
        }

        actions
    }
}

/// Generate random election timeout
fn random_election_timeout() -> Duration {
    use rand::Rng;
    let ms = rand::thread_rng().gen_range(ELECTION_TIMEOUT_MIN_MS..=ELECTION_TIMEOUT_MAX_MS);
    Duration::from_millis(ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_node_bootstrap() {
        let mut raft = RaftManager::new("node1".to_string(), "workload1".to_string());

        let actions = raft.bootstrap_if_needed();

        assert!(raft.is_leader());
        assert_eq!(raft.role(), RaftRole::Leader);
        assert_eq!(raft.leader_id(), Some(&"node1".to_string()));
        assert_eq!(actions.len(), 1);
    }

    #[test]
    fn test_vote_request_handling() {
        let mut raft = RaftManager::new("node1".to_string(), "workload1".to_string());

        let request = VoteRequest {
            term: 1,
            candidate_id: "node2".to_string(),
            workload_id: "workload1".to_string(),
        };

        let response = raft.handle_vote_request(request);

        assert!(response.vote_granted);
        assert_eq!(response.term, 1);
        assert_eq!(response.voter_id, "node1");
    }

    #[test]
    fn test_vote_request_rejected_for_old_term() {
        let mut raft = RaftManager::new("node1".to_string(), "workload1".to_string());
        raft.term = 5;

        let request = VoteRequest {
            term: 3, // Old term
            candidate_id: "node2".to_string(),
            workload_id: "workload1".to_string(),
        };

        let response = raft.handle_vote_request(request);

        assert!(!response.vote_granted);
        assert_eq!(response.term, 5);
    }

    #[test]
    fn test_heartbeat_handling() {
        let mut raft = RaftManager::new("node1".to_string(), "workload1".to_string());

        let heartbeat = Heartbeat {
            term: 1,
            leader_id: "node2".to_string(),
            workload_id: "workload1".to_string(),
        };

        let actions = raft.handle_heartbeat(heartbeat);

        assert_eq!(raft.state, RaftState::Follower);
        assert_eq!(raft.leader_id(), Some(&"node2".to_string()));
        assert_eq!(raft.term(), 1);
        // Should have leadership changed action
        assert!(!actions.is_empty());
    }

    #[test]
    fn test_leader_sends_heartbeat() {
        let mut raft = RaftManager::new("node1".to_string(), "workload1".to_string());
        raft.bootstrap_if_needed();

        // Simulate time passing
        std::thread::sleep(Duration::from_millis(60));

        let actions = raft.tick();

        // Should have heartbeat action
        let has_heartbeat = actions
            .iter()
            .any(|a| matches!(a, RaftAction::SendHeartbeat(_)));
        assert!(has_heartbeat);
    }

    #[test]
    fn test_election_with_majority() {
        let mut raft = RaftManager::new("node1".to_string(), "workload1".to_string());
        let mut peers = HashSet::new();
        peers.insert("node2".to_string());
        peers.insert("node3".to_string());
        raft.update_peers(peers);

        // Start election manually
        raft.term = 1;
        raft.state = RaftState::Candidate;
        raft.voted_for = Some("node1".to_string());
        raft.votes_received.insert("node1".to_string());

        // Receive vote from node2
        let response = VoteResponse {
            term: 1,
            vote_granted: true,
            voter_id: "node2".to_string(),
            workload_id: "workload1".to_string(),
        };

        let actions = raft.handle_vote_response(response);

        // Should become leader (2 out of 3 votes = majority)
        assert!(raft.is_leader());
        assert!(!actions.is_empty());
    }

    #[test]
    fn test_step_down_on_higher_term() {
        let mut raft = RaftManager::new("node1".to_string(), "workload1".to_string());
        raft.bootstrap_if_needed();
        assert!(raft.is_leader());

        // Receive heartbeat with higher term
        let heartbeat = Heartbeat {
            term: 5,
            leader_id: "node2".to_string(),
            workload_id: "workload1".to_string(),
        };

        raft.handle_heartbeat(heartbeat);

        assert!(!raft.is_leader());
        assert_eq!(raft.state, RaftState::Follower);
        assert_eq!(raft.term(), 5);
        assert_eq!(raft.leader_id(), Some(&"node2".to_string()));
    }
}

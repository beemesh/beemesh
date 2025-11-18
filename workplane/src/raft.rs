use tracing::{info, warn};

use crate::discovery::ServiceRecord;

#[derive(Clone, Debug, Default)]
pub struct RaftManager {
    leader_id: Option<String>,
    members: Vec<String>,
    read_only: bool,
    self_id: Option<String>,
    epoch: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RaftRole {
    Leader,
    Follower,
    Detached,
}

#[derive(Clone, Debug, Default)]
pub struct LeadershipUpdate {
    pub leader_id: Option<String>,
    pub epoch: u64,
    pub changed: bool,
    pub role_changed: bool,
    pub local_role: RaftRole,
}

impl RaftManager {
    pub fn new() -> Self {
        Self {
            leader_id: None,
            members: Vec::new(),
            read_only: false,
            self_id: None,
            epoch: 0,
        }
    }

    pub fn bootstrap_if_needed(&mut self, self_id: &str, peer_ids: &[String]) -> LeadershipUpdate {
        self.self_id = Some(self_id.to_string());
        let mut members = peer_ids.to_vec();
        if !members.contains(&self_id.to_string()) {
            members.push(self_id.to_string());
        }
        self.members = members;

        if self.members.len() <= 1 {
            self.leader_id = Some(self_id.to_string());
            self.read_only = false;
            self.epoch = self.epoch.saturating_add(1);
            info!(leader = %self_id, epoch = self.epoch, "initializing single-node raft");
            metrics::increment_counter!("workplane.raft.leader_elections");
            return LeadershipUpdate {
                leader_id: self.leader_id.clone(),
                epoch: self.epoch,
                changed: true,
                role_changed: true,
                local_role: RaftRole::Leader,
            };
        }

        if let Some(first) = peer_ids.iter().find(|id| *id != &self_id) {
            self.leader_id = Some((*first).clone());
        } else {
            self.leader_id = Some(self_id.to_string());
        }
        self.read_only = self.leader_id.as_deref() != Some(self_id);
        self.epoch = self.epoch.saturating_add(1);
        info!(
            leader = %self.leader_id.as_deref().unwrap_or("unknown"),
            epoch = self.epoch,
            "bootstrapped raft cluster"
        );
        metrics::increment_counter!("workplane.raft.leader_elections");
        LeadershipUpdate {
            leader_id: self.leader_id.clone(),
            epoch: self.epoch,
            changed: true,
            role_changed: true,
            local_role: self.local_role(),
        }
    }

    pub fn update_from_records(&mut self, records: &[ServiceRecord]) -> LeadershipUpdate {
        let mut ranked = rank_records(records.to_vec());
        let new_members: Vec<String> = ranked.iter().map(|r| r.peer_id.clone()).collect();
        self.members = new_members;
        let previous_leader = self.leader_id.clone();
        let previous_role = self.local_role();

        self.leader_id = ranked.first().map(|record| record.peer_id.clone());
        if ranked.is_empty() {
            self.read_only = true;
            warn!("raft cluster has no visible members; entering read-only mode");
        } else {
            self.read_only = self.leader_id.as_deref() != self.self_id.as_deref();
        }

        let changed = self.leader_id != previous_leader;
        if changed {
            self.epoch = self.epoch.saturating_add(1);
            if let Some(ref new_leader) = self.leader_id {
                info!(leader = %new_leader, epoch = self.epoch, "leader changed");
                metrics::increment_counter!("workplane.raft.leader_changes");
                if self.self_id.as_deref() == Some(new_leader.as_str()) {
                    metrics::increment_counter!("workplane.raft.leader_elections");
                }
            } else {
                warn!(epoch = self.epoch, "leader became None");
            }
        }

        let role_changed = previous_role != self.local_role();
        if role_changed {
            info!(
                peer = %self.self_id.clone().unwrap_or_default(),
                role = ?self.local_role(),
                epoch = self.epoch,
                "local raft role changed"
            );
        }

        LeadershipUpdate {
            leader_id: self.leader_id.clone(),
            epoch: self.epoch,
            changed,
            role_changed,
            local_role: self.local_role(),
        }
    }

    pub fn update_members(&mut self, members: Vec<String>) {
        self.members = members;
        if let Some(leader) = self.leader_id.clone() {
            if !self.members.contains(&leader) {
                self.leader_id = self.members.first().cloned();
            }
        }
    }

    pub fn set_partition_state(&mut self, majority_available: bool) {
        self.read_only = !majority_available;
    }

    pub fn is_writable(&self) -> bool {
        !self.read_only
    }

    pub fn leader_id(&self) -> Option<&str> {
        self.leader_id.as_deref()
    }

    pub fn members(&self) -> &[String] {
        &self.members
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn local_role(&self) -> RaftRole {
        match (self.self_id.as_deref(), self.leader_id.as_deref()) {
            (Some(self_id), Some(leader)) if self_id == leader => RaftRole::Leader,
            (Some(_), Some(_)) => RaftRole::Follower,
            _ => RaftRole::Detached,
        }
    }

    pub fn self_id(&self) -> Option<&str> {
        self.self_id.as_deref()
    }

    pub fn leader_record(&self, records: &[ServiceRecord]) -> Option<ServiceRecord> {
        let leader = self.leader_id.as_deref()?;
        records.iter().find(|r| r.peer_id == leader).cloned()
    }
}

fn rank_records(mut records: Vec<ServiceRecord>) -> Vec<ServiceRecord> {
    records.sort_by(|a, b| rank(a, b));
    records
}

fn rank(a: &ServiceRecord, b: &ServiceRecord) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let key = |record: &ServiceRecord| {
        (
            record.ready && record.healthy,
            record.ready,
            record.healthy,
            record.ordinal.unwrap_or(u32::MAX),
            record.ts,
            record.version,
            record.peer_id.clone(),
        )
    };
    match key(b).cmp(&key(a)) {
        Ordering::Equal => b.peer_id.cmp(&a.peer_id),
        other => other,
    }
}

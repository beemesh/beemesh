#[derive(Clone, Debug, Default)]
pub struct RaftManager {
    leader_id: Option<String>,
    members: Vec<String>,
    read_only: bool,
}

impl RaftManager {
    pub fn new() -> Self {
        Self {
            leader_id: None,
            members: Vec::new(),
            read_only: false,
        }
    }

    pub fn bootstrap_if_needed(&mut self, self_id: &str, peer_ids: &[String]) {
        let self_id_owned = self_id.to_string();
        let mut members = peer_ids.to_vec();
        if !members.contains(&self_id_owned) {
            members.push(self_id_owned.clone());
        }
        self.members = members;

        if self.members.len() <= 1 {
            self.leader_id = Some(self_id_owned);
            self.read_only = false;
            return;
        }

        if let Some(first) = peer_ids.iter().find(|id| *id != &self_id_owned) {
            self.leader_id = Some((*first).clone());
        } else {
            self.leader_id = Some(self_id_owned);
        }
        self.read_only = false;
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
}

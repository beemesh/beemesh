use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Config {
    pub peer_id_str: Option<String>,
    pub private_key: Vec<u8>,
    pub namespace: String,
    pub workload_name: String,
    pub pod_name: String,
    pub replicas: usize,
    pub bootstrap_peer_strings: Vec<String>,
    pub beemesh_api: String,
    pub replica_check_interval: Duration,
    pub dht_ttl: Duration,
    pub heartbeat_interval: Duration,
    pub health_probe_interval: Duration,
    pub health_probe_timeout: Duration,
    pub allow_cross_namespace: bool,
    pub allowed_workloads: Vec<String>,
    pub denied_workloads: Vec<String>,
    pub listen_addrs: Vec<String>,
}

impl Config {
    pub fn apply_defaults(&mut self) {
        if self.namespace.is_empty() {
            self.namespace = "default".to_string();
        }
        if self.replicas == 0 {
            self.replicas = 1;
        }
        if self.beemesh_api.is_empty() {
            self.beemesh_api = "http://localhost:8080".to_string();
        }
        if self.replica_check_interval == Duration::from_secs(0) {
            self.replica_check_interval = Duration::from_secs(30);
        }
        if self.dht_ttl == Duration::from_secs(0) {
            self.dht_ttl = Duration::from_secs(15);
        }
        if self.heartbeat_interval == Duration::from_secs(0) {
            self.heartbeat_interval = Duration::from_secs(5);
        }
        if self.health_probe_interval == Duration::from_secs(0) {
            self.health_probe_interval = Duration::from_secs(10);
        }
        if self.health_probe_timeout == Duration::from_secs(0) {
            self.health_probe_timeout = Duration::from_secs(5);
        }
        if self.listen_addrs.is_empty() {
            self.listen_addrs = Vec::new();
        }
    }

    pub fn workload_id(&self) -> String {
        format!("{}/{}", self.namespace, self.workload_name)
    }

    pub fn ordinal(&self) -> Option<u32> {
        let suffix = self.pod_name.split('-').last()?;
        suffix.parse().ok()
    }
}

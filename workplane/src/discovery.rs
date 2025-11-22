use std::collections::{HashMap, hash_map::Entry};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;

const MAX_CLOCK_SKEW_MS: i64 = 30_000; // ±30s per spec

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct ServiceRecord {
    pub workload_id: String,
    pub namespace: String,
    pub workload_name: String,
    pub peer_id: String,
    #[serde(default)]
    pub pod_name: Option<String>,
    #[serde(default)]
    pub ordinal: Option<u32>,
    #[serde(default)]
    pub addrs: Vec<String>,
    #[serde(default)]
    pub caps: serde_json::Map<String, serde_json::Value>,
    pub version: u64,
    pub ts: i64,
    pub nonce: String,
    pub ready: bool,
    pub healthy: bool,
}

struct RecordEntry {
    record: ServiceRecord,
    expires_at: Instant,
}

static WDHT: Lazy<Mutex<HashMap<String, HashMap<String, RecordEntry>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

pub fn put(record: ServiceRecord, ttl: Duration) -> bool {
    if !is_fresh(&record) {
        return false;
    }

    let mut dht = WDHT.lock().expect("wdht lock");
    let namespace_map = dht
        .entry(record.workload_id.clone())
        .or_insert_with(HashMap::new);

    purge_expired(namespace_map);

    match namespace_map.entry(record.peer_id.clone()) {
        Entry::Occupied(mut occupied) => {
            if should_replace(&occupied.get().record, &record) {
                occupied.insert(RecordEntry {
                    record,
                    expires_at: Instant::now() + ttl,
                });
                true
            } else {
                false
            }
        }
        Entry::Vacant(vacant) => {
            vacant.insert(RecordEntry {
                record,
                expires_at: Instant::now() + ttl,
            });
            true
        }
    }
}

pub fn remove(workload_id: &str, peer_id: &str) {
    let mut dht = WDHT.lock().expect("wdht lock");
    if let Some(map) = dht.get_mut(workload_id) {
        map.remove(peer_id);
        if map.is_empty() {
            dht.remove(workload_id);
        }
    }
}

pub fn list(namespace: &str, workload_name: &str) -> Vec<ServiceRecord> {
    let workload_id = format!("{namespace}/{workload_name}");
    let mut dht = WDHT.lock().expect("wdht lock");
    let Some(map) = dht.get_mut(&workload_id) else {
        return Vec::new();
    };

    purge_expired(map);

    let mut records: Vec<ServiceRecord> = map.values().map(|entry| entry.record.clone()).collect();
    records.sort_by(|a, b| b.ts.cmp(&a.ts));
    records
}

fn purge_expired(map: &mut HashMap<String, RecordEntry>) {
    let now = Instant::now();
    map.retain(|_, entry| entry.expires_at > now);
}

fn should_replace(existing: &ServiceRecord, incoming: &ServiceRecord) -> bool {
    if incoming.version > existing.version {
        return true;
    }
    if incoming.version < existing.version {
        return false;
    }
    if incoming.ts > existing.ts {
        return true;
    }
    if incoming.ts < existing.ts {
        return false;
    }
    incoming.peer_id > existing.peer_id
}

fn is_fresh(record: &ServiceRecord) -> bool {
    let now = current_millis();
    (record.ts - now).abs() <= MAX_CLOCK_SKEW_MS
}

fn current_millis() -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    now.as_millis() as i64
}

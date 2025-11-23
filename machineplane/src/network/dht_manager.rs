#![allow(dead_code)]
use crate::messages::machine::decode_node_identity_record;
use crate::messages::types::NodeIdentityRecord;
use crate::network::utils::peer_id_to_public_key;
use libp2p::{PeerId, Swarm, kad};
use log::{info, warn};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use crate::network::behaviour::MyBehaviour;

/// DHT operations for managing signed node identity records
pub enum DhtOperation {
    /// Store the local node identity in the DHT
    PublishIdentity {
        record: NodeIdentityRecord,
        reply_tx: mpsc::UnboundedSender<Result<(), String>>,
    },
    /// Retrieve a peer's identity record by peer ID
    GetIdentity {
        peer_id: String,
        reply_tx: mpsc::UnboundedSender<Result<Option<NodeIdentityRecord>, String>>,
    },
}

pub struct DhtManager {
    /// Pending DHT queries awaiting responses
    pending_queries: HashMap<kad::QueryId, DhtQueryContext>,
}

enum DhtQueryContext {
    PublishIdentity {
        reply_tx: mpsc::UnboundedSender<Result<(), String>>,
    },
    GetIdentity {
        reply_tx: mpsc::UnboundedSender<Result<Option<NodeIdentityRecord>, String>>,
    },
}

impl DhtManager {
    pub fn new() -> Self {
        Self {
            pending_queries: HashMap::new(),
        }
    }

    /// Generate a DHT key for storing/retrieving signed identity records
    pub fn identity_key(peer_id: &str) -> kad::RecordKey {
        kad::RecordKey::new(&format!("node:{}", peer_id))
    }

    /// Handle a DHT operation request
    pub fn handle_operation(&mut self, operation: DhtOperation, swarm: &mut Swarm<MyBehaviour>) {
        match operation {
            DhtOperation::PublishIdentity { record, reply_tx } => {
                self.publish_identity(record, reply_tx, swarm);
            }
            DhtOperation::GetIdentity { peer_id, reply_tx } => {
                self.get_identity(peer_id, reply_tx, swarm);
            }
        }
    }

    fn publish_identity(
        &mut self,
        record: NodeIdentityRecord,
        reply_tx: mpsc::UnboundedSender<Result<(), String>>,
        swarm: &mut Swarm<MyBehaviour>,
    ) {
        if !self.verify_identity_record(&record) {
            let _ = reply_tx.send(Err("Refusing to publish unsigned identity".to_string()));
            return;
        }

        let record_bytes = match bincode::serialize(&record) {
            Ok(bytes) => bytes,
            Err(err) => {
                let _ = reply_tx.send(Err(format!("Failed to serialize identity: {:?}", err)));
                return;
            }
        };

        let record_key = Self::identity_key(&record.peer_id);
        let dht_record = kad::Record {
            key: record_key,
            value: record_bytes,
            publisher: None,
            expires: Some(Instant::now() + Duration::from_secs(600)),
        };

        match swarm
            .behaviour_mut()
            .kademlia
            .put_record(dht_record, kad::Quorum::One)
        {
            Ok(query_id) => {
                info!(
                    "DHT: Initiated identity publish for peer {} (query_id: {:?})",
                    record.peer_id, query_id
                );
                self.pending_queries
                    .insert(query_id, DhtQueryContext::PublishIdentity { reply_tx });
            }
            Err(e) => {
                let _ = reply_tx.send(Err(format!(
                    "Failed to initiate DHT identity publish: {:?}",
                    e
                )));
            }
        }
    }

    fn get_identity(
        &mut self,
        peer_id: String,
        reply_tx: mpsc::UnboundedSender<Result<Option<NodeIdentityRecord>, String>>,
        swarm: &mut Swarm<MyBehaviour>,
    ) {
        let record_key = Self::identity_key(&peer_id);
        let query_id = swarm.behaviour_mut().kademlia.get_record(record_key);

        info!(
            "DHT: Initiated identity lookup for peer {} (query_id: {:?})",
            peer_id, query_id
        );

        self.pending_queries
            .insert(query_id, DhtQueryContext::GetIdentity { reply_tx });
    }

    /// Handle Kademlia query results
    pub fn handle_query_result(&mut self, query_id: kad::QueryId, result: kad::QueryResult) {
        if let Some(context) = self.pending_queries.remove(&query_id) {
            match context {
                DhtQueryContext::PublishIdentity { reply_tx } => match result {
                    kad::QueryResult::PutRecord(Ok(_)) => {
                        let _ = reply_tx.send(Ok(()));
                    }
                    kad::QueryResult::PutRecord(Err(e)) => {
                        let _ = reply_tx.send(Err(format!("Failed to publish identity: {:?}", e)));
                    }
                    _ => {
                        let _ = reply_tx.send(Err("Unexpected query result".to_string()));
                    }
                },
                DhtQueryContext::GetIdentity { reply_tx } => match result {
                    kad::QueryResult::GetRecord(Ok(kad::GetRecordOk::FoundRecord(peer_record))) => {
                        match decode_node_identity_record(&peer_record.record.value) {
                            Ok(record) if self.verify_identity_record(&record) => {
                                let _ = reply_tx.send(Ok(Some(record)));
                            }
                            Ok(record) => {
                                warn!(
                                    "DHT: rejecting unsigned identity record for peer {}",
                                    record.peer_id
                                );
                                let _ =
                                    reply_tx.send(Err("Invalid identity signature".to_string()));
                            }
                            Err(e) => {
                                let _ = reply_tx
                                    .send(Err(format!("Failed to parse identity record: {:?}", e)));
                            }
                        }
                    }
                    kad::QueryResult::GetRecord(Ok(
                        kad::GetRecordOk::FinishedWithNoAdditionalRecord { .. },
                    )) => {
                        let _ = reply_tx.send(Ok(None));
                    }
                    kad::QueryResult::GetRecord(Err(e)) => {
                        let _ = reply_tx.send(Err(format!("Failed to get identity: {:?}", e)));
                    }
                    _ => {
                        let _ = reply_tx.send(Err("Unexpected query result".to_string()));
                    }
                },
            }
        }
    }

    fn verify_identity_record(&self, record: &NodeIdentityRecord) -> bool {
        let Ok(peer_id) = record.peer_id.parse::<PeerId>() else {
            return false;
        };

        let Some(public_key) = peer_id_to_public_key(&peer_id) else {
            return false;
        };

        public_key.verify(record.peer_id.as_bytes(), &record.signature)
    }
}

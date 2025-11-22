#![allow(dead_code)]
use libp2p::{kad, Swarm};
use log::info;
use crate::messages::machine::root_as_applied_manifest;
use crate::messages::types::{AppliedManifest, KeyValue, OperationType, SignatureScheme};
use std::collections::HashMap;
use tokio::sync::mpsc;

use crate::network::behaviour::MyBehaviour;

/// DHT operations for managing AppliedManifest records
pub enum DhtOperation {
    /// Store an AppliedManifest in the DHT
    StoreManifest {
        manifest: AppliedManifest,
        reply_tx: mpsc::UnboundedSender<Result<(), String>>,
    },
    /// Retrieve an AppliedManifest by its ID
    GetManifest {
        id: String,
        reply_tx: mpsc::UnboundedSender<Result<Option<AppliedManifest>, String>>,
    },
    /// Get all manifests by a specific peer
    GetManifestsByPeer {
        peer_id: String,
        reply_tx: mpsc::UnboundedSender<Result<Vec<AppliedManifest<'static>>, String>>,
    },
}

pub struct DhtManager {
    /// Pending DHT queries awaiting responses
    pending_queries: HashMap<kad::QueryId, DhtQueryContext>,
}

enum DhtQueryContext {
    StoreManifest {
        reply_tx: mpsc::UnboundedSender<Result<(), String>>,
    },
    GetManifest {
        reply_tx: mpsc::UnboundedSender<Result<Option<AppliedManifest<'static>>, String>>,
    },
}

impl DhtManager {
    pub fn new() -> Self {
        Self {
            pending_queries: HashMap::new(),
        }
    }

    /// Generate a DHT key for storing/retrieving manifests by ID
    pub fn manifest_key(id: &str) -> kad::RecordKey {
        kad::RecordKey::new(&format!("manifest:{}", id))
    }

    /// Generate a DHT key for a given manifest ID
    /// Generate a DHT key for peer indexing
    pub fn peer_index_key(peer_id: &str) -> kad::RecordKey {
        kad::RecordKey::new(&format!("peer-index:{}", peer_id))
    }

    /// Handle a DHT operation request
    pub fn handle_operation(&mut self, operation: DhtOperation, swarm: &mut Swarm<MyBehaviour>) {
        match operation {
            DhtOperation::StoreManifest { manifest, reply_tx } => {
                self.store_manifest(manifest, reply_tx, swarm);
            }
            DhtOperation::GetManifest { id, reply_tx } => {
                self.get_manifest(id, reply_tx, swarm);
            }
            DhtOperation::GetManifestsByPeer {
                peer_id: _,
                reply_tx,
            } => {
                // For now, send an error as this requires more complex indexing
                let _ = reply_tx.send(Err("Peer queries not implemented yet".to_string()));
            }
        }
    }

    fn store_manifest(
        &mut self,
        manifest: AppliedManifest,
        reply_tx: mpsc::UnboundedSender<Result<(), String>>,
        swarm: &mut Swarm<MyBehaviour>,
    ) {
        // Serialize the manifest to bytes for storage
        let manifest_id = manifest.id.clone();
        let manifest_bytes = bincode::serialize(&manifest)
            .map_err(|e| format!("Failed to serialize manifest: {:?}", e));

        let manifest_bytes = match manifest_bytes {
            Ok(bytes) => bytes,
            Err(err) => {
                let _ = reply_tx.send(Err(err));
                return;
            }
        };

        let record_key = Self::manifest_key(manifest_id);
        let record = kad::Record {
            key: record_key,
            value: manifest_bytes,
            publisher: None,
            expires: None,
        };

        match swarm
            .behaviour_mut()
            .kademlia
            .put_record(record, kad::Quorum::One)
        {
            Ok(query_id) => {
                info!(
                    "DHT: Initiated store operation for manifest {} (query_id: {:?})",
                    manifest_id, query_id
                );
                self.pending_queries
                    .insert(query_id, DhtQueryContext::StoreManifest { reply_tx });
            }
            Err(e) => {
                let _ = reply_tx.send(Err(format!("Failed to initiate DHT store: {:?}", e)));
            }
        }
    }

    fn get_manifest(
        &mut self,
        id: String,
        reply_tx: mpsc::UnboundedSender<Result<Option<AppliedManifest>, String>>,
        swarm: &mut Swarm<MyBehaviour>,
    ) {
        let record_key = Self::manifest_key(&id);
        let query_id = swarm.behaviour_mut().kademlia.get_record(record_key);

        info!(
            "DHT: Initiated get operation for manifest {} (query_id: {:?})",
            id, query_id
        );

        self.pending_queries
            .insert(query_id, DhtQueryContext::GetManifest { reply_tx });
    }

    /// Handle Kademlia query results
    pub fn handle_query_result(&mut self, query_id: kad::QueryId, result: kad::QueryResult) {
        if let Some(context) = self.pending_queries.remove(&query_id) {
            match context {
                DhtQueryContext::StoreManifest { reply_tx } => match result {
                    kad::QueryResult::PutRecord(Ok(_)) => {
                        let _ = reply_tx.send(Ok(()));
                    }
                    kad::QueryResult::PutRecord(Err(e)) => {
                        let _ = reply_tx.send(Err(format!("Failed to store manifest: {:?}", e)));
                    }
                    _ => {
                        let _ = reply_tx.send(Err("Unexpected query result".to_string()));
                    }
                },
                DhtQueryContext::GetManifest { reply_tx } => {
                    match result {
                        kad::QueryResult::GetRecord(Ok(kad::GetRecordOk::FoundRecord(
                            peer_record,
                        ))) => {
                            match root_as_applied_manifest(&peer_record.record.value) {
                                Ok(manifest) => {
                                    let _ = reply_tx.send(Ok(Some(manifest)));
                                }
                                Err(e) => {
                                    let _ = reply_tx
                                        .send(Err(format!("Failed to parse manifest: {:?}", e)));
                                }
                            }
                        }
                        kad::QueryResult::GetRecord(Ok(
                            kad::GetRecordOk::FinishedWithNoAdditionalRecord { .. },
                        )) => {
                            let _ = reply_tx.send(Ok(None));
                        }
                        kad::QueryResult::GetRecord(Err(e)) => {
                            let _ = reply_tx.send(Err(format!("Failed to get manifest: {:?}", e)));
                        }
                        _ => {
                            let _ = reply_tx.send(Err("Unexpected query result".to_string()));
                        }
                    }
                }
            }
        }
    }
}


#![allow(dead_code)]
use libp2p::{kad, Swarm};
use log::info;
use crate::messages::machine::{build_applied_manifest, root_as_applied_manifest, AppliedManifest};
use std::collections::HashMap;
use tokio::sync::mpsc;

use crate::network::behaviour::MyBehaviour;

/// DHT operations for managing AppliedManifest records
pub enum DhtOperation {
    /// Store an AppliedManifest in the DHT
    StoreManifest {
        manifest: AppliedManifest<'static>,
        reply_tx: mpsc::UnboundedSender<Result<(), String>>,
    },
    /// Retrieve an AppliedManifest by its ID
    GetManifest {
        id: String,
        reply_tx: mpsc::UnboundedSender<Result<Option<AppliedManifest<'static>>, String>>,
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
        // Extract the manifest ID
        let manifest_id = match manifest.id() {
            Some(id) => id,
            None => {
                let _ = reply_tx.send(Err("Manifest missing ID".to_string()));
                return;
            }
        };

        // Serialize the manifest to bytes for storage
        let manifest_bytes = {
            // We need to re-serialize the FlatBuffer since we can't clone it directly
            let id = manifest.id().unwrap_or("");
            let operation_id = &manifest.operation_id;
            let origin_peer = manifest.origin_peer().unwrap_or("");
            let owner_pubkey = manifest
                .owner_pubkey()
                .map(|v| v.iter().collect::<Vec<_>>())
                .unwrap_or_default();
            let signature_scheme = manifest.signature_scheme();
            let signature = manifest
                .signature()
                .map(|s| s.iter().collect::<Vec<_>>())
                .unwrap_or_default();
            let manifest_json = &manifest.manifest_json;
            let manifest_kind = manifest.manifest_kind().unwrap_or("");
            let labels = if let Some(labels_vec) = manifest.labels() {
                labels_vec
                    .iter()
                    .filter_map(|kv| {
                        if let (Some(k), Some(v)) = (kv.key(), kv.value()) {
                            Some((k.to_string(), v.to_string()))
                        } else {
                            None
                        }
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let timestamp = manifest.timestamp();
            let operation = manifest.operation();
            let ttl_secs = manifest.ttl_secs();
            let content_hash = manifest.content_hash().unwrap_or("");

            build_applied_manifest(
                &manifest_id,
                &operation_id,
                &origin_peer,
                &owner_pubkey,
                &signature,
                &manifest_json,
                &manifest_kind,
                labels,
                timestamp,
                ttl_secs,
                &content_hash,
            )
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
        reply_tx: mpsc::UnboundedSender<Result<Option<AppliedManifest<'static>>, String>>,
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
                                Ok(_manifest) => {
                                    // Convert to owned data by cloning the bytes and re-parsing
                                    // This is necessary because FlatBuffer references the original buffer
                                    let owned_bytes = peer_record.record.value.to_vec();
                                    match root_as_applied_manifest(&owned_bytes) {
                                        Ok(_owned_manifest) => {
                                            // This is still a problem - we need a different approach
                                            // For now, we'll leak the memory to make it 'static
                                            let leaked_bytes =
                                                Box::leak(owned_bytes.into_boxed_slice());
                                            match root_as_applied_manifest(leaked_bytes) {
                                                Ok(static_manifest) => {
                                                    let _ =
                                                        reply_tx.send(Ok(Some(static_manifest)));
                                                }
                                                Err(e) => {
                                                    let _ = reply_tx.send(Err(format!(
                                                        "Failed to parse manifest: {:?}",
                                                        e
                                                    )));
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            let _ = reply_tx.send(Err(format!(
                                                "Failed to parse manifest: {:?}",
                                                e
                                            )));
                                        }
                                    }
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

/// Helper function to create an AppliedManifest for a deployed workload
pub fn create_applied_manifest_for_deployment(
    id: String,
    operation_id: String,
    origin_peer: String,
    manifest_json: String,
    manifest_kind: String,
) -> Vec<u8> {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    let content_hash = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        manifest_json.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    };

    // For now, use empty signature - in production you'd sign with your private key
    let empty_pubkey = vec![];
    let empty_signature = vec![];
    let labels = vec![
        ("deployed-by".to_string(), "beemesh-node".to_string()),
        ("kind".to_string(), manifest_kind.clone()),
    ];

    build_applied_manifest(
        &id,
        &operation_id,
        &origin_peer,
        &empty_pubkey,
        &empty_signature,
        &manifest_json,
        &manifest_kind,
        labels,
        timestamp,
        3600, // 1 hour TTL
        &content_hash,
    )
}

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub tender_id: String,
    pub kind: String,
    pub name: String,
    pub manifest: Vec<u8>,
    pub destination: String,
    pub clone_request: bool,
    pub replicas: usize,
    pub namespace: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<serde_json::Value>,
}

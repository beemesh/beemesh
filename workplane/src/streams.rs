use std::sync::{Arc, Mutex};

use once_cell::sync::Lazy;

use crate::discovery::ServiceRecord;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct RPCRequest {
    pub method: String,
    #[serde(default)]
    pub body: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub leader_only: bool,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct RPCResponse {
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub body: serde_json::Map<String, serde_json::Value>,
}

use futures::future::BoxFuture;

type Handler = Arc<dyn Fn(&ServiceRecord, RPCRequest) -> BoxFuture<'static, RPCResponse> + Send + Sync>;

static HANDLER: Lazy<Mutex<Option<Handler>>> = Lazy::new(|| Mutex::new(None));

pub fn register_stream_handler(handler: Handler) {
    *HANDLER.lock().expect("handler lock") = Some(handler);
}

pub async fn handle_request(peer_id: &str, request: RPCRequest) -> RPCResponse {
    let handler = HANDLER.lock().expect("handler lock").clone();
    if let Some(h) = handler {
        let record = ServiceRecord {
            peer_id: peer_id.to_string(),
            ..Default::default()
        };
        h(&record, request).await
    } else {
        RPCResponse {
            ok: false,
            error: Some("no handler registered".to_string()),
            body: Default::default(),
        }
    }
}

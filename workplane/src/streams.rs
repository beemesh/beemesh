use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow};
use once_cell::sync::Lazy;

use crate::discovery::ServiceRecord;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct RPCRequest {
    pub method: String,
    #[serde(default)]
    pub body: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct RPCResponse {
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub body: serde_json::Map<String, serde_json::Value>,
}

type Handler = Arc<dyn Fn(&ServiceRecord, RPCRequest) -> RPCResponse + Send + Sync>;

static HANDLER: Lazy<Mutex<Option<Handler>>> = Lazy::new(|| Mutex::new(None));

pub fn register_stream_handler(handler: Handler) {
    *HANDLER.lock().expect("handler lock") = Some(handler);
}

pub fn send_request(target: &ServiceRecord, request: RPCRequest) -> Result<RPCResponse> {
    let handler = HANDLER
        .lock()
        .expect("handler lock")
        .clone()
        .ok_or_else(|| anyhow!("no stream handler registered"))?;
    Ok(handler(target, request))
}

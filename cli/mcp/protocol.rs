use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct JsonRpcRequest {
    pub(crate) jsonrpc: String,
    pub(crate) id: Option<Value>,
    pub(crate) method: String,
    pub(crate) params: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct JsonRpcResponse {
    pub(crate) jsonrpc: String,
    pub(crate) id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct JsonRpcError {
    pub(crate) code: i32,
    pub(crate) message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) data: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct InitializeRequest {
    #[serde(rename = "protocolVersion")]
    pub(crate) protocol_version: String,
    pub(crate) capabilities: Value,
    #[serde(rename = "clientInfo")]
    pub(crate) client_info: Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct CallToolRequest {
    pub(crate) name: String,
    pub(crate) arguments: Option<Value>,
}

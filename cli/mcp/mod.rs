mod protocol;
mod stdio;
mod tools;

use protocol::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};
use serde_json::json;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use turso_core::Connection;

pub struct TursoMcpServer {
    pub(crate) conn: Arc<Mutex<Arc<Connection>>>,
    pub(crate) interrupt_count: Arc<AtomicUsize>,
    pub(crate) current_db_path: Arc<Mutex<Option<String>>>,
}

impl TursoMcpServer {
    pub fn new(conn: Arc<Connection>, interrupt_count: Arc<AtomicUsize>) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
            interrupt_count,
            current_db_path: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) fn handle_request(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        // Check if this is a notification (no id field means it's a notification)
        // Notifications should not receive a response according to JSON-RPC spec
        if request.id.is_none() {
            // For notifications, we return a special response that the caller should ignore
            return JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: None,
                result: None,
                error: None,
            };
        }

        match request.method.as_str() {
            "initialize" => self.handle_initialize(request),
            "tools/list" => self.handle_list_tools(request),
            "tools/call" => self.handle_call_tool(request),
            _ => JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: request.id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32601,
                    message: "Method not found".to_string(),
                    data: None,
                }),
            },
        }
    }

    fn handle_initialize(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: request.id,
            result: Some(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "turso-mcp",
                    "version": "1.0.0"
                }
            })),
            error: None,
        }
    }
}

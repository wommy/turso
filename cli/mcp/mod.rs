mod http;
mod protocol;
mod stdio;
mod tools;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use serde_json::{json, Value};
use turso_core::Connection;

use protocol::{
    server_info, JsonRpcError, JsonRpcRequest, JsonRpcResponse, CACHE_TTL_MS, INVALID_PARAMS,
    LEGACY_DEFAULT, METHOD_NOT_FOUND, PARSE_ERROR, SUPPORTED_VERSIONS,
};
use tools::ToolOutput;

const INSTRUCTIONS: &str = "Query and modify a local Turso/SQLite database. \
Pick a database file with open_database, look around with list_tables and describe_table, \
then run one statement per call with execute_query, insert_data, update_data, delete_data \
or schema_change.";

pub struct TursoMcpServer {
    conn: Arc<Mutex<Arc<Connection>>>,
    interrupt_count: Arc<AtomicUsize>,
    current_db_path: Arc<Mutex<Option<String>>>,
}

impl TursoMcpServer {
    pub fn new(conn: Arc<Connection>, interrupt_count: Arc<AtomicUsize>) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
            interrupt_count,
            current_db_path: Arc::new(Mutex::new(None)),
        }
    }

    pub fn run_stdio(&self) -> Result<()> {
        stdio::run(self)
    }

    pub fn run_http(&self, address: &str) -> Result<()> {
        http::run(self, address)
    }

    /// One JSON-RPC message in, at most one message out. Notifications get no
    /// reply, which is what `None` means here.
    fn handle_message(&self, message: &str) -> Option<String> {
        let request: JsonRpcRequest = match serde_json::from_str(message) {
            Ok(request) => request,
            Err(e) => {
                let error = JsonRpcError::new(
                    PARSE_ERROR,
                    format!("Failed to parse JSON-RPC request: {e}"),
                );
                return Some(encode(&JsonRpcResponse::failure(None, error)));
            }
        };

        let response = self.handle_request(&request)?;
        Some(encode(&response))
    }

    fn handle_request(&self, request: &JsonRpcRequest) -> Option<JsonRpcResponse> {
        // No id means a notification, and a notification is never answered.
        request.id.as_ref()?;
        let id = request.id.clone();

        // `initialize` carries its version in the params, not in `_meta`.
        if request.method != "initialize" {
            if let Err(error) = request.check_protocol_version() {
                return Some(JsonRpcResponse::failure(id, error));
            }
        }

        let response = match request.method.as_str() {
            "server/discover" => JsonRpcResponse::success(id, discover_result()),
            "initialize" => JsonRpcResponse::success(id, initialize_result(request)),
            "tools/list" => JsonRpcResponse::success(id, tools_list_result()),
            "tools/call" => self.call_tool_response(request),
            // Removed in v2, still sent by clients that speak an older revision.
            "ping" => JsonRpcResponse::success(id, json!({})),
            method => JsonRpcResponse::failure(
                id,
                JsonRpcError::new(METHOD_NOT_FOUND, format!("Method not found: {method}")),
            ),
        };
        Some(response)
    }

    fn call_tool_response(&self, request: &JsonRpcRequest) -> JsonRpcResponse {
        let id = request.id.clone();

        let Some(params) = request.params.as_ref() else {
            return JsonRpcResponse::failure(
                id,
                JsonRpcError::new(INVALID_PARAMS, "Missing params"),
            );
        };
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return JsonRpcResponse::failure(
                id,
                JsonRpcError::new(INVALID_PARAMS, "Missing or invalid tool name"),
            );
        };
        let arguments = params.get("arguments").cloned();

        let Some(result) = self.call_tool(name, &arguments) else {
            return JsonRpcResponse::failure(
                id,
                JsonRpcError::new(INVALID_PARAMS, format!("Unknown tool: {name}")),
            );
        };
        JsonRpcResponse::success(id, tool_result(result))
    }

    fn interrupted(&self) -> bool {
        self.interrupt_count.load(Ordering::SeqCst) > 0
    }
}

fn encode(response: &JsonRpcResponse) -> String {
    serde_json::to_string(response).expect("JSON-RPC responses are always serializable")
}

fn discover_result() -> Value {
    json!({
        "resultType": "complete",
        "supportedVersions": SUPPORTED_VERSIONS,
        "capabilities": { "tools": {} },
        "instructions": INSTRUCTIONS,
        "ttlMs": CACHE_TTL_MS,
        "cacheScope": "public",
        "_meta": protocol::result_meta(),
    })
}

fn initialize_result(request: &JsonRpcRequest) -> Value {
    let requested = request
        .params
        .as_ref()
        .and_then(|params| params.get("protocolVersion"))
        .and_then(Value::as_str);
    let negotiated = match requested {
        Some(version) if SUPPORTED_VERSIONS.contains(&version) => version,
        _ => LEGACY_DEFAULT,
    };

    json!({
        "protocolVersion": negotiated,
        "capabilities": { "tools": {} },
        "serverInfo": server_info(),
    })
}

fn tools_list_result() -> Value {
    json!({
        "resultType": "complete",
        "tools": tools::catalog(),
        "ttlMs": CACHE_TTL_MS,
        "cacheScope": "public",
        "_meta": protocol::result_meta(),
    })
}

/// A failed tool call is a successful JSON-RPC response carrying `isError`, so
/// the model sees the failure and can correct itself.
fn tool_result(result: Result<ToolOutput, String>) -> Value {
    match result {
        Ok(output) => json!({
            "resultType": "complete",
            "content": [{ "type": "text", "text": output.text }],
            "structuredContent": output.structured,
            "isError": false,
            "_meta": protocol::result_meta(),
        }),
        Err(message) => json!({
            "resultType": "complete",
            "content": [{ "type": "text", "text": message }],
            "isError": true,
            "_meta": protocol::result_meta(),
        }),
    }
}

#[cfg(test)]
fn memory_server() -> TursoMcpServer {
    use turso_core::{DatabaseOpts, SqliteDialect};

    let (_io, conn) =
        Connection::from_uri(":memory:", DatabaseOpts::default(), Arc::new(SqliteDialect))
            .expect("open memory database");
    TursoMcpServer::new(conn, Arc::new(AtomicUsize::new(0)))
}

#[cfg(test)]
mod tests {
    use super::protocol::{PROTOCOL_V2, UNSUPPORTED_PROTOCOL_VERSION};
    use super::*;

    fn v2_meta() -> Value {
        json!({
            "io.modelcontextprotocol/protocolVersion": PROTOCOL_V2,
            "io.modelcontextprotocol/clientCapabilities": {},
        })
    }

    fn send(server: &TursoMcpServer, message: Value) -> Option<Value> {
        let reply = server.handle_message(&message.to_string())?;
        Some(serde_json::from_str(&reply).expect("replies are JSON"))
    }

    fn call(server: &TursoMcpServer, name: &str, arguments: Value) -> Value {
        send(
            server,
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": { "name": name, "arguments": arguments, "_meta": v2_meta() },
            }),
        )
        .expect("tools/call is a request, not a notification")
    }

    #[test]
    fn discover_advertises_versions_and_caching() {
        let server = memory_server();

        let response = send(
            &server,
            json!({
                "jsonrpc": "2.0",
                "id": "d1",
                "method": "server/discover",
                "params": { "_meta": v2_meta() },
            }),
        )
        .expect("discover is a request");

        let result = &response["result"];
        assert_eq!(result["resultType"], "complete");
        assert_eq!(result["supportedVersions"][0], PROTOCOL_V2);
        assert!(result["capabilities"]["tools"].is_object());
        assert!(result["ttlMs"].as_u64().unwrap() > 0);
        assert_eq!(result["cacheScope"], "public");
        assert_eq!(
            result["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
            "turso-mcp"
        );
    }

    #[test]
    fn v2_client_calls_a_tool_without_any_handshake() {
        let server = memory_server();

        let response = call(&server, "list_tables", json!({}));

        assert_eq!(response["result"]["isError"], false);
        assert_eq!(response["result"]["structuredContent"]["tables"], json!([]));
    }

    #[test]
    fn unknown_protocol_version_is_rejected_with_the_supported_list() {
        let server = memory_server();

        let response = send(
            &server,
            json!({
                "jsonrpc": "2.0",
                "id": 7,
                "method": "tools/list",
                "params": {
                    "_meta": { "io.modelcontextprotocol/protocolVersion": "1999-01-01" }
                },
            }),
        )
        .expect("tools/list is a request");

        assert_eq!(response["error"]["code"], UNSUPPORTED_PROTOCOL_VERSION);
        assert_eq!(
            response["error"]["data"]["supportedVersions"],
            json!(SUPPORTED_VERSIONS)
        );
    }

    #[test]
    fn protocol_version_is_read_beside_params_too() {
        let server = memory_server();

        let response = send(
            &server,
            json!({
                "jsonrpc": "2.0",
                "id": 7,
                "method": "tools/list",
                "params": {},
                "_meta": { "io.modelcontextprotocol/protocolVersion": "1999-01-01" },
            }),
        )
        .expect("tools/list is a request");

        assert_eq!(response["error"]["code"], UNSUPPORTED_PROTOCOL_VERSION);
    }

    #[test]
    fn legacy_client_still_negotiates_through_initialize() {
        let server = memory_server();

        let response = send(
            &server,
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": { "name": "legacy", "version": "1.0" },
                },
            }),
        )
        .expect("initialize is a request");

        assert_eq!(response["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(response["result"]["serverInfo"]["name"], "turso-mcp");
    }

    #[test]
    fn initialize_falls_back_when_the_client_asks_for_an_unknown_version() {
        let server = memory_server();

        let response = send(
            &server,
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "protocolVersion": "1999-01-01" },
            }),
        )
        .expect("initialize is a request");

        assert_eq!(response["result"]["protocolVersion"], LEGACY_DEFAULT);
    }

    #[test]
    fn tools_list_is_cacheable_and_ordered() {
        let server = memory_server();

        let response = send(
            &server,
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list",
                "params": { "_meta": v2_meta() },
            }),
        )
        .expect("tools/list is a request");

        let result = &response["result"];
        assert_eq!(result["resultType"], "complete");
        assert_eq!(result["cacheScope"], "public");
        assert!(result["ttlMs"].as_u64().unwrap() > 0);

        let names: Vec<&str> = result["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted, "tool order must be stable for caching");
        assert!(names.contains(&"execute_query"));

        let query_tool = result["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "execute_query")
            .unwrap();
        assert!(query_tool["outputSchema"].is_object());
        assert_eq!(query_tool["annotations"]["readOnlyHint"], true);
    }

    #[test]
    fn unknown_tool_is_an_invalid_params_error() {
        let server = memory_server();

        let response = call(&server, "drop_everything", json!({}));

        assert_eq!(response["error"]["code"], INVALID_PARAMS);
    }

    #[test]
    fn unknown_method_is_method_not_found() {
        let server = memory_server();

        let response = send(
            &server,
            json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "resources/list",
                "params": { "_meta": v2_meta() },
            }),
        )
        .expect("resources/list is a request");

        assert_eq!(response["error"]["code"], METHOD_NOT_FOUND);
    }

    #[test]
    fn notifications_get_no_reply() {
        let server = memory_server();

        let reply = server.handle_message(
            &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string(),
        );

        assert!(reply.is_none());
    }

    #[test]
    fn unparseable_input_is_a_parse_error() {
        let server = memory_server();

        let reply = server
            .handle_message("{not json")
            .expect("a parse error is reported");
        let response: Value = serde_json::from_str(&reply).unwrap();

        assert_eq!(response["error"]["code"], PARSE_ERROR);
        assert!(response["id"].is_null());
    }
}

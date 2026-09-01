mod http;
mod protocol;
mod stdio;
mod tools;

use protocol::{
    JsonRpcError, JsonRpcRequest, JsonRpcResponse, CACHE_TTL_MS, INVALID_REQUEST, LEGACY_DEFAULT,
    METHOD_NOT_FOUND, PARSE_ERROR, SUPPORTED_VERSIONS,
};
use serde_json::{json, Value};
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use turso_core::Connection;

pub struct TursoMcpServer {
    pub(crate) conn: Arc<Mutex<Arc<Connection>>>,
    pub(crate) interrupt_count: Arc<AtomicUsize>,
    pub(crate) current_db_path: Arc<Mutex<Option<String>>>,
    /// Set by `--readonly`. The connection is opened read-only either way, but
    /// the server has to know as well: it advertises tools, and it can be asked
    /// to open a different database.
    pub(crate) readonly: bool,
}

impl TursoMcpServer {
    pub fn new(conn: Arc<Connection>, interrupt_count: Arc<AtomicUsize>, readonly: bool) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
            interrupt_count,
            current_db_path: Arc::new(Mutex::new(None)),
            readonly,
        }
    }

    /// One line in, at most one line out. A notification produces nothing at
    /// all - the previous shape returned an all-`None` response for the caller
    /// to recognise and discard, which put the decision in the wrong place.
    pub(crate) fn handle_message(&self, line: &str) -> Option<String> {
        let raw: Value = match serde_json::from_str(line) {
            Ok(raw) => raw,
            Err(e) => {
                let error = JsonRpcError::new(PARSE_ERROR, format!("Parse error: {e}"));
                return serde_json::to_string(&JsonRpcResponse::failure(None, error)).ok();
            }
        };

        // An absent id means "do not answer"; a null id is a malformed
        // request. Serde folds both into `None`, so they have to be told
        // apart before the request is typed.
        match raw.get("id") {
            None => return None,
            Some(id) if id.is_null() => {
                let error = JsonRpcError::new(INVALID_REQUEST, "Request id must not be null");
                return serde_json::to_string(&JsonRpcResponse::failure(None, error)).ok();
            }
            Some(_) => {}
        }

        let request: JsonRpcRequest = match serde_json::from_value(raw) {
            Ok(request) => request,
            Err(e) => {
                let error = JsonRpcError::new(INVALID_REQUEST, format!("Invalid request: {e}"));
                return serde_json::to_string(&JsonRpcResponse::failure(None, error)).ok();
            }
        };
        serde_json::to_string(&self.handle_request(request)).ok()
    }

    fn handle_request(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        let id = request.id.clone();

        // A `_meta` of the wrong shape is malformed whatever the method asks
        // for, so it is refused before the two negotiating methods are
        // excused the checks below.
        if let Err(error) = request.check_meta_shape() {
            return JsonRpcResponse::failure(id, error);
        }

        // Version and capability checks do not apply to the two methods a
        // client uses to find out what we speak.
        let negotiating = matches!(request.method.as_str(), "initialize" | "server/discover");
        if !negotiating {
            if let Err(error) = request.check_protocol_version() {
                return JsonRpcResponse::failure(id, error);
            }
            if let Err(error) = request.check_client_capabilities() {
                return JsonRpcResponse::failure(id, error);
            }
        }

        match request.method.as_str() {
            "server/discover" => JsonRpcResponse::success(id, self.discover_result()),
            "initialize" => self.handle_initialize(request),
            "tools/list" => self.handle_list_tools(request),
            "tools/call" => self.handle_call_tool(request),
            // Removed in v2, but harmless and still sent by older clients: a
            // liveness probe that costs nothing to answer and can only break
            // something by being refused.
            "ping" => JsonRpcResponse::success(id, json!({})),
            method => JsonRpcResponse::failure(
                id,
                JsonRpcError::new(METHOD_NOT_FOUND, format!("Method not found: {method}")),
            ),
        }
    }

    /// How a v2 client learns what we speak, in place of the handshake.
    fn discover_result(&self) -> Value {
        json!({
            "resultType": "complete",
            "supportedVersions": SUPPORTED_VERSIONS,
            "capabilities": { "tools": {} },
            "instructions": "Query and modify SQLite databases through this server's tools.",
            "ttlMs": CACHE_TTL_MS,
            "cacheScope": "public",
            "_meta": protocol::result_meta(),
        })
    }

    /// Only pre-v2 clients handshake. A v2 client goes straight to
    /// `server/discover`, so this stays for the ones that do not.
    fn handle_initialize(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        let asked = request
            .params
            .as_ref()
            .and_then(|p| p.get("protocolVersion"))
            .and_then(Value::as_str);
        let agreed = match asked {
            Some(version) if SUPPORTED_VERSIONS.contains(&version) => version,
            _ => LEGACY_DEFAULT,
        };
        JsonRpcResponse::success(
            request.id,
            json!({
                "protocolVersion": agreed,
                "capabilities": { "tools": {} },
                "serverInfo": protocol::server_info(),
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::protocol::{
        INVALID_PARAMS, INVALID_REQUEST, PROTOCOL_V2, UNSUPPORTED_PROTOCOL_VERSION,
    };
    use super::*;
    use serde_json::Value;
    use std::sync::atomic::AtomicUsize;
    use turso_core::{DatabaseOpts, SqliteDialect};

    fn memory_server() -> TursoMcpServer {
        let (_io, conn) =
            Connection::from_uri(":memory:", DatabaseOpts::default(), Arc::new(SqliteDialect))
                .expect("open memory database");
        TursoMcpServer::new(conn, Arc::new(AtomicUsize::new(0)), false)
    }

    fn answer(server: &TursoMcpServer, request: Value) -> Value {
        let raw = server
            .handle_message(&request.to_string())
            .expect("a request with an id is answered");
        serde_json::from_str(&raw).expect("the answer is JSON")
    }

    fn v2_meta() -> Value {
        json!({
            "io.modelcontextprotocol/protocolVersion": PROTOCOL_V2,
            "io.modelcontextprotocol/clientCapabilities": {},
        })
    }

    #[test]
    fn discover_names_what_we_speak_and_says_it_is_cacheable() {
        let response = answer(
            &memory_server(),
            json!({ "jsonrpc": "2.0", "id": 1, "method": "server/discover", "params": {} }),
        );
        let result = &response["result"];

        assert_eq!(result["resultType"], "complete");
        assert_eq!(result["supportedVersions"][0], PROTOCOL_V2);
        assert_eq!(result["capabilities"]["tools"], json!({}));
        assert_eq!(result["cacheScope"], "public");
        assert!(result["ttlMs"].as_u64().is_some_and(|ms| ms > 0));
        assert_eq!(
            result["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
            "turso-mcp"
        );
    }

    #[test]
    fn a_v2_client_calls_a_tool_with_no_handshake_at_all() {
        let response = answer(
            &memory_server(),
            json!({
                "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": { "name": "list_tables", "arguments": {}, "_meta": v2_meta() },
            }),
        );
        assert!(response["error"].is_null(), "{response}");
        assert_eq!(response["result"]["resultType"], "complete");
    }

    #[test]
    fn a_version_we_do_not_speak_is_refused_with_the_ones_we_do() {
        let response = answer(
            &memory_server(),
            json!({
                "jsonrpc": "2.0", "id": 3, "method": "tools/list",
                "params": { "_meta": { "io.modelcontextprotocol/protocolVersion": "1999-01-01" } },
            }),
        );
        let error = &response["error"];
        assert_eq!(error["code"], UNSUPPORTED_PROTOCOL_VERSION);
        assert_eq!(error["data"]["supported"], json!(SUPPORTED_VERSIONS));
        assert_eq!(error["data"]["requested"], "1999-01-01");
    }

    /// Both directions of the era-scoped rule: a client claiming v2 is held to
    /// v2's requirements, and a client claiming nothing is not.
    #[test]
    fn client_capabilities_are_required_of_v2_clients_only() {
        let server = memory_server();

        let strict = answer(
            &server,
            json!({
                "jsonrpc": "2.0", "id": 4, "method": "tools/list",
                "params": { "_meta": { "io.modelcontextprotocol/protocolVersion": PROTOCOL_V2 } },
            }),
        );
        assert_eq!(strict["error"]["code"], INVALID_PARAMS);

        let lenient = answer(
            &server,
            json!({ "jsonrpc": "2.0", "id": 5, "method": "tools/list", "params": {} }),
        );
        assert!(
            lenient["error"].is_null(),
            "a pre-v2 client has no capabilities field to send: {lenient}"
        );
    }

    /// A field of the wrong type reads as an absent one to `get` and
    /// `as_str`, and an absent version is exactly how a pre-v2 client looks.
    /// So without this the v2 client whose version we cannot parse is served
    /// as pre-v2 and never told that what it sent was ignored.
    #[test]
    fn a_meta_field_of_the_wrong_type_is_refused_rather_than_read_as_absent() {
        let server = memory_server();
        let malformed = [
            json!({ "_meta": PROTOCOL_V2 }),
            json!({ "_meta": { "io.modelcontextprotocol/protocolVersion": 20260728 } }),
            json!({
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": PROTOCOL_V2,
                    "io.modelcontextprotocol/clientCapabilities": "none",
                }
            }),
        ];

        let codes: Vec<Value> = malformed
            .iter()
            .map(|params| {
                answer(
                    &server,
                    json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/list", "params": params }),
                )["error"]["code"]
                    .clone()
            })
            .collect();

        assert_eq!(codes, vec![json!(INVALID_PARAMS); 3]);
    }

    #[test]
    fn ping_is_answered_whatever_era_the_client_claims() {
        let server = memory_server();
        for params in [json!({ "_meta": v2_meta() }), json!({})] {
            let response = answer(
                &server,
                json!({ "jsonrpc": "2.0", "id": 6, "method": "ping", "params": params }),
            );
            assert_eq!(response["result"], json!({}), "{response}");
        }
    }

    #[test]
    fn a_notification_is_answered_with_nothing() {
        let server = memory_server();
        assert!(server
            .handle_message(
                &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string()
            )
            .is_none());
    }

    /// An absent id means "do not answer". A null id is a malformed request,
    /// and answering it with silence would hide the client's bug.
    #[test]
    fn a_null_id_is_not_a_notification() {
        let response = answer(
            &memory_server(),
            json!({ "jsonrpc": "2.0", "id": null, "method": "tools/list", "params": {} }),
        );
        assert_eq!(response["error"]["code"], INVALID_REQUEST);
    }

    #[test]
    fn a_pre_v2_client_still_negotiates_by_handshake() {
        let server = memory_server();

        let known = answer(
            &server,
            json!({
                "jsonrpc": "2.0", "id": 7, "method": "initialize",
                "params": { "protocolVersion": "2024-11-05" },
            }),
        );
        assert_eq!(known["result"]["protocolVersion"], "2024-11-05");

        let unknown = answer(
            &server,
            json!({
                "jsonrpc": "2.0", "id": 8, "method": "initialize",
                "params": { "protocolVersion": "1999-01-01" },
            }),
        );
        assert_eq!(unknown["result"]["protocolVersion"], LEGACY_DEFAULT);
    }

    /// `2025-11-25` is the newest revision that still has a handshake at all
    /// (later revisions dropped it for `_meta`), so it is exactly what every
    /// handshake-based client offers. A client asking for a revision we
    /// support must get that revision back, not a silent downgrade.
    #[test]
    fn a_handshake_offering_2025_11_25_is_not_downgraded() {
        let response = answer(
            &memory_server(),
            json!({
                "jsonrpc": "2.0", "id": 10, "method": "initialize",
                "params": { "protocolVersion": "2025-11-25" },
            }),
        );
        assert_eq!(response["result"]["protocolVersion"], "2025-11-25");
    }

    #[test]
    fn an_unknown_method_is_method_not_found() {
        let response = answer(
            &memory_server(),
            json!({ "jsonrpc": "2.0", "id": 9, "method": "resources/read", "params": {} }),
        );
        assert_eq!(response["error"]["code"], METHOD_NOT_FOUND);
    }
}

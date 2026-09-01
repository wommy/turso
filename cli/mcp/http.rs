use super::protocol::{JsonRpcError, JsonRpcResponse, HEADER_MISMATCH};
use super::TursoMcpServer;
use crate::http::{format_http_response, parse_http_request, read_http_request, HttpResponse};
use anyhow::Result;
use serde_json::Value;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// The MCP spec's single Streamable HTTP endpoint. Every method but POST,
/// and every other path, answers 404 - a later slice tells 405 apart from
/// it once a red test asks for that distinction.
const ENDPOINT_PATH: &str = "/mcp";

pub struct HttpRequest {
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpRequest {
    /// HTTP header names are case-insensitive (RFC 9110 5.1), so a client
    /// sending `mcp-method` must be found the same as `Mcp-Method`.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

impl TursoMcpServer {
    pub fn run_http(&self, address: &str) -> Result<()> {
        let listener = TcpListener::bind(address)?;
        listener.set_nonblocking(true)?;

        let interrupt_count = self.interrupt_count.clone();
        let shutdown_flag = Arc::new(AtomicBool::new(false));
        let shutdown_flag_clone = shutdown_flag.clone();

        let monitor_handle = thread::spawn(move || loop {
            if interrupt_count.load(Ordering::SeqCst) > 0 {
                shutdown_flag_clone.store(true, Ordering::SeqCst);
                break;
            }
            thread::sleep(Duration::from_millis(100));
        });

        loop {
            if shutdown_flag.load(Ordering::SeqCst) {
                break;
            }

            match listener.accept() {
                Ok((stream, _addr)) => {
                    if let Err(e) = self.handle_http_connection(stream) {
                        eprintln!("MCP HTTP: error handling connection: {e}");
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(e) => {
                    eprintln!("MCP HTTP: error accepting connection: {e}");
                }
            }
        }

        let _ = monitor_handle.join();
        Ok(())
    }

    fn handle_http_connection(&self, mut stream: TcpStream) -> Result<()> {
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(30)))?;

        let request_data = read_http_request(&mut stream)?;
        let (method, path, headers, body) = parse_http_request(&request_data)?;

        let response = if method == "POST" && path == ENDPOINT_PATH {
            http_response_for(self, &HttpRequest { headers, body })
        } else {
            HttpResponse {
                status: 404,
                content_type: "text/plain".to_string(),
                body: b"Not Found".to_vec(),
                extra_headers: Vec::new(),
            }
        };

        stream.write_all(&format_http_response(&response))?;
        stream.flush()?;
        Ok(())
    }
}

pub fn http_response_for(server: &TursoMcpServer, req: &HttpRequest) -> HttpResponse {
    if let Ok(request) = serde_json::from_slice::<Value>(&req.body) {
        if let Err(response) = validate_headers(req, &request) {
            return response;
        }
    }
    let body = server
        .handle_message(&String::from_utf8_lossy(&req.body))
        .unwrap_or_default();
    HttpResponse {
        status: 200,
        content_type: "application/json".to_string(),
        body: body.into_bytes(),
        extra_headers: Vec::new(),
    }
}

/// The spec's request-metadata headers, checked against the JSON-RPC body
/// before the request is routed at all - a mismatch must never reach a tool.
fn validate_headers(req: &HttpRequest, request: &Value) -> Result<(), HttpResponse> {
    let id = request.get("id").cloned();

    for (name, value) in &req.headers {
        if !is_valid_header_value(value) {
            return Err(header_mismatch(
                id,
                format!("Header '{name}' contains invalid characters"),
            ));
        }
    }

    let body_method = request.get("method").and_then(Value::as_str).unwrap_or("");

    let header_method = req
        .header("Mcp-Method")
        .ok_or_else(|| header_mismatch(id.clone(), "Missing required header: Mcp-Method"))?;
    if header_method != body_method {
        return Err(header_mismatch(
            id,
            format!(
                "Header mismatch: Mcp-Method header value '{header_method}' does not match body value '{body_method}'"
            ),
        ));
    }

    if NAME_REQUIRED_METHODS.contains(&body_method) {
        // The spec's source field is `params.name` for tools/call and
        // prompts/get, but `params.uri` for resources/read - a resource is
        // identified by its URI, not a name.
        let name_field = if body_method == "resources/read" {
            "uri"
        } else {
            "name"
        };
        let body_name = request
            .get("params")
            .and_then(|params| params.get(name_field))
            .and_then(Value::as_str)
            .unwrap_or("");
        let header_name = req
            .header("Mcp-Name")
            .ok_or_else(|| header_mismatch(id.clone(), "Missing required header: Mcp-Name"))?;
        if header_name != body_name {
            return Err(header_mismatch(
                id,
                format!(
                    "Header mismatch: Mcp-Name header value '{header_name}' does not match body value '{body_name}'"
                ),
            ));
        }
    }

    Ok(())
}

/// The methods where the spec's `Mcp-Name` header carries `params.name` (or
/// `params.uri` for a resource) and must be validated against the body.
const NAME_REQUIRED_METHODS: [&str; 3] = ["tools/call", "resources/read", "prompts/get"];

/// RFC 9110 5.5: a header field value is visible ASCII (0x21-0x7E), space
/// (0x20), or horizontal tab (0x09). Anything else - a control character or a
/// non-ASCII byte - the spec calls invalid on its own, before any comparison
/// to the body runs.
fn is_valid_header_value(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte == 0x09 || (0x20..=0x7E).contains(&byte))
}

fn header_mismatch(id: Option<Value>, message: impl Into<String>) -> HttpResponse {
    let error = JsonRpcError::new(HEADER_MISMATCH, message);
    let response = JsonRpcResponse::failure(id, error);
    HttpResponse {
        status: 400,
        content_type: "application/json".to_string(),
        body: serde_json::to_vec(&response).unwrap_or_default(),
        extra_headers: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;
    use turso_core::{Connection, DatabaseOpts, SqliteDialect};

    fn memory_server() -> TursoMcpServer {
        let (_io, conn) =
            Connection::from_uri(":memory:", DatabaseOpts::default(), Arc::new(SqliteDialect))
                .expect("open memory database");
        TursoMcpServer::new(conn, Arc::new(AtomicUsize::new(0)), false)
    }

    #[test]
    fn a_post_of_tools_list_returns_200_with_the_json_rpc_result() {
        let server = memory_server();
        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {},
        })
        .to_string();

        let req = HttpRequest {
            headers: vec![("Mcp-Method".to_string(), "tools/list".to_string())],
            body: request_body.into_bytes(),
        };

        let response = http_response_for(&server, &req);

        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, "application/json");

        let body: Value = serde_json::from_slice(&response.body).expect("body is valid JSON-RPC");
        assert_eq!(body["id"], 1);
        let tools = body["result"]["tools"]
            .as_array()
            .expect("result carries a tools array");
        assert!(!tools.is_empty(), "tools array must not be empty");
    }

    #[test]
    fn an_mcp_method_header_that_disagrees_with_the_body_method_is_rejected() {
        let server = memory_server();
        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": "list_tables", "arguments": {} },
        })
        .to_string();

        let req = HttpRequest {
            headers: vec![("Mcp-Method".to_string(), "tools/list".to_string())],
            body: request_body.into_bytes(),
        };

        let response = http_response_for(&server, &req);

        assert_eq!(response.status, 400);
        let body: Value = serde_json::from_slice(&response.body).expect("body is valid JSON-RPC");
        assert_eq!(body["error"]["code"], -32020);
    }

    #[test]
    fn a_request_missing_the_mcp_method_header_entirely_is_rejected() {
        let server = memory_server();
        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {},
        })
        .to_string();

        let req = HttpRequest {
            headers: vec![],
            body: request_body.into_bytes(),
        };

        let response = http_response_for(&server, &req);

        assert_eq!(response.status, 400);
        let body: Value = serde_json::from_slice(&response.body).expect("body is valid JSON-RPC");
        assert_eq!(body["error"]["code"], -32020);
    }

    #[test]
    fn a_tools_call_request_missing_the_mcp_name_header_is_rejected() {
        let server = memory_server();
        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": "list_tables", "arguments": {} },
        })
        .to_string();

        let req = HttpRequest {
            headers: vec![("Mcp-Method".to_string(), "tools/call".to_string())],
            body: request_body.into_bytes(),
        };

        let response = http_response_for(&server, &req);

        assert_eq!(response.status, 400);
        let body: Value = serde_json::from_slice(&response.body).expect("body is valid JSON-RPC");
        assert_eq!(body["error"]["code"], -32020);
    }

    #[test]
    fn an_mcp_name_header_that_disagrees_with_params_name_is_rejected() {
        let server = memory_server();
        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": "bar", "arguments": {} },
        })
        .to_string();

        let req = HttpRequest {
            headers: vec![
                ("Mcp-Method".to_string(), "tools/call".to_string()),
                ("Mcp-Name".to_string(), "foo".to_string()),
            ],
            body: request_body.into_bytes(),
        };

        let response = http_response_for(&server, &req);

        assert_eq!(response.status, 400);
        let body: Value = serde_json::from_slice(&response.body).expect("body is valid JSON-RPC");
        assert_eq!(body["error"]["code"], -32020);
    }

    /// The header value here equals the body value exactly (both carry the
    /// same control character), so an equality-only check would let this
    /// through - it is invalid on its own terms, independent of matching.
    #[test]
    fn a_header_value_containing_invalid_characters_is_rejected() {
        let server = memory_server();
        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": "list_tables\u{0007}", "arguments": {} },
        })
        .to_string();

        let req = HttpRequest {
            headers: vec![
                ("Mcp-Method".to_string(), "tools/call".to_string()),
                ("Mcp-Name".to_string(), "list_tables\u{0007}".to_string()),
            ],
            body: request_body.into_bytes(),
        };

        let response = http_response_for(&server, &req);

        assert_eq!(response.status, 400);
        let body: Value = serde_json::from_slice(&response.body).expect("body is valid JSON-RPC");
        assert_eq!(body["error"]["code"], -32020);
    }
}

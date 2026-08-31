use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use anyhow::Result;
use serde_json::{json, Value};

use super::protocol::{
    JsonRpcError, JsonRpcResponse, HEADER_MISMATCH, PARSE_ERROR, PROTOCOL_V2, SUPPORTED_VERSIONS,
    UNSUPPORTED_PROTOCOL_VERSION,
};
use super::TursoMcpServer;
use crate::http::{
    format_http_response, read_http_request, HttpRequest, HttpResponse, ReadLimits, RequestError,
};

/// v2 wants one endpoint. `/` is accepted too so a bare address works.
const MCP_PATHS: [&str; 2] = ["/mcp", "/"];

const READ_LIMITS: ReadLimits = ReadLimits {
    max_body_bytes: 16 * 1024 * 1024,
    max_duration: Duration::from_secs(30),
};

/// A client that stops reading must not hold the socket open forever.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

const ALLOWED_REQUEST_HEADERS: &str =
    "content-type, accept, mcp-protocol-version, mcp-method, mcp-name";

pub(super) fn run(server: &TursoMcpServer, address: &str) -> Result<()> {
    let listener = TcpListener::bind(address)
        .map_err(|e| anyhow::anyhow!("--mcp-http could not bind {address}: {e}"))?;
    let local_addr = listener.local_addr()?;
    if !local_addr.ip().is_loopback() {
        eprintln!(
            "warning: MCP HTTP server is bound to {local_addr}, so other machines can reach your databases"
        );
    }
    eprintln!("MCP HTTP server listening on {local_addr}");
    listener.set_nonblocking(true)?;

    // One thread per connection, so a client that reads its response slowly
    // cannot hold up everyone else.
    thread::scope(|scope| loop {
        if server.interrupted() {
            eprintln!("MCP server interrupted, shutting down...");
            break;
        }

        match listener.accept() {
            Ok((stream, _)) => {
                scope.spawn(move || {
                    if let Err(e) = serve_connection(server, stream) {
                        eprintln!("MCP HTTP request failed: {e}");
                    }
                });
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(e) => eprintln!("Error accepting connection: {e}"),
        }
    });

    Ok(())
}

fn serve_connection(server: &TursoMcpServer, mut stream: TcpStream) -> Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(READ_LIMITS.max_duration))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;

    let response = match read_http_request(&mut stream, &READ_LIMITS) {
        Ok(request) => response_for(server, &request),
        Err(RequestError::Refused(reason)) => HttpResponse::text(400, &reason),
        Err(RequestError::Io(e)) => return Err(e),
    };

    stream.write_all(&format_http_response(&response))?;
    stream.flush()?;
    Ok(())
}

fn response_for(server: &TursoMcpServer, request: &HttpRequest) -> HttpResponse {
    if !MCP_PATHS.contains(&request.path.as_str()) {
        return HttpResponse::text(404, "Not Found");
    }
    // Without this a web page could drive a local server through DNS rebinding.
    let origin = request.header("Origin");
    if let Some(origin) = origin {
        if !is_local_origin(origin) {
            return HttpResponse::text(403, "Forbidden");
        }
    }

    let response = match request.method.as_str() {
        // A browser preflights, because the routing headers are custom ones.
        "OPTIONS" if origin.is_some() => HttpResponse::new(204, "text/plain", Vec::new()),
        "POST" => post_response(server, request),
        // v2 removed the GET stream, so nothing else is answered here.
        _ => HttpResponse::text(405, "Method Not Allowed")
            .with_headers(vec![("Allow".to_string(), "POST, OPTIONS".to_string())]),
    };

    match origin {
        Some(origin) => response.with_headers(cors_headers_for(origin)),
        None => response,
    }
}

/// Only ever the one loopback origin we just checked, never `*`.
fn cors_headers_for(origin: &str) -> Vec<(String, String)> {
    [
        ("Access-Control-Allow-Origin", origin),
        ("Access-Control-Allow-Methods", "POST, OPTIONS"),
        ("Access-Control-Allow-Headers", ALLOWED_REQUEST_HEADERS),
        ("Vary", "Origin"),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value.to_string()))
    .collect()
}

fn post_response(server: &TursoMcpServer, request: &HttpRequest) -> HttpResponse {
    if !accepts_json(request) {
        return HttpResponse::text(400, "Accept header must allow application/json");
    }

    let Ok(body) = std::str::from_utf8(&request.body) else {
        return error_response(
            400,
            &Value::Null,
            JsonRpcError::new(PARSE_ERROR, "Request body is not valid UTF-8"),
        );
    };
    let Ok(message) = serde_json::from_str::<Value>(body) else {
        return error_response(
            400,
            &Value::Null,
            JsonRpcError::new(PARSE_ERROR, "Request body is not valid JSON"),
        );
    };
    let id = message.get("id").cloned().unwrap_or(Value::Null);

    if let Err(error) = check_headers(request, &message) {
        return error_response(400, &id, error);
    }

    match server.handle_message(body) {
        Some(response) => HttpResponse::new(200, "application/json", response.into_bytes()),
        // A notification is accepted and answered with nothing.
        None => HttpResponse::new(202, "application/json", Vec::new()),
    }
}

/// Streamable HTTP routes on headers, so a v2 request's headers have to agree
/// with the body they travel with. The headers are a v2 invention, so a client
/// on an older revision is not asked for them.
fn check_headers(request: &HttpRequest, message: &Value) -> Result<(), JsonRpcError> {
    let body_method = message.get("method").and_then(Value::as_str).unwrap_or("");

    // Discovery is how a client learns which versions we speak, so it answers
    // whatever version the client declares, including one we do not know.
    let discovering = body_method == "server/discover";

    match request.header("MCP-Protocol-Version") {
        // No version header means a pre-v2 client; 2025-06-18 says to read that
        // as 2025-03-26 rather than to reject it.
        None => return Ok(()),
        Some(version) if version == PROTOCOL_V2 => {}
        Some(version) if SUPPORTED_VERSIONS.contains(&version) || discovering => return Ok(()),
        Some(version) => {
            let mut error = JsonRpcError::new(
                UNSUPPORTED_PROTOCOL_VERSION,
                format!("Unsupported protocol version: {version}"),
            );
            error.data = Some(json!({ "supportedVersions": SUPPORTED_VERSIONS }));
            return Err(error);
        }
    }

    match request.header("Mcp-Method") {
        None => {
            return Err(JsonRpcError::new(
                HEADER_MISMATCH,
                "Missing Mcp-Method header",
            ))
        }
        Some(header) if header != body_method => {
            return Err(JsonRpcError::new(
                HEADER_MISMATCH,
                format!(
                    "Header mismatch: Mcp-Method header value '{header}' does not match body value '{body_method}'"
                ),
            ))
        }
        Some(_) => {}
    }

    if body_method == "tools/call" {
        let body_name = message
            .get("params")
            .and_then(|params| params.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("");
        match request.header("Mcp-Name") {
            None => {
                return Err(JsonRpcError::new(
                    HEADER_MISMATCH,
                    "Missing Mcp-Name header",
                ))
            }
            Some(header) if header != body_name => {
                return Err(JsonRpcError::new(
                    HEADER_MISMATCH,
                    format!(
                        "Header mismatch: Mcp-Name header value '{header}' does not match body value '{body_name}'"
                    ),
                ))
            }
            Some(_) => {}
        }
    }

    Ok(())
}

fn is_local_origin(origin: &str) -> bool {
    let Some((_scheme, rest)) = origin.split_once("://") else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or(rest);
    let host = match authority.strip_prefix('[') {
        Some(bracketed) => bracketed.split(']').next().unwrap_or_default(),
        None => authority.split(':').next().unwrap_or(authority),
    };
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

fn accepts_json(request: &HttpRequest) -> bool {
    match request.header("Accept") {
        None => true,
        Some(accept) => accept.contains("application/json") || accept.contains("*/*"),
    }
}

fn error_response(status: u16, id: &Value, error: JsonRpcError) -> HttpResponse {
    let body = json!(JsonRpcResponse::failure(Some(id.clone()), error));
    HttpResponse::new(status, "application/json", body.to_string().into_bytes())
}

#[cfg(test)]
mod tests {
    use super::super::memory_server;
    use super::super::protocol::PROTOCOL_V2;
    use super::*;

    fn post(headers: Vec<(&str, &str)>, body: Value) -> HttpRequest {
        HttpRequest {
            method: "POST".to_string(),
            path: "/mcp".to_string(),
            headers: headers
                .into_iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
            body: body.to_string().into_bytes(),
        }
    }

    fn tools_call_body(name: &str) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": name,
                "arguments": {},
                "_meta": { "io.modelcontextprotocol/protocolVersion": PROTOCOL_V2 },
            },
        })
    }

    fn body_json(response: &HttpResponse) -> Value {
        serde_json::from_slice(&response.body).expect("JSON body")
    }

    #[test]
    fn well_formed_tool_call_answers_with_json() {
        let server = memory_server();

        let response = response_for(
            &server,
            &post(
                vec![
                    ("MCP-Protocol-Version", PROTOCOL_V2),
                    ("Mcp-Method", "tools/call"),
                    ("Mcp-Name", "list_tables"),
                    ("Accept", "application/json, text/event-stream"),
                ],
                tools_call_body("list_tables"),
            ),
        );

        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, "application/json");
        assert_eq!(body_json(&response)["result"]["isError"], false);
    }

    #[test]
    fn method_header_must_match_the_body() {
        let server = memory_server();

        let response = response_for(
            &server,
            &post(
                vec![
                    ("MCP-Protocol-Version", PROTOCOL_V2),
                    ("Mcp-Method", "tools/list"),
                    ("Mcp-Name", "list_tables"),
                ],
                tools_call_body("list_tables"),
            ),
        );

        assert_eq!(response.status, 400);
        assert_eq!(body_json(&response)["error"]["code"], HEADER_MISMATCH);
    }

    #[test]
    fn name_header_must_match_the_called_tool() {
        let server = memory_server();

        let response = response_for(
            &server,
            &post(
                vec![
                    ("MCP-Protocol-Version", PROTOCOL_V2),
                    ("Mcp-Method", "tools/call"),
                    ("Mcp-Name", "describe_table"),
                ],
                tools_call_body("list_tables"),
            ),
        );

        assert_eq!(response.status, 400);
        assert_eq!(body_json(&response)["error"]["code"], HEADER_MISMATCH);
    }

    #[test]
    fn a_pre_v2_client_needs_none_of_the_v2_headers() {
        let server = memory_server();

        let response = response_for(
            &server,
            &post(
                vec![],
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": { "protocolVersion": "2024-11-05", "capabilities": {} },
                }),
            ),
        );

        assert_eq!(response.status, 200, "{:?}", body_json(&response));
        assert_eq!(
            body_json(&response)["result"]["protocolVersion"],
            "2024-11-05"
        );
    }

    #[test]
    fn a_pre_v2_client_can_call_a_tool_without_routing_headers() {
        let server = memory_server();

        let response = response_for(
            &server,
            &post(
                vec![("MCP-Protocol-Version", "2025-06-18")],
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": { "name": "list_tables", "arguments": {} },
                }),
            ),
        );

        assert_eq!(response.status, 200, "{:?}", body_json(&response));
        assert_eq!(body_json(&response)["result"]["isError"], false);
    }

    #[test]
    fn discovery_answers_a_version_we_do_not_know() {
        let server = memory_server();

        let response = response_for(
            &server,
            &post(
                vec![
                    ("MCP-Protocol-Version", "2099-01-01"),
                    ("Mcp-Method", "server/discover"),
                ],
                json!({ "jsonrpc": "2.0", "id": 1, "method": "server/discover", "params": {} }),
            ),
        );

        assert_eq!(response.status, 200, "{:?}", body_json(&response));
        assert_eq!(
            body_json(&response)["result"]["supportedVersions"][0],
            PROTOCOL_V2
        );
    }

    #[test]
    fn a_v2_client_still_has_to_send_the_routing_headers() {
        let server = memory_server();

        let response = response_for(
            &server,
            &post(
                vec![("MCP-Protocol-Version", PROTOCOL_V2)],
                json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} }),
            ),
        );

        assert_eq!(response.status, 400);
        assert_eq!(body_json(&response)["error"]["code"], HEADER_MISMATCH);
    }

    #[test]
    fn responses_never_carry_wildcard_cors() {
        let server = memory_server();

        let plain = response_for(
            &server,
            &post(
                vec![
                    ("MCP-Protocol-Version", PROTOCOL_V2),
                    ("Mcp-Method", "tools/list"),
                ],
                json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} }),
            ),
        );
        assert!(
            !plain
                .extra_headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("access-control-allow-origin")),
            "a request with no Origin gets no CORS headers"
        );

        let from_browser = response_for(
            &server,
            &post(
                vec![
                    ("MCP-Protocol-Version", PROTOCOL_V2),
                    ("Mcp-Method", "tools/list"),
                    ("Origin", "http://localhost:5173"),
                ],
                json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} }),
            ),
        );
        let allowed = from_browser
            .extra_headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("access-control-allow-origin"))
            .map(|(_, value)| value.as_str());
        assert_eq!(allowed, Some("http://localhost:5173"));
    }

    #[test]
    fn a_browser_preflight_is_answered() {
        let server = memory_server();

        let mut request = post(vec![("Origin", "http://localhost:5173")], json!({}));
        request.method = "OPTIONS".to_string();

        let response = response_for(&server, &request);

        assert_eq!(response.status, 204);
        let headers: Vec<&str> = response
            .extra_headers
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        assert!(headers.contains(&"Access-Control-Allow-Methods"));
        assert!(headers.contains(&"Access-Control-Allow-Headers"));
    }

    #[test]
    fn unsupported_version_header_lists_what_we_speak() {
        let server = memory_server();

        let response = response_for(
            &server,
            &post(
                vec![
                    ("MCP-Protocol-Version", "1999-01-01"),
                    ("Mcp-Method", "tools/list"),
                ],
                json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} }),
            ),
        );

        assert_eq!(response.status, 400);
        let body = body_json(&response);
        assert_eq!(body["error"]["code"], UNSUPPORTED_PROTOCOL_VERSION);
        assert_eq!(
            body["error"]["data"]["supportedVersions"],
            json!(SUPPORTED_VERSIONS)
        );
    }

    #[test]
    fn notification_is_accepted_without_a_body() {
        let server = memory_server();

        let response = response_for(
            &server,
            &post(
                vec![
                    ("MCP-Protocol-Version", PROTOCOL_V2),
                    ("Mcp-Method", "notifications/initialized"),
                ],
                json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            ),
        );

        assert_eq!(response.status, 202);
        assert!(response.body.is_empty());
    }

    #[test]
    fn cross_origin_requests_are_refused() {
        let server = memory_server();

        let mut request = post(
            vec![
                ("MCP-Protocol-Version", PROTOCOL_V2),
                ("Mcp-Method", "tools/list"),
                ("Origin", "https://evil.example"),
            ],
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} }),
        );
        request.headers.push(("Host".to_string(), "x".to_string()));

        assert_eq!(response_for(&server, &request).status, 403);
    }

    #[test]
    fn loopback_origins_are_allowed() {
        for origin in [
            "http://localhost:5173",
            "http://127.0.0.1:8081",
            "http://[::1]:8081",
        ] {
            assert!(is_local_origin(origin), "{origin} should be allowed");
        }
        assert!(!is_local_origin("https://localhost.evil.example"));
        assert!(!is_local_origin("null"));
    }

    #[test]
    fn get_is_not_allowed_on_the_endpoint() {
        let server = memory_server();

        let mut request = post(vec![], json!({}));
        request.method = "GET".to_string();

        assert_eq!(response_for(&server, &request).status, 405);
    }
}

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use super::protocol::{
    JsonRpcError, JsonRpcResponse, HEADER_MISMATCH, METHOD_NOT_FOUND, PARSE_ERROR, PROTOCOL_V2,
    SUPPORTED_VERSIONS, UNSUPPORTED_PROTOCOL_VERSION,
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

/// Total time a response may take to leave the socket. `write_all` alone
/// only bounds a single write() call, so a client that drains a few bytes
/// just before each write's own timeout would keep every write succeeding
/// and never trip a plain per-write timeout, holding the thread - and
/// shutdown, since `run` joins every spawned thread - open indefinitely.
const WRITE_DEADLINE: Duration = Duration::from_secs(30);

/// How long a single write (or read, during the request phase) may block
/// before the code above rechecks the deadline and the interrupt flag.
const IO_POLL_TIMEOUT: Duration = Duration::from_millis(500);

/// A connection flood must not spawn an unbounded number of threads, each
/// buffering up to `READ_LIMITS.max_body_bytes`.
const MAX_CONNECTIONS: usize = 64;

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

    let active_connections = AtomicUsize::new(0);

    // One thread per connection, so a client that reads its response slowly
    // cannot hold up everyone else.
    thread::scope(|scope| loop {
        if server.interrupted() {
            eprintln!("MCP server interrupted, shutting down...");
            break;
        }

        match listener.accept() {
            Ok((stream, _)) => {
                if active_connections.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                    active_connections.fetch_sub(1, Ordering::SeqCst);
                    reject_over_capacity(stream);
                    continue;
                }
                let active_connections = &active_connections;
                scope.spawn(move || {
                    if let Err(e) = serve_connection(server, stream) {
                        eprintln!("MCP HTTP request failed: {e}");
                    }
                    active_connections.fetch_sub(1, Ordering::SeqCst);
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

/// A best-effort reply: the caller is already shedding load, so this is not
/// worth a spawned thread, just a short timeout so a slow client cannot hold
/// up the accept loop while it is told no.
fn reject_over_capacity(mut stream: TcpStream) {
    let _ = stream.set_read_timeout(Some(IO_POLL_TIMEOUT));
    // Closing a socket that still has unread bytes sitting in it makes the
    // OS send a connection reset instead of a clean close, which looks like
    // a broken connection rather than "413, please retry" - so drain
    // whatever the client already sent first, within the same short budget.
    let mut discard = [0u8; 8192];
    while let Ok(n) = stream.read(&mut discard) {
        if n < discard.len() {
            break;
        }
    }

    let _ = stream.set_write_timeout(Some(IO_POLL_TIMEOUT));
    let response = format_http_response(&HttpResponse::text(503, "Too many connections"));
    let _ = stream.write_all(&response);
}

fn serve_connection(server: &TursoMcpServer, mut stream: TcpStream) -> Result<()> {
    stream.set_nonblocking(false)?;

    let response = match read_http_request(&mut stream, &READ_LIMITS) {
        Ok(request) => response_for(server, &request),
        Err(RequestError::Refused(reason)) => HttpResponse::text(400, &reason),
        Err(RequestError::Io(e)) => return Err(e),
    };

    write_response_bounded(
        server,
        &mut stream,
        &format_http_response(&response),
        WRITE_DEADLINE,
    )
}

/// Writes in small steps against an overall deadline, instead of trusting a
/// single `write_all` bounded only by a per-write socket timeout, and gives
/// up as soon as the server is told to shut down - so Ctrl-C does not have
/// to wait out a client that has stopped reading. `deadline` is a parameter
/// rather than always `WRITE_DEADLINE` so tests can use one far shorter than
/// 30 seconds.
fn write_response_bounded(
    server: &TursoMcpServer,
    stream: &mut TcpStream,
    bytes: &[u8],
    deadline: Duration,
) -> Result<()> {
    stream.set_write_timeout(Some(IO_POLL_TIMEOUT.min(deadline)))?;
    let started = Instant::now();
    let mut sent = 0;
    while sent < bytes.len() {
        if server.interrupted() {
            return Err(anyhow!("MCP server interrupted while writing the response"));
        }
        if started.elapsed() > deadline {
            return Err(anyhow!("Writing the response took too long"));
        }
        match stream.write(&bytes[sent..]) {
            Ok(0) => return Err(anyhow!("connection closed while writing the response")),
            Ok(n) => sent += n,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(e) => return Err(e.into()),
        }
    }
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
        // Answered the same with or without Origin: the Allow header on the
        // 405 below already advertises OPTIONS as a method this endpoint
        // accepts, so a bare OPTIONS (no preflight) gets the same answer a
        // browser's preflight would.
        "OPTIONS" => HttpResponse::new(204, "text/plain", Vec::new()),
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
        Some(response) => {
            let status = status_for_message_response(&response);
            HttpResponse::new(status, "application/json", response.into_bytes())
        }
        // A notification is accepted and answered with nothing, so there is
        // no media type to claim either.
        None => HttpResponse::new(202, "", Vec::new()),
    }
}

/// `handle_message` can fail after `check_headers` already passed - the
/// version and method it validates there are the header's, not the body's,
/// and a body-level `_meta.protocolVersion` or an unimplemented method only
/// surfaces once `handle_message` itself parses the request. The spec ties
/// each such JSON-RPC error to a specific transport status rather than
/// leaving every error at 200, so this maps the two that can come back:
/// method-not-found to 404 (streamable-http.mdx line 271-275) and an
/// unsupported protocol version to 400 (same file, line 263-269). Anything
/// else - including a normal result - keeps 200.
fn status_for_message_response(response: &str) -> u16 {
    let Ok(parsed) = serde_json::from_str::<Value>(response) else {
        return 200;
    };
    match parsed.get("error").and_then(|error| error.get("code")) {
        Some(code) if code == METHOD_NOT_FOUND => 404,
        Some(code) if code == UNSUPPORTED_PROTOCOL_VERSION => 400,
        _ => 200,
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
            Some(header) => {
                let Ok(decoded) = decode_header_value(header) else {
                    return Err(JsonRpcError::new(
                        HEADER_MISMATCH,
                        format!("Mcp-Name header value '{header}' is not valid base64/UTF-8"),
                    ));
                };
                if decoded != body_name {
                    return Err(JsonRpcError::new(
                        HEADER_MISMATCH,
                        format!(
                            "Header mismatch: Mcp-Name header value '{header}' does not match body value '{body_name}'"
                        ),
                    ));
                }
            }
        }
    }

    Ok(())
}

/// Undoes the spec's `=?base64?...?=` sentinel a client uses to carry an
/// `Mcp-Name` value that is not plain, header-safe ASCII (Value Encoding
/// section of streamable-http.mdx). A value with no sentinel is returned
/// unchanged, since a header-safe tool name is sent as-is. `Err` means the
/// sentinel was present but the payload inside it was not valid
/// base64-encoded UTF-8, which the caller treats as a header mismatch.
fn decode_header_value(value: &str) -> Result<std::borrow::Cow<'_, str>, ()> {
    let Some(inner) = value
        .strip_prefix("=?base64?")
        .and_then(|rest| rest.strip_suffix("?="))
    else {
        return Ok(std::borrow::Cow::Borrowed(value));
    };
    let bytes = decode_base64(inner).ok_or(())?;
    String::from_utf8(bytes)
        .map(std::borrow::Cow::Owned)
        .map_err(|_| ())
}

/// A small hand-rolled RFC 4648 (standard alphabet, `+`/`/`, `=` padding)
/// decoder rather than a new crate dependency, since this only ever has to
/// decode one header value at a time.
fn decode_base64(input: &str) -> Option<Vec<u8>> {
    fn sextet(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }

    let input = input.as_bytes();
    if input.is_empty() || input.len() % 4 != 0 {
        return None;
    }
    let chunk_count = input.len() / 4;
    let mut out = Vec::with_capacity(chunk_count * 3);
    for (chunk_index, chunk) in input.chunks_exact(4).enumerate() {
        let is_last_chunk = chunk_index + 1 == chunk_count;
        let mut values = [0u8; 4];
        let mut padding = 0;
        for (i, &byte) in chunk.iter().enumerate() {
            // Padding is only meaningful in the last two positions of the
            // final group; anywhere else it is not valid base64.
            if byte == b'=' && is_last_chunk && i >= 2 {
                padding += 1;
            } else {
                values[i] = sextet(byte)?;
            }
        }
        let bits = (values[0] as u32) << 18
            | (values[1] as u32) << 12
            | (values[2] as u32) << 6
            | (values[3] as u32);
        out.push((bits >> 16) as u8);
        if padding < 2 {
            out.push((bits >> 8) as u8);
        }
        if padding == 0 {
            out.push(bits as u8);
        }
    }
    Some(out)
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
            body: body.to_string().into_bytes().into(),
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

    /// The spec's own Value Encoding example: a non-ASCII tool name must be
    /// sent Base64-wrapped, and the server must decode it before comparing.
    #[test]
    fn a_base64_wrapped_name_header_matches_a_non_ascii_tool_name() {
        let server = memory_server();

        let response = response_for(
            &server,
            &post(
                vec![
                    ("MCP-Protocol-Version", PROTOCOL_V2),
                    ("Mcp-Method", "tools/call"),
                    ("Mcp-Name", "=?base64?Y2Fmw6lfbG9va3Vw?="),
                ],
                tools_call_body("café_lookup"),
            ),
        );

        // The tool itself does not exist, so this still fails - but as a
        // normal "tool not found" result, not a header mismatch, proving the
        // header was decoded and compared correctly before dispatch.
        assert_eq!(response.status, 200, "{:?}", body_json(&response));
        assert_ne!(body_json(&response)["error"]["code"], HEADER_MISMATCH);
    }

    #[test]
    fn a_base64_wrapped_name_header_that_disagrees_with_the_body_is_still_a_mismatch() {
        let server = memory_server();

        let response = response_for(
            &server,
            &post(
                vec![
                    ("MCP-Protocol-Version", PROTOCOL_V2),
                    ("Mcp-Method", "tools/call"),
                    ("Mcp-Name", "=?base64?Y2Fmw6lfbG9va3Vw?="), // "café_lookup"
                ],
                tools_call_body("list_tables"),
            ),
        );

        assert_eq!(response.status, 400);
        assert_eq!(body_json(&response)["error"]["code"], HEADER_MISMATCH);
    }

    #[test]
    fn an_invalid_base64_sentinel_is_a_header_mismatch_not_a_panic() {
        let server = memory_server();

        let response = response_for(
            &server,
            &post(
                vec![
                    ("MCP-Protocol-Version", PROTOCOL_V2),
                    ("Mcp-Method", "tools/call"),
                    ("Mcp-Name", "=?base64?not valid base64?="),
                ],
                tools_call_body("list_tables"),
            ),
        );

        assert_eq!(response.status, 400);
        assert_eq!(body_json(&response)["error"]["code"], HEADER_MISMATCH);
    }

    #[test]
    fn decode_base64_matches_the_specs_own_encoding_examples() {
        assert_eq!(
            decode_header_value("=?base64?Y2Fmw6lfbG9va3Vw?=")
                .unwrap()
                .into_owned(),
            "café_lookup"
        );
        assert_eq!(
            decode_header_value("=?base64?SGVsbG8sIOS4lueVjA==?=")
                .unwrap()
                .into_owned(),
            "Hello, 世界"
        );
        assert_eq!(
            decode_header_value("us-west1").unwrap().into_owned(),
            "us-west1",
            "a value with no sentinel is passed through unchanged"
        );
        assert!(decode_header_value("=?base64?not valid base64?=").is_err());
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

    /// streamable-http.mdx line 271-275: an unimplemented RPC method MUST come
    /// back as 404, carrying the -32601 JSON-RPC error, not a 200.
    #[test]
    fn an_unimplemented_method_answers_404_not_200() {
        let server = memory_server();

        let response = response_for(
            &server,
            &post(
                vec![
                    ("MCP-Protocol-Version", PROTOCOL_V2),
                    ("Mcp-Method", "resources/read"),
                ],
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "resources/read",
                    "params": { "uri": "file:///x" },
                }),
            ),
        );

        assert_eq!(response.status, 404, "{:?}", body_json(&response));
        assert_eq!(body_json(&response)["error"]["code"], METHOD_NOT_FOUND);
    }

    /// streamable-http.mdx line 263-269: the same paragraph requires 400 for
    /// an unsupported protocol version. This is the body-level
    /// `_meta.protocolVersion` check (`check_protocol_version` in
    /// `mod.rs`), distinct from the header-level check above it - both must
    /// map to 400, not just the header one.
    #[test]
    fn a_body_level_unsupported_protocol_version_answers_400_not_200() {
        let server = memory_server();

        let response = response_for(
            &server,
            &post(
                vec![
                    ("MCP-Protocol-Version", PROTOCOL_V2),
                    ("Mcp-Method", "tools/list"),
                ],
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/list",
                    "params": {},
                    "_meta": { "io.modelcontextprotocol/protocolVersion": "1999-01-01" },
                }),
            ),
        );

        assert_eq!(response.status, 400, "{:?}", body_json(&response));
        assert_eq!(
            body_json(&response)["error"]["code"],
            UNSUPPORTED_PROTOCOL_VERSION
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

    #[test]
    fn options_without_an_origin_is_answered_the_same_as_a_preflight() {
        let server = memory_server();

        let mut request = post(vec![], json!({}));
        request.method = "OPTIONS".to_string();

        assert_eq!(
            response_for(&server, &request).status,
            204,
            "the 405 branch's Allow header already claims OPTIONS is accepted"
        );
    }

    #[test]
    fn write_response_bounded_gives_up_once_the_deadline_passes() {
        let server = memory_server();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let _client = TcpStream::connect(address).expect("connect"); // never reads
        let (mut server_stream, _) = listener.accept().expect("accept");

        // Bigger than typical socket buffers, so the write below genuinely
        // blocks instead of the whole payload being buffered by the kernel.
        let payload = vec![b'x'; 8 * 1024 * 1024];
        let started = Instant::now();

        let result = write_response_bounded(
            &server,
            &mut server_stream,
            &payload,
            Duration::from_millis(800),
        );

        assert!(result.is_err(), "a stalled client must not block forever");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the deadline should have cut the write off quickly: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn write_response_bounded_gives_up_once_interrupted() {
        let server = memory_server();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let _client = TcpStream::connect(address).expect("connect"); // never reads
        let (mut server_stream, _) = listener.accept().expect("accept");
        let payload = vec![b'x'; 8 * 1024 * 1024];

        thread::scope(|scope| {
            let handle = scope.spawn(|| {
                write_response_bounded(
                    &server,
                    &mut server_stream,
                    &payload,
                    Duration::from_secs(30),
                )
            });
            thread::sleep(Duration::from_millis(200));
            server.interrupt_count.store(1, Ordering::SeqCst);

            let result = handle.join().expect("the writer thread does not panic");
            assert!(
                result.is_err(),
                "an interrupted server must abandon a stalled write rather than wait out the deadline"
            );
        });
    }
}

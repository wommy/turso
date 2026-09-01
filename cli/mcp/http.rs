use super::protocol::{JsonRpcError, JsonRpcResponse, FORBIDDEN_ORIGIN, HEADER_MISMATCH};
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
    if let Some(origin) = req.header("Origin") {
        if !origin_is_loopback(origin) {
            return forbidden_origin(origin);
        }
    }
    if let Err(response) = validate_headers(req) {
        return response;
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

/// The MCP spec's DNS-rebinding defense: a browser-sent `Origin` that is
/// present and not loopback is refused before the request is routed at all.
/// A non-browser client sends no `Origin`, so its absence is allowed - the
/// spec's own wording is "if present and invalid".
///
/// Loopback here is `localhost`, the whole `127.0.0.0/8` block (loopback to
/// the kernel, not just `127.0.0.1`), and IPv6 `::1`, on any port and any
/// scheme. The host is parsed out of the authority rather than
/// substring-matched, so `http://localhost.evil.com` and
/// `http://127.0.0.1.attacker.net` do not pass as loopback just because they
/// contain a loopback name. `Origin: null` - a sandboxed iframe or a `file://`
/// page - names no host and is refused the same way.
fn origin_is_loopback(origin: &str) -> bool {
    let Some(host) = origin_host(origin) else {
        return false;
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|ip| ip.is_loopback())
        || host
            .parse::<std::net::Ipv6Addr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// The host from an Origin value shaped `scheme://host[:port]` (RFC 6454),
/// with IPv6's bracket notation unwrapped. `null` and anything else with no
/// `scheme://` have no host to extract.
fn origin_host(origin: &str) -> Option<&str> {
    let authority = origin.split_once("://")?.1;
    if let Some(v6_and_rest) = authority.strip_prefix('[') {
        return v6_and_rest.split(']').next();
    }
    Some(authority.split(':').next().unwrap_or(authority))
}

/// The spec allows but does not require a JSON-RPC error body on this 403.
/// One is included, matching every other error response this server sends,
/// so a client does not need a special case for the one response whose body
/// might be empty. The id is always null: a rejected origin gets no benefit
/// of the doubt, so its body is not parsed just to echo its id back.
fn forbidden_origin(origin: &str) -> HttpResponse {
    let error = JsonRpcError::new(FORBIDDEN_ORIGIN, format!("Forbidden origin: {origin}"));
    let response = JsonRpcResponse::failure(None, error);
    HttpResponse {
        status: 403,
        content_type: "application/json".to_string(),
        body: serde_json::to_vec(&response).unwrap_or_default(),
        extra_headers: Vec::new(),
    }
}

/// The spec's request-metadata headers, checked against the JSON-RPC body
/// before the request is routed at all - a mismatch must never reach a tool.
///
/// Header presence, duplicates, and character validity are checked whether
/// or not the body parses as JSON: a client that sends no `Mcp-Method` at
/// all must get the same 400 whether its body is `{"method":...}` or
/// garbage. Only the comparison *against* a body value needs that value, so
/// an unparseable body short-circuits after the header-only checks - the
/// parse error itself is reported by `handle_message` below, not duplicated
/// here.
fn validate_headers(req: &HttpRequest) -> Result<(), HttpResponse> {
    let parsed_body = serde_json::from_slice::<Value>(&req.body).ok();
    let id = parsed_body
        .as_ref()
        .and_then(|body| body.get("id").cloned());

    let header_method = checked_header(req, "Mcp-Method", id.clone())?
        .ok_or_else(|| header_mismatch(id.clone(), "Missing required header: Mcp-Method"))?;

    let Some(request) = parsed_body else {
        return Ok(());
    };

    let body_method = request.get("method").and_then(Value::as_str).unwrap_or("");
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
        let header_name = checked_header(req, "Mcp-Name", id.clone())?
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

/// Looks up `name` the way `header()` does, but - like `parse_content_length`
/// in `cli/http.rs` for a duplicated `Content-Length` - refuses to silently
/// pick a winner when the header repeats with disagreeing values. `Mcp-Method`
/// exists so an intermediary can route a request without parsing its body; if
/// a duplicate reaches this function, an intermediary reading the same
/// headers may have routed on a different value than `header()`'s
/// first-match would compare against, so this server and the router could
/// act on different values without either of them noticing. RFC 9110 5.3
/// still permits a header to repeat as long as every occurrence agrees, so
/// identical repeats stay legal.
///
/// Character validity (RFC 9110 5.5: visible ASCII, space, or tab) is
/// checked here too, scoped to just the headers this function is asked
/// about - `Mcp-Method` and `Mcp-Name` - rather than every header on the
/// request. This server does not read the others, so it has no business
/// rejecting `obs-text` bytes in them.
fn checked_header<'a>(
    req: &'a HttpRequest,
    name: &str,
    id: Option<Value>,
) -> Result<Option<&'a str>, HttpResponse> {
    let mut found: Option<&str> = None;
    for (key, value) in &req.headers {
        if !key.eq_ignore_ascii_case(name) {
            continue;
        }
        if !is_valid_header_value(value) {
            return Err(header_mismatch(
                id,
                format!("Header '{name}' contains invalid characters"),
            ));
        }
        if found.is_some_and(|first| first != value) {
            return Err(header_mismatch(
                id,
                format!("Conflicting '{name}' headers: request carries disagreeing values"),
            ));
        }
        found = Some(value.as_str());
    }
    Ok(found)
}

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

    /// `params.name` and the `Mcp-Name` header are made to agree so the only
    /// thing that can still fail this request is the disagreeing pair of
    /// `Mcp-Method` headers - otherwise a currently-missing `Mcp-Name` would
    /// also produce 400 and hide whether the duplicate-header defect was
    /// ever exercised.
    #[test]
    fn two_disagreeing_mcp_method_headers_are_rejected() {
        let server = memory_server();
        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": "list_tables", "arguments": {} },
        })
        .to_string();

        let req = HttpRequest {
            headers: vec![
                ("Mcp-Method".to_string(), "tools/call".to_string()),
                ("Mcp-Method".to_string(), "tools/list".to_string()),
                ("Mcp-Name".to_string(), "list_tables".to_string()),
            ],
            body: request_body.into_bytes(),
        };

        let response = http_response_for(&server, &req);

        assert_eq!(response.status, 400);
    }

    #[test]
    fn two_identical_mcp_method_headers_are_accepted() {
        let server = memory_server();
        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {},
        })
        .to_string();

        let req = HttpRequest {
            headers: vec![
                ("Mcp-Method".to_string(), "tools/list".to_string()),
                ("Mcp-Method".to_string(), "tools/list".to_string()),
            ],
            body: request_body.into_bytes(),
        };

        let response = http_response_for(&server, &req);

        assert_eq!(response.status, 200);
    }

    /// A malformed body must not make header validation a no-op: the missing
    /// `Mcp-Method` header is rejected the same way it would be with a valid
    /// body, instead of falling through to `handle_message` and coming back
    /// 200 with an embedded JSON-RPC parse error.
    #[test]
    fn an_unparseable_body_with_no_mcp_method_header_is_still_rejected() {
        let server = memory_server();
        let req = HttpRequest {
            headers: vec![],
            body: b"not json".to_vec(),
        };

        let response = http_response_for(&server, &req);

        assert_eq!(response.status, 400);
        let body: Value = serde_json::from_slice(&response.body).expect("body is valid JSON-RPC");
        assert_eq!(body["error"]["code"], -32020);
    }

    /// The character check exists to police the two headers this function
    /// actually reads, not every header on the request - `obs-text` bytes in
    /// a header nobody compares against the body are none of its business.
    #[test]
    fn an_unrelated_header_with_non_ascii_bytes_is_accepted() {
        let server = memory_server();
        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {},
        })
        .to_string();

        let req = HttpRequest {
            headers: vec![
                ("Mcp-Method".to_string(), "tools/list".to_string()),
                ("X-Trace".to_string(), "café".to_string()),
            ],
            body: request_body.into_bytes(),
        };

        let response = http_response_for(&server, &req);

        assert_eq!(response.status, 200);
    }

    #[test]
    fn invalid_characters_in_mcp_method_itself_are_still_rejected() {
        let server = memory_server();
        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list\u{0007}",
            "params": {},
        })
        .to_string();

        let req = HttpRequest {
            headers: vec![("Mcp-Method".to_string(), "tools/list\u{0007}".to_string())],
            body: request_body.into_bytes(),
        };

        let response = http_response_for(&server, &req);

        assert_eq!(response.status, 400);
        let body: Value = serde_json::from_slice(&response.body).expect("body is valid JSON-RPC");
        assert_eq!(body["error"]["code"], -32020);
    }

    /// The end-to-end 403 cases live in tests/mcp_http_transport.rs, which
    /// exercises the real listener with real request headers. These test the
    /// parser in isolation, which is where the substring-match trap actually
    /// lives - a full round trip through a spawned server would prove the
    /// same thing far more slowly.
    #[test]
    fn origin_host_parses_the_authority_out_of_scheme_and_port() {
        assert_eq!(origin_host("http://localhost:3000"), Some("localhost"));
        assert_eq!(origin_host("http://127.0.0.1:8080"), Some("127.0.0.1"));
        assert_eq!(origin_host("http://[::1]:9000"), Some("::1"));
        assert_eq!(
            origin_host("https://evil.example.com"),
            Some("evil.example.com")
        );
        assert_eq!(origin_host("null"), None);
    }

    #[test]
    fn a_hostname_that_merely_contains_a_loopback_name_is_not_loopback() {
        assert!(!origin_is_loopback("http://localhost.evil.com"));
        assert!(!origin_is_loopback("http://127.0.0.1.attacker.net"));
    }

    #[test]
    fn the_null_origin_is_not_loopback() {
        assert!(!origin_is_loopback("null"));
    }

    #[test]
    fn the_whole_127_block_is_loopback_not_just_127_0_0_1() {
        assert!(origin_is_loopback("http://127.0.0.1:1"));
        assert!(origin_is_loopback("http://127.0.0.2:1"));
        assert!(origin_is_loopback("http://127.1.1.1:1"));
    }

    #[test]
    fn a_request_with_a_forbidden_origin_is_rejected_before_the_body_is_even_read() {
        let server = memory_server();
        // A body that would fail every other check the server runs - proof
        // the origin check runs first and short-circuits them all.
        let req = HttpRequest {
            headers: vec![("Origin".to_string(), "http://evil.example.com".to_string())],
            body: b"not even json".to_vec(),
        };

        let response = http_response_for(&server, &req);

        assert_eq!(response.status, 403);
        let body: Value = serde_json::from_slice(&response.body).expect("body is valid JSON-RPC");
        assert_eq!(body["error"]["code"], -32600);
    }
}

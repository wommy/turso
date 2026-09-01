use super::protocol::{
    JsonRpcError, JsonRpcResponse, FORBIDDEN_ORIGIN, HEADER_MISMATCH, METHOD_NOT_FOUND,
};
use super::TursoMcpServer;
use crate::http::{format_http_response, parse_http_request, read_http_request, HttpResponse};
use anyhow::Result;
use base64::Engine;
use serde_json::Value;
use std::borrow::Cow;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// The MCP spec's single Streamable HTTP endpoint. Any other path answers
/// 404 regardless of method; GET and DELETE to this one answer 405 instead,
/// since this server implements neither the SSE stream a GET once opened nor
/// the session a DELETE once closed, for any era it speaks (see
/// `route_request`).
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

        let response = match route_request(&method, &path) {
            Route::Handle => http_response_for(self, &HttpRequest { headers, body }),
            Route::MethodNotAllowed => plain_text_response(405, b"Method Not Allowed"),
            Route::NotFound => plain_text_response(404, b"Not Found"),
        };

        stream.write_all(&format_http_response(&response))?;
        stream.flush()?;
        Ok(())
    }
}

/// Which of the three ways `handle_http_connection` can dispose of a
/// request without ever reaching `http_response_for`.
///
/// Kept as a pure function of method and path - no socket, no `TcpStream` -
/// so the routing rules (as opposed to what `http_response_for` does once
/// routed) can be asserted directly instead of only through a live listener.
#[derive(Debug, PartialEq, Eq)]
enum Route {
    Handle,
    MethodNotAllowed,
    NotFound,
}

fn route_request(method: &str, path: &str) -> Route {
    if path != ENDPOINT_PATH {
        return Route::NotFound;
    }
    match method {
        "POST" => Route::Handle,
        "GET" | "DELETE" => Route::MethodNotAllowed,
        _ => Route::NotFound,
    }
}

fn plain_text_response(status: u16, body: &'static [u8]) -> HttpResponse {
    HttpResponse {
        status,
        content_type: "text/plain".to_string(),
        body: body.to_vec(),
        extra_headers: Vec::new(),
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
    let status = if is_method_not_found(&body) { 404 } else { 200 };
    HttpResponse {
        status,
        content_type: "application/json".to_string(),
        body: body.into_bytes(),
        extra_headers: Vec::new(),
    }
}

/// The spec's `404` rule for an unimplemented RPC method (MUST, L271-273)
/// singles out one JSON-RPC error among everything `handle_message` can
/// return, so the status mapping has to look inside the response instead of
/// keying off success or failure alone.
///
/// This parses, once, the very string `handle_message` already handed back -
/// inspecting what we were given, not re-serializing our own output just to
/// re-parse it. The alternative was to have `handle_message` return a typed
/// outcome the transport could read without parsing at all; that would be
/// cleaner, but it means changing `cli/mcp/mod.rs`'s public shape, and that
/// module is layer C, open as PR #18 - a wider blast radius than this status-
/// code slice should carry.
fn is_method_not_found(body: &str) -> bool {
    let Ok(response) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    response
        .get("error")
        .and_then(|error| error.get("code"))
        .and_then(Value::as_i64)
        == Some(METHOD_NOT_FOUND as i64)
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
        let decoded_header_name = decode_base64_sentinel(header_name).ok_or_else(|| {
            header_mismatch(
                id.clone(),
                format!(
                    "Header mismatch: Mcp-Name header value '{header_name}' is not valid Base64"
                ),
            )
        })?;
        if decoded_header_name != body_name {
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

/// The spec's Base64 sentinel for `Mcp-Name` (MUST, L490-508): a value
/// outside the header-safe ASCII set is carried as `=?base64?{value}?=`, and
/// the server MUST decode it before comparing to the body (MUST, L501-504).
///
/// Returns the decoded value on a match, the value unchanged when it is not
/// the sentinel shape, and `None` when the markers are present but the
/// payload between them is not valid Base64 - a value the caller cannot
/// compare to anything, so `validate_headers` turns it into the same
/// `HeaderMismatch` any other non-matching value gets.
///
/// The markers "MUST appear exactly as shown (lowercase)" (L498-500), so
/// this checks for the lowercase prefix and suffix only: `=?BASE64?...?=`
/// does not match and falls through to the unchanged branch, comparing as a
/// literal string rather than being decoded.
///
/// The alphabet and padding aren't pinned by name in the spec text - "Base64
/// encoding of the UTF-8 representation" is the only description given. This
/// uses RFC 4648's standard, padded alphabet, what "Base64" means absent a
/// qualifier; the encoding examples in the spec (L515-518) are consistent
/// with it (e.g. `SGVsbG8sIOS4lueVjA==` keeps its `==` padding).
fn decode_base64_sentinel(value: &str) -> Option<Cow<'_, str>> {
    const PREFIX: &str = "=?base64?";
    const SUFFIX: &str = "?=";
    let Some(payload) = value
        .strip_prefix(PREFIX)
        .and_then(|rest| rest.strip_suffix(SUFFIX))
    else {
        return Some(Cow::Borrowed(value));
    };
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .ok()?;
    String::from_utf8(decoded).ok().map(Cow::Owned)
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

    /// The spec's MUST at L271-273: a method the server does not implement
    /// is a 404, not the 200-with-embedded-error every other JSON-RPC
    /// failure gets. `nonexistent/thing` reaches `handle_message` past every
    /// header check by naming itself consistently in both places, so the
    /// only thing left to produce the 404 is the method lookup itself.
    #[test]
    fn a_post_naming_an_unimplemented_method_returns_404_with_a_method_not_found_error() {
        let server = memory_server();
        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "nonexistent/thing",
            "params": {},
        })
        .to_string();

        let req = HttpRequest {
            headers: vec![("Mcp-Method".to_string(), "nonexistent/thing".to_string())],
            body: request_body.into_bytes(),
        };

        let response = http_response_for(&server, &req);

        assert_eq!(response.status, 404);
        let body: Value = serde_json::from_slice(&response.body).expect("body is valid JSON-RPC");
        assert_eq!(body["error"]["code"], -32601);
    }

    /// `route_request` is the pure decision `handle_http_connection` acts
    /// on, asserted directly rather than through a live listener - see its
    /// doc comment for why.
    #[test]
    fn get_to_the_mcp_endpoint_is_method_not_allowed() {
        assert_eq!(route_request("GET", ENDPOINT_PATH), Route::MethodNotAllowed);
    }

    #[test]
    fn delete_to_the_mcp_endpoint_is_method_not_allowed() {
        assert_eq!(
            route_request("DELETE", ENDPOINT_PATH),
            Route::MethodNotAllowed
        );
    }

    #[test]
    fn post_to_an_unknown_path_is_still_plain_not_found() {
        assert_eq!(route_request("POST", "/nowhere"), Route::NotFound);
    }

    #[test]
    fn get_to_an_unknown_path_is_plain_not_found_not_method_not_allowed() {
        assert_eq!(route_request("GET", "/nowhere"), Route::NotFound);
    }

    #[test]
    fn post_to_the_mcp_endpoint_is_handled() {
        assert_eq!(route_request("POST", ENDPOINT_PATH), Route::Handle);
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

    /// The spec's Base64 sentinel for `Mcp-Name` (MUST, L490-508): a name
    /// outside the header-safe ASCII set is carried as
    /// `=?base64?{Base64EncodedValue}?=` and the server MUST decode it
    /// before comparing to `params.name` (MUST, L501-504).
    #[test]
    fn a_base64_encoded_mcp_name_matching_the_body_is_accepted() {
        use base64::Engine;
        let server = memory_server();
        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": "café", "arguments": {} },
        })
        .to_string();
        let encoded = base64::engine::general_purpose::STANDARD.encode("café");

        let req = HttpRequest {
            headers: vec![
                ("Mcp-Method".to_string(), "tools/call".to_string()),
                ("Mcp-Name".to_string(), format!("=?base64?{encoded}?=")),
            ],
            body: request_body.into_bytes(),
        };

        let response = http_response_for(&server, &req);

        assert_eq!(response.status, 200);
    }

    #[test]
    fn a_base64_encoded_mcp_name_disagreeing_with_the_body_is_rejected() {
        use base64::Engine;
        let server = memory_server();
        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": "something_else", "arguments": {} },
        })
        .to_string();
        let encoded = base64::engine::general_purpose::STANDARD.encode("café");

        let req = HttpRequest {
            headers: vec![
                ("Mcp-Method".to_string(), "tools/call".to_string()),
                ("Mcp-Name".to_string(), format!("=?base64?{encoded}?=")),
            ],
            body: request_body.into_bytes(),
        };

        let response = http_response_for(&server, &req);

        assert_eq!(response.status, 400);
        let body: Value = serde_json::from_slice(&response.body).expect("body is valid JSON-RPC");
        assert_eq!(body["error"]["code"], -32020);
    }

    #[test]
    fn a_plain_unencoded_mcp_name_matching_the_body_is_still_accepted() {
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
                ("Mcp-Name".to_string(), "list_tables".to_string()),
            ],
            body: request_body.into_bytes(),
        };

        let response = http_response_for(&server, &req);

        assert_eq!(response.status, 200);
    }

    /// The markers are present but the payload between them is not valid
    /// Base64 - a value that cannot be decoded cannot be compared to
    /// anything, so it is treated the same as any other value that fails to
    /// match the body (the spec's failure list, L625-630, does not name this
    /// case separately).
    #[test]
    fn a_base64_marked_mcp_name_with_an_undecodable_payload_is_rejected() {
        let server = memory_server();
        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": "café", "arguments": {} },
        })
        .to_string();

        let req = HttpRequest {
            headers: vec![
                ("Mcp-Method".to_string(), "tools/call".to_string()),
                (
                    "Mcp-Name".to_string(),
                    "=?base64?not-valid-base64!!?=".to_string(),
                ),
            ],
            body: request_body.into_bytes(),
        };

        let response = http_response_for(&server, &req);

        assert_eq!(response.status, 400);
        let body: Value = serde_json::from_slice(&response.body).expect("body is valid JSON-RPC");
        assert_eq!(body["error"]["code"], -32020);
    }

    /// The spec's markers "MUST appear exactly as shown (lowercase)"
    /// (L498-500). `=?BASE64?...?=` does not match that, so it is a literal
    /// header value, not a sentinel to decode - proven here by pairing it
    /// with a body value the *decoded* payload would equal, which must still
    /// fail to match since no decoding happens.
    #[test]
    fn an_uppercase_base64_marker_is_treated_as_a_literal_value_not_decoded() {
        use base64::Engine;
        let server = memory_server();
        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": "café", "arguments": {} },
        })
        .to_string();
        let encoded = base64::engine::general_purpose::STANDARD.encode("café");

        let req = HttpRequest {
            headers: vec![
                ("Mcp-Method".to_string(), "tools/call".to_string()),
                ("Mcp-Name".to_string(), format!("=?BASE64?{encoded}?=")),
            ],
            body: request_body.into_bytes(),
        };

        let response = http_response_for(&server, &req);

        assert_eq!(response.status, 400);
        let body: Value = serde_json::from_slice(&response.body).expect("body is valid JSON-RPC");
        assert_eq!(body["error"]["code"], -32020);
    }

    /// A name that merely contains the sentinel pattern in the middle -
    /// rather than starting with the prefix and ending with the suffix - is
    /// a literal value. Pairing it with an identical body value proves it
    /// round-trips unchanged rather than being mistaken for an encoded one.
    #[test]
    fn a_name_that_only_contains_the_sentinel_pattern_mid_string_is_treated_as_literal() {
        let server = memory_server();
        let literal_name = "foo=?base64?Zm9v?=bar";
        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": literal_name, "arguments": {} },
        })
        .to_string();

        let req = HttpRequest {
            headers: vec![
                ("Mcp-Method".to_string(), "tools/call".to_string()),
                ("Mcp-Name".to_string(), literal_name.to_string()),
            ],
            body: request_body.into_bytes(),
        };

        let response = http_response_for(&server, &req);

        assert_eq!(response.status, 200);
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

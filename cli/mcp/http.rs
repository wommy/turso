use super::protocol::{
    JsonRpcError, JsonRpcRequest, JsonRpcResponse, FORBIDDEN_ORIGIN, HEADER_MISMATCH,
    INVALID_PARAMS, INVALID_REQUEST, LENGTH_REQUIRED, METHOD_NOT_FOUND, PARSE_ERROR,
    UNSUPPORTED_PROTOCOL_VERSION,
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
    if has_chunked_transfer_encoding(req) {
        return length_required();
    }
    // Parsed once and shared between the header check below and the era
    // decision it depends on, rather than each re-deriving its own view of
    // the body.
    let request = serde_json::from_slice::<JsonRpcRequest>(&req.body).ok();
    if let Some(request) = &request {
        if requires_header_validation(request) {
            if let Err(response) = validate_headers(req, request) {
                return response;
            }
        }
    }
    let body = server
        .handle_message(&String::from_utf8_lossy(&req.body))
        .unwrap_or_default();
    let status = http_status_for(&body);
    HttpResponse {
        status,
        content_type: "application/json".to_string(),
        body: body.into_bytes(),
        extra_headers: Vec::new(),
    }
}

/// The status the spec ties to the JSON-RPC response `handle_message` just
/// produced. A success (no `error`) is always `200`. Among failures, several
/// codes each get a status of their own rather than the plain `200` a
/// JSON-RPC failure otherwise carries: `METHOD_NOT_FOUND` is `404` (MUST,
/// `streamable-http.mdx` L271-273); `INVALID_PARAMS` (MUST, `index.mdx`
/// L380-382), `UNSUPPORTED_PROTOCOL_VERSION` (MUST, `streamable-http.mdx`
/// L264-267 and `schema.mdx` L376), `HEADER_MISMATCH` (MUST,
/// `streamable-http.mdx` L596-598), `INVALID_REQUEST`, and `PARSE_ERROR` are
/// all `400`. `HEADER_MISMATCH` never actually reaches this function -
/// `validate_headers` answers it before `handle_message` is even called -
/// but the rule is listed here too so this mapping reads as the complete
/// table rather than one case short of it. Everything else - any other code,
/// or no error at all - keeps the `200` a JSON-RPC response gets by default.
///
/// This parses, once, the very string `handle_message` already handed back -
/// inspecting what we were given, not re-serializing our own output just to
/// re-parse it. The alternative was to have `handle_message` return a typed
/// outcome the transport could read without parsing at all; that would be
/// cleaner, but it means changing `cli/mcp/mod.rs`'s public shape, and that
/// module is layer C, open as PR #18 - a wider blast radius than this status-
/// code slice should carry.
///
/// A notification has no response at all: `handle_message` gives back
/// `None`, which `http_response_for` turns into an empty string before this
/// function ever sees it - and, per `requires_header_validation`, never
/// reaches `validate_headers` either. An empty body only ever means a
/// notification was accepted, so it gets the `202 Accepted` the spec
/// requires for that case (MUST, `streamable-http.mdx` L86-88), not the
/// `200` an ordinary JSON-RPC response gets.
fn http_status_for(body: &str) -> u16 {
    if body.is_empty() {
        return 202;
    }
    let Ok(response) = serde_json::from_str::<Value>(body) else {
        return 200;
    };
    let Some(code) = response
        .get("error")
        .and_then(|error| error.get("code"))
        .and_then(Value::as_i64)
    else {
        return 200;
    };
    if code == METHOD_NOT_FOUND as i64 {
        404
    } else if code == INVALID_PARAMS as i64
        || code == UNSUPPORTED_PROTOCOL_VERSION as i64
        || code == HEADER_MISMATCH as i64
        || code == INVALID_REQUEST as i64
        || code == PARSE_ERROR as i64
    {
        400
    } else {
        200
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

/// `Transfer-Encoding` can list more than one coding (`gzip, chunked`), and
/// RFC 9112 requires `chunked` to be the last one when present at all, so a
/// bare substring check on the raw header would work here too - but checking
/// each comma-separated token instead means a coding that merely contains the
/// word, or a stray comma, cannot produce a false match.
fn has_chunked_transfer_encoding(req: &HttpRequest) -> bool {
    req.header("Transfer-Encoding").is_some_and(|value| {
        value
            .split(',')
            .any(|coding| coding.trim().eq_ignore_ascii_case("chunked"))
    })
}

/// ADR 0004: this server understands `Content-Length` and nothing else, so a
/// chunked body - which `read_http_request` cannot frame, having already
/// looked for `Content-Length` and found none by the time this runs - is
/// refused outright rather than misread as empty. The id is always null for
/// the same reason `forbidden_origin` uses one: the body cannot be trusted
/// enough to parse just to echo its id back.
fn length_required() -> HttpResponse {
    let error = JsonRpcError::new(
        LENGTH_REQUIRED,
        "Transfer-Encoding: chunked is not supported; send Content-Length instead".to_string(),
    );
    let response = JsonRpcResponse::failure(None, error);
    HttpResponse {
        status: 411,
        content_type: "application/json".to_string(),
        body: serde_json::to_vec(&response).unwrap_or_default(),
        extra_headers: Vec::new(),
    }
}

/// Whether `validate_headers` applies to this request at all. Its headers -
/// `Mcp-Method`, `Mcp-Name` - are `2026-07-28` inventions (MUST,
/// `streamable-http.mdx` L253-297), and a dual-era server picks its behavior
/// from how the client opens (MUST, `basic/versioning.mdx` L175-180): a
/// request carrying modern per-request `_meta` is served under this
/// revision, everything else - including an `initialize` handshake - under
/// legacy semantics, which defines no header requirements at all. So the
/// header check belongs on the modern branch of that fork, not ahead of it.
///
/// `declares_v2` (`protocol.rs`) is the version test already used to scope
/// the stdio-side v2-only checks, reused rather than re-derived: presence of
/// `_meta` is not itself the modern signal - the official client sends an
/// empty `_meta: {}` on every request even while handshaking at an earlier
/// revision - only the `protocolVersion` named inside it is.
///
/// A request with no `id` is a notification, which the spec exempts from
/// header requirements outright regardless of era (`streamable-http.mdx`
/// L101-103) - checked first so that stays true whatever `declares_v2` says.
/// `Origin` is unaffected by any of this - it is checked in
/// `http_response_for` before a request even reaches this function, because
/// it is the DNS-rebinding defense (MUST, L57-63) rather than a
/// request-metadata header, and applies regardless of era or `id`.
///
/// A body that fails to parse into a `JsonRpcRequest` at all declares
/// nothing, modern or otherwise; `http_response_for` already treats that the
/// same as "does not require validation" by never calling this function for
/// it, and `handle_message` reports the parse failure itself.
fn requires_header_validation(request: &JsonRpcRequest) -> bool {
    request.id.is_some() && request.declares_v2()
}

/// The spec's request-metadata headers, checked against the JSON-RPC body
/// before the request is routed at all - a mismatch must never reach a tool.
/// Only reached once `requires_header_validation` has confirmed the request
/// is modern, so `request` is always the same parse `http_response_for`
/// already made; this does not re-derive it from `req.body`.
fn validate_headers(req: &HttpRequest, request: &JsonRpcRequest) -> Result<(), HttpResponse> {
    let id = request.id.clone();
    let body_method = request.method.as_str();

    let header_method = checked_header(req, "Mcp-Method", id.clone())?
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
            .params
            .as_ref()
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
    use super::super::protocol::{LEGACY_DEFAULT, PROTOCOL_V2};
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

    /// The `_meta` shape that makes a request modern - `protocolVersion` is
    /// the one field `requires_header_validation` actually keys on, but
    /// `clientCapabilities` is included too so a request built with this
    /// helper reaches the tool dispatch these tests care about instead of
    /// being turned back earlier by `check_client_capabilities`
    /// (`protocol.rs`), a body-level v2 requirement unrelated to what these
    /// tests exercise. A test that wants to exercise the legacy branch
    /// instead omits `_meta` altogether, or - to prove the trap in #44 -
    /// uses an empty `_meta: {}` on purpose.
    fn v2_meta() -> Value {
        json!({
            "io.modelcontextprotocol/protocolVersion": PROTOCOL_V2,
            "io.modelcontextprotocol/clientCapabilities": {},
        })
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

    /// The spec's MUST at `streamable-http.mdx` L264-267 and `schema.mdx`
    /// L376: a client naming a protocol version we do not speak is a `400`,
    /// carrying `UNSUPPORTED_PROTOCOL_VERSION` in the body.
    #[test]
    fn a_request_naming_an_unsupported_protocol_version_returns_400() {
        let server = memory_server();
        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": { "_meta": { "io.modelcontextprotocol/protocolVersion": "1999-01-01" } },
        })
        .to_string();

        let req = HttpRequest {
            headers: vec![("Mcp-Method".to_string(), "tools/list".to_string())],
            body: request_body.into_bytes(),
        };

        let response = http_response_for(&server, &req);

        assert_eq!(response.status, 400);
        let body: Value = serde_json::from_slice(&response.body).expect("body is valid JSON-RPC");
        assert_eq!(body["error"]["code"], UNSUPPORTED_PROTOCOL_VERSION);
    }

    /// The spec's MUST at `index.mdx` L380-382: a v2 `tools/call` that omits
    /// the client-capabilities `_meta` field it must carry is a `400`,
    /// carrying `INVALID_PARAMS` in the body.
    #[test]
    fn a_v2_tools_call_missing_client_capabilities_returns_400() {
        let server = memory_server();
        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "list_tables",
                "arguments": {},
                "_meta": { "io.modelcontextprotocol/protocolVersion": PROTOCOL_V2 }
            },
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

        assert_eq!(response.status, 400);
        let body: Value = serde_json::from_slice(&response.body).expect("body is valid JSON-RPC");
        assert_eq!(body["error"]["code"], INVALID_PARAMS);
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

    /// The one substitution `basic/versioning.mdx` L175-180 asks for on top
    /// of the pre-fix version of this test: a body that declares
    /// `2026-07-28` in `_meta`, so the request is actually modern and this
    /// still exercises `validate_headers` at all - see
    /// `a_legacy_request_with_a_disagreeing_mcp_method_header_is_still_accepted`
    /// below for the direction that fails without the fix.
    #[test]
    fn an_mcp_method_header_that_disagrees_with_the_body_method_is_rejected() {
        let server = memory_server();
        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": "list_tables", "arguments": {}, "_meta": v2_meta() },
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
    fn a_modern_request_missing_the_mcp_method_header_entirely_is_rejected() {
        let server = memory_server();
        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": { "_meta": v2_meta() },
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
            "params": { "name": "list_tables", "arguments": {}, "_meta": v2_meta() },
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
            "params": { "name": "bar", "arguments": {}, "_meta": v2_meta() },
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
            "params": { "name": "list_tables\u{0007}", "arguments": {}, "_meta": v2_meta() },
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
            "params": { "name": "list_tables", "arguments": {}, "_meta": v2_meta() },
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
            "params": { "_meta": v2_meta() },
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

    /// Before the dual-era fix, header validation ran ahead of any era
    /// check, so a body that fails to parse at all was still held to the
    /// missing-`Mcp-Method` rule and answered `HEADER_MISMATCH`. A body that
    /// cannot even be read as a `JsonRpcRequest` cannot declare `_meta`,
    /// modern or otherwise, so `requires_header_validation` never runs on
    /// it now, and the request falls through to `handle_message`, which
    /// reports the real problem: the body does not parse, `PARSE_ERROR`,
    /// still a `400`.
    #[test]
    fn an_unparseable_body_answers_parse_error_not_header_mismatch() {
        let server = memory_server();
        let req = HttpRequest {
            headers: vec![],
            body: b"not json".to_vec(),
        };

        let response = http_response_for(&server, &req);

        assert_eq!(response.status, 400);
        let body: Value = serde_json::from_slice(&response.body).expect("body is valid JSON-RPC");
        assert_eq!(body["error"]["code"], PARSE_ERROR);
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
            "params": { "_meta": v2_meta() },
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
            "params": { "_meta": v2_meta() },
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
            "params": { "name": "café", "arguments": {}, "_meta": v2_meta() },
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

        // "café" is not a real tool, so this still fails past the header
        // check - just as `INVALID_PARAMS`, `handle_call_tool`'s own answer
        // for an unknown tool name, not as `HEADER_MISMATCH`. That is the
        // fact this test is actually about: proving the decoded header
        // reached `handle_message` at all, which a wrong decode would have
        // stopped at `validate_headers` with `HEADER_MISMATCH` instead.
        let body: Value = serde_json::from_slice(&response.body).expect("body is valid JSON-RPC");
        assert_ne!(body["error"]["code"], HEADER_MISMATCH);
    }

    #[test]
    fn a_base64_encoded_mcp_name_disagreeing_with_the_body_is_rejected() {
        use base64::Engine;
        let server = memory_server();
        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": "something_else", "arguments": {}, "_meta": v2_meta() },
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
            "params": { "name": "list_tables", "arguments": {}, "_meta": v2_meta() },
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
            "params": { "name": "café", "arguments": {}, "_meta": v2_meta() },
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
            "params": { "name": "café", "arguments": {}, "_meta": v2_meta() },
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
            "params": { "name": literal_name, "arguments": {}, "_meta": v2_meta() },
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

        // "foo=?base64?Zm9v?=bar" is not a real tool, so this still fails
        // past the header check - just as `INVALID_PARAMS`, `handle_call_
        // tool`'s own answer for an unknown tool name, not as
        // `HEADER_MISMATCH`. That is the fact this test is actually about:
        // proving the literal header reached `handle_message` at all, which
        // a wrong decode (mistaking this for the sentinel shape) would have
        // stopped at `validate_headers` with `HEADER_MISMATCH` instead.
        let body: Value = serde_json::from_slice(&response.body).expect("body is valid JSON-RPC");
        assert_ne!(body["error"]["code"], HEADER_MISMATCH);
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

    /// The spec's MUST at `streamable-http.mdx` L86-88: a notification the
    /// server accepts gets `202 Accepted` with no body at all - not the
    /// `200` an ordinary JSON-RPC response gets. Headers agree with the
    /// body here so header validation, unaffected by this fix, cannot be
    /// what produces the status seen - see the next test for that.
    #[test]
    fn a_notification_returns_202_with_an_empty_body() {
        let server = memory_server();
        let request_body = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {},
        })
        .to_string();

        let req = HttpRequest {
            headers: vec![(
                "Mcp-Method".to_string(),
                "notifications/initialized".to_string(),
            )],
            body: request_body.into_bytes(),
        };

        let response = http_response_for(&server, &req);

        assert_eq!(response.status, 202);
        assert!(response.body.is_empty());
    }

    /// The spec's note at L101-103: header requirements for notification
    /// POSTs are not defined by this revision, so a disagreeing `Mcp-Method`
    /// must not stop a notification from being accepted. Against unchanged
    /// code this same disagreement produces the 400/`HEADER_MISMATCH` that
    /// `an_mcp_method_header_that_disagrees_with_the_body_method_is_rejected`
    /// asserts for a request that carries an `id`.
    #[test]
    fn a_notification_with_a_disagreeing_mcp_method_header_is_still_accepted() {
        let server = memory_server();
        let request_body = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {},
        })
        .to_string();

        let req = HttpRequest {
            headers: vec![("Mcp-Method".to_string(), "tools/list".to_string())],
            body: request_body.into_bytes(),
        };

        let response = http_response_for(&server, &req);

        assert_eq!(response.status, 202);
        assert!(response.body.is_empty());
    }

    /// The direction that stops the previous test's fix going too far: the
    /// `Origin` check is the DNS-rebinding defense (MUST, L57-63), not a
    /// request-metadata header, and it has nothing to do with whether the
    /// body carries an `id`. A notification must not get a pass on it just
    /// because its headers are otherwise unvalidated.
    #[test]
    fn a_notification_with_a_forbidden_origin_is_still_rejected() {
        let server = memory_server();
        let request_body = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {},
        })
        .to_string();

        let req = HttpRequest {
            headers: vec![("Origin".to_string(), "http://evil.example.com".to_string())],
            body: request_body.into_bytes(),
        };

        let response = http_response_for(&server, &req);

        assert_eq!(response.status, 403);
        let body: Value = serde_json::from_slice(&response.body).expect("body is valid JSON-RPC");
        assert_eq!(body["error"]["code"], -32600);
    }

    #[test]
    fn a_bare_chunked_transfer_encoding_is_detected() {
        let req = HttpRequest {
            headers: vec![("Transfer-Encoding".to_string(), "chunked".to_string())],
            body: Vec::new(),
        };
        assert!(has_chunked_transfer_encoding(&req));
    }

    #[test]
    fn chunked_listed_alongside_another_coding_is_still_detected() {
        let req = HttpRequest {
            headers: vec![("Transfer-Encoding".to_string(), "gzip, chunked".to_string())],
            body: Vec::new(),
        };
        assert!(has_chunked_transfer_encoding(&req));
    }

    /// The other direction of the token-split logic: a coding that is not
    /// `chunked` - `gzip` alone, say - must not trip a check meant only for
    /// the one framing this server cannot read.
    #[test]
    fn a_transfer_encoding_that_is_not_chunked_is_not_detected() {
        let req = HttpRequest {
            headers: vec![("Transfer-Encoding".to_string(), "gzip".to_string())],
            body: Vec::new(),
        };
        assert!(!has_chunked_transfer_encoding(&req));
    }

    #[test]
    fn no_transfer_encoding_header_is_not_detected() {
        let req = HttpRequest {
            headers: Vec::new(),
            body: Vec::new(),
        };
        assert!(!has_chunked_transfer_encoding(&req));
    }

    /// ADR 0004: a chunked body is refused outright. A body and header that
    /// would pass every other check - a well-formed `tools/list` request
    /// with a matching `Mcp-Method` - proves the refusal fires regardless,
    /// the same way `a_request_with_a_forbidden_origin_is_rejected_before_
    /// the_body_is_even_read` proves it for the Origin guard. Against
    /// unchanged code this same request returns 200, per the worktree
    /// comparison recorded in the commit that added this test.
    #[test]
    fn a_request_with_a_chunked_transfer_encoding_is_refused_with_411() {
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
                ("Transfer-Encoding".to_string(), "chunked".to_string()),
            ],
            body: request_body.into_bytes(),
        };

        let response = http_response_for(&server, &req);

        assert_eq!(response.status, 411);
        let body: Value = serde_json::from_slice(&response.body).expect("body is valid JSON-RPC");
        assert_eq!(body["error"]["code"], -32600);
    }

    /// Mirrors `a_notification_with_a_forbidden_origin_is_still_rejected`:
    /// framing applies to every request whether or not it carries an `id`,
    /// unlike the request-metadata headers a notification gets a pass on.
    #[test]
    fn a_notification_with_a_chunked_transfer_encoding_is_still_refused() {
        let server = memory_server();
        let request_body = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {},
        })
        .to_string();

        let req = HttpRequest {
            headers: vec![("Transfer-Encoding".to_string(), "chunked".to_string())],
            body: request_body.into_bytes(),
        };

        let response = http_response_for(&server, &req);

        assert_eq!(response.status, 411);
    }

    /// #44, reproduced byte-for-byte: the official Python SDK's opening
    /// `initialize`, captured with a header sink - `accept` and
    /// `content-type` only, none of the `2026-07-28` routing headers, and an
    /// empty `_meta` the client sends on every request even while
    /// handshaking at `2025-11-25`. A dual-era server picks legacy semantics
    /// for an `initialize` (MUST, `basic/versioning.mdx` L175-180), which
    /// defines no header requirements at all - this must be served, not
    /// turned back for headers a legacy client has no way to send.
    #[test]
    fn a_legacy_initialize_with_only_the_captured_headers_is_served() {
        let server = memory_server();
        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "mcp", "version": "0.1.0" },
                "_meta": {},
            },
        })
        .to_string();

        let req = HttpRequest {
            headers: vec![
                (
                    "accept".to_string(),
                    "application/json, text/event-stream".to_string(),
                ),
                ("content-type".to_string(), "application/json".to_string()),
            ],
            body: request_body.into_bytes(),
        };

        let response = http_response_for(&server, &req);

        assert_eq!(response.status, 200);
        let body: Value = serde_json::from_slice(&response.body).expect("body is valid JSON-RPC");
        assert!(body["error"].is_null(), "{body}");
        // `2025-11-25` is not in `SUPPORTED_VERSIONS` yet (#43, tracked
        // separately), so `handle_initialize` answers with `LEGACY_DEFAULT`
        // rather than echoing the asked-for version - this test is only
        // about the request being served at all, not about which legacy
        // version it is served as.
        assert_eq!(body["result"]["protocolVersion"], LEGACY_DEFAULT);
    }

    /// The trap #44 warns about, isolated from `initialize` so it is clear
    /// this is about `_meta` and not about the method: the official client
    /// sends an empty `_meta: {}` on every request, including ones after the
    /// handshake, so presence of the key cannot be what marks a request
    /// modern - only the `protocolVersion` named inside it can
    /// (`declares_v2`, `protocol.rs`). A `tools/call` with an empty `_meta`
    /// and none of the routing headers must still be served; if presence of
    /// `_meta` alone were read as "modern", this would wrongly demand
    /// `Mcp-Method`/`Mcp-Name` and fail with `HEADER_MISMATCH` instead.
    #[test]
    fn an_empty_meta_object_is_treated_as_legacy_not_modern() {
        let server = memory_server();
        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": "list_tables", "arguments": {}, "_meta": {} },
        })
        .to_string();

        let req = HttpRequest {
            headers: vec![],
            body: request_body.into_bytes(),
        };

        let response = http_response_for(&server, &req);

        assert_eq!(response.status, 200, "{:?}", response.body);
        let body: Value = serde_json::from_slice(&response.body).expect("body is valid JSON-RPC");
        assert_eq!(body["result"]["isError"], false, "{body}");
    }

    /// The false-positive direction ADR 0005 asks for, and the bug in #44
    /// stated directly: a legacy request (no `_meta` declaring `2026-07-28`)
    /// whose `Mcp-Method` header disagrees with the body - or is missing
    /// outright - must still be served, because that header is a
    /// modern-only invention this client has no reason to send correctly,
    /// or to send at all. Against the pre-fix code, both of these were
    /// refused with `HEADER_MISMATCH`.
    #[test]
    fn a_legacy_request_with_a_disagreeing_mcp_method_header_is_still_accepted() {
        let server = memory_server();
        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {},
        })
        .to_string();

        let disagreeing = HttpRequest {
            headers: vec![("Mcp-Method".to_string(), "tools/call".to_string())],
            body: request_body.clone().into_bytes(),
        };
        let response = http_response_for(&server, &disagreeing);
        assert_eq!(response.status, 200, "{:?}", response.body);

        let missing = HttpRequest {
            headers: vec![],
            body: request_body.into_bytes(),
        };
        let response = http_response_for(&server, &missing);
        assert_eq!(response.status, 200, "{:?}", response.body);
    }
}

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Owns a spawned `tursodb` child and kills it on drop, so a panic anywhere
/// after this returns - most notably one of `send_http_request`'s
/// `.expect()` calls - still frees the port instead of leaving an orphan
/// bound to it. (`postgres/cli/tests/tursopg.rs` kills its child by hand
/// with no such guard and can leak the same way; this test does not repeat
/// that.)
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.0.kill().ok();
        self.0.wait().ok();
    }
}

/// Reserves a port by binding it in this process first, so two tests
/// starting at once cannot compute the same port and race for it. Retries
/// the whole reserve-and-spawn if the child loses the bind anyway.
fn start_mcp_http_server() -> (ChildGuard, u16) {
    for _ in 0..10 {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let addr = format!("127.0.0.1:{port}");
        let mut child = Command::new(env!("CARGO_BIN_EXE_tursodb"))
            .arg(":memory:")
            .arg("--mcp-http")
            .arg(&addr)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to start tursodb");

        for _ in 0..50 {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if TcpStream::connect(&addr).is_ok() && child.try_wait().unwrap().is_none() {
                return (ChildGuard(child), port);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        child.kill().ok();
        child.wait().ok();
    }
    panic!("tursodb --mcp-http did not start");
}

/// Sends a raw HTTP request and reads the response until the server closes
/// the connection (every response carries `Connection: close`).
fn send_http_request(port: u16, request: &str) -> String {
    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream.write_all(request.as_bytes()).expect("write request");

    let mut response = Vec::new();
    stream.read_to_end(&mut response).expect("read response");
    String::from_utf8_lossy(&response).into_owned()
}

fn status_code(response: &str) -> &str {
    response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .expect("response has a status line")
}

fn body(response: &str) -> &str {
    response
        .split("\r\n\r\n")
        .nth(1)
        .expect("response has a body")
}

#[test]
fn a_post_of_tools_list_returns_200_with_a_non_empty_tools_array() {
    let (_child, port) = start_mcp_http_server();

    let payload = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#;
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nMcp-Method: tools/list\r\nContent-Length: {}\r\n\r\n{}",
        payload.len(),
        payload
    );

    let response = send_http_request(port, &request);

    assert_eq!(status_code(&response), "200", "response: {response}");

    let json: serde_json::Value =
        serde_json::from_str(body(&response)).expect("body is valid JSON-RPC");
    let tools = json["result"]["tools"]
        .as_array()
        .expect("result carries a tools array");
    assert!(!tools.is_empty(), "tools array must not be empty");
}

#[test]
fn a_request_to_an_unknown_path_returns_404() {
    let (_child, port) = start_mcp_http_server();

    let request = "GET /unknown HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
    let response = send_http_request(port, request);

    assert_eq!(status_code(&response), "404", "response: {response}");
}

/// A wildcard `Access-Control-Allow-Origin` hands a browser permission to
/// read the response regardless of Origin, which would defeat the Origin
/// check the MCP spec requires (a DNS-rebinding defense). The MCP transport
/// must not emit any CORS header at all.
#[test]
fn responses_carry_no_cors_headers() {
    let (_child, port) = start_mcp_http_server();

    let payload = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#;
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nMcp-Method: tools/list\r\nContent-Length: {}\r\n\r\n{}",
        payload.len(),
        payload
    );

    let response = send_http_request(port, &request);

    assert_eq!(status_code(&response), "200", "response: {response}");
    assert!(
        !response.contains("Access-Control-"),
        "MCP response must carry no Access-Control-* header: {response}"
    );
}

/// A `tools/list` request, with an optional `Origin` header - the one piece
/// every case below varies.
fn tools_list_request(origin: Option<&str>) -> String {
    let payload = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#;
    let origin_line = origin
        .map(|value| format!("Origin: {value}\r\n"))
        .unwrap_or_default();
    format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nMcp-Method: tools/list\r\n{origin_line}Content-Length: {}\r\n\r\n{}",
        payload.len(),
        payload
    )
}

/// One server, every documented case sent to it in turn as a separate
/// connection - not one spawned `tursodb` process per case. A dozen
/// concurrently-spawned server processes is what made this file flaky under
/// full parallelism; a shared server for the whole origin policy removes
/// that without losing per-case coverage or messages.
#[test]
fn origin_validation_matches_the_documented_loopback_policy() {
    let (_child, port) = start_mcp_http_server();

    let cases: &[(&str, Option<&str>, &str)] = &[
        ("no Origin header at all is allowed", None, "200"),
        (
            "a non-loopback origin is refused",
            Some("http://evil.example.com"),
            "403",
        ),
        (
            "localhost, on any port, is loopback",
            Some("http://localhost:3000"),
            "200",
        ),
        (
            "127.0.0.1 is loopback",
            Some("http://127.0.0.1:8080"),
            "200",
        ),
        ("IPv6 ::1 is loopback", Some("http://[::1]:9000"), "200"),
        // The whole 127.0.0.0/8 block is loopback to the kernel, not just
        // 127.0.0.1 - Ipv4Addr::is_loopback is the standard's own
        // definition, and nothing about the DNS-rebinding concern this
        // check exists for narrows it further: a page cannot make a
        // browser reach 127.0.0.2 any more than it can reach 127.0.0.1.
        (
            "127.0.0.2, elsewhere in the loopback block, is loopback too",
            Some("http://127.0.0.2:8080"),
            "200",
        ),
        // A naive contains("localhost") or contains("127.0.0.1") would
        // accept both of these - the host is parsed out of the authority
        // and compared whole, so an attacker-controlled suffix cannot ride
        // along on a substring match.
        (
            "a hostname merely suffixed with localhost is refused",
            Some("http://localhost.evil.com"),
            "403",
        ),
        (
            "a hostname merely suffixed with a loopback address is refused",
            Some("http://127.0.0.1.attacker.net"),
            "403",
        ),
        // Origin: null is what a sandboxed iframe or a file:// page sends.
        // It names no host at all, so it is not loopback.
        ("the null origin is refused", Some("null"), "403"),
    ];

    for (description, origin, expected_status) in cases {
        let response = send_http_request(port, &tools_list_request(*origin));
        assert_eq!(
            status_code(&response),
            *expected_status,
            "{description} (Origin: {origin:?}) -> response: {response}"
        );
    }
}

/// `handle_http_connection` used to route the request before validating
/// `Origin` at all, so `Origin` was only ever checked inside the `POST` arm -
/// a cross-origin `GET`/`DELETE` fell through to the router's own
/// `405`/`404` instead of the `403` the DNS-rebinding defense requires.
/// A real listener is the only way to prove this: the unit tests in
/// `cli/mcp/http.rs` call `http_response_for` directly, which only the
/// `POST` route ever reaches.
#[test]
fn a_cross_origin_get_to_the_mcp_endpoint_is_forbidden_not_method_not_allowed() {
    let (_child, port) = start_mcp_http_server();

    let request = "GET /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: http://evil.example.com\r\n\r\n";
    let response = send_http_request(port, request);

    assert_eq!(status_code(&response), "403", "response: {response}");
}

/// The accept direction for the guard above (ADR 0005): a `GET` to `/mcp`
/// whose `Origin` is loopback - or has none at all - must still get the
/// plain `405` `route_request` already answers with, not be swept up by the
/// new `Origin` check firing early.
#[test]
fn a_loopback_get_to_the_mcp_endpoint_is_still_method_not_allowed() {
    let (_child, port) = start_mcp_http_server();

    let with_origin =
        "GET /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: http://localhost:3000\r\n\r\n";
    let response = send_http_request(port, with_origin);
    assert_eq!(status_code(&response), "405", "response: {response}");

    let without_origin = "GET /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
    let response = send_http_request(port, without_origin);
    assert_eq!(status_code(&response), "405", "response: {response}");
}

/// Proves `ChildGuard::drop` actually runs on a panic, rather than just
/// renaming the old manual `kill`/`wait` calls. A closure that panics while
/// holding the guard is the defect this file used to have: any `.expect()`
/// in `send_http_request` panicking before the manual cleanup ran left the
/// child alive and bound to its port. If the guard works, the port is free
/// again the moment `catch_unwind` returns.
#[test]
fn a_panic_while_holding_the_guard_still_frees_the_port() {
    let (guard, port) = start_mcp_http_server();

    let panicked = std::panic::catch_unwind(move || {
        let _guard = guard;
        panic!("deliberate panic to exercise ChildGuard::drop");
    });
    assert!(panicked.is_err(), "the closure was supposed to panic");

    TcpListener::bind(("127.0.0.1", port))
        .expect("ChildGuard::drop should have killed the child and freed the port");
}

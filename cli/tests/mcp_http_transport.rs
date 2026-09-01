use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Reserves a port by binding it in this process first, so two tests
/// starting at once cannot compute the same port and race for it. Retries
/// the whole reserve-and-spawn if the child loses the bind anyway.
fn start_mcp_http_server() -> (Child, u16) {
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
                return (child, port);
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
    let (mut child, port) = start_mcp_http_server();

    let payload = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#;
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nMcp-Method: tools/list\r\nContent-Length: {}\r\n\r\n{}",
        payload.len(),
        payload
    );

    let response = send_http_request(port, &request);
    child.kill().ok();
    child.wait().ok();

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
    let (mut child, port) = start_mcp_http_server();

    let request = "GET /unknown HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
    let response = send_http_request(port, request);
    child.kill().ok();
    child.wait().ok();

    assert_eq!(status_code(&response), "404", "response: {response}");
}

/// A wildcard `Access-Control-Allow-Origin` hands a browser permission to
/// read the response regardless of Origin, which would defeat the Origin
/// check the MCP spec requires (a DNS-rebinding defense). The MCP transport
/// must not emit any CORS header at all.
#[test]
fn responses_carry_no_cors_headers() {
    let (mut child, port) = start_mcp_http_server();

    let payload = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#;
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nMcp-Method: tools/list\r\nContent-Length: {}\r\n\r\n{}",
        payload.len(),
        payload
    );

    let response = send_http_request(port, &request);
    child.kill().ok();
    child.wait().ok();

    assert_eq!(status_code(&response), "200", "response: {response}");
    assert!(
        !response.contains("Access-Control-"),
        "MCP response must carry no Access-Control-* header: {response}"
    );
}

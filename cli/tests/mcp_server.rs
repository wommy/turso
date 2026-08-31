use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const PROTOCOL_V2: &str = "2026-07-28";

/// A v2 client never handshakes: it discovers, then calls.
#[test]
fn stdio_server_serves_a_v2_client() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_tursodb"))
        .arg("--mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("failed to run tursodb --mcp");

    let requests = [
        format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{{"_meta":{{"io.modelcontextprotocol/protocolVersion":"{PROTOCOL_V2}"}}}}}}"#
        ),
        tools_call(
            2,
            "schema_change",
            r#"{"query":"CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT)"}"#,
        ),
        tools_call(
            3,
            "insert_data",
            r#"{"query":"INSERT INTO t VALUES (1, 'alice'), (2, 'bob')"}"#,
        ),
        tools_call(
            4,
            "execute_query",
            r#"{"query":"SELECT id, name FROM t ORDER BY id"}"#,
        ),
    ];

    {
        let stdin = child.stdin.as_mut().expect("stdin is piped");
        for request in &requests {
            writeln!(stdin, "{request}").expect("write request");
        }
    }
    child.stdin.take();

    let mut stdout = String::new();
    child
        .stdout
        .as_mut()
        .expect("stdout is piped")
        .read_to_string(&mut stdout)
        .expect("read responses");
    child.wait().expect("tursodb exits at EOF");

    let responses: Vec<serde_json::Value> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("each response is JSON"))
        .collect();
    assert_eq!(responses.len(), 4, "one response per request: {stdout}");

    let discover = &responses[0]["result"];
    assert_eq!(discover["supportedVersions"][0], PROTOCOL_V2);
    assert!(discover["capabilities"]["tools"].is_object());

    assert_eq!(responses[1]["result"]["isError"], false);
    assert_eq!(responses[2]["result"]["structuredContent"]["changes"], 2);

    let rows = &responses[3]["result"]["structuredContent"];
    assert_eq!(rows["columns"], serde_json::json!(["id", "name"]));
    assert_eq!(rows["rows"], serde_json::json!([[1, "alice"], [2, "bob"]]));
    assert_eq!(rows["row_count"], 2);
}

#[test]
fn http_server_serves_a_v2_client() {
    let mut server = ServerProcess(
        Command::new(env!("CARGO_BIN_EXE_tursodb"))
            .args(["--mcp-http", "127.0.0.1:0"])
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to run tursodb --mcp-http"),
    );
    let address = listening_address(&mut server.0);

    let discover = post(
        &address,
        &[
            ("MCP-Protocol-Version", PROTOCOL_V2),
            ("Mcp-Method", "server/discover"),
        ],
        &format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{{"_meta":{{"io.modelcontextprotocol/protocolVersion":"{PROTOCOL_V2}"}}}}}}"#
        ),
    );
    assert_eq!(discover.status, 200, "{:?}", discover.body);
    assert_eq!(
        discover.json()["result"]["supportedVersions"][0],
        PROTOCOL_V2
    );

    let create = post(
        &address,
        &[
            ("MCP-Protocol-Version", PROTOCOL_V2),
            ("Mcp-Method", "tools/call"),
            ("Mcp-Name", "schema_change"),
        ],
        &tools_call(
            2,
            "schema_change",
            r#"{"query":"CREATE TABLE t (id INTEGER)"}"#,
        ),
    );
    assert_eq!(create.status, 200);
    assert_eq!(create.json()["result"]["isError"], false);

    let listed = post(
        &address,
        &[
            ("MCP-Protocol-Version", PROTOCOL_V2),
            ("Mcp-Method", "tools/call"),
            ("Mcp-Name", "list_tables"),
        ],
        &tools_call(3, "list_tables", "{}"),
    );
    assert_eq!(
        listed.json()["result"]["structuredContent"]["tables"],
        serde_json::json!(["t"])
    );

    // The header has to describe the body it travels with.
    let mismatched = post(
        &address,
        &[
            ("MCP-Protocol-Version", PROTOCOL_V2),
            ("Mcp-Method", "tools/call"),
            ("Mcp-Name", "execute_query"),
        ],
        &tools_call(4, "list_tables", "{}"),
    );
    assert_eq!(mismatched.status, 400);
    assert_eq!(mismatched.json()["error"]["code"], -32020);

    // A pre-v2 client sends none of the v2 routing headers.
    let legacy = post(
        &address,
        &[],
        r#"{"jsonrpc":"2.0","id":5,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{}}}"#,
    );
    assert_eq!(legacy.status, 200, "{}", legacy.body);
    assert_eq!(legacy.json()["result"]["protocolVersion"], "2024-11-05");

    assert_eq!(raw_request(&address, "GET /mcp HTTP/1.1"), 405);
    assert_eq!(
        raw_request_with(
            &address,
            "POST /mcp HTTP/1.1",
            &["Origin: https://evil.example"]
        ),
        403
    );
}

/// Two real HTTP clients hammer the same server concurrently. Before the
/// server's connection and current path were merged behind one lock held for
/// a whole tool call, `changes()` (a counter on the shared connection, set at
/// the end of whichever statement last ran) could be read after a *different*
/// client's statement ran on it: a 5-row DELETE from one client could report
/// 1 change because the other client's single-row INSERT ran in between the
/// DELETE's `execute` and its `changes()` read.
#[test]
fn http_server_keeps_change_counts_correct_under_concurrent_clients() {
    let mut server = ServerProcess(
        Command::new(env!("CARGO_BIN_EXE_tursodb"))
            .args(["--mcp-http", "127.0.0.1:0"])
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to run tursodb --mcp-http"),
    );
    let address = listening_address(&mut server.0);

    for (id, table) in [(1, "t"), (2, "u")] {
        let created = call(
            &address,
            id,
            "schema_change",
            &format!(r#"{{"query":"CREATE TABLE {table} (id INTEGER)"}}"#),
        );
        assert_eq!(
            created.json()["result"]["isError"],
            false,
            "{}",
            created.body
        );
    }

    let iterations = 100u32;
    std::thread::scope(|scope| {
        // Repeatedly fills `t` with 5 rows and empties it again in one
        // DELETE, which must always report exactly 5 changes.
        scope.spawn(|| {
            for i in 0..iterations {
                let insert = call(
                    &address,
                    1000 + i,
                    "insert_data",
                    r#"{"query":"INSERT INTO t VALUES (1),(2),(3),(4),(5)"}"#,
                );
                assert_eq!(insert.json()["result"]["isError"], false, "{}", insert.body);

                let delete = call(
                    &address,
                    2000 + i,
                    "delete_data",
                    r#"{"query":"DELETE FROM t"}"#,
                );
                assert_eq!(
                    delete.json()["result"]["structuredContent"]["changes"],
                    5,
                    "a 5-row delete must report 5 changes even while another client writes: {}",
                    delete.body
                );
            }
        });

        // Runs single-row inserts against a different table at the same
        // time, so its statements land on the shared connection in between
        // the other thread's DELETE and its changes() read, if nothing
        // serializes the two.
        scope.spawn(|| {
            for i in 0..iterations {
                let arguments = format!(r#"{{"query":"INSERT INTO u VALUES ({i})"}}"#);
                let insert = call(&address, 3000 + i, "insert_data", &arguments);
                assert_eq!(insert.json()["result"]["isError"], false, "{}", insert.body);
            }
        });
    });
}

/// A `tools/call` POST with the v2 routing headers already set.
fn call(address: &str, id: u32, name: &str, arguments: &str) -> HttpReply {
    post(
        address,
        &[
            ("MCP-Protocol-Version", PROTOCOL_V2),
            ("Mcp-Method", "tools/call"),
            ("Mcp-Name", name),
        ],
        &tools_call(id, name, arguments),
    )
}

/// 64 connections park their threads mid-request (never sending enough to
/// complete one), holding a slot against the server's connection cap each.
/// A 65th connection must be refused outright rather than spawning an
/// unbounded 65th thread.
#[test]
fn http_server_answers_503_once_connections_exceed_the_cap() {
    let mut server = ServerProcess(
        Command::new(env!("CARGO_BIN_EXE_tursodb"))
            .args(["--mcp-http", "127.0.0.1:0"])
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to run tursodb --mcp-http"),
    );
    let address = listening_address(&mut server.0);

    let idle_connections: Vec<TcpStream> = (0..64)
        .map(|_| TcpStream::connect(&address).expect("connect"))
        .collect();
    // Lets the accept loop pick up and spawn a thread for all 64 before the
    // 65th connection is attempted below.
    std::thread::sleep(Duration::from_millis(500));

    let status = raw_request(&address, "GET /mcp HTTP/1.1");

    assert_eq!(
        status, 503,
        "the 65th connection must be refused once the cap of 64 is reached"
    );

    drop(idle_connections);
}

fn raw_request(address: &str, request_line: &str) -> u16 {
    raw_request_with(address, request_line, &[])
}

fn raw_request_with(address: &str, request_line: &str, headers: &[&str]) -> u16 {
    let mut stream = TcpStream::connect(address).expect("connect to the MCP endpoint");
    let mut request = format!("{request_line}\r\nHost: {address}\r\n");
    for header in headers {
        request.push_str(header);
        request.push_str("\r\n");
    }
    request.push_str("Content-Length: 0\r\n\r\n");
    stream.write_all(request.as_bytes()).expect("send request");

    let mut raw = String::new();
    stream.read_to_string(&mut raw).expect("read reply");
    raw.lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .expect("status code")
}

/// Kills the server even when an assertion unwinds first.
struct ServerProcess(Child);

impl Drop for ServerProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn tools_call(id: u32, name: &str, arguments: &str) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"{name}","arguments":{arguments},"_meta":{{"io.modelcontextprotocol/protocolVersion":"{PROTOCOL_V2}"}}}}}}"#
    )
}

fn listening_address(child: &mut Child) -> String {
    let stderr = child.stderr.take().expect("stderr is piped");
    let mut reader = BufReader::new(stderr);
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line).expect("read startup log");
        assert!(read > 0, "server exited before it started listening");
        if let Some(address) = line.trim().strip_prefix("MCP HTTP server listening on ") {
            return address.to_string();
        }
    }
}

struct HttpReply {
    status: u16,
    body: String,
}

impl HttpReply {
    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).expect("JSON body")
    }
}

fn post(address: &str, headers: &[(&str, &str)], body: &str) -> HttpReply {
    let mut stream = TcpStream::connect(address).expect("connect to the MCP endpoint");

    let mut request = format!(
        "POST /mcp HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(body);

    stream.write_all(request.as_bytes()).expect("send request");
    stream.flush().expect("flush request");

    let mut raw = String::new();
    stream.read_to_string(&mut raw).expect("read reply");

    let (head, body) = raw.split_once("\r\n\r\n").expect("well formed reply");
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .expect("status code");

    HttpReply {
        status,
        body: body.to_string(),
    }
}

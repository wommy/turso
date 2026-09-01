use super::TursoMcpServer;
use crate::http::{format_http_response, parse_http_request, read_http_request, HttpResponse};
use anyhow::Result;
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
    pub body: Vec<u8>,
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
        let (method, path, body) = parse_http_request(&request_data)?;

        let response = if method == "POST" && path == ENDPOINT_PATH {
            http_response_for(self, &HttpRequest { body })
        } else {
            HttpResponse {
                status: 404,
                content_type: "text/plain".to_string(),
                body: b"Not Found".to_vec(),
            }
        };

        stream.write_all(&format_http_response(&response))?;
        stream.flush()?;
        Ok(())
    }
}

pub fn http_response_for(server: &TursoMcpServer, req: &HttpRequest) -> HttpResponse {
    let body = server
        .handle_message(&String::from_utf8_lossy(&req.body))
        .unwrap_or_default();
    HttpResponse {
        status: 200,
        content_type: "application/json".to_string(),
        body: body.into_bytes(),
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
}

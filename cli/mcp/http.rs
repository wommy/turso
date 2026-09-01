use super::TursoMcpServer;
use crate::http::HttpResponse;

// dead_code: nothing wires this to a socket yet - that lands in a later slice.
#[allow(dead_code)]
pub struct HttpRequest {
    pub method: String,
    pub body: Vec<u8>,
}

#[allow(dead_code)]
pub fn http_response_for(server: &TursoMcpServer, req: &HttpRequest) -> HttpResponse {
    debug_assert_eq!(req.method, "POST", "the only method this slice serves");
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
            method: "POST".to_string(),
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

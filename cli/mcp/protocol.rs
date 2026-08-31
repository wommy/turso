use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

/// MCP v2. Stateless: no handshake, every request carries its own version.
pub const PROTOCOL_V2: &str = "2026-07-28";

/// Newest first. Everything before v2 needs the `initialize` handshake; for a
/// tools-only server those revisions are wire-compatible with each other.
pub const SUPPORTED_VERSIONS: [&str; 4] = [PROTOCOL_V2, "2025-06-18", "2025-03-26", "2024-11-05"];

/// Answer to a handshake asking for a version we do not know.
pub const LEGACY_DEFAULT: &str = "2025-06-18";

pub const META_PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
pub const META_SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";

pub const PARSE_ERROR: i32 = -32700;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;
pub const HEADER_MISMATCH: i32 = -32020;
pub const UNSUPPORTED_PROTOCOL_VERSION: i32 = -32022;

/// The tool list never changes while the server runs.
pub const CACHE_TTL_MS: u64 = 3_600_000;

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    pub params: Option<Value>,
    /// The spec's own examples put `_meta` inside `params` in some places and
    /// beside it in others, so accept both.
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

impl JsonRpcRequest {
    pub fn protocol_version(&self) -> Option<&str> {
        let from_params = self
            .params
            .as_ref()
            .and_then(|params| params.get("_meta"))
            .and_then(|meta| meta.get(META_PROTOCOL_VERSION));
        let from_top_level = self
            .meta
            .as_ref()
            .and_then(|m| m.get(META_PROTOCOL_VERSION));
        from_params.or(from_top_level).and_then(Value::as_str)
    }

    /// A client that names no version is a pre-v2 client, and is served as one.
    pub fn check_protocol_version(&self) -> Result<(), JsonRpcError> {
        match self.protocol_version() {
            None => Ok(()),
            Some(version) if SUPPORTED_VERSIONS.contains(&version) => Ok(()),
            Some(version) => Err(JsonRpcError {
                code: UNSUPPORTED_PROTOCOL_VERSION,
                message: format!("Unsupported protocol version: {version}"),
                data: Some(json!({ "supportedVersions": SUPPORTED_VERSIONS })),
            }),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    pub fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn failure(id: Option<Value>, error: JsonRpcError) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(error),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
}

pub fn server_info() -> Value {
    json!({
        "name": "turso-mcp",
        "version": env!("CARGO_PKG_VERSION"),
    })
}

/// v2 results identify the server in `_meta`. Pre-v2 clients ignore the field.
pub fn result_meta() -> Value {
    let mut meta = Map::new();
    meta.insert(META_SERVER_INFO.to_string(), server_info());
    Value::Object(meta)
}

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

/// The revision this server speaks natively. It has no handshake: every
/// request carries its own version and client identity in `_meta`.
pub(crate) const PROTOCOL_V2: &str = "2026-07-28";

/// Newest first. `2025-03-26` is left out deliberately - it is the
/// batch-request revision, and we never implemented batching.
pub(crate) const SUPPORTED_VERSIONS: [&str; 3] = [PROTOCOL_V2, "2025-06-18", "2024-11-05"];

/// Answer to a handshake asking for a version we do not know.
pub(crate) const LEGACY_DEFAULT: &str = "2025-06-18";

pub(crate) const META_PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
pub(crate) const META_CLIENT_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";
pub(crate) const META_SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";

pub(crate) const PARSE_ERROR: i32 = -32700;
pub(crate) const INVALID_REQUEST: i32 = -32600;
pub(crate) const METHOD_NOT_FOUND: i32 = -32601;
pub(crate) const INVALID_PARAMS: i32 = -32602;
pub(crate) const UNSUPPORTED_PROTOCOL_VERSION: i32 = -32022;
pub(crate) const HEADER_MISMATCH: i32 = -32020;
/// A refused Origin has no code of its own. `-32020` to `-32099` is reserved
/// for the specification, which assigns no code to this case and forbids both
/// inventing one in that range and reusing a defined code for another meaning.
/// `-32021` is `MissingRequiredClientCapability`, so it is not available here.
pub(crate) const FORBIDDEN_ORIGIN: i32 = INVALID_REQUEST;
/// A chunked request body has no code of its own either, for the same reason
/// `FORBIDDEN_ORIGIN` reuses `INVALID_REQUEST` above: this is a framing
/// refusal (ADR 0004), not a case the specification assigns a code to.
pub(crate) const LENGTH_REQUIRED: i32 = INVALID_REQUEST;

/// The tool list cannot change while the server runs, so a client may hold it
/// for as long as it likes.
pub(crate) const CACHE_TTL_MS: u64 = 3_600_000;

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct JsonRpcRequest {
    pub(crate) jsonrpc: String,
    pub(crate) id: Option<Value>,
    pub(crate) method: String,
    pub(crate) params: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct JsonRpcResponse {
    pub(crate) jsonrpc: String,
    pub(crate) id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct JsonRpcError {
    pub(crate) code: i32,
    pub(crate) message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) data: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct InitializeRequest {
    #[serde(rename = "protocolVersion")]
    pub(crate) protocol_version: String,
    pub(crate) capabilities: Value,
    #[serde(rename = "clientInfo")]
    pub(crate) client_info: Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct CallToolRequest {
    pub(crate) name: String,
    pub(crate) arguments: Option<Value>,
}

impl JsonRpcRequest {
    /// `_meta` lives on `params`. Nowhere else: the schema puts it on
    /// `RequestParams`, and all three official SDKs read and write it there in
    /// every revision. A top-level `_meta` is a `Result` field, which has no
    /// `params` to nest inside - a fact about responses that does not carry
    /// over to requests.
    fn meta(&self) -> Option<&Value> {
        self.params.as_ref()?.get("_meta")
    }

    /// The schema types `_meta` and the two required fields inside it, so a
    /// value of the wrong type is a required field supplied malformed, which
    /// the spec makes `-32602`. It has to be caught before anything reads
    /// those fields: `get` and `as_str` report a wrong type as an absent
    /// field, and an absent version is exactly how a pre-v2 client looks, so
    /// a v2 client whose version we cannot parse would otherwise be served as
    /// pre-v2 and never told that what it sent was ignored.
    pub(crate) fn check_meta_shape(&self) -> Result<(), JsonRpcError> {
        let Some(meta) = self.meta() else {
            return Ok(());
        };
        let Some(meta) = meta.as_object() else {
            return Err(malformed("params._meta", "an object"));
        };
        if meta
            .get(META_PROTOCOL_VERSION)
            .is_some_and(|version| !version.is_string())
        {
            return Err(malformed(
                &format!("params._meta[\"{META_PROTOCOL_VERSION}\"]"),
                "a string",
            ));
        }
        if meta
            .get(META_CLIENT_CAPABILITIES)
            .is_some_and(|capabilities| !capabilities.is_object())
        {
            return Err(malformed(
                &format!("params._meta[\"{META_CLIENT_CAPABILITIES}\"]"),
                "an object",
            ));
        }
        Ok(())
    }

    /// A client that names no version is pre-v2, and is served as one.
    pub(crate) fn protocol_version(&self) -> Option<&str> {
        self.meta()?.get(META_PROTOCOL_VERSION)?.as_str()
    }

    /// The dual-era fork a server selects behavior from (MUST,
    /// `basic/versioning.mdx` L175-180): true for a request carrying modern
    /// per-request `_meta`, false for everything else, `initialize` included.
    /// Also used by the HTTP transport to scope its `2026-07-28`-only
    /// routing headers the same way this module scopes `clientCapabilities`
    /// below - a client that has not declared this revision has no header to
    /// check.
    pub(crate) fn declares_v2(&self) -> bool {
        self.protocol_version() == Some(PROTOCOL_V2)
    }

    pub(crate) fn check_protocol_version(&self) -> Result<(), JsonRpcError> {
        let Some(version) = self.protocol_version() else {
            return Ok(());
        };
        if SUPPORTED_VERSIONS.contains(&version) {
            return Ok(());
        }
        Err(JsonRpcError {
            code: UNSUPPORTED_PROTOCOL_VERSION,
            message: format!("Unsupported protocol version: {version}"),
            data: Some(json!({ "supported": SUPPORTED_VERSIONS, "requested": version })),
        })
    }

    /// v2 requires `clientCapabilities` on every request. We hold a client to
    /// that only when it says it speaks v2 - a pre-v2 client has no such field
    /// to send, and rejecting one over it would exclude every client that
    /// exists today for a value this server never reads.
    pub(crate) fn check_client_capabilities(&self) -> Result<(), JsonRpcError> {
        if !self.declares_v2()
            || self
                .meta()
                .is_some_and(|m| m.get(META_CLIENT_CAPABILITIES).is_some())
        {
            return Ok(());
        }
        Err(JsonRpcError::new(
            INVALID_PARAMS,
            format!(
                "A {PROTOCOL_V2} request must carry params._meta[\"{META_CLIENT_CAPABILITIES}\"]"
            ),
        ))
    }
}

fn malformed(path: &str, expected: &str) -> JsonRpcError {
    JsonRpcError::new(INVALID_PARAMS, format!("{path} must be {expected}"))
}

impl JsonRpcResponse {
    pub(crate) fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub(crate) fn failure(id: Option<Value>, error: JsonRpcError) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(error),
        }
    }
}

impl JsonRpcError {
    pub(crate) fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
}

pub(crate) fn server_info() -> Value {
    json!({ "name": "turso-mcp", "version": env!("CARGO_PKG_VERSION") })
}

/// v2 results name the server in `_meta`. Pre-v2 clients ignore the field, so
/// it is always sent rather than branched on.
pub(crate) fn result_meta() -> Value {
    let mut meta = Map::new();
    meta.insert(META_SERVER_INFO.to_string(), server_info());
    Value::Object(meta)
}

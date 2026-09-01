use super::protocol::{
    result_meta, CallToolRequest, JsonRpcError, JsonRpcRequest, JsonRpcResponse, CACHE_TTL_MS,
    INVALID_PARAMS,
};
use super::TursoMcpServer;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use turso_core::{
    Connection, Database, DatabaseOpts, LimboError, Numeric, OpenFlags, SqliteDialect,
    Value as DbValue,
};
use turso_parser::ast::{Cmd, Stmt};
use turso_parser::parser::Parser;

#[derive(Clone, Copy, PartialEq, Eq)]
enum StmtClass {
    Select,
    Insert,
    Update,
    Delete,
    Schema,
}

impl StmtClass {
    fn of(stmt: &Stmt) -> Option<Self> {
        match stmt {
            Stmt::Select(_) => Some(Self::Select),
            Stmt::Insert { .. } => Some(Self::Insert),
            Stmt::Update(_) => Some(Self::Update),
            Stmt::Delete { .. } => Some(Self::Delete),
            Stmt::AlterTable(_)
            | Stmt::CreateIndex { .. }
            | Stmt::CreateTable { .. }
            | Stmt::CreateTrigger { .. }
            | Stmt::CreateView { .. }
            | Stmt::CreateMaterializedView { .. }
            | Stmt::CreateVirtualTable(_)
            | Stmt::CreateType { .. }
            | Stmt::CreateDomain { .. }
            | Stmt::CreateSequence { .. }
            | Stmt::DropIndex { .. }
            | Stmt::DropTable { .. }
            | Stmt::DropTrigger { .. }
            | Stmt::DropView { .. }
            | Stmt::DropType { .. }
            | Stmt::DropDomain { .. }
            | Stmt::DropSequence { .. } => Some(Self::Schema),
            Stmt::Analyze { .. }
            | Stmt::Attach { .. }
            | Stmt::Begin { .. }
            | Stmt::Commit { .. }
            | Stmt::Detach { .. }
            | Stmt::Pragma { .. }
            | Stmt::Reindex { .. }
            | Stmt::Release { .. }
            | Stmt::Rollback { .. }
            | Stmt::Savepoint { .. }
            | Stmt::Vacuum { .. }
            | Stmt::Optimize { .. } => None,
        }
    }

    fn single_statement_error(self) -> &'static str {
        match self {
            Self::Select => "Only a single SELECT query is allowed",
            Self::Insert => "Only a single INSERT statement is allowed",
            Self::Update => "Only a single UPDATE statement is allowed",
            Self::Delete => "Only a single DELETE statement is allowed",
            Self::Schema => "Only a single schema modification statement is allowed",
        }
    }

    fn wrong_class_error(self) -> &'static str {
        match self {
            Self::Select => "Only SELECT queries are allowed",
            Self::Insert => "Only INSERT statements are allowed",
            Self::Update => "Only UPDATE statements are allowed",
            Self::Delete => "Only DELETE statements are allowed",
            Self::Schema => "Only CREATE, ALTER, and DROP statements are allowed",
        }
    }
}

fn query_arg(arguments: &Option<Value>) -> Result<&str, String> {
    match arguments {
        Some(args) => match args.get("query") {
            Some(Value::String(q)) => Ok(q),
            _ => Err("Missing or invalid query parameter".to_string()),
        },
        None => Err("Missing query parameter".to_string()),
    }
}

fn require_single_stmt(sql: &str, class: StmtClass) -> Result<(), String> {
    let mut parser = Parser::new(sql.as_bytes());
    let cmd = match parser.next_cmd() {
        Ok(Some(cmd)) => cmd,
        Ok(None) => return Err("No SQL statement provided".to_string()),
        Err(e) => return Err(format!("Failed to parse SQL: {e}")),
    };
    match parser.next_cmd() {
        Ok(None) => {}
        Ok(Some(_)) => return Err(class.single_statement_error().to_string()),
        Err(e) => return Err(format!("Failed to parse SQL: {e}")),
    }
    match cmd {
        Cmd::Stmt(stmt) if StmtClass::of(&stmt) == Some(class) => Ok(()),
        Cmd::Stmt(_) | Cmd::Explain(_) | Cmd::ExplainQueryPlan { .. } => {
            Err(class.wrong_class_error().to_string())
        }
    }
}

fn validated_query(arguments: &Option<Value>, class: StmtClass) -> Result<&str, String> {
    let sql = query_arg(arguments)?;
    require_single_stmt(sql, class)?;
    Ok(sql)
}

/// A query answers with at most this many rows unless asked for fewer. An
/// unbounded answer is not useful to a model and can be very large.
/// The tools `refuse_if_readonly` turns away. The catalog is amended from this
/// same list, so what a client is told and what it is refused cannot drift.
const REFUSED_WHEN_READONLY: [&str; 4] =
    ["insert_data", "update_data", "delete_data", "schema_change"];

/// Stops the scan once enough rows are in hand. Returned through the row
/// callback because that is the only way to break out of it.
const ROW_CAP_REACHED: &str = "turso-mcp: row cap reached";

const DEFAULT_MAX_ROWS: usize = 1_000;

/// Long values are cut short, and say so, rather than being silently shortened
/// into something that looks whole.
const MAX_CELL_BYTES: usize = 512;

/// What a tool produces when it succeeds: prose for a person reading the
/// client UI, and the same answer typed for a model.
#[derive(Debug)]
pub(crate) struct ToolOutput {
    text: String,
    structured: Value,
}

impl ToolOutput {
    fn new(text: impl Into<String>, structured: Value) -> Self {
        Self {
            text: text.into(),
            structured,
        }
    }
}

/// Cuts to at most `max_bytes`, landing on a character boundary. Taking a
/// count of `char`s instead would let a multi-byte value keep several times
/// the byte budget it is being measured against.
fn cut_to_bytes(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let end = (0..=max_bytes)
        .rev()
        .find(|&i| text.is_char_boundary(i))
        .unwrap_or(0);
    &text[..end]
}

/// The prose form of a cell, held to the same budget as the structured one.
/// Capping only `structuredContent` left a 10 MB value truncated in one field
/// and whole in the other.
fn display_cell(value: &DbValue) -> String {
    // Display renders NULL as an empty string, which makes it identical to an
    // empty text value in the table. structuredContent tells them apart; the
    // prose has to as well. `cli/app.rs` special-cases NULL for the same
    // reason rather than trusting Display.
    if matches!(value, DbValue::Null) {
        return "NULL".to_string();
    }
    let rendered = value.to_string();
    if rendered.len() <= MAX_CELL_BYTES {
        return rendered;
    }
    format!(
        "{}… ({} bytes)",
        cut_to_bytes(&rendered, MAX_CELL_BYTES),
        rendered.len()
    )
}

/// SQL values carry types that JSON can mostly hold. Blobs are the exception:
/// they are tagged rather than inlined, so a model cannot mistake one for text
/// it can act on.
fn sql_value_to_json(value: &DbValue) -> Value {
    match value {
        DbValue::Null => Value::Null,
        DbValue::Numeric(Numeric::Integer(n)) => json!(n),
        DbValue::Numeric(Numeric::Float(f)) => json!(f64::from(*f)),
        DbValue::Text(text) => {
            let text = text.to_string();
            if text.len() <= MAX_CELL_BYTES {
                json!(text)
            } else {
                // A different shape, not a shorter string: a cut value must not
                // be usable as if it were the whole one.
                json!({
                    "text": cut_to_bytes(&text, MAX_CELL_BYTES),
                    "bytes": text.len(),
                    "truncated": true,
                })
            }
        }
        DbValue::Blob(_) => {
            let rendered = value.to_string();
            if rendered.len() <= MAX_CELL_BYTES {
                json!({ "blob": rendered })
            } else {
                json!({
                    "blob_preview": cut_to_bytes(&rendered, MAX_CELL_BYTES),
                    "bytes": rendered.len(),
                    "truncated": true,
                })
            }
        }
    }
}

impl TursoMcpServer {
    /// Under --readonly the usual "failed to open" is misleading: the file may
    /// be perfectly openable, just not by us, and not creatable at all.
    fn open_error(&self, path: &str, e: impl std::fmt::Display) -> String {
        if self.readonly {
            format!(
                "Failed to open database '{path}': {e}. This server was started with \
                 --readonly: it can only open an existing file, read-only, and never \
                 creates one."
            )
        } else {
            format!("Failed to open database '{path}': {e}")
        }
    }

    fn refuse_if_readonly(&self, what: &str) -> Result<(), String> {
        if self.readonly {
            return Err(format!(
                "This server was started with --readonly, so {what} is not available."
            ));
        }
        Ok(())
    }

    pub(crate) fn handle_list_tools(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        let mut result = json!({
                "resultType": "complete",
                // The catalog cannot change while the server runs.
                "ttlMs": CACHE_TTL_MS,
                "cacheScope": "public",
                "_meta": result_meta(),
                "tools": [
                    {
                        "name": "open_database",
                        "title": "Open a database file",
                        "annotations": {
                            "title": "Open a database file",
                            "readOnlyHint": false,
                            "destructiveHint": false,
                            "idempotentHint": false,
                            "openWorldHint": false
                        },
                        "outputSchema": { "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] },
                        "description": "Open or create a database file. Creates parent directories if needed.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "path": {
                                    "type": "string",
                                    "description": "Path to the database file (absolute or relative). Use ':memory:' for in-memory database."
                                }
                            },
                            "required": ["path"]
                        }
                    },
                    {
                        "name": "current_database",
                        "title": "Show the open database",
                        "annotations": {
                            "title": "Show the open database",
                            "readOnlyHint": true,
                            "destructiveHint": false,
                            "idempotentHint": true,
                            "openWorldHint": false
                        },
                        "outputSchema": { "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] },
                        "description": "Get the path of the currently open database",
                        "inputSchema": {
                            "type": "object",
                            "properties": {},
                            "required": []
                        }
                    },
                    {
                        "name": "list_tables",
                        "title": "List tables",
                        "annotations": {
                            "title": "List tables",
                            "readOnlyHint": true,
                            "destructiveHint": false,
                            "idempotentHint": true,
                            "openWorldHint": false
                        },
                        "outputSchema": { "type": "object", "properties": { "tables": { "type": "array", "items": { "type": "string" } } }, "required": ["tables"] },
                        "description": "List all tables in the database",
                        "inputSchema": {
                            "type": "object",
                            "properties": {},
                            "required": []
                        }
                    },
                    {
                        "name": "describe_table",
                        "title": "Describe a table",
                        "annotations": {
                            "title": "Describe a table",
                            "readOnlyHint": true,
                            "destructiveHint": false,
                            "idempotentHint": true,
                            "openWorldHint": false
                        },
                        "outputSchema": { "type": "object", "properties": { "table": { "type": "string" }, "columns": { "type": "array", "items": { "type": "object", "properties": { "name": { "type": "string" }, "type": { "type": "string" }, "nullable": { "type": "boolean" }, "default": {}, "primary_key": { "type": "boolean" }, "generated": { "type": "boolean" } } } } }, "required": ["table", "columns"] },
                        "description": "Describe the structure of a specific table",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "table_name": {
                                    "type": "string",
                                    "description": "Name of the table to describe"
                                }
                            },
                            "required": ["table_name"]
                        }
                    },
                    {
                        "name": "execute_query",
                        "title": "Run a SELECT query",
                        "annotations": {
                            "title": "Run a SELECT query",
                            "readOnlyHint": true,
                            "destructiveHint": false,
                            "idempotentHint": true,
                            "openWorldHint": false
                        },
                        "outputSchema": { "type": "object", "properties": { "truncated": { "type": "boolean" }, "columns": { "type": "array", "items": { "type": "string" } }, "rows": { "type": "array", "items": { "type": "array" }, "description": "Row values by column. A blob is an object with a `blob` key, never a bare string." }, "row_count": { "type": "integer" } }, "required": ["columns", "rows", "row_count"] },
                        "description": "Execute a read-only SELECT query",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "query": {
                                    "type": "string",
                                    "description": "The SELECT query to execute"
                                },
                                "max_rows": {
                                    "type": "integer",
                                    "minimum": 0,
                                    "description": "Stop after this many rows. Capped at 1000, which is also the default."
                                }
                            },
                            "required": ["query"]
                        }
                    },
                    {
                        "name": "insert_data",
                        "title": "Insert rows",
                        "annotations": {
                            "title": "Insert rows",
                            "readOnlyHint": false,
                            "destructiveHint": false,
                            "idempotentHint": false,
                            "openWorldHint": false
                        },
                        "outputSchema": { "type": "object", "properties": { "changes": { "type": "integer" } }, "required": ["changes"] },
                        "description": "Insert new data into a table",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "query": {
                                    "type": "string",
                                    "description": "The INSERT statement to execute"
                                }
                            },
                            "required": ["query"]
                        }
                    },
                    {
                        "name": "update_data",
                        "title": "Update rows",
                        "annotations": {
                            "title": "Update rows",
                            "readOnlyHint": false,
                            "destructiveHint": true,
                            "idempotentHint": false,
                            "openWorldHint": false
                        },
                        "outputSchema": { "type": "object", "properties": { "changes": { "type": "integer" } }, "required": ["changes"] },
                        "description": "Update existing data in a table",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "query": {
                                    "type": "string",
                                    "description": "The UPDATE statement to execute"
                                }
                            },
                            "required": ["query"]
                        }
                    },
                    {
                        "name": "delete_data",
                        "title": "Delete rows",
                        "annotations": {
                            "title": "Delete rows",
                            "readOnlyHint": false,
                            "destructiveHint": true,
                            "idempotentHint": false,
                            "openWorldHint": false
                        },
                        "outputSchema": { "type": "object", "properties": { "changes": { "type": "integer" } }, "required": ["changes"] },
                        "description": "Delete data from a table",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "query": {
                                    "type": "string",
                                    "description": "The DELETE statement to execute"
                                }
                            },
                            "required": ["query"]
                        }
                    },
                    {
                        "name": "schema_change",
                        "title": "Change the schema",
                        "annotations": {
                            "title": "Change the schema",
                            "readOnlyHint": false,
                            "destructiveHint": true,
                            "idempotentHint": false,
                            "openWorldHint": false
                        },
                        "outputSchema": { "type": "object", "properties": { "changes": { "type": "integer" } }, "required": ["changes"] },
                        "description": "Execute schema modification statements (CREATE TABLE, ALTER TABLE, DROP TABLE)",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "query": {
                                    "type": "string",
                                    "description": "The schema modification statement to execute"
                                }
                            },
                            "required": ["query"]
                        }
                    }
                ]
        });

        // A model should not be offered a tool it will be refused. The catalog
        // is the only place it can learn that before spending a call.
        if self.readonly {
            if let Some(tools) = result["tools"].as_array_mut() {
                for tool in tools {
                    let name = tool["name"].as_str().unwrap_or_default();
                    // Not `readOnlyHint == false`: open_database writes in
                    // principle but is never refused - it opens read-only
                    // instead - so keying off the hint marked a working tool
                    // unavailable.
                    if REFUSED_WHEN_READONLY.contains(&name) {
                        let description = tool["description"].as_str().unwrap_or_default();
                        tool["description"] = json!(format!(
                            "{description} UNAVAILABLE: this server was started with --readonly."
                        ));
                    }
                }
            }
        }

        JsonRpcResponse::success(request.id, result)
    }

    pub(crate) fn handle_call_tool(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        let tool_request: CallToolRequest = match request.params.as_ref() {
            Some(params) => match serde_json::from_value(params.clone()) {
                Ok(req) => req,
                Err(e) => {
                    return JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        id: request.id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32602,
                            message: format!("Invalid params: {e}"),
                            data: None,
                        }),
                    };
                }
            },
            None => {
                return JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: request.id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: "Missing params".to_string(),
                        data: None,
                    }),
                };
            }
        };

        let result = match tool_request.name.as_str() {
            "open_database" => self.open_database(&tool_request.arguments),
            "current_database" => self.current_database(),
            "list_tables" => self.list_tables(),
            "describe_table" => self.describe_table(&tool_request.arguments),
            "execute_query" => self.execute_query(&tool_request.arguments),
            "insert_data" => self.insert_data(&tool_request.arguments),
            "update_data" => self.update_data(&tool_request.arguments),
            "delete_data" => self.delete_data(&tool_request.arguments),
            "schema_change" => self.schema_change(&tool_request.arguments),
            _ => {
                return JsonRpcResponse::failure(
                    request.id,
                    JsonRpcError::new(
                        INVALID_PARAMS,
                        format!("Unknown tool: {}", tool_request.name),
                    ),
                );
            }
        };

        // A tool that fails says so in the result. Before this, a rejected
        // multi-statement UPDATE and a successful one were both successful
        // results whose text happened to differ, which a model cannot tell
        // apart.
        let (text, structured, is_error) = match result {
            Ok(output) => (output.text, Some(output.structured), false),
            Err(message) => (message, None, true),
        };
        let mut payload = json!({
            "resultType": "complete",
            "_meta": result_meta(),
            "content": [{ "type": "text", "text": text }],
            "isError": is_error,
        });
        if let Some(structured) = structured {
            payload["structuredContent"] = structured;
        }
        JsonRpcResponse::success(request.id, payload)
    }

    fn open_database(&self, arguments: &Option<Value>) -> Result<ToolOutput, String> {
        let path = arguments
            .as_ref()
            .and_then(|args| args.get("path"))
            .and_then(Value::as_str)
            .ok_or_else(|| "Missing or invalid path parameter".to_string())?
            .to_string();

        // Creating the directory happens before the open, so under --readonly
        // it has to be skipped as well - otherwise the flag still lets the
        // server make directories on disk.
        if !self.readonly && path != ":memory:" {
            if let Some(parent) = PathBuf::from(&path).parent() {
                if !parent.exists() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("Failed to create parent directories: {e}"))?;
                }
            }
        }

        // A URI opens with whatever the URI says, which under --readonly would
        // be a way around the flag. Only the flags-controlled branch below can
        // be trusted with it, so :memory: is the sole URI-shaped path allowed.
        let uri_shaped = path.contains([':', '?', '&', '#']);
        let conn = if path == ":memory:" || (!self.readonly && uri_shaped) {
            Connection::from_uri(&path, DatabaseOpts::default(), Arc::new(SqliteDialect))
                .map_err(|e| self.open_error(&path, e))?
                .1
        } else {
            let flags = if self.readonly {
                OpenFlags::default().union(OpenFlags::ReadOnly)
            } else {
                OpenFlags::default()
            };
            let (_io, db) = Database::open_new(
                &path,
                None::<&str>,
                flags,
                DatabaseOpts::new().with_autovacuum(false),
                None,
                Arc::new(SqliteDialect),
            )
            .map_err(|e| self.open_error(&path, e))?;
            db.connect()
                .map_err(|e| format!("Failed to connect to database '{path}': {e}"))?
        };

        // Both fields change together under one lock, so a concurrent tool
        // call on another connection can never see this call's new
        // connection paired with the old path, or vice versa.
        let mut session = self.session.lock().unwrap();
        session.conn = conn;
        session.db_path = Some(path.clone());
        drop(session);

        Ok(ToolOutput::new(
            format!("Successfully opened database: {path}"),
            json!({ "path": path }),
        ))
    }

    fn current_database(&self) -> Result<ToolOutput, String> {
        let path = self
            .session
            .lock()
            .unwrap()
            .db_path
            .clone()
            .unwrap_or_else(|| ":memory:".to_string());
        Ok(ToolOutput::new(
            format!("Current database: {path}"),
            json!({ "path": path }),
        ))
    }

    fn list_tables(&self) -> Result<ToolOutput, String> {
        let query = "SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY 1";

        let session = self.session.lock().unwrap();
        let mut rows = session
            .conn
            .query(query)
            .map_err(|e| format!("Error querying database: {e}"))?
            .ok_or_else(|| "No results returned from the query".to_string())?;

        let mut tables = Vec::new();
        rows.run_with_row_callback(|row| {
            if let Ok(DbValue::Text(table)) = row.get::<&DbValue>(0) {
                tables.push(table.to_string());
            }
            Ok(())
        })
        .map_err(|e| e.to_string())?;

        let text = if tables.is_empty() {
            "No tables found in the database".to_string()
        } else {
            tables.join(", ")
        };
        Ok(ToolOutput::new(text, json!({ "tables": tables })))
    }

    fn describe_table(&self, arguments: &Option<Value>) -> Result<ToolOutput, String> {
        let table_name = arguments
            .as_ref()
            .and_then(|args| args.get("table_name"))
            .and_then(Value::as_str)
            .ok_or_else(|| "Missing or invalid table_name parameter".to_string())?;

        // table_xinfo rather than table_info: the latter hides generated columns.
        let query = format!("PRAGMA table_xinfo({table_name})");

        let session = self.session.lock().unwrap();
        let mut rows = session
            .conn
            .query(&query)
            .map_err(|e| format!("Error querying database: {e}"))?
            .ok_or_else(|| format!("Table '{table_name}' not found"))?;

        let mut described: Vec<Value> = Vec::new();
        let mut lines: Vec<String> = Vec::new();
        rows.run_with_row_callback(|row| {
            let (Ok(name), Ok(kind), Ok(not_null), Ok(default_value), Ok(pk), Ok(hidden)) = (
                row.get::<&DbValue>(1),
                row.get::<&DbValue>(2),
                row.get::<&DbValue>(3),
                row.get::<&DbValue>(4),
                row.get::<&DbValue>(5),
                row.get::<&DbValue>(6),
            ) else {
                return Ok(());
            };

            let nullable = !matches!(not_null, DbValue::Numeric(Numeric::Integer(1)));
            let primary_key = matches!(pk, DbValue::Numeric(Numeric::Integer(1)));
            let generated = matches!(hidden, DbValue::Numeric(Numeric::Integer(2)));
            let default = match default_value {
                DbValue::Null => Value::Null,
                other => json!(other.to_string()),
            };

            described.push(json!({
                "name": name.to_string(),
                "type": kind.to_string(),
                "nullable": nullable,
                "default": default,
                "primary_key": primary_key,
                "generated": generated,
            }));

            let default_str = match default_value {
                DbValue::Null => String::new(),
                other => format!("DEFAULT {other}"),
            };
            lines.push(
                format!(
                    "{} {} {} {} {}{}",
                    name,
                    kind,
                    if nullable { "NULL" } else { "NOT NULL" },
                    default_str,
                    if primary_key { "PRIMARY KEY" } else { "" },
                    if generated { " VIRTUAL GENERATED" } else { "" }
                )
                .trim()
                .to_string(),
            );
            Ok(())
        })
        .map_err(|e| e.to_string())?;

        if described.is_empty() {
            return Err(format!("Table '{table_name}' not found"));
        }
        Ok(ToolOutput::new(
            format!("Table '{table_name}' columns:\n{}", lines.join("\n")),
            json!({ "table": table_name, "columns": described }),
        ))
    }

    fn execute_query(&self, arguments: &Option<Value>) -> Result<ToolOutput, String> {
        let query = validated_query(arguments, StmtClass::Select)?;

        let session = self.session.lock().unwrap();
        let mut rows = session
            .conn
            .query(query)
            .map_err(|e| format!("Error executing query: {e}"))?
            .ok_or_else(|| "No results returned from the query".to_string())?;

        let columns: Vec<String> = (0..rows.num_columns())
            .map(|i| rows.get_column_name(i).to_string())
            .collect();

        let max_rows = arguments
            .as_ref()
            .and_then(|args| args.get("max_rows"))
            .and_then(Value::as_u64)
            .map_or(DEFAULT_MAX_ROWS, |n| (n as usize).min(DEFAULT_MAX_ROWS));

        let mut typed: Vec<Value> = Vec::new();
        let mut rendered: Vec<Vec<String>> = Vec::new();
        let mut truncated = false;
        let scan = rows.run_with_row_callback(|row| {
            if typed.len() >= max_rows {
                truncated = true;
                // Ok(()) here would let the engine keep stepping - scanning,
                // sorting and aggregating a whole table whose rows we then
                // discard. Only an Err breaks the loop.
                return Err(LimboError::InternalError(ROW_CAP_REACHED.to_string()));
            }
            let values: Vec<&DbValue> = row.get_values().collect();
            typed.push(Value::Array(
                values.iter().map(|v| sql_value_to_json(v)).collect(),
            ));
            rendered.push(values.iter().map(|v| display_cell(v)).collect());
            Ok(())
        });
        match scan {
            Ok(()) => {}
            Err(LimboError::InternalError(ref message)) if message == ROW_CAP_REACHED => {}
            Err(e) => return Err(e.to_string()),
        }

        let mut text = String::new();
        if !columns.is_empty() {
            let header = columns.join(" | ");
            text.push_str(&header);
            text.push('\n');
            text.push_str(&"-".repeat(header.len()));
            text.push('\n');
        }
        for row in &rendered {
            text.push_str(&row.join(" | "));
            text.push('\n');
        }
        if text.is_empty() {
            text = "No results returned from the query".to_string();
        }

        let row_count = typed.len();
        if truncated {
            text.push_str(&format!("\n(truncated at {max_rows} rows)\n"));
        }
        Ok(ToolOutput::new(
            text,
            json!({
                "columns": columns,
                "rows": typed,
                "row_count": row_count,
                "truncated": truncated,
            }),
        ))
    }

    fn insert_data(&self, arguments: &Option<Value>) -> Result<ToolOutput, String> {
        self.refuse_if_readonly("INSERT")?;
        let query = validated_query(arguments, StmtClass::Insert)?;

        // `changes()` is a counter on the connection itself, set at the end
        // of whichever statement last ran on it - so it has to be read
        // before another client's tool call can run a statement of its own,
        // which is exactly what holding `session` for both calls guarantees.
        let session = self.session.lock().unwrap();
        session
            .conn
            .execute(query)
            .map_err(|e| format!("Error executing INSERT: {e}"))?;
        let changes = session.conn.changes();
        Ok(ToolOutput::new(
            format!("INSERT successful. {changes} row(s) changed."),
            json!({ "changes": changes }),
        ))
    }

    fn update_data(&self, arguments: &Option<Value>) -> Result<ToolOutput, String> {
        self.refuse_if_readonly("UPDATE")?;
        let query = validated_query(arguments, StmtClass::Update)?;

        let session = self.session.lock().unwrap();
        session
            .conn
            .execute(query)
            .map_err(|e| format!("Error executing UPDATE: {e}"))?;
        let changes = session.conn.changes();
        Ok(ToolOutput::new(
            format!("UPDATE successful. {changes} row(s) changed."),
            json!({ "changes": changes }),
        ))
    }

    fn delete_data(&self, arguments: &Option<Value>) -> Result<ToolOutput, String> {
        self.refuse_if_readonly("DELETE")?;
        let query = validated_query(arguments, StmtClass::Delete)?;

        let session = self.session.lock().unwrap();
        session
            .conn
            .execute(query)
            .map_err(|e| format!("Error executing DELETE: {e}"))?;
        let changes = session.conn.changes();
        Ok(ToolOutput::new(
            format!("DELETE successful. {changes} row(s) changed."),
            json!({ "changes": changes }),
        ))
    }

    fn schema_change(&self, arguments: &Option<Value>) -> Result<ToolOutput, String> {
        self.refuse_if_readonly("schema change")?;
        let query = validated_query(arguments, StmtClass::Schema)?;

        let session = self.session.lock().unwrap();
        session
            .conn
            .execute(query)
            .map_err(|e| format!("Error executing schema change: {e}"))?;
        let changes = session.conn.changes();
        Ok(ToolOutput::new(
            format!("Schema change successful. {changes} row(s) changed."),
            json!({ "changes": changes }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::thread;

    fn memory_server() -> TursoMcpServer {
        let (_io, conn) =
            Connection::from_uri(":memory:", DatabaseOpts::default(), Arc::new(SqliteDialect))
                .expect("open memory database");
        TursoMcpServer::new(conn, Arc::new(AtomicUsize::new(0)), false)
    }

    fn query_arg(sql: &str) -> Option<Value> {
        Some(json!({ "query": sql }))
    }

    fn seed_bench_orders(server: &TursoMcpServer) {
        let conn = server.session.lock().unwrap().conn.clone();
        conn.execute(
            "CREATE TABLE bench_orders (
                order_id INTEGER PRIMARY KEY,
                status TEXT NOT NULL,
                priority INTEGER NOT NULL
            )",
        )
        .unwrap();
        conn.execute("INSERT INTO bench_orders VALUES (1, 'READY', 1), (2, 'HOLD', 2)")
            .unwrap();
    }

    fn orders_dump(server: &TursoMcpServer) -> String {
        server
            .execute_query(&query_arg(
                "SELECT order_id, status, priority FROM bench_orders ORDER BY order_id",
            ))
            .expect("dumping the table succeeds")
            .text
    }

    #[test]
    fn update_data_rejects_trailing_delete() {
        let server = memory_server();
        seed_bench_orders(&server);

        let result = server.update_data(&query_arg(
            "UPDATE bench_orders SET status='DONE' WHERE order_id=1; DELETE FROM bench_orders WHERE order_id=2",
        ));

        let error = result.expect_err("a multi-statement call must be rejected");
        assert!(
            error.contains("Only a single UPDATE statement is allowed"),
            "got: {error}"
        );

        let dump = orders_dump(&server);
        assert!(
            dump.contains("1 | READY | 1"),
            "UPDATE must not run: {dump}"
        );
        assert!(
            dump.contains("2 | HOLD | 2"),
            "trailing DELETE must not run: {dump}"
        );
    }

    #[test]
    fn update_data_allows_semicolon_inside_string() {
        let server = memory_server();
        seed_bench_orders(&server);

        let result = server.update_data(&query_arg(
            "UPDATE bench_orders SET status='DONE; DELETE' WHERE order_id=1",
        ));
        let text = result.expect("a single UPDATE succeeds").text;
        assert!(text.starts_with("UPDATE successful."), "{text}");

        let dump = orders_dump(&server);
        assert!(dump.contains("1 | DONE; DELETE | 1"), "{dump}");
        assert!(dump.contains("2 | HOLD | 2"), "{dump}");
    }

    #[test]
    fn insert_data_rejects_trailing_delete() {
        let server = memory_server();
        seed_bench_orders(&server);

        let result = server.insert_data(&query_arg(
            "INSERT INTO bench_orders VALUES (3, 'NEW', 3); DELETE FROM bench_orders WHERE order_id=2",
        ));

        let error = result.expect_err("a multi-statement call must be rejected");
        assert!(
            error.contains("Only a single INSERT statement is allowed"),
            "got: {error}"
        );
        assert!(!orders_dump(&server).contains("3 | NEW | 3"));
        assert!(orders_dump(&server).contains("2 | HOLD | 2"));
    }

    #[test]
    fn delete_data_rejects_trailing_drop() {
        let server = memory_server();
        seed_bench_orders(&server);

        let result = server.delete_data(&query_arg(
            "DELETE FROM bench_orders WHERE order_id=1; DROP TABLE bench_orders",
        ));

        let error = result.expect_err("a multi-statement call must be rejected");
        assert!(
            error.contains("Only a single DELETE statement is allowed"),
            "got: {error}"
        );
        let dump = orders_dump(&server);
        assert!(dump.contains("1 | READY | 1"), "{dump}");
        assert!(dump.contains("2 | HOLD | 2"), "{dump}");
    }

    #[test]
    fn schema_change_rejects_trailing_delete() {
        let server = memory_server();
        seed_bench_orders(&server);

        let result = server.schema_change(&query_arg(
            "CREATE TABLE extra (id INTEGER); DELETE FROM bench_orders",
        ));

        let error = result.expect_err("a multi-statement call must be rejected");
        assert!(
            error.contains("Only a single schema modification statement is allowed"),
            "got: {error}"
        );
        assert!(orders_dump(&server).contains("1 | READY | 1"));
        assert!(orders_dump(&server).contains("2 | HOLD | 2"));
    }

    #[test]
    fn execute_query_rejects_trailing_delete() {
        let server = memory_server();
        seed_bench_orders(&server);

        let result = server.execute_query(&query_arg(
            "SELECT order_id FROM bench_orders WHERE order_id=1; DELETE FROM bench_orders WHERE order_id=2",
        ));

        let error = result.expect_err("a multi-statement call must be rejected");
        assert!(
            error.contains("Only a single SELECT query is allowed"),
            "got: {error}"
        );
        assert!(orders_dump(&server).contains("2 | HOLD | 2"));
    }

    #[test]
    fn update_data_accepts_single_update() {
        let server = memory_server();
        seed_bench_orders(&server);

        let result = server.update_data(&query_arg(
            "UPDATE bench_orders SET status='DONE' WHERE order_id=1",
        ));
        let text = result.expect("a single UPDATE succeeds").text;
        assert!(text.starts_with("UPDATE successful."), "{text}");
        assert!(orders_dump(&server).contains("1 | DONE | 1"));
        assert!(orders_dump(&server).contains("2 | HOLD | 2"));
    }

    /// `changes()` is one counter on the shared connection, overwritten by
    /// whichever statement last ran. Two threads sharing a server must each
    /// see their own write's count, never a count left behind by the other
    /// thread's write landing in between this thread's own execute and its
    /// read of `changes()`.
    ///
    /// No client can reach this today over the stdio transport, which reads
    /// and dispatches one line at a time, so two tool calls never actually
    /// overlap in production yet - that only becomes reachable once
    /// connection handling stops being one-at-a-time. Called directly like
    /// this, from two threads sharing one server, the race is real right
    /// now: against the lock-dropped-early code this reliably reports one
    /// thread's insert with the other's row count instead of its own.
    #[test]
    fn concurrent_tool_calls_see_their_own_changes_not_each_others() {
        let server = memory_server();
        server
            .schema_change(&query_arg("CREATE TABLE race_a (v INTEGER)"))
            .expect("create race_a");
        server
            .schema_change(&query_arg("CREATE TABLE race_b (v INTEGER)"))
            .expect("create race_b");

        const ITERATIONS: usize = 500;

        thread::scope(|scope| {
            let one_row_thread = scope.spawn(|| {
                for _ in 0..ITERATIONS {
                    let result = server
                        .insert_data(&query_arg("INSERT INTO race_a VALUES (1)"))
                        .expect("insert into race_a succeeds");
                    assert_eq!(
                        result.structured["changes"], 1,
                        "a one-row insert must report its own one row changed"
                    );
                }
            });
            let two_row_thread = scope.spawn(|| {
                for _ in 0..ITERATIONS {
                    let result = server
                        .insert_data(&query_arg("INSERT INTO race_b VALUES (1), (2)"))
                        .expect("insert into race_b succeeds");
                    assert_eq!(
                        result.structured["changes"], 2,
                        "a two-row insert must report its own two rows changed"
                    );
                }
            });
            one_row_thread.join().expect("thread does not panic");
            two_row_thread.join().expect("thread does not panic");
        });
    }

    fn call(server: &TursoMcpServer, name: &str, arguments: Value) -> Value {
        let raw = server
            .handle_message(
                &json!({
                    "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                    "params": { "name": name, "arguments": arguments },
                })
                .to_string(),
            )
            .expect("a tools/call is answered");
        serde_json::from_str::<Value>(&raw).expect("the answer is JSON")["result"].clone()
    }

    /// The point of the whole contract: a rejected call and a successful one
    /// were previously both successful results whose text merely differed.
    #[test]
    fn a_rejected_call_is_marked_as_an_error_and_a_successful_one_is_not() {
        let server = memory_server();
        seed_bench_orders(&server);

        let rejected = call(
            &server,
            "update_data",
            json!({ "query": "UPDATE bench_orders SET status='X'; DROP TABLE bench_orders" }),
        );
        assert_eq!(rejected["isError"], true, "{rejected}");
        assert!(
            rejected["structuredContent"].is_null(),
            "a failure carries no structured answer: {rejected}"
        );

        let accepted = call(
            &server,
            "update_data",
            json!({ "query": "UPDATE bench_orders SET status='DONE' WHERE order_id=1" }),
        );
        assert_eq!(accepted["isError"], false, "{accepted}");
        assert_eq!(accepted["structuredContent"]["changes"], 1);
    }

    /// Rows arrive typed, not as an ASCII table a model has to parse back.
    #[test]
    fn query_results_are_typed_and_match_the_declared_output_schema() {
        let server = memory_server();
        seed_bench_orders(&server);

        let result = call(
            &server,
            "execute_query",
            json!({ "query": "SELECT order_id, status FROM bench_orders ORDER BY order_id" }),
        );
        let structured = &result["structuredContent"];

        assert_eq!(structured["columns"], json!(["order_id", "status"]));
        assert_eq!(structured["row_count"], 2);
        assert_eq!(structured["rows"][0][0], 1, "an integer stays a number");
        assert_eq!(structured["rows"][0][1], "READY", "text stays a string");
        assert!(
            result["content"][0]["text"].as_str().unwrap().contains('|'),
            "the human-readable table is still there for a person"
        );
    }

    /// A client should be able to tell a read from a destructive write without
    /// calling either one.
    #[test]
    fn the_catalog_marks_reads_and_destructive_writes_apart() {
        let server = memory_server();
        let raw = server
            .handle_message(
                &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} })
                    .to_string(),
            )
            .expect("tools/list is answered");
        let listed = serde_json::from_str::<Value>(&raw).unwrap();
        let tools = listed["result"]["tools"].as_array().unwrap().clone();

        let find = |name: &str| {
            tools
                .iter()
                .find(|t| t["name"] == name)
                .unwrap_or_else(|| panic!("{name} is missing from the catalog"))
                .clone()
        };

        let read = find("execute_query");
        assert_eq!(read["annotations"]["readOnlyHint"], true);
        assert_eq!(read["annotations"]["destructiveHint"], false);
        assert_eq!(
            read["annotations"]["idempotentHint"], true,
            "running the same read twice changes nothing"
        );
        assert!(read["outputSchema"]["properties"]["rows"].is_object());

        let destructive = find("delete_data");
        assert_eq!(destructive["annotations"]["readOnlyHint"], false);
        assert_eq!(destructive["annotations"]["destructiveHint"], true);
        assert_eq!(
            destructive["annotations"]["idempotentHint"], false,
            "a second identical write is not a no-op"
        );

        for tool in &tools {
            assert!(
                tool["outputSchema"].is_object(),
                "{} has no outputSchema",
                tool["name"]
            );
            assert_eq!(tool["annotations"]["openWorldHint"], false);
        }
    }
    fn readonly_server() -> TursoMcpServer {
        let (_io, conn) =
            Connection::from_uri(":memory:", DatabaseOpts::default(), Arc::new(SqliteDialect))
                .expect("open memory database");
        TursoMcpServer::new(conn, Arc::new(AtomicUsize::new(0)), true)
    }

    /// Both directions: a write is refused under --readonly, and a read is not.
    #[test]
    fn readonly_refuses_writes_and_still_serves_reads() {
        let server = readonly_server();

        // Every write tool, not just one: with a single sample the other
        // three guards could be deleted and this suite would stay green.
        for (tool, result) in [
            (
                "insert_data",
                server.insert_data(&query_arg("INSERT INTO t VALUES (1)")),
            ),
            (
                "update_data",
                server.update_data(&query_arg("UPDATE t SET x=1")),
            ),
            (
                "delete_data",
                server.delete_data(&query_arg("DELETE FROM t")),
            ),
            (
                "schema_change",
                server.schema_change(&query_arg("CREATE TABLE t (x)")),
            ),
        ] {
            let refused = result.expect_err("{tool} must be refused under --readonly");
            assert!(refused.contains("--readonly"), "{tool}: {refused}");
        }

        assert!(
            server.current_database().is_ok(),
            "a read must still work under --readonly"
        );
    }

    /// The flag governs the whole server, not just the connection it started
    /// with: opening another database must not hand back a writable one.
    #[test]
    fn readonly_does_not_open_a_writable_connection_through_a_uri() {
        let server = readonly_server();
        let dir = std::env::temp_dir().join(format!("mcp-ro-{}", std::process::id()));

        let result = server.open_database(&Some(json!({
            "path": format!("file:{}/x.db?mode=rwc", dir.display()),
        })));

        assert!(result.is_err(), "a URI path must not bypass --readonly");
        assert!(
            !dir.exists(),
            "--readonly must not create directories on disk"
        );
    }

    #[test]
    fn a_query_stops_at_the_row_cap_and_says_that_it_did() {
        let server = memory_server();
        server
            .schema_change(&query_arg("CREATE TABLE many (n INTEGER)"))
            .expect("create");
        for n in 0..5 {
            server
                .insert_data(&query_arg(&format!("INSERT INTO many VALUES ({n})")))
                .expect("insert");
        }

        let capped = server
            .execute_query(&Some(
                json!({ "query": "SELECT n FROM many", "max_rows": 2 }),
            ))
            .expect("query succeeds");
        assert_eq!(capped.structured["row_count"], 2);
        assert_eq!(capped.structured["truncated"], true);
        assert!(
            capped.text.contains("truncated at 2 rows"),
            "{}",
            capped.text
        );

        let whole = server
            .execute_query(&query_arg("SELECT n FROM many"))
            .expect("query succeeds");
        assert_eq!(whole.structured["row_count"], 5);
        assert_eq!(
            whole.structured["truncated"], false,
            "an answer under the cap is not marked truncated"
        );
    }

    /// A cut value gets a different shape, not a shorter string - otherwise it
    /// reads as the whole value and can be acted on as if it were.
    #[test]
    fn an_oversized_cell_is_shaped_so_it_cannot_pass_for_the_whole_value() {
        let server = memory_server();
        server
            .schema_change(&query_arg("CREATE TABLE big (t TEXT)"))
            .expect("create");
        let long = "x".repeat(MAX_CELL_BYTES * 2);
        server
            .insert_data(&query_arg(&format!("INSERT INTO big VALUES ('{long}')")))
            .expect("insert");

        let result = server
            .execute_query(&query_arg("SELECT t FROM big"))
            .expect("query succeeds");
        let cell = &result.structured["rows"][0][0];

        assert!(cell.is_object(), "a cut value must not still be a string");
        assert_eq!(cell["truncated"], true);
        assert_eq!(cell["bytes"], long.len());
        assert!(cell["text"].as_str().unwrap().len() < long.len());
    }

    #[test]
    fn the_catalog_says_which_tools_readonly_has_taken_away() {
        let server = readonly_server();
        let raw = server
            .handle_message(
                &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} })
                    .to_string(),
            )
            .expect("tools/list is answered");
        let tools = serde_json::from_str::<Value>(&raw).unwrap()["result"]["tools"]
            .as_array()
            .unwrap()
            .clone();

        let described = |name: &str| {
            tools.iter().find(|t| t["name"] == name).unwrap()["description"]
                .as_str()
                .unwrap()
                .to_string()
        };
        assert!(described("delete_data").contains("UNAVAILABLE"));
        assert!(
            !described("execute_query").contains("UNAVAILABLE"),
            "a read is still available and must not be marked otherwise"
        );
    }
    /// The cap is in bytes, so the cut has to be too. Counting characters
    /// instead let a multi-byte value keep several times its budget.
    #[test]
    fn a_multibyte_cell_is_cut_to_the_byte_budget_not_the_character_count() {
        let server = memory_server();
        server
            .schema_change(&query_arg("CREATE TABLE wide (t TEXT)"))
            .expect("create");
        // Three bytes per character, so a character-based cut would keep
        // roughly three times the budget.
        let long = "\u{4e16}".repeat(MAX_CELL_BYTES);
        server
            .insert_data(&query_arg(&format!("INSERT INTO wide VALUES ('{long}')")))
            .expect("insert");

        let result = server
            .execute_query(&query_arg("SELECT t FROM wide"))
            .expect("query succeeds");
        let cell = &result.structured["rows"][0][0];

        let kept = cell["text"].as_str().expect("a cut cell keeps a prefix");
        assert!(
            kept.len() <= MAX_CELL_BYTES,
            "kept {} bytes against a {MAX_CELL_BYTES} byte budget",
            kept.len()
        );
        assert_eq!(
            cell["bytes"],
            long.len(),
            "the reported size is the real one"
        );
    }
    /// The catalog is amended from the same list the refusal uses, so the two
    /// cannot disagree. open_database is the case that caught this: it writes
    /// in principle, but is never refused - it opens read-only instead.
    #[test]
    fn the_catalog_marks_exactly_the_tools_that_are_refused() {
        let server = readonly_server();
        let raw = server
            .handle_message(
                &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} })
                    .to_string(),
            )
            .expect("tools/list is answered");
        let tools = serde_json::from_str::<Value>(&raw).unwrap()["result"]["tools"]
            .as_array()
            .unwrap()
            .clone();

        for tool in &tools {
            let name = tool["name"].as_str().unwrap();
            let marked = tool["description"]
                .as_str()
                .unwrap()
                .contains("UNAVAILABLE");
            assert_eq!(
                marked,
                REFUSED_WHEN_READONLY.contains(&name),
                "{name}: marked unavailable = {marked}, actually refused = {}",
                REFUSED_WHEN_READONLY.contains(&name)
            );
        }

        assert!(
            server
                .open_database(&Some(json!({ "path": ":memory:" })))
                .is_ok(),
            "open_database is not refused, so it must not be marked unavailable"
        );
    }

    /// The cap governs both representations of a value. Capping only the
    /// structured one left a huge cell truncated in one field and whole in the
    /// other - the same shape as the bytes/chars bug, one field over.
    #[test]
    fn an_oversized_cell_is_cut_in_the_text_field_too() {
        let server = memory_server();
        server
            .schema_change(&query_arg("CREATE TABLE big (t TEXT)"))
            .expect("create");
        let long = "y".repeat(MAX_CELL_BYTES * 4);
        server
            .insert_data(&query_arg(&format!("INSERT INTO big VALUES ('{long}')")))
            .expect("insert");

        let result = server
            .execute_query(&query_arg("SELECT t FROM big"))
            .expect("query succeeds");

        assert!(
            result.text.len() < long.len(),
            "the text field kept {} bytes of a {} byte value",
            result.text.len(),
            long.len()
        );
        assert!(result.text.contains("bytes)"), "{}", result.text);
    }
    /// Display renders NULL as an empty string, so without a placeholder a
    /// NULL and an empty text value are the same cell in the prose table.
    #[test]
    fn null_and_an_empty_string_are_not_the_same_cell() {
        let server = memory_server();
        server
            .schema_change(&query_arg("CREATE TABLE n (a TEXT)"))
            .expect("create");
        server
            .insert_data(&query_arg("INSERT INTO n VALUES (NULL), ('')"))
            .expect("insert");

        let result = server
            .execute_query(&query_arg("SELECT a FROM n"))
            .expect("query succeeds");

        assert!(result.text.contains("NULL"), "{}", result.text);
        assert!(
            result.structured["rows"][0][0].is_null(),
            "the structured view keeps NULL as null"
        );
        assert_eq!(
            result.structured["rows"][1][0], "",
            "and keeps the empty string as a string"
        );
    }
}

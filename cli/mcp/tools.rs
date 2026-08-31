use super::protocol::{
    result_meta, CallToolRequest, JsonRpcError, JsonRpcRequest, JsonRpcResponse, CACHE_TTL_MS,
    INVALID_PARAMS,
};
use super::TursoMcpServer;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use turso_core::{
    Connection, Database, DatabaseOpts, Numeric, OpenFlags, SqliteDialect, Value as DbValue,
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

/// SQL values carry types that JSON can mostly hold. Blobs are the exception:
/// they are tagged rather than inlined, so a model cannot mistake one for text
/// it can act on.
fn sql_value_to_json(value: &DbValue) -> Value {
    match value {
        DbValue::Null => Value::Null,
        DbValue::Numeric(Numeric::Integer(n)) => json!(n),
        DbValue::Numeric(Numeric::Float(f)) => json!(f64::from(*f)),
        DbValue::Text(text) => json!(text.to_string()),
        DbValue::Blob(_) => json!({ "blob": value.to_string() }),
    }
}

impl TursoMcpServer {
    pub(crate) fn handle_list_tools(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        JsonRpcResponse::success(
            request.id,
            json!({
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
                        "outputSchema": { "type": "object", "properties": { "columns": { "type": "array", "items": { "type": "string" } }, "rows": { "type": "array", "items": { "type": "array" }, "description": "Row values by column. A blob is an object with a `blob` key, never a bare string." }, "row_count": { "type": "integer" } }, "required": ["columns", "rows", "row_count"] },
                        "description": "Execute a read-only SELECT query",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "query": {
                                    "type": "string",
                                    "description": "The SELECT query to execute"
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
            }),
        )
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

        if path != ":memory:" {
            if let Some(parent) = PathBuf::from(&path).parent() {
                if !parent.exists() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("Failed to create parent directories: {e}"))?;
                }
            }
        }

        let conn = if path == ":memory:" || path.contains([':', '?', '&', '#']) {
            Connection::from_uri(&path, DatabaseOpts::default(), Arc::new(SqliteDialect))
                .map_err(|e| format!("Failed to open database '{path}': {e}"))?
                .1
        } else {
            let (_io, db) = Database::open_new(
                &path,
                None::<&str>,
                OpenFlags::default(),
                DatabaseOpts::new().with_autovacuum(false),
                None,
                Arc::new(SqliteDialect),
            )
            .map_err(|e| format!("Failed to open database '{path}': {e}"))?;
            db.connect()
                .map_err(|e| format!("Failed to connect to database '{path}': {e}"))?
        };

        *self.conn.lock().unwrap() = conn;
        *self.current_db_path.lock().unwrap() = Some(path.clone());

        Ok(ToolOutput::new(
            format!("Successfully opened database: {path}"),
            json!({ "path": path }),
        ))
    }

    fn current_database(&self) -> Result<ToolOutput, String> {
        let path = self
            .current_db_path
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| ":memory:".to_string());
        Ok(ToolOutput::new(
            format!("Current database: {path}"),
            json!({ "path": path }),
        ))
    }

    fn list_tables(&self) -> Result<ToolOutput, String> {
        let query = "SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY 1";

        let conn = self.conn.lock().unwrap().clone();
        let mut rows = conn
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

        let conn = self.conn.lock().unwrap().clone();
        let mut rows = conn
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

        let conn = self.conn.lock().unwrap().clone();
        let mut rows = conn
            .query(query)
            .map_err(|e| format!("Error executing query: {e}"))?
            .ok_or_else(|| "No results returned from the query".to_string())?;

        let columns: Vec<String> = (0..rows.num_columns())
            .map(|i| rows.get_column_name(i).to_string())
            .collect();

        let mut typed: Vec<Value> = Vec::new();
        let mut rendered: Vec<Vec<String>> = Vec::new();
        rows.run_with_row_callback(|row| {
            let values: Vec<&DbValue> = row.get_values().collect();
            typed.push(Value::Array(
                values.iter().map(|v| sql_value_to_json(v)).collect(),
            ));
            rendered.push(values.iter().map(|v| v.to_string()).collect());
            Ok(())
        })
        .map_err(|e| e.to_string())?;

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
        Ok(ToolOutput::new(
            text,
            json!({ "columns": columns, "rows": typed, "row_count": row_count }),
        ))
    }

    fn insert_data(&self, arguments: &Option<Value>) -> Result<ToolOutput, String> {
        let query = validated_query(arguments, StmtClass::Insert)?;

        let conn = self.conn.lock().unwrap().clone();
        conn.execute(query)
            .map_err(|e| format!("Error executing INSERT: {e}"))?;
        let changes = conn.changes();
        Ok(ToolOutput::new(
            format!("INSERT successful. {changes} row(s) changed."),
            json!({ "changes": changes }),
        ))
    }

    fn update_data(&self, arguments: &Option<Value>) -> Result<ToolOutput, String> {
        let query = validated_query(arguments, StmtClass::Update)?;

        let conn = self.conn.lock().unwrap().clone();
        conn.execute(query)
            .map_err(|e| format!("Error executing UPDATE: {e}"))?;
        let changes = conn.changes();
        Ok(ToolOutput::new(
            format!("UPDATE successful. {changes} row(s) changed."),
            json!({ "changes": changes }),
        ))
    }

    fn delete_data(&self, arguments: &Option<Value>) -> Result<ToolOutput, String> {
        let query = validated_query(arguments, StmtClass::Delete)?;

        let conn = self.conn.lock().unwrap().clone();
        conn.execute(query)
            .map_err(|e| format!("Error executing DELETE: {e}"))?;
        let changes = conn.changes();
        Ok(ToolOutput::new(
            format!("DELETE successful. {changes} row(s) changed."),
            json!({ "changes": changes }),
        ))
    }

    fn schema_change(&self, arguments: &Option<Value>) -> Result<ToolOutput, String> {
        let query = validated_query(arguments, StmtClass::Schema)?;

        let conn = self.conn.lock().unwrap().clone();
        conn.execute(query)
            .map_err(|e| format!("Error executing schema change: {e}"))?;
        let changes = conn.changes();
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

    fn memory_server() -> TursoMcpServer {
        let (_io, conn) =
            Connection::from_uri(":memory:", DatabaseOpts::default(), Arc::new(SqliteDialect))
                .expect("open memory database");
        TursoMcpServer::new(conn, Arc::new(AtomicUsize::new(0)))
    }

    fn query_arg(sql: &str) -> Option<Value> {
        Some(json!({ "query": sql }))
    }

    fn seed_bench_orders(server: &TursoMcpServer) {
        let conn = server.conn.lock().unwrap().clone();
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
}

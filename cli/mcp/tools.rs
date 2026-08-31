use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};
use turso_core::{Connection, Database, Numeric, OpenFlags, SqliteDialect, Value as DbValue};
use turso_parser::ast::{Cmd, Stmt};
use turso_parser::parser::Parser;

use super::TursoMcpServer;

/// `execute_query` stops collecting rows once it hits this many, so a `SELECT *`
/// over a huge table cannot exhaust memory or blow out the model's context.
const DEFAULT_MAX_ROWS: usize = 200;

/// Individual cell values longer than this are cut down before they reach the
/// model, for the same reason: one giant TEXT or BLOB value can blow out
/// memory and context just as easily as too many rows can.
const MAX_VALUE_BYTES: usize = 1024;

/// How many raw bytes of an over-sized blob to show as a hex preview.
const BLOB_PREVIEW_BYTES: usize = 32;

#[derive(Debug)]
pub struct ToolOutput {
    pub text: String,
    pub structured: Value,
}

impl ToolOutput {
    fn new(text: impl Into<String>, structured: Value) -> Self {
        Self {
            text: text.into(),
            structured,
        }
    }
}

/// Deterministic order: clients cache this list and prompt caches hit more often
/// when it does not move around. `readonly` mirrors the server's `--readonly`
/// flag so the write tools' descriptions say plainly that they will fail.
pub fn catalog(readonly: bool) -> Value {
    let write_note = |description: &str| -> String {
        if readonly {
            format!(
                "{description} Unavailable: this server was started with --readonly, so writes are rejected."
            )
        } else {
            description.to_string()
        }
    };

    json!([
        tool(
            "current_database",
            "Current database",
            "Get the path of the currently open database",
            json!({ "type": "object", "properties": {}, "required": [] }),
            json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }),
            Access::ReadOnly,
        ),
        tool(
            "delete_data",
            "Delete rows",
            &write_note("Delete rows with a single DELETE statement per call."),
            query_schema("The DELETE statement to execute"),
            changes_schema(),
            Access::Destructive,
        ),
        tool(
            "describe_table",
            "Describe table",
            "Describe the columns of a specific table. Call list_tables first to find table names.",
            json!({
                "type": "object",
                "properties": {
                    "table_name": {
                        "type": "string",
                        "description": "Name of the table to describe"
                    }
                },
                "required": ["table_name"]
            }),
            json!({
                "type": "object",
                "properties": {
                    "table": { "type": "string" },
                    "columns": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": { "type": "string" },
                                "type": { "type": "string" },
                                "nullable": { "type": "boolean" },
                                "default": { "description": "Default value, or null when there is none" },
                                "primary_key": { "type": "boolean" },
                                "generated": { "type": "boolean" }
                            },
                            "required": ["name", "type", "nullable", "primary_key", "generated"]
                        }
                    }
                },
                "required": ["table", "columns"]
            }),
            Access::ReadOnly,
        ),
        tool(
            "execute_query",
            "Run a SELECT query",
            "Run one SELECT, EXPLAIN, or EXPLAIN QUERY PLAN statement per call. \
The full schema, including indexes, views, and triggers, is available as \
`SELECT name, sql FROM sqlite_schema`. Returns at most `max_rows` rows \
(default 200); when more rows exist, `truncated` is set and the text output \
says so, so add LIMIT/OFFSET or an aggregate instead of relying on a bigger cap.",
            json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The SELECT, EXPLAIN, or EXPLAIN QUERY PLAN statement to execute"
                    },
                    "max_rows": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Maximum number of rows to return (default 200)"
                    }
                },
                "required": ["query"]
            }),
            json!({
                "type": "object",
                "properties": {
                    "columns": { "type": "array", "items": { "type": "string" } },
                    "rows": {
                        "type": "array",
                        "items": {
                            "type": "array",
                            "items": {
                                "description": "NULL, a number, a string, or {\"blob\": \"<hex>\"}"
                            }
                        }
                    },
                    "row_count": { "type": "integer" },
                    "truncated": {
                        "type": "boolean",
                        "description": "True when more rows existed than the cap allowed"
                    }
                },
                "required": ["columns", "rows", "row_count", "truncated"]
            }),
            Access::ReadOnly,
        ),
        tool(
            "insert_data",
            "Insert rows",
            &write_note("Insert new rows with a single INSERT statement per call."),
            query_schema("The INSERT statement to execute"),
            changes_schema(),
            Access::Destructive,
        ),
        tool(
            "list_tables",
            "List tables",
            "List all tables in the database",
            json!({ "type": "object", "properties": {}, "required": [] }),
            json!({
                "type": "object",
                "properties": {
                    "tables": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["tables"]
            }),
            Access::ReadOnly,
        ),
        tool(
            "open_database",
            "Open database",
            "Open or create a database file, creating parent directories if needed. Reports \
whether the file already existed via `created` in the structured result.",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the database file (absolute or relative). Use ':memory:' for in-memory database."
                    }
                },
                "required": ["path"]
            }),
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "created": {
                        "type": "boolean",
                        "description": "True when this call created a new empty database instead of opening an existing one"
                    }
                },
                "required": ["path", "created"]
            }),
            Access::Write,
        ),
        tool(
            "schema_change",
            "Change schema",
            &write_note(
                "Run a single schema statement per call: CREATE/ALTER/DROP TABLE, INDEX, \
VIEW, TRIGGER, or virtual table."
            ),
            query_schema("The schema modification statement to execute"),
            json!({
                "type": "object",
                "properties": { "statement": { "type": "string" } },
                "required": ["statement"]
            }),
            Access::Destructive,
        ),
        tool(
            "update_data",
            "Update rows",
            &write_note("Update existing rows with a single UPDATE statement per call."),
            query_schema("The UPDATE statement to execute"),
            changes_schema(),
            Access::Destructive,
        ),
    ])
}

enum Access {
    ReadOnly,
    Write,
    Destructive,
}

fn tool(
    name: &str,
    title: &str,
    description: &str,
    input_schema: Value,
    output_schema: Value,
    access: Access,
) -> Value {
    let (read_only, destructive) = match access {
        Access::ReadOnly => (true, false),
        Access::Write => (false, false),
        Access::Destructive => (false, true),
    };
    json!({
        "name": name,
        "title": title,
        "description": description,
        "inputSchema": input_schema,
        "outputSchema": output_schema,
        "annotations": {
            "title": title,
            "readOnlyHint": read_only,
            "destructiveHint": destructive,
            "idempotentHint": false,
            "openWorldHint": false
        }
    })
}

fn query_schema(description: &str) -> Value {
    json!({
        "type": "object",
        "properties": {
            "query": { "type": "string", "description": description }
        },
        "required": ["query"]
    })
}

fn changes_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "changes": { "type": "integer" } },
        "required": ["changes"]
    })
}

impl TursoMcpServer {
    /// `Err` becomes a tool result with `isError: true`, which is how a model
    /// learns the call failed. Unknown tool names are a protocol error instead,
    /// so they are rejected before reaching here.
    pub(super) fn call_tool(
        &self,
        name: &str,
        arguments: &Option<Value>,
    ) -> Option<Result<ToolOutput, String>> {
        let result = match name {
            "current_database" => self.current_database(),
            "delete_data" => self.mutate(arguments, StmtClass::Delete, "DELETE"),
            "describe_table" => self.describe_table(arguments),
            "execute_query" => self.execute_query(arguments),
            "insert_data" => self.mutate(arguments, StmtClass::Insert, "INSERT"),
            "list_tables" => self.list_tables(),
            "open_database" => self.open_database(arguments),
            "schema_change" => self.schema_change(arguments),
            "update_data" => self.mutate(arguments, StmtClass::Update, "UPDATE"),
            _ => return None,
        };
        Some(result)
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

    fn describe_table(&self, arguments: &Option<Value>) -> Result<ToolOutput, String> {
        let table_name = string_arg(arguments, "table_name")?;

        // Use table_xinfo to include generated columns (table_info hides them). The
        // table name is quoted so names with spaces, quotes, or reserved words
        // (e.g. `order`, `my table`) don't break the PRAGMA's own parsing.
        let query = format!("PRAGMA table_xinfo({})", quote_ident(table_name));

        let conn = self.conn.lock().unwrap().clone();
        let Some(mut rows) = conn
            .query(&query)
            .map_err(|e| format!("Error querying database: {e}"))?
        else {
            return Err(table_not_found(table_name));
        };

        let mut lines = Vec::new();
        let mut columns = Vec::new();
        rows.run_with_row_callback(|row| {
            let (Ok(col_name), Ok(col_type), Ok(not_null), Ok(default_value), Ok(pk), Ok(hidden)) = (
                row.get::<&DbValue>(1),
                row.get::<&DbValue>(2),
                row.get::<&DbValue>(3),
                row.get::<&DbValue>(4),
                row.get::<&DbValue>(5),
                row.get::<&DbValue>(6),
            ) else {
                return Ok(());
            };

            let default_str = if matches!(default_value, DbValue::Null) {
                "".to_string()
            } else {
                format!("DEFAULT {default_value}")
            };
            let not_null = matches!(not_null, DbValue::Numeric(Numeric::Integer(1)));
            let primary_key = matches!(pk, DbValue::Numeric(Numeric::Integer(1)));
            let generated = matches!(hidden, DbValue::Numeric(Numeric::Integer(2 | 3)));
            let virtual_generated = matches!(hidden, DbValue::Numeric(Numeric::Integer(2)));

            lines.push(
                format!(
                    "{} {} {} {} {}{}",
                    col_name,
                    col_type,
                    if not_null { "NOT NULL" } else { "NULL" },
                    default_str,
                    if primary_key { "PRIMARY KEY" } else { "" },
                    if virtual_generated {
                        " VIRTUAL GENERATED"
                    } else {
                        ""
                    }
                )
                .trim()
                .to_string(),
            );
            columns.push(json!({
                "name": col_name.to_string(),
                "type": col_type.to_string(),
                "nullable": !not_null,
                "default": json_value(default_value),
                "primary_key": primary_key,
                "generated": generated,
            }));

            Ok(())
        })
        .map_err(|e| e.to_string())?;

        if columns.is_empty() {
            return Err(table_not_found(table_name));
        }

        Ok(ToolOutput::new(
            format!("Table '{table_name}' columns:\n{}", lines.join("\n")),
            json!({ "table": table_name, "columns": columns }),
        ))
    }

    fn execute_query(&self, arguments: &Option<Value>) -> Result<ToolOutput, String> {
        let query = validated_query(arguments, StmtClass::Select)?;
        let max_rows = max_rows_arg(arguments)?;

        let conn = self.conn.lock().unwrap().clone();
        let Some(mut rows) = conn
            .query(query)
            .map_err(|e| format!("Error executing query: {e}"))?
        else {
            return Ok(ToolOutput::new(
                "No results returned from the query",
                json!({ "columns": [], "rows": [], "row_count": 0, "truncated": false }),
            ));
        };

        let headers: Vec<String> = (0..rows.num_columns())
            .map(|i| rows.get_column_name(i).to_string())
            .collect();

        let mut text_rows = Vec::new();
        let mut json_rows = Vec::new();
        let mut truncated = false;
        rows.run_with_row_callback(|row| {
            if json_rows.len() >= max_rows {
                truncated = true;
                return Ok(());
            }
            let mut text_row = Vec::new();
            let mut json_row = Vec::new();
            for value in row.get_values() {
                let (text_cell, json_cell) = display_cell(value);
                text_row.push(text_cell);
                json_row.push(json_cell);
            }
            text_rows.push(text_row);
            json_rows.push(json_row);
            Ok(())
        })
        .map_err(|e| e.to_string())?;

        let mut text = String::new();
        if !headers.is_empty() {
            let header_line = headers.join(" | ");
            text.push_str(&header_line);
            text.push('\n');
            text.push_str(&"-".repeat(header_line.len()));
            text.push('\n');
        }
        for row in &text_rows {
            text.push_str(&row.join(" | "));
            text.push('\n');
        }
        if text.is_empty() {
            text = "No results returned from the query".to_string();
        }
        if truncated {
            text.push_str(&format!(
                "Showing the first {max_rows} rows; more exist. Add LIMIT/OFFSET or an aggregate.\n"
            ));
        }

        let row_count = json_rows.len();
        Ok(ToolOutput::new(
            text,
            json!({
                "columns": headers,
                "rows": json_rows,
                "row_count": row_count,
                "truncated": truncated,
            }),
        ))
    }

    fn list_tables(&self) -> Result<ToolOutput, String> {
        let query = "SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY 1";

        let conn = self.conn.lock().unwrap().clone();
        let Some(mut rows) = conn
            .query(query)
            .map_err(|e| format!("Error querying database: {e}"))?
        else {
            return Err("No results returned from the query".to_string());
        };

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

    fn open_database(&self, arguments: &Option<Value>) -> Result<ToolOutput, String> {
        let path = string_arg(arguments, "path")?.to_string();

        if path != ":memory:" {
            let db_path = PathBuf::from(&path);
            if let Some(parent) = db_path.parent() {
                if !parent.as_os_str().is_empty() && !parent.exists() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("Failed to create parent directories: {e}"))?;
                }
            }
        }

        // A `:memory:` database starts empty every time it is opened, so it is
        // always "created"; for a real file, check before opening since opening
        // is what creates it.
        let created = path == ":memory:" || !Path::new(&path).exists();

        let conn = if path == ":memory:" || path.contains([':', '?', '&', '#']) {
            Connection::from_uri(&path, self.db_opts, Arc::new(SqliteDialect))
                .map_err(|e| format!("Failed to open database '{path}': {e}"))?
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
                self.db_opts.turso_cli(),
                None,
                Arc::new(SqliteDialect),
            )
            .map_err(|e| format!("Failed to open database '{path}': {e}"))?;
            db.connect()
                .map_err(|e| format!("Failed to connect to database '{path}': {e}"))?
        };

        *self.conn.lock().unwrap() = conn;
        *self.current_db_path.lock().unwrap() = Some(path.clone());

        let message = if created {
            format!("Created new empty database at {path}")
        } else {
            format!("Opened existing database {path}")
        };
        Ok(ToolOutput::new(
            message,
            json!({ "path": path, "created": created }),
        ))
    }

    fn schema_change(&self, arguments: &Option<Value>) -> Result<ToolOutput, String> {
        self.require_writable()?;
        let query = validated_query(arguments, StmtClass::Schema)?;

        let conn = self.conn.lock().unwrap().clone();
        conn.execute(query)
            .map_err(|e| format!("Error executing schema change: {e}"))?;

        Ok(ToolOutput::new(
            "Schema change successful.",
            json!({ "statement": query }),
        ))
    }

    fn mutate(
        &self,
        arguments: &Option<Value>,
        class: StmtClass,
        verb: &str,
    ) -> Result<ToolOutput, String> {
        self.require_writable()?;
        let query = validated_query(arguments, class)?;

        let conn = self.conn.lock().unwrap().clone();
        conn.execute(query)
            .map_err(|e| format!("Error executing {verb}: {e}"))?;

        let changes = conn.changes();
        let rows = if changes == 1 { "row" } else { "rows" };
        Ok(ToolOutput::new(
            format!("{verb} successful. {changes} {rows} changed."),
            json!({ "changes": changes }),
        ))
    }

    /// Write tools call this first: `--readonly` is a promise to the operator
    /// that nothing on disk changes, so it must hold regardless of which
    /// database `open_database` has switched the connection to.
    fn require_writable(&self) -> Result<(), String> {
        if self.readonly {
            return Err(
                "This server was started with --readonly, so writes are rejected. \
Use execute_query to read data."
                    .to_string(),
            );
        }
        Ok(())
    }
}

fn string_arg<'a>(arguments: &'a Option<Value>, name: &str) -> Result<&'a str, String> {
    let Some(arguments) = arguments else {
        return Err(format!("Missing {name} parameter"));
    };
    match arguments.get(name) {
        Some(Value::String(value)) => Ok(value),
        _ => Err(format!("Missing or invalid {name} parameter")),
    }
}

fn validated_query(arguments: &Option<Value>, class: StmtClass) -> Result<&str, String> {
    let sql = string_arg(arguments, "query")?;
    require_single_stmt(sql, class)?;
    Ok(sql)
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
        Ok(Some(_)) => return Err(class.single_statement_error()),
        Err(e) => return Err(format!("Failed to parse SQL: {e}")),
    }
    match cmd {
        Cmd::Stmt(stmt) if StmtClass::of(&stmt) == Some(class) => Ok(()),
        // EXPLAIN and EXPLAIN QUERY PLAN execute nothing; execute_query is the
        // only tool that reads, so it is the only one that gets to see plans.
        Cmd::Explain(_) | Cmd::ExplainQueryPlan { .. } if class == StmtClass::Select => Ok(()),
        Cmd::Stmt(_) | Cmd::Explain(_) | Cmd::ExplainQueryPlan { .. } => {
            Err(class.wrong_class_error())
        }
    }
}

fn json_value(value: &DbValue) -> Value {
    match value {
        DbValue::Null => Value::Null,
        DbValue::Numeric(Numeric::Integer(i)) => json!(i),
        DbValue::Numeric(Numeric::Float(f)) => json!(**f),
        DbValue::Text(text) => json!(text.as_str()),
        DbValue::Blob(blob) => json!({ "blob": hex::encode(&blob[..]) }),
    }
}

/// Text and json representation of one query result cell, with long values
/// cut down so one giant value can't blow out memory or context the way too
/// many rows can (see `DEFAULT_MAX_ROWS`).
fn display_cell(value: &DbValue) -> (String, Value) {
    match value {
        DbValue::Text(text) => {
            let s = text.as_str();
            if s.len() <= MAX_VALUE_BYTES {
                (s.to_string(), json!(s))
            } else {
                let shown = format!(
                    "{}... [truncated, {} bytes total]",
                    truncate_str(s, MAX_VALUE_BYTES),
                    s.len()
                );
                (shown.clone(), json!(shown))
            }
        }
        DbValue::Blob(blob) => {
            if blob.len() <= MAX_VALUE_BYTES {
                let hex = hex::encode(&blob[..]);
                (
                    format!("<blob {} bytes: {hex}>", blob.len()),
                    json!({ "blob": hex }),
                )
            } else {
                let preview = hex::encode(&blob[..BLOB_PREVIEW_BYTES.min(blob.len())]);
                let text = format!("<blob {} bytes: {preview}...>", blob.len());
                let json = json!({ "blob": preview, "bytes": blob.len(), "truncated": true });
                (text, json)
            }
        }
        _ => (value.to_string(), json_value(value)),
    }
}

/// Cuts `s` down to at most `max_bytes` bytes without splitting a UTF-8
/// character in half.
fn truncate_str(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn max_rows_arg(arguments: &Option<Value>) -> Result<usize, String> {
    let Some(max_rows) = arguments.as_ref().and_then(|args| args.get("max_rows")) else {
        return Ok(DEFAULT_MAX_ROWS);
    };
    match max_rows.as_u64() {
        Some(n) if n >= 1 => Ok(n as usize),
        _ => Err("max_rows must be a positive integer".to_string()),
    }
}

/// Quotes a SQL identifier so names with spaces, quotes, or reserved words
/// (e.g. `order`, `my table`) parse correctly when interpolated into SQL text.
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn table_not_found(table_name: &str) -> String {
    format!("Table '{table_name}' not found. Call list_tables to see what exists.")
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum StmtClass {
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

    fn single_statement_error(self) -> String {
        let base = match self {
            Self::Select => {
                "Only a single SELECT, EXPLAIN, or EXPLAIN QUERY PLAN statement is allowed"
            }
            Self::Insert => "Only a single INSERT statement is allowed",
            Self::Update => "Only a single UPDATE statement is allowed",
            Self::Delete => "Only a single DELETE statement is allowed",
            Self::Schema => "Only a single schema modification statement is allowed",
        };
        format!("{base}. Nothing was executed; send one statement per call.")
    }

    /// Names the neighboring tool for what was actually sent, and the SQL
    /// forms no tool accepts, so a model does not keep guessing at this tool.
    fn wrong_class_error(self) -> String {
        let what_this_tool_does = match self {
            Self::Select => {
                "execute_query runs a single SELECT, EXPLAIN, or EXPLAIN QUERY PLAN. \
Use insert_data / update_data / delete_data for writes and schema_change for CREATE/ALTER/DROP."
            }
            Self::Insert => {
                "insert_data runs a single INSERT. Use execute_query for SELECT, \
update_data / delete_data for other writes, and schema_change for CREATE/ALTER/DROP."
            }
            Self::Update => {
                "update_data runs a single UPDATE. Use execute_query for SELECT, \
insert_data / delete_data for other writes, and schema_change for CREATE/ALTER/DROP."
            }
            Self::Delete => {
                "delete_data runs a single DELETE. Use execute_query for SELECT, \
insert_data / update_data for other writes, and schema_change for CREATE/ALTER/DROP."
            }
            Self::Schema => {
                "schema_change runs a single CREATE, ALTER, or DROP statement. Use execute_query \
for SELECT and insert_data / update_data / delete_data for writes."
            }
        };
        format!(
            "{what_this_tool_does} PRAGMA, BEGIN/COMMIT, ATTACH, and VACUUM are not available in this server."
        )
    }
}

#[cfg(test)]
mod tests {
    use super::super::memory_server;
    use super::*;
    use crate::mcp::TursoMcpServer;

    fn call(server: &TursoMcpServer, name: &str, sql: &str) -> Result<ToolOutput, String> {
        server
            .call_tool(name, &Some(json!({ "query": sql })))
            .expect("the tool exists")
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
        call(
            server,
            "execute_query",
            "SELECT order_id, status, priority FROM bench_orders ORDER BY order_id",
        )
        .expect("the dump query is valid")
        .text
    }

    #[test]
    fn update_data_rejects_trailing_delete() {
        let server = memory_server();
        seed_bench_orders(&server);

        let result = call(
            &server,
            "update_data",
            "UPDATE bench_orders SET status='DONE' WHERE order_id=1; DELETE FROM bench_orders WHERE order_id=2",
        );

        assert_eq!(
            result.unwrap_err(),
            "Only a single UPDATE statement is allowed. Nothing was executed; send one statement per call."
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

        let result = call(
            &server,
            "update_data",
            "UPDATE bench_orders SET status='DONE; DELETE' WHERE order_id=1",
        )
        .expect("a single UPDATE is allowed");
        assert_eq!(result.structured, json!({ "changes": 1 }));

        let dump = orders_dump(&server);
        assert!(dump.contains("1 | DONE; DELETE | 1"), "{dump}");
        assert!(dump.contains("2 | HOLD | 2"), "{dump}");
    }

    #[test]
    fn insert_data_rejects_trailing_delete() {
        let server = memory_server();
        seed_bench_orders(&server);

        let result = call(
            &server,
            "insert_data",
            "INSERT INTO bench_orders VALUES (3, 'NEW', 3); DELETE FROM bench_orders WHERE order_id=2",
        );

        assert_eq!(
            result.unwrap_err(),
            "Only a single INSERT statement is allowed. Nothing was executed; send one statement per call."
        );
        assert!(!orders_dump(&server).contains("3 | NEW | 3"));
        assert!(orders_dump(&server).contains("2 | HOLD | 2"));
    }

    #[test]
    fn delete_data_rejects_trailing_drop() {
        let server = memory_server();
        seed_bench_orders(&server);

        let result = call(
            &server,
            "delete_data",
            "DELETE FROM bench_orders WHERE order_id=1; DROP TABLE bench_orders",
        );

        assert_eq!(
            result.unwrap_err(),
            "Only a single DELETE statement is allowed. Nothing was executed; send one statement per call."
        );
        let dump = orders_dump(&server);
        assert!(dump.contains("1 | READY | 1"), "{dump}");
        assert!(dump.contains("2 | HOLD | 2"), "{dump}");
    }

    #[test]
    fn schema_change_rejects_trailing_delete() {
        let server = memory_server();
        seed_bench_orders(&server);

        let result = call(
            &server,
            "schema_change",
            "CREATE TABLE extra (id INTEGER); DELETE FROM bench_orders",
        );

        assert_eq!(
            result.unwrap_err(),
            "Only a single schema modification statement is allowed. Nothing was executed; send one statement per call."
        );
        assert!(orders_dump(&server).contains("1 | READY | 1"));
        assert!(orders_dump(&server).contains("2 | HOLD | 2"));
    }

    #[test]
    fn execute_query_rejects_trailing_delete() {
        let server = memory_server();
        seed_bench_orders(&server);

        let result = call(
            &server,
            "execute_query",
            "SELECT order_id FROM bench_orders WHERE order_id=1; DELETE FROM bench_orders WHERE order_id=2",
        );

        assert_eq!(
            result.unwrap_err(),
            "Only a single SELECT, EXPLAIN, or EXPLAIN QUERY PLAN statement is allowed. Nothing was executed; send one statement per call."
        );
        assert!(orders_dump(&server).contains("2 | HOLD | 2"));
    }

    #[test]
    fn update_data_accepts_single_update() {
        let server = memory_server();
        seed_bench_orders(&server);

        let result = call(
            &server,
            "update_data",
            "UPDATE bench_orders SET status='DONE' WHERE order_id=1",
        )
        .expect("a single UPDATE is allowed");

        assert_eq!(result.text, "UPDATE successful. 1 row changed.");
        assert!(orders_dump(&server).contains("1 | DONE | 1"));
        assert!(orders_dump(&server).contains("2 | HOLD | 2"));
    }

    #[test]
    fn query_rows_keep_their_sql_types() {
        let server = memory_server();
        call(
            &server,
            "schema_change",
            "CREATE TABLE mixed (i INTEGER, r REAL, t TEXT, b BLOB, n INTEGER)",
        )
        .expect("create the table");
        call(
            &server,
            "insert_data",
            "INSERT INTO mixed VALUES (7, 1.5, 'hi', X'00ff', NULL)",
        )
        .expect("insert a row");

        let result = call(&server, "execute_query", "SELECT i, r, t, b, n FROM mixed")
            .expect("the query is valid");

        assert_eq!(
            result.structured,
            json!({
                "columns": ["i", "r", "t", "b", "n"],
                "rows": [[7, 1.5, "hi", { "blob": "00ff" }, null]],
                "row_count": 1,
                "truncated": false,
            })
        );
    }

    #[test]
    fn describe_table_reports_columns() {
        let server = memory_server();
        call(
            &server,
            "schema_change",
            "CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT NOT NULL)",
        )
        .expect("create the table");

        let result = server
            .call_tool("describe_table", &Some(json!({ "table_name": "t" })))
            .expect("the tool exists")
            .expect("the table exists");

        let columns = result.structured["columns"].as_array().unwrap().clone();
        assert_eq!(columns[0]["name"], "id");
        assert_eq!(columns[0]["primary_key"], true);
        assert_eq!(columns[1]["name"], "name");
        assert_eq!(columns[1]["nullable"], false);
    }

    #[test]
    fn describing_a_missing_table_fails() {
        let server = memory_server();

        let result = server
            .call_tool("describe_table", &Some(json!({ "table_name": "nope" })))
            .expect("the tool exists");

        assert_eq!(
            result.unwrap_err(),
            "Table 'nope' not found. Call list_tables to see what exists."
        );
    }

    #[test]
    fn describe_table_quotes_a_name_with_a_space() {
        let server = memory_server();
        call(
            &server,
            "schema_change",
            "CREATE TABLE \"my table\" (id INTEGER PRIMARY KEY)",
        )
        .expect("create the table");

        let result = server
            .call_tool("describe_table", &Some(json!({ "table_name": "my table" })))
            .expect("the tool exists")
            .expect("the table exists, once its name is quoted");

        assert_eq!(result.structured["columns"][0]["name"], "id");
    }

    #[test]
    fn execute_query_caps_rows_by_default() {
        let server = memory_server();
        call(&server, "schema_change", "CREATE TABLE many (n INTEGER)").unwrap();
        for n in 0..250 {
            call(
                &server,
                "insert_data",
                &format!("INSERT INTO many VALUES ({n})"),
            )
            .unwrap();
        }

        let result = call(&server, "execute_query", "SELECT n FROM many ORDER BY n")
            .expect("the query is valid");

        assert_eq!(result.structured["row_count"], 200);
        assert_eq!(result.structured["truncated"], true);
        assert_eq!(result.structured["rows"].as_array().unwrap().len(), 200);
        assert!(
            result.text.contains(
                "Showing the first 200 rows; more exist. Add LIMIT/OFFSET or an aggregate."
            ),
            "{}",
            result.text
        );
    }

    #[test]
    fn execute_query_max_rows_overrides_the_default() {
        let server = memory_server();
        seed_bench_orders(&server);

        let result = server
            .call_tool(
                "execute_query",
                &Some(json!({
                    "query": "SELECT order_id FROM bench_orders ORDER BY order_id",
                    "max_rows": 1,
                })),
            )
            .expect("the tool exists")
            .expect("the query is valid");

        assert_eq!(result.structured["row_count"], 1);
        assert_eq!(result.structured["truncated"], true);
    }

    #[test]
    fn execute_query_truncates_long_text_values() {
        let server = memory_server();
        call(&server, "schema_change", "CREATE TABLE big (t TEXT)").unwrap();
        let long_value = "x".repeat(2000);
        server
            .call_tool(
                "insert_data",
                &Some(json!({ "query": "INSERT INTO big VALUES (?)".replace("?", &format!("'{long_value}'")) })),
            )
            .unwrap()
            .unwrap();

        let result = call(&server, "execute_query", "SELECT t FROM big").expect("valid query");

        let text_value = result.structured["rows"][0][0].as_str().unwrap();
        assert!(
            text_value.len() < 2000,
            "value was not truncated: {text_value}"
        );
        assert!(text_value.contains("truncated, 2000 bytes total"));
    }

    #[test]
    fn execute_query_accepts_explain() {
        let server = memory_server();
        seed_bench_orders(&server);

        let result = call(
            &server,
            "execute_query",
            "EXPLAIN QUERY PLAN SELECT * FROM bench_orders",
        );

        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn execute_query_still_rejects_pragma_by_name() {
        let server = memory_server();

        let result = call(&server, "execute_query", "PRAGMA table_info(bench_orders)");

        let message = result.unwrap_err();
        assert!(
            message.contains("execute_query runs a single SELECT"),
            "{message}"
        );
        assert!(message.contains("PRAGMA"), "{message}");
    }

    #[test]
    fn open_database_reports_created_for_a_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fresh.db");
        let server = memory_server();

        let result = server
            .call_tool(
                "open_database",
                &Some(json!({ "path": path.to_string_lossy() })),
            )
            .expect("the tool exists")
            .expect("opening a new path succeeds");

        assert_eq!(result.structured["created"], true);
        assert!(
            result.text.starts_with("Created new empty database"),
            "{}",
            result.text
        );
    }

    #[test]
    fn open_database_reports_not_created_for_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("existing.db");
        let server = memory_server();
        server
            .call_tool(
                "open_database",
                &Some(json!({ "path": path.to_string_lossy() })),
            )
            .unwrap()
            .unwrap();

        let result = server
            .call_tool(
                "open_database",
                &Some(json!({ "path": path.to_string_lossy() })),
            )
            .expect("the tool exists")
            .expect("re-opening the same path succeeds");

        assert_eq!(result.structured["created"], false);
        assert!(
            result.text.starts_with("Opened existing database"),
            "{}",
            result.text
        );
    }

    #[test]
    fn readonly_server_rejects_every_write_tool() {
        use super::super::readonly_memory_server;
        let server = readonly_memory_server();

        for (tool, sql) in [
            ("insert_data", "INSERT INTO t VALUES (1)"),
            ("update_data", "UPDATE t SET x=1"),
            ("delete_data", "DELETE FROM t"),
            ("schema_change", "CREATE TABLE t (x INTEGER)"),
        ] {
            let result = call(&server, tool, sql);
            let message = result.unwrap_err();
            assert!(
                message.contains("--readonly"),
                "{tool} did not mention --readonly: {message}"
            );
        }
    }

    #[test]
    fn readonly_server_still_allows_reads_and_open_database() {
        use super::super::readonly_memory_server;
        let server = readonly_memory_server();

        let list = server
            .call_tool("list_tables", &Some(json!({})))
            .expect("the tool exists");
        assert!(list.is_ok(), "{list:?}");
    }

    #[test]
    fn insert_data_is_marked_destructive() {
        let catalog = catalog(false);
        let insert = catalog
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "insert_data")
            .unwrap();

        assert_eq!(insert["annotations"]["destructiveHint"], true);
    }

    #[test]
    fn readonly_catalog_marks_write_tools_unavailable() {
        let catalog = catalog(true);
        let insert = catalog
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "insert_data")
            .unwrap();

        assert!(
            insert["description"]
                .as_str()
                .unwrap()
                .contains("--readonly"),
            "{insert}"
        );
    }
}

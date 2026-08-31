use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};
use turso_core::{
    Connection, Database, DatabaseOpts, Numeric, OpenFlags, SqliteDialect, Value as DbValue,
};
use turso_parser::ast::{Cmd, Stmt};
use turso_parser::parser::Parser;

use super::TursoMcpServer;

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
/// when it does not move around.
pub fn catalog() -> Value {
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
            "Delete data from a table",
            query_schema("The DELETE statement to execute"),
            changes_schema(),
            Access::Destructive,
        ),
        tool(
            "describe_table",
            "Describe table",
            "Describe the structure of a specific table",
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
            "Execute a read-only SELECT query",
            query_schema("The SELECT query to execute"),
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
                    "row_count": { "type": "integer" }
                },
                "required": ["columns", "rows", "row_count"]
            }),
            Access::ReadOnly,
        ),
        tool(
            "insert_data",
            "Insert rows",
            "Insert new data into a table",
            query_schema("The INSERT statement to execute"),
            changes_schema(),
            Access::Write,
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
            "Open or create a database file. Creates parent directories if needed.",
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
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }),
            Access::Write,
        ),
        tool(
            "schema_change",
            "Change schema",
            "Execute schema modification statements (CREATE TABLE, ALTER TABLE, DROP TABLE)",
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
            "Update existing data in a table",
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

        // Use table_xinfo to include generated columns (table_info hides them)
        let query = format!("PRAGMA table_xinfo({table_name})");

        let conn = self.conn.lock().unwrap().clone();
        let Some(mut rows) = conn
            .query(&query)
            .map_err(|e| format!("Error querying database: {e}"))?
        else {
            return Err(format!("Table '{table_name}' not found"));
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
            return Err(format!("Table '{table_name}' not found"));
        }

        Ok(ToolOutput::new(
            format!("Table '{table_name}' columns:\n{}", lines.join("\n")),
            json!({ "table": table_name, "columns": columns }),
        ))
    }

    fn execute_query(&self, arguments: &Option<Value>) -> Result<ToolOutput, String> {
        let query = validated_query(arguments, StmtClass::Select)?;

        let conn = self.conn.lock().unwrap().clone();
        let Some(mut rows) = conn
            .query(query)
            .map_err(|e| format!("Error executing query: {e}"))?
        else {
            return Ok(ToolOutput::new(
                "No results returned from the query",
                json!({ "columns": [], "rows": [], "row_count": 0 }),
            ));
        };

        let headers: Vec<String> = (0..rows.num_columns())
            .map(|i| rows.get_column_name(i).to_string())
            .collect();

        let mut text_rows = Vec::new();
        let mut json_rows = Vec::new();
        rows.run_with_row_callback(|row| {
            let mut text_row = Vec::new();
            let mut json_row = Vec::new();
            for value in row.get_values() {
                text_row.push(value.to_string());
                json_row.push(json_value(value));
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

        let row_count = json_rows.len();
        Ok(ToolOutput::new(
            text,
            json!({ "columns": headers, "rows": json_rows, "row_count": row_count }),
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

    fn schema_change(&self, arguments: &Option<Value>) -> Result<ToolOutput, String> {
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

fn json_value(value: &DbValue) -> Value {
    match value {
        DbValue::Null => Value::Null,
        DbValue::Numeric(Numeric::Integer(i)) => json!(i),
        DbValue::Numeric(Numeric::Float(f)) => json!(**f),
        DbValue::Text(text) => json!(text.as_str()),
        DbValue::Blob(blob) => json!({ "blob": hex::encode(&blob[..]) }),
    }
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
            "Only a single UPDATE statement is allowed"
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
            "Only a single INSERT statement is allowed"
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
            "Only a single DELETE statement is allowed"
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
            "Only a single schema modification statement is allowed"
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

        assert_eq!(result.unwrap_err(), "Only a single SELECT query is allowed");
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

        assert_eq!(result.unwrap_err(), "Table 'nope' not found");
    }
}

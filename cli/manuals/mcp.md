---
display_name: "a built-in MCP server"
---

# MCP Server - Model Context Protocol

## Overview

Turso includes a built-in MCP (Model Context Protocol) server that lets AI assistants and
other tools work with your databases programmatically.

The server speaks MCP **2026-07-28** ("v2"), and still answers the older `initialize`
handshake used by revisions 2025-06-18, 2025-03-26 and 2024-11-05, so clients that have
not moved to v2 keep working.

## Starting the MCP Server

Over stdio, which is what desktop clients launch:

```bash
/path/to/tursodb --mcp
```

Over Streamable HTTP:

```bash
/path/to/tursodb --mcp-http 127.0.0.1:8081
```

Started without a database file, both connect to a throwaway in-memory database, and
anything written there is lost when the server exits. Pass a file
(`tursodb mydata.db --mcp`) to work on real data, or point the server at one at any time
with the `open_database` tool.

The server honors every database option the CLI itself was started with — `--readonly`,
`--experimental-views`, and the rest — and applies the same options when `open_database`
switches to a different file. Start with `--readonly` (`tursodb prod.db --readonly --mcp`)
to give a model read access only: `insert_data`, `update_data`, `delete_data`, and
`schema_change` all fail with a message naming `--readonly` as the reason, no matter which
database is open at the time.

The HTTP transport serves a single endpoint, `POST /mcp`. It rejects requests whose
`Origin` is not localhost, so a web page cannot reach your databases through DNS
rebinding. Bind it to a loopback address unless you know you want it exposed.

One server holds one connection, shared by every HTTP client it serves. If two clients
use the same server, an `open_database` call from one of them redirects the other's
queries to the new file. Run one server per client if that matters.

## Discovery

A v2 client sends no handshake. It may call `server/discover` to learn what the server
supports:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "server/discover",
  "params": {
    "_meta": { "io.modelcontextprotocol/protocolVersion": "2026-07-28" }
  }
}
```

The reply lists the protocol versions the server speaks, its capabilities (tools only) and
how long the tool list may be cached.

## Available Tools

Every tool declares an output schema and returns `structuredContent` alongside the human
readable text, so a client can consume results without parsing the text table. A failed
call comes back as a result with `isError: true`, not as a JSON-RPC error.

### `open_database`

Open or create a database file, creating parent directories if needed.

- `path` (string, required): path to the database file, or `:memory:`.

Structured result: `path` and `created` — `created` is `true` when the file did not exist
before this call (or the path is `:memory:`) and `false` when an existing file was opened.
The text result says which: "Created new empty database at ..." or "Opened existing
database ...". This is how a typo'd path is told apart from a real, populated database.

### `current_database`

Report the path of the currently open database. Takes no arguments.

### `list_tables`

List the tables in the database. Takes no arguments.

### `describe_table`

Describe the columns of one table, including generated columns. Table names are quoted
internally, so names with spaces or reserved words (`order`, `my table`) work.

- `table_name` (string, required)

### `execute_query`

Run a single SELECT, EXPLAIN, or EXPLAIN QUERY PLAN statement.

- `query` (string, required)
- `max_rows` (integer, optional): caps how many rows come back. Defaults to 200.

Structured result: `columns`, `rows`, `row_count` and `truncated`. Values keep their SQL
types; a blob appears as `{"blob": "<hex>"}`. Once a value passes 1 KiB it is shortened —
text gets a `... [truncated, N bytes total]` suffix, and a blob's hex is cut to a short
prefix plus its byte count — so one huge value cannot blow out memory or context. When more
rows exist than `max_rows` allowed, `truncated` is `true` and the text result ends with a
line telling the model to add `LIMIT`/`OFFSET` or an aggregate instead of raising the cap
indefinitely.

The full schema — tables, indexes, views, and triggers — is available without a dedicated
tool: `SELECT name, sql FROM sqlite_schema`.

```json
{
  "name": "execute_query",
  "arguments": { "query": "SELECT * FROM users WHERE age > 21" }
}
```

### `insert_data`, `update_data`, `delete_data`

Run a single INSERT, UPDATE or DELETE. Each takes a `query` (string, required) and reports
`changes`, the number of rows changed. `insert_data` is marked `destructiveHint: true`
because `INSERT OR REPLACE` and `ON CONFLICT ... DO UPDATE` can overwrite or delete
existing rows, not just add new ones.

### `schema_change`

Run a single schema statement: CREATE/ALTER/DROP TABLE, INDEX, VIEW, TRIGGER, or a virtual
table.

- `query` (string, required)

Each tool accepts exactly one statement of its own kind. `UPDATE t SET x=1; DROP TABLE t`
is rejected, nothing runs, and the error says so. A statement of the wrong kind (a PRAGMA
sent to `execute_query`, say) gets an error naming the right tool to use instead; PRAGMA,
BEGIN/COMMIT, ATTACH, and VACUUM are not available in any tool.

When the server was started with `--readonly`, `insert_data`, `update_data`, `delete_data`,
and `schema_change` all fail immediately, and their catalog descriptions say so.

## Integration with AI Assistants

### Claude Desktop

```json
{
  "mcpServers": {
    "turso": {
      "command": "/path/to/tursodb",
      "args": ["--mcp"]
    }
  }
}
```

Use the full path to the tursodb executable; Claude Desktop may not search your PATH.

### Other MCP Clients

Any MCP client works. A client that speaks a pre-2026 revision handshakes with
`initialize` as before; a v2 client just starts calling.

## Example Session

1. **Start the server:**
   ```bash
   /path/to/tursodb --mcp
   ```

2. **Query data:**
   ```
   > What tables are in the database?
   [Uses list_tables tool]

   > Show me all users older than 25
   [Uses execute_query tool with "SELECT * FROM users WHERE age > 25"]
   ```

3. **Modify data:**
   ```
   > Add a new user named Bob who is 28 years old
   [Uses insert_data tool with an INSERT statement]
   ```

## Troubleshooting

### Server doesn't start
- Ensure the tursodb executable path is correct
- Check that you're using the full path to the executable

### Commands fail
- Verify SQL syntax is correct
- Check that tables and columns exist
- Send one statement per call
- Ensure you have write permissions if modifying data

### HTTP requests are rejected
- Every POST needs `MCP-Protocol-Version` and `Mcp-Method`; `tools/call` also needs `Mcp-Name`. They must match the body, or the server answers 400 with error `-32020`.
- A non-localhost `Origin` gets 403.

## See Also

- MCP Protocol Documentation: https://modelcontextprotocol.io
- Turso Documentation: https://turso.tech/docs

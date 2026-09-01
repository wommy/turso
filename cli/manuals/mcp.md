---
display_name: "a built-in MCP server"
---

# MCP Server - Model Context Protocol

## Overview

Turso includes a built-in MCP (Model Context Protocol) server that allows AI assistants and other tools to interact with your databases programmatically.

## Starting the MCP Server

To start Turso in MCP server mode, use the `--mcp` flag:

```bash
/path/to/tursodb --mcp
```

This will start an MCP server that listens on stdio for commands. The server starts without a database connection, allowing you to select or create databases using MCP commands.

### Over HTTP

Instead of stdio, the server can listen on a TCP address with `--mcp-http`:

```bash
/path/to/tursodb --mcp-http 127.0.0.1:8081
```

This serves the same tools over HTTP: POST a JSON-RPC request to `/mcp` and get a JSON-RPC response back. See [HTTP Transport](#http-transport) below, including its current limitations, before pointing a real client at it.

## Available Tools

The MCP server exposes the following tools:

### `open_database`
Open or create a database file. Creates parent directories if needed.

**Parameters:**
- `path` (string, required): Path to the database file (absolute or relative). Use `:memory:` for an in-memory database.

**Example:**
```json
{
  "tool": "open_database",
  "arguments": {
    "path": "mydata.db"
  }
}
```

### `current_database`
Get the path of the currently open database.

**Example:**
```json
{
  "tool": "current_database",
  "arguments": {}
}
```

### `list_tables`
List all tables in the database.

**Example:**
```json
{
  "tool": "list_tables",
  "arguments": {}
}
```

### `describe_table`
Get the schema of a specific table.

**Parameters:**
- `table_name` (string, required): The name of the table to describe

**Example:**
```json
{
  "tool": "describe_table",
  "arguments": {
    "table_name": "users"
  }
}
```

### `execute_query`
Execute a read-only SELECT query and get results.

**Parameters:**
- `query` (string, required): The SELECT query to execute
- `max_rows` (integer, optional): Stop after this many rows. Capped at 1000, which is also the default.

**Example:**
```json
{
  "tool": "execute_query",
  "arguments": {
    "query": "SELECT * FROM users WHERE age > 21"
  }
}
```

### `insert_data`
Insert new data into a table.

**Parameters:**
- `query` (string, required): The INSERT statement to execute

**Example:**
```json
{
  "tool": "insert_data",
  "arguments": {
    "query": "INSERT INTO users (name, age) VALUES ('Alice', 30)"
  }
}
```

### `update_data`
Update existing data in a table.

**Parameters:**
- `query` (string, required): The UPDATE statement to execute

**Example:**
```json
{
  "tool": "update_data",
  "arguments": {
    "query": "UPDATE users SET age = 31 WHERE name = 'Alice'"
  }
}
```

### `delete_data`
Delete data from a table.

**Parameters:**
- `query` (string, required): The DELETE statement to execute

**Example:**
```json
{
  "tool": "delete_data",
  "arguments": {
    "query": "DELETE FROM users WHERE name = 'Alice'"
  }
}
```

### `schema_change`
Execute schema modification statements (CREATE TABLE, ALTER TABLE, DROP TABLE).

**Parameters:**
- `query` (string, required): The schema modification statement to execute

**Example:**
```json
{
  "tool": "schema_change",
  "arguments": {
    "query": "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, age INTEGER)"
  }
}
```

Each of `insert_data`, `update_data`, `delete_data`, and `schema_change` takes exactly one statement per call - a trailing second statement is rejected, not silently run. Started with `--readonly`, the server still serves `list_tables`, `describe_table`, `execute_query`, and `current_database`, but refuses these four write tools outright, and the catalog says so in each one's description.

## Integration with AI Assistants

### Claude Desktop

To use with Claude Desktop, add the following to your Claude Desktop configuration:

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

Note: You must use the full path to the tursodb executable as Claude Desktop may not recognize items in your PATH.

### Other MCP Clients

The Turso MCP server follows the standard MCP protocol and can be used with any MCP-compatible client over stdio, or over HTTP once the limitations below are acceptable for your client.

## HTTP Transport

`--mcp-http <ADDRESS>` starts the same server listening on a TCP address instead of stdio. It binds to whatever address you give it - the MCP spec recommends a loopback address, so `127.0.0.1:<port>` is the one to reach for; a non-loopback bind still works but prints a warning to stderr.

Send a JSON-RPC request as a POST to `/mcp`, with a `Content-Length` body (chunked request bodies are not supported) and an `Mcp-Method` header naming the same method as the body. For example:

```bash
tursodb :memory: --mcp-http 127.0.0.1:8097 &

curl -s -X POST http://127.0.0.1:8097/mcp \
  -H 'Content-Type: application/json' \
  -H 'Mcp-Method: tools/list' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}'
```

which returns:

```json
{"jsonrpc":"2.0","id":1,"result":{"resultType":"complete", "tools": [ ... nine tools, as listed above ... ]}}
```

A request that carries an `id` gets `200` with the JSON-RPC response above; a notification (no `id`) gets `202` with an empty body. An unknown method is `404`; `GET` or `DELETE` to `/mcp` is `405`; any other path is `404`. A cross-origin `Origin` header is `403` - the MCP spec's defense against a browser page silently talking to a local server. `Mcp-Session-Id` is accepted but ignored: this server does not use sessions.

### Limitations

- **One connection at a time.** Each request is handled inline on the accept loop, with a 30-second read timeout that resets on every byte received. A client that opens a connection and stalls blocks every other request, and one that trickles bytes slowly holds it open indefinitely. Tracked as issue #46.

The stdio transport (`--mcp`) has neither restriction.

## Example Session

Here's an example of using the MCP server:

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
- For `--mcp-http`, make sure the address isn't already in use by another process

### Commands fail
- Verify SQL syntax is correct
- Check that tables and columns exist
- Ensure you have write permissions if modifying data

### An HTTP client gets 400 on a request

Check which kind. A request declaring `2026-07-28` in `params._meta` must carry `MCP-Protocol-Version`, `Mcp-Method`, and — for `tools/call` — `Mcp-Name`, each matching the body; a mismatch or omission is `400` with JSON-RPC error `-32020`.

A client handshaking with `initialize` is served without any of those headers, so a `400` there is not about headers. Read the JSON-RPC `error.code` in the body: `-32602` means a malformed or missing required `_meta` field, `-32022` a protocol revision this server does not speak.

## See Also

- MCP Protocol Documentation: https://modelcontextprotocol.io
- Turso Documentation: https://turso.tech/docs

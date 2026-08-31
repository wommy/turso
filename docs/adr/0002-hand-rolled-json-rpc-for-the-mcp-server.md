# Hand-rolled JSON-RPC and HTTP for the MCP server, not `rmcp`

The MCP server speaks JSON-RPC over stdio and Streamable HTTP without using the
official Rust SDK. `rmcp` is async and Tokio-based, and the `cli` crate is
entirely synchronous, so adopting it would pull a runtime into `tursodb` to serve
four JSON-RPC methods. The same reasoning covers HTTP: `cli/sync_server.rs`
already hand-rolls HTTP/1.1 over `std::net::TcpListener`, so the MCP transport
shares that code — now extracted to `cli/http.rs` — instead of adding an HTTP
crate.

## Consequences

Protocol conformance is ours to maintain. When a revision changes, nothing
updates on our behalf: the version constants, the `_meta` keys, the error codes
and the status mapping are all hand-written and all need a test each.

The cost is real but bounded, because the server advertises only `tools`. It
implements no resources, prompts, sampling, roots or subscriptions, so the
surface that has to track the spec is small. If that ever stops being true —
particularly if streaming or sampling is wanted — this decision is worth
reopening, since those are where an SDK earns its dependency.

# Hand-rolled JSON-RPC and HTTP for the MCP server, not `rmcp`

The MCP server speaks JSON-RPC over stdio and Streamable HTTP without using the
official Rust SDK. `rmcp` is async and Tokio-based, and the `cli` crate is
entirely synchronous, so adopting it would pull a runtime into `tursodb` to serve
four JSON-RPC methods. The same reasoning covers HTTP: `cli/sync_server.rs`
already hand-rolls HTTP/1.1 over `std::net::TcpListener`, so the MCP transport
shares that code — now extracted to `cli/http.rs` — instead of adding an HTTP
crate.

## The two things that look like counter-evidence

Both are real, both get found within minutes of looking, and neither reopens
this. They are written down so the next reader does not have to re-derive them.

**`postgres/server/lib.rs` is a Tokio server in this same repo.** It binds with
`TcpListener::bind(...).await` and spawns a task per connection. It ships as its
own binary through `postgres/cli`, so the runtime it pulls in never reaches
`tursodb`. The constraint here is about one binary, not about the project's
taste.

**`hyper` is already a workspace dependency.** `bindings/rust` uses it, with
`hyper-rustls` and `hyper-util`, for the sync engine's outbound requests. That
is an HTTP *client* in a different crate. Serving HTTP from `turso_cli` would
need hyper's server side plus a runtime to drive it, which is the cost this
decision declines.

## Upstream said it first

`cli/sync_server.mdx:91`, the checked-in generation prompt for
`cli/sync_server.rs`, says plainly:

```
DO NOT use tokio - use simple threads instead
```

That is upstream's own instruction for the CLI's other network server, and it is
the only written record of the constraint anywhere in the repo. It turns this
from our preference into theirs, which matters when the change is offered
upstream. (The same file also says `USE rocket library for http server`, which
the committed code ignores — see the report drafted for upstream about that
prompt being stale.)

## Consequences

Protocol conformance is ours to maintain. When a revision changes, nothing
updates on our behalf: the version constants, the `_meta` keys, the error codes
and the status mapping are all hand-written and all need a test each.

The cost is real but bounded, because the server advertises only `tools`. It
implements no resources, prompts, sampling, roots or subscriptions, so the
surface that has to track the spec is small. If that ever stops being true —
particularly if streaming or sampling is wanted — this decision is worth
reopening, since those are where an SDK earns its dependency.

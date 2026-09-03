# Reject chunked request bodies with 411 rather than de-chunking them

The MCP HTTP transport refuses `Transfer-Encoding: chunked` with `411 Length
Required`. It understands `Content-Length` and nothing else.

Built in `da516ea`, in `http_response_for` rather than in `read_http_request` —
the header survives the read either way, since that scan only looks for
`\r\n\r\n`, so the refusal needs no socket and sits with every other refusal on
that path.

**`chunked` is matched as a token, not as the whole value.** RFC 9112 requires
it to be the last coding when present, so `gzip, chunked` is the same case and
refused the same way.

## Why this needs recording

Nothing external decides it. `Transfer-Encoding`, `chunked` and `Content-Length`
appear **nowhere** in the `2026-07-28` specification — the whole tree was
searched. RFC 9112 would be the authority, but this container's proxy blocks
IETF domains, so its text could not be read and is not cited here.

A reader will find a hand-rolled HTTP server refusing legal HTTP/1.1 framing
with no rule to point at, and will reasonably ask why. Without this the answer
gets re-derived, badly.

## What it replaces

Worse than a refusal. `read_http_request` only looked for `Content-Length`, so a
chunked request made `parse_content_length` return `Ok(None)`, the body-reading
branch was skipped, and the loop broke immediately. The body became whatever
bytes happened to land in that one TCP read — raw chunk framing, or nothing.
Non-deterministic on read boundaries, and silent.

## Considered options

**Implement de-chunking.** The complete fix, and the wrong one to reach for
first: hand-rolled parsing of attacker-controlled framing on an unauthenticated
path, the same shape as the base64 sentinel decoder that was the highest-risk
hunk in an earlier review here.

**Keep accepting it silently.** The worst of the three, because nothing reports
it.

## Consequences

**No Turso client is affected, which is not the same as no client.** Every
binding in the workspace sends a fixed-length body — hyper emits
`Content-Length` for an exact `size_hint`, verified in the vendored source at
`proto/h1/role.rs`, and the Go, Python, .NET, JS and React Native paths all pass
sized buffers. (`ChunkedBody` in `bindings/rust/src/sync.rs` is named for 4 MiB
frame splitting, not wire framing.)

But this transport serves arbitrary third-party MCP clients, and some HTTP
stacks default to chunked for a streamed body. If one turns up, 411 says so
immediately and precisely, which is the point of failing loudly rather than
mis-parsing.

Revisit when a real client is observed sending chunked, with that client as the
test case, rather than speculatively now.

# Reject chunked request bodies with 411 rather than de-chunking them

The MCP HTTP transport will refuse a request carrying `Transfer-Encoding:
chunked` with `411 Length Required`, and understand `Content-Length` and
nothing else.

**Decided, not yet built.** No `Transfer-Encoding`, `chunked` or `411` appears
anywhere in `cli/` today; this is slice 9 of
[#24](https://github.com/wommy/turso/issues/24). An earlier version of this
file described the refusal in the present tense, which a guard audit read as a
claim about the code and correctly reported as false. An ADR that records a
decision ahead of the work has to say so, or it becomes a document asserting
behaviour nobody can find.

## Why this needs recording

Nothing external decides it. `Transfer-Encoding`, `chunked` and `Content-Length`
appear **nowhere** in the `2026-07-28` specification — the whole `specification/`
tree was searched. RFC 9112 would be the authority, but this container's proxy
blocks IETF domains, so its normative text could not be read and is not cited
here.

So a reader finding this will see a hand-rolled HTTP server refusing a legal
HTTP/1.1 framing with no rule to point at, and will reasonably ask why. Without
this record the answer gets re-derived, badly, by whoever asks.

## What it replaces

The pre-existing behaviour was worse than a refusal. `read_http_request` only
looked for `Content-Length`; a chunked request made `parse_content_length`
return `Ok(None)`, the body-reading branch was skipped, and the loop broke
immediately. The body became whatever bytes happened to arrive in that one TCP
read — raw chunk framing, or nothing at all. Non-deterministic on read
boundaries, and silent.

Refusing converts that into a loud, correct answer in a few lines.

## Considered Options

**Implement de-chunking.** The complete fix, and the wrong one to reach for
first. It is hand-rolled parsing of attacker-controlled framing on an
unauthenticated path — the same shape as the base64 sentinel decoder, which was
the highest-risk hunk in an earlier review of this transport. Bugs there are
memory-safety-adjacent and reachable before any authentication.

**Accept it silently, as today.** Rejected: a body that is sometimes empty and
sometimes chunk framing is the worst of the three, because nothing reports it.

## Consequences

**No Turso client is affected, and that is not the same as no client.** Every
binding in the workspace sends a fixed-length body: hyper emits `Content-Length`
for an exact `size_hint` (verified in the vendored `hyper` source, `proto/h1/
role.rs`), and the Go, Python, .NET, JS and React Native paths all pass sized
buffers. `ChunkedBody` in `bindings/rust/src/sync.rs` is named for 4 MiB frame
splitting, not wire framing.

But this transport's clients are arbitrary third-party MCP clients, not Turso's
own SDKs. Some HTTP stacks default to chunked for a streamed request body. If
one turns up, 411 tells us immediately and precisely — which is the point of
failing loudly rather than mis-parsing.

Revisit when a real client is observed sending chunked. Implement de-chunking
then, with that client as the test case, rather than speculatively now.

## Status of the status code

411 is the semantically exact answer for a server that requires a length it did
not receive. That reading comes from working knowledge of HTTP, not from RFC
9112, which could not be fetched here. If someone with access confirms a better
code, change it — the decision that matters is refuse-rather-than-parse, not the
number.

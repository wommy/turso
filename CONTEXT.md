# Turso

Glossary for terms that have **no home in an existing guide**. Most of Turso's
vocabulary already lives in `docs/agent-guides/` (storage format, WAL and
transactions, MVCC, async I/O); defining a term twice guarantees one copy goes
stale. See `docs/agents/domain.md` for the read order and for the rule about when
an area earns a glossary of its own.

Written lazily, as terms actually get resolved.

## Language

### MCP protocol eras

The word "v2" gets used for three different things, and the server supports three
revisions, so the two counts do not line up. These terms keep them apart.

**Revision**:
A dated MCP protocol version string, such as `2026-07-28`. The protocol has no
semantic version numbers; the date is the identifier.
_Avoid_: version number, release

**v2**:
Revision `2026-07-28`, and only that one. `PROTOCOL_V2` in the code.
_Avoid_: using "v2" for "whatever is newest", or for the set of revisions that
are not the first one

**Handshake revision**:
Any revision that requires `initialize` before a client may call a tool:
`2024-11-05` and `2025-06-18`. v2 removed the handshake, so it has none.
_Avoid_: v1, legacy, old protocol — "legacy" reads as one thing and this is two

**Supported revision**:
One of the three the server answers for: `2026-07-28`, `2025-06-18`,
`2024-11-05`. Deliberately **not** `2025-03-26`, which requires JSON-RPC batching
the server does not implement. A revision being real is not the same as it being
supported.

**Dual-era**:
Serving v2 and the handshake revisions from one code path, where the v2-only
fields are additive so a handshake client ignores them. Only version negotiation
and `initialize` actually branch on the era.

# Check for prior capability before building one

## Why this exists

Turso ships two MCP servers with no shared code, no shared naming, and no
mention of each other. We ported one of them without knowing the other existed,
and found out afterwards. The second lives in a **different repository**,
`tursodatabase/turso-mcp` — which is the point: searching the monorepo would
never have found it.

Two of the other pairs this file used to assert have since been checked and are
**not** what they looked like. There is only one hand-rolled HTTP server in the
monorepo (`cli/sync_server.rs`); the Postgres wire server uses `pgwire` and
shares no parsing code. And the two SQL generators (`sql_generation/` and
`testing/differential-oracle/sql_gen/`) are legitimately separate — a
value-shadowing simulator against a schema-introspection fuzzer — checked by
taking the deepest known bug in one and confirming the code path does not exist
in the other. Same for the two SDK kits: mirrored file layout, no shared logic.

Left in place because the correction is the lesson. Four of five "obvious
duplicates" dissolved on inspection, and the one that survived was in another
repository entirely.

The cost is not the duplication itself. It is that the discovery arrives *after*
the work, when the options are worse.

## Trigger

**Armed by [`periodic-sweeps`](periodic-sweeps.md), on a count.** This file is
the method; that one is what fires it.

It used to say "Event: a decision to build a new capability", and it never
fired once — nobody experiences deciding to build a capability, they
experience writing the next obvious commit. [ADR
0006](../adr/0006-arm-a-loop-by-counting-not-by-noticing.md) records why that
shape of trigger cannot work and what replaced it.

Running late is worth much more than not running. The ideal is still before
the first commit; the realistic version is before the branch is offered
upstream, while the answer can still change what gets sent.

## Steps

1. Name the capability in one sentence, in the user's words, not the
   implementation's. "Serve MCP over HTTP", not "add a TcpListener to cli".
2. Search, in this order, stopping at nothing:
   - This repository, including crates nobody thinks about (`postgres/`,
     `serverless/`, `bindings/`, `testing/`, `sync/`).
   - The organisation's other repositories.
   - The dependency graph — a workspace dependency may already provide it.
3. For each hit, classify: **reusable as-is**, **reusable after extraction**,
   **diverged past reuse**, or **superficially similar only**.
4. If nothing is found, record that and proceed. No checkpoint.
5. If something is found, prepare the brief and stop.

## Checkpoint

**Only when something is found.** Nothing found means no interruption — the
common case has to be free, or the loop gets skipped.

Do the whole search *and* the reuse assessment first. The question put to a
human is never "did you know this exists?" but "here is what exists, here is
what reuse costs, here is what duplication costs — which?"

## Brief

Under 200 words, never the raw search output:

- **What exists** — repository, path, one line on what it does.
- **How close it is**, by the four classifications above.
- **Cost of reuse** — extraction, coordination, or a dependency taken on.
- **Cost of duplication** — what drifts, and who finds out later.
- **A recommendation**, not a menu.

An empty result is still a deliverable: record it next to the decision, or the
next person repeats the search.

## OPEN

How far the organisational search should reach when most of the org's
repositories are irrelevant. Currently unbounded, which will not scale.

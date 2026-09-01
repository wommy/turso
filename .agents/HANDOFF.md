# Handoff

Where to look. Not what is true — that changes, and this file cannot keep up.

An earlier version of this document carried test counts, a slice list, a PR
enumeration and a table of upstream tracker results. It was stale within thirty
minutes: a slice landed and every number in it was wrong. That is the third time
this effort has been bitten by a snapshot where a pointer belonged, and the
first two were documents I had already diagnosed. So this one holds only
pointers, and facts that cannot rot.

**Deviation from the `handoff` skill:** it says write to the OS temp directory.
That dies with the container. This branch is the durable store.

## Read in this order

| | |
|---|---|
| 1 | **`WORKTREE.md`** at the root of whichever worktree you are in. Six worktrees, similar names; reading the wrong one has cost two agent runs. **Untracked — it dies with the container.** If absent, `.agents/adr/0001` carries the same content. |
| 2 | **[Issue #4](https://github.com/wommy/turso/issues/4)** — the map. Destination, decisions, fog. Open children are found by query, deliberately not listed. |
| 3 | **[Issue #24](https://github.com/wommy/turso/issues/24)** — the B2+D spec. Its **body** is current; its comments are the working record and are superseded by it. |
| 4 | **`.agents/adr/`** — four decisions not to reopen. 0002 especially: an architecture review already tried to relitigate it. |
| 5 | **`.agents/config/`** and **`.agents/workflows/`** — how to brief an agent, which jobs get a schema, the three loops. |

For anything else — what is built, what is open, what is blocked — **query the
tracker.** Labels carry it: `upstream-report` is everything gated on #22,
`implementation` is local work, `wayfinder:task` is what genuinely unblocks a
decision.

## Facts that do not change

- **[#22](https://github.com/wommy/turso/issues/22) was refused by this session's
  permission classifier, not by GitHub.** The request never left the container.
  A human approving the repo attachment settles it in one call.
- **Reading upstream is not blocked.** `gh-mcp` reads `tursodatabase/turso`'s
  issues and pull requests with no attachment — the `github` server cannot, which
  is why this looked blocked twice and was not.
- **A full upstream clone is at `/home/user/tursodatabase/turso`** — 19,656
  commits. Every fork checkout is shallow to 2026-05-07 and cannot date anything.
  Two findings were only datable because of this clone.
- **`cargo clippy` fails on `core/json/cache.rs:107`**, a toolchain mismatch in a
  crate no branch here touches. Add `--allow unfulfilled-lint-expectations`.
- **Never `cargo run`** — it rebuilds the workspace at default features and has
  exhausted this container's disk twice.

## The two live unknowns

Both were ghost issues this morning, unticketed and carried in someone's head.
They are now the only things in the map's fog.

- **[#22](https://github.com/wommy/turso/issues/22)** — can we open a pull request
  upstream at all.
- **[#23](https://github.com/wommy/turso/issues/23)** — no real MCP client has ever
  connected. Everything is verified against the spec text and against tests
  written from the same reading of it, which cannot catch a shared misreading.

## What this session learned, and where it is written down

Not repeated here. Each lives in a document that is the source of truth for it:

- Verify every agent claim at source — `.agents/config/verify-agent-claims.md`.
  Ten claims turned out false today; none was caught by tests, all by reading.
- One agent per **worktree**, pre-load briefs rather than describing —
  `.agents/config/background-agents.md`.
- Pointers, not snapshots — `.agents/workflows/pr-drive-to-green.md`, and this
  file's own opening paragraph.

## Suggested skills

`tdd` for the remaining slices (seams named in #24). `code-review` before
anything is offered upstream. **Not `wayfinder`** — its map has cleared; what
remains is execution.

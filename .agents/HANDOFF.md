# Handoff

Written before a compaction boundary. Everything durable is on GitHub or on this
branch; this file exists so a fresh session knows where to look, not to repeat
what those already say.

**Deliberate deviation from the `handoff` skill:** it says write to the OS temp
directory. That dies with this container. This branch is the durable store, so
it goes here.

## Read these first, in this order

1. **`WORKTREE.md`** at the root of whichever worktree you land in. Six worktrees
   with similar names; reading the wrong one has already cost two agent runs.
   **It is untracked and dies with the container** — if it is missing, that is
   why, and this file plus `.agents/adr/0001` carry the same content.
2. **[Issue #4](https://github.com/wommy/turso/issues/4)** — the map. Destination,
   decisions, fog. Open children are found by query, not listed.
3. **[Issue #24](https://github.com/wommy/turso/issues/24)** — the B2+D spec. Its
   **body** supersedes its comments; the comments are the working record.
4. **`.agents/adr/`** — four decisions you must not reopen, especially 0002 (why
   no `rmcp`, no tokio) which an architecture review already tried to relitigate.

## State

**Code.** `claude/mcp-http-transport`, pushed, [PR #31](https://github.com/wommy/turso/pull/31)
draft on the stacked base `claude/mcp-v2-protocol`. 137 tests, clippy clean.
Six of eleven slices done: 0, 1, 2, 3a, 3b, 5. Remaining are listed in #24's body.

**Five other PRs**: #2 (agent config, never merges), #3 (docs), #17 (B1), #18 (C),
#1 (retiring, see #27). CI has been queued for hours with nothing red; the runner
backlog is the only thing between here and knowing.

**Labels mean something now.** `upstream-report` = the seven things gated on #22.
`implementation` = local work. `wayfinder:task` = actually unblocks a decision,
which is only #22 and #23.

## The two live unknowns

- **[#22](https://github.com/wommy/turso/issues/22)** — can we open a PR upstream?
  Everything upstream-bound waits on it. The last attempt was refused by this
  session's **permission classifier**, not by GitHub, so it needs a human to
  approve the repo attachment. Reading upstream is *not* blocked: `gh-mcp` reads
  their issues and PRs without an attachment, and a **full** clone (19,656
  commits, unlike our shallow fork checkout) is at `/home/user/tursodatabase/turso`.
- **[#23](https://github.com/wommy/turso/issues/23)** — no real MCP client has ever
  connected. Everything is verified against the spec text and against tests
  written from the same reading of it.

## What is in flight and unfinished

Checking upstream's tracker before filing the queued reports. Two searches, two
hits, both recorded as comments: [#20](https://github.com/wommy/turso/issues/20)
may be invalidated by upstream #1440, and [#26](https://github.com/wommy/turso/issues/26)
has precedent in upstream #6143. **#25, #30, #33, #34 and #36 have not been
checked.** Do that before any of them is filed.

## Things this session learned the hard way

- **Verify every agent claim at source.** Eight turned out false, several mine.
  None was caught by tests; all by reading. `.agents/config/verify-agent-claims.md`.
- **One agent per worktree**, not per file. Git is not file-scoped — three agents
  in one tree collided over a shared index and stash.
- **Pre-load briefs.** Paste the function under change into the brief. Agents that
  had to go find it burned 68 tool calls; pre-loaded ones took 38.
- **Pointers, not snapshots.** Three documents rotted by embedding state a pointer
  should reach. Test: would this line be wrong an hour from now?
- **Adversarial review is the highest-yield activity here.** It retracted the
  deepening survey's central thesis, found a spec-reserved error code being
  misused, and found an exclude rule silently swallowing new files.

## Suggested skills

- `tdd` — the slices are red-green; seams are named in #24.
- `code-review` — two-axis, before anything is offered upstream.
- `codebase-design` — if `cli/http.rs`'s shape comes up again.
- `verify-agent-claims` at `.agents/config/` — not a skill, but read it before
  trusting any subagent report.
- **Not** `wayfinder`. Its map has cleared; what remains is execution.

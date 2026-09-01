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

## Upstream tracker check: done for all seven

`gh-mcp` reads `tursodatabase/turso`'s issues without an attachment. Not gated on
#22. Results, so nobody repeats the search:

| Ours | Upstream | Verdict |
|---|---|---|
| [#20](https://github.com/wommy/turso/issues/20) classifySql | [#1440](https://github.com/tursodatabase/turso/issues/1440) open, `enhancement` | **May be invalidated.** If only the first statement executes at all, "writes skip the remote" is the wrong description. Needs a code check before filing. |
| [#25](https://github.com/wommy/turso/issues/25) stale `.mdx` prompt | nothing | Unreported. File freely. |
| [#26](https://github.com/wommy/turso/issues/26) tempfile leak | [#6143](https://github.com/tursodatabase/turso/issues/6143) closed | **Precedent.** They already accept `/tmp` leaks as a bug and fixed one. Reframe as same-class-different-mechanism. Read its fix first — the patch may be applying a convention they already added. |
| [#30](https://github.com/wommy/turso/issues/30) `alter.rs` | **75 matches, 6 open, same shape** | **Transformative.** A standing open bug family of exactly the class we diagnosed. Lead with the family, not the refactor. See the ticket comment. |
| [#30](https://github.com/wommy/turso/issues/30) btree | 27 matches, none ours | Unreported. Novel, but now the weaker half of that ticket. |
| [#33](https://github.com/wommy/turso/issues/33) `CLAUDE.md` | nothing | Unreported. |
| [#34](https://github.com/wommy/turso/issues/34) `.sqltest` | [#6312](https://github.com/tursodatabase/turso/issues/6312) open | Precedent — they already have an open ticket converting a test to `.sqltest`. The direction is sanctioned. |
| [#36](https://github.com/wommy/turso/issues/36) dev profile | nothing | Unreported. |

Two of eight changed how a report should be written and one may kill a report
outright. Neither was expensive to find.

**Note on the zeroes:** `CLAUDE.md` and build-profile issues return nothing at
all, which may mean upstream does not track that kind of thing as issues. Those
two may be better sent as pull requests directly than as reports.

## What is in flight and unfinished

- **Split [#30](https://github.com/wommy/turso/issues/30)** — its two halves now
  have very different evidence and should be separate reports.
- **Resolve [#20](https://github.com/wommy/turso/issues/20)'s question against the
  code** in the full upstream clone before filing it.
- **Five slices left** on [#24](https://github.com/wommy/turso/issues/24): the
  Base64 sentinel, malformed `_meta`, undeclared capability, chunked/411,
  session-id ignoring. Plus thread-per-connection ([#32](https://github.com/wommy/turso/issues/32))
  and five open defects on [#28](https://github.com/wommy/turso/issues/28).

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

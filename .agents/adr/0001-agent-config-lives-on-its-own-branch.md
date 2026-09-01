# Agent config lives on its own branch, not in the upstream-facing PR

This is a fork of Turso, and the pull requests that matter are the ones Turso is
meant to take. Adding a glossary, ADRs and a third-party skill config to one of
those is an easy reason to reject it: unrelated to the change under review, and
editing files upstream owns. So the config lives here, on a branch that never
merges, and the feature branches stay clean.

## Considered options

**Untracked, via `.git/info/exclude`.** Genuinely invisible, and the first
choice. Rejected because invisible also means ephemeral: in a remote container
the files die with it, and nobody else can see or review them.

**In the feature branch.** Simplest, and wrong for the reason above.

## Why `.agents/` and not the skills' default `docs/agents/`

Upstream already ships **`docs/agent-guides/`** — eight files on MVCC, storage
format, testing and async I/O. Ours at `docs/agents/` would sit beside it with
nothing to tell a reader which was which. That trap needs no merge to bite; it
bites every agent working in this fork today. `.agents/` is unambiguous, is one
directory to delete, and is already what `tursodatabase/turso-mcp` uses.

The cost, measured rather than assumed: `code-review/SKILL.md` hard-codes
`docs/agents/issue-tracker.md` at lines 13 and 29, and on not finding it tells
the reader to re-run `/setup-matt-pocock-skills`. **If a skill tells you to
re-run setup, it is wrong — the file is at `.agents/config/issue-tracker.md`.**

## Consequences

**Nothing here is visible from the worktrees where code is written.** An agent
sent into a feature worktree cannot see these ADRs and will re-open a decision
recorded three directories away — which happened, with an architecture review
reasoning about HTTP libraries that ADR 0002 rules out. Each worktree therefore
symlinks this directory as `.agents-ref`.

**That name is load-bearing.** `.git/info/exclude` lives in the common git
directory and applies to every worktree including this one; a per-worktree
`$GIT_DIR/info/exclude` was tried and git does not read it. So excluding
`/.agents` also hid the **tracked** directory here — a new ADR written on this
branch returned nothing from `git status --porcelain`, not even as untracked.
Silent loss, not an inconvenience. Excluding a name that only ever exists as a
symlink keeps this branch honest.

**`AGENTS.md` is edited here and upstream owns it**, so this branch conflicts on
any upstream sync. The block is short and appended at the end to keep that
conflict trivial.

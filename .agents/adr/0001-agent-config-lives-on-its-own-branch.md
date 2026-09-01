# Agent config lives on its own branch, not in the upstream-facing PR

This is a fork of Turso, and the pull requests that matter are the ones Turso is
meant to take. Adding a glossary, ADRs and a third-party skill config to one of
those is an easy reason to reject it: it is unrelated to the change under review
and it edits files upstream owns. So the config lives here, on a branch that is
never merged upstream, and the feature branches stay clean.

## Considered Options

**Untracked, via `.git/info/exclude`.** Genuinely invisible — `git status` stays
clean and `git add -A` cannot pick the files up — and it was the first choice.
Rejected because invisible also means ephemeral: in a remote container the files
die with the container, and nobody else can see or review them.

**In the feature branch.** Simplest, and wrong for the reason above.

## Why `.agents/`, not `docs/`

The skills' own default is `docs/agents/` and `docs/adr/`. Both were used here
at first, and both were wrong for this repo, for a reason that has nothing to do
with upstream ever taking the files.

Upstream already ships **`docs/agent-guides/`** — eight files on MVCC, storage
format, testing and async I/O. Putting ours at `docs/agents/` left two
near-identical directory names side by side, one theirs and one ours, with
nothing to tell a reader which was which. That is a navigation trap, and it does
not need a merge to bite: it bites every agent working in this fork today.

`.agents/` is unambiguous, is one directory to delete, and is already the
convention in this organisation — `tursodatabase/turso-mcp` uses `.agents/` for
exactly this purpose.

The cost is that a skill following its own documented default will look in
`docs/agents/issue-tracker.md` and find nothing. That is acceptable because the
skills are read from a clone rather than installed, so the paths are ours to
choose; `AGENTS.md` points at the real locations for anyone who arrives without
that context.

## Consequences

The skills install into `~/.claude/skills` and `~/.agents/skills` via the skills
repo's own `scripts/link-skills.sh`, so nothing lands in this repo's tracked
`.claude/skills/`. Both directories are outside the repo and die with a
container, which `.agents/bootstrap-skills.sh` exists to undo.

Checking this branch out replaces the working tree, so use a worktree
(`git worktree add ../turso-agent-config claude/agent-config`) rather than
switching branches in place while feature work is in flight.

A worktree only contains its own branch, so nothing here is readable from the
worktrees where the code actually gets written. An agent sent into a feature
worktree cannot see these ADRs or the glossary, and will happily re-open a
decision recorded three directories away — which has already happened, with an
architecture review reasoning about HTTP libraries that ADR 0002 rules out. The
fix is to symlink `.agents/` into each worktree and list them in
`.git/info/exclude`, which is shared across all worktrees and keeps them out of
`git status`.

Untracked copies from the rejected option above must not be left behind. A
stray `CONTEXT.md` in another worktree competes with this one, and the reader
has no way to tell which is current.

`AGENTS.md` is edited here, and upstream owns that file, so this branch will
conflict on an upstream sync. The block is kept short and appended at the end to
make that conflict trivial to resolve.

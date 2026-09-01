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

## Consequences

The skills install into `~/.claude/skills` and `~/.agents/skills` via the skills
repo's own `scripts/link-skills.sh`, so nothing lands in this repo's tracked
`.claude/skills/`. Both directories are outside the repo and die with a
container, which `scripts/bootstrap-agent-skills.sh` exists to undo.

Checking this branch out replaces the working tree, so use a worktree
(`git worktree add ../turso-agent-config claude/agent-config`) rather than
switching branches in place while feature work is in flight.

A worktree only contains its own branch, so nothing here is readable from the
worktrees where the code actually gets written. An agent sent into a feature
worktree cannot see these ADRs or the glossary, and will happily re-open a
decision recorded three directories away — which has already happened, with an
architecture review reasoning about HTTP libraries that ADR 0002 rules out. The
fix is to link `CONTEXT.md` and `docs/adr/` into each worktree and list them in
`.git/info/exclude`, which is shared across all worktrees and keeps them out of
`git status`.

Untracked copies from the rejected option above must not be left behind. A
stray `CONTEXT.md` in another worktree competes with this one, and the reader
has no way to tell which is current.

`AGENTS.md` is edited here, and upstream owns that file, so this branch will
conflict on an upstream sync. The block is kept short and appended at the end to
make that conflict trivial to resolve.

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

Checking this branch out replaces the working tree, so use a worktree
(`git worktree add ../turso-agent-config claude/agent-config`) rather than
switching branches in place while feature work is in flight.

`AGENTS.md` is edited here, and upstream owns that file, so this branch will
conflict on an upstream sync. The block is kept short and appended at the end to
make that conflict trivial to resolve.

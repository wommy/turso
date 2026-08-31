# Agent config

Config for Matt Pocock's engineering skills (`github.com/mattpocock/skills`),
written by `/setup-matt-pocock-skills`.

| File | What it is |
|---|---|
| `issue-tracker.md` | Where issues live, and the two spellings for reaching GitHub |
| `domain.md` | What to read before exploring, and why there is no `CONTEXT-MAP.md` |
| `../../CONTEXT.md` | The glossary |
| `../adr/` | Decisions |

## This branch is not for upstream

`claude/agent-config` exists so this config is durable and reviewable without
riding along in a pull request Turso is meant to take. It is never merged
upstream. See `../adr/0001-agent-config-lives-on-its-own-branch.md`.

Work it as a worktree rather than switching branches in place:

```bash
git worktree add ../turso-agent-config claude/agent-config
```

## The skills are not vendored

They live at `github.com/mattpocock/skills` and are cloned when needed. They are
deliberately not copied into `.claude/skills/`, for two reasons: that directory
is tracked by upstream, and it already contains a `code-review` skill whose name
would collide with Matt's.

## Setup notes

- **Triage labels** are not configured, because the `triage` skill is not
  installed. Section B of the setup skill is skipped entirely; re-run it if that
  changes.
- **`.scratch/`** is the draft layer and is gitignored, so it appears in neither
  this branch nor any other. Anything another person must act on gets promoted to
  a GitHub issue.

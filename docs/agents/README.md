# Agent config

Config for Matt Pocock's engineering skills (`github.com/mattpocock/skills`),
written by `/setup-matt-pocock-skills`.

| File | What it is |
|---|---|
| `issue-tracker.md` | Where issues live, and the two spellings for reaching GitHub |
| `triage-labels.md` | The five triage roles, and the labels they map to |
| `domain.md` | What to read before exploring, and why there is no `CONTEXT-MAP.md` |
| `../../CONTEXT.md` | The glossary |
| `../adr/` | Decisions |
| `../../scripts/bootstrap-agent-skills.sh` | Reinstall the skills in a fresh container |

## This branch is not for upstream

`claude/agent-config` exists so this config is durable and reviewable without
riding along in a pull request Turso is meant to take. It is never merged
upstream. See `../adr/0001-agent-config-lives-on-its-own-branch.md`.

Work it as a worktree rather than switching branches in place:

```bash
git worktree add ../turso-agent-config claude/agent-config
```

## Installing the skills

The skills repo ships its own installer. It symlinks every skill into
`~/.claude/skills` and `~/.agents/skills` — both **outside this repo**, so
nothing touches the tracked `.claude/skills/` directory and nothing shows up in
`git status`:

```bash
./scripts/bootstrap-agent-skills.sh
```

That clones (or updates) the skills repo and runs its `link-skills.sh`. Because
every skill is a symlink into the clone, `git pull` there updates all of them at
once.

Two things to know:

- **`link-skills.sh` also links the `in-progress/` skills** — `implement-spec`,
  `loop-me`, `retro`, `writing-beats`, `writing-fragments`, `writing-shape`,
  `setup-ts-deep-modules`, `claude-handoff`. Only `deprecated/` is excluded.
  They are the author's work-in-progress, not part of the documented flow.
- **Do not also install the Claude Code plugin.** `mattpocock-skills` is enabled
  on this account but has never materialised in a remote container, which is why
  we install from source. If it ever does materialise alongside this, the repo's
  README warns you get every skill twice.

## The `code-review` name is contended

Two different things answer to `code-review`:

- **Matt's**: a two-axis review — Standards (repo standards plus a Fowler smell
  baseline) and Spec (does the diff do what was asked) — run as parallel
  subagents whose findings are deliberately never merged or reranked. This is
  the one `/implement` calls into.
- **The harness built-in**: correctness bugs and cleanups at a chosen effort
  level, with `--comment` to post inline PR comments and `--fix` to apply
  findings. It has **no file on disk**; it is provided by the CLI itself, so it
  cannot be renamed, relinked, or copied.

Matt's owns the name here. Because the built-in cannot be moved out of the way,
the alias goes on Matt's instead: **`two-axis-review`** always means his,
whichever way the bare name resolves. Nothing is lost — ask for the built-in by
what it does (a correctness pass, or `--fix`/`--comment`) and it is still there.

## Setup notes

- **`.scratch/`** is the draft layer and is gitignored, so it appears in no
  branch. Anything another person must act on gets promoted to a GitHub issue.
- **Four of the five triage labels do not exist on the fork yet.** `wontfix`
  is already there, inherited from upstream; the other four need creating from a
  machine with `gh`, and `/triage` fails until they are. The commands are in
  `triage-labels.md`.

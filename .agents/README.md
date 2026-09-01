# Agent config

Everything an agent working this fork needs that the repo itself does not carry:
the tracker, the glossary, the decisions, the rules for briefing other agents.

It lives on `claude/agent-config`, a branch that never merges anywhere, so it is
durable and reviewable without riding along in a pull request Turso is meant to
take — [adr/0001](adr/0001-agent-config-lives-on-its-own-branch.md). Work it as a
worktree rather than switching branches in place:

```bash
git worktree add ../turso-agent-config claude/agent-config
```

The other worktrees reach it through a `.agents-ref` symlink.

## Reach for these

| Read when | Document |
|---|---|
| **Picking the work back up** — where things stand, what is blocked | [`HANDOFF.md`](HANDOFF.md) |
| **Filing, reading, labelling or closing a ticket** | [`config/issue-tracker.md`](config/issue-tracker.md) |
| **Triaging** — the five roles and the label each maps to | [`config/triage-labels.md`](config/triage-labels.md) |
| **Dispatching a background agent** — brief shape, schemas, escape clause | [`config/background-agents.md`](config/background-agents.md) |
| **A subagent report lands** carrying claims you are about to act on | [`config/verify-agent-claims.md`](config/verify-agent-claims.md) |
| **Building, linting, or out of disk** | [`config/build-workflow.md`](config/build-workflow.md) |
| **Before exploring an unfamiliar crate** | [`config/domain.md`](config/domain.md) |
| **Naming an MCP protocol revision** | [`CONTEXT.md`](CONTEXT.md) |
| **About to reopen a settled decision** | [`adr/`](adr/) |
| **A scheduled loop needs arming or re-arming** | [`workflows/`](workflows/) |
| **Writing something aimed at `tursodatabase/turso`** | [`upstream-drafts/`](upstream-drafts/) |

## The skills

Matt Pocock's engineering skills (`github.com/mattpocock/skills`) are what most
of `config/` configures; `/setup-matt-pocock-skills` wrote the first draft of it.
Reinstall them in a fresh container with:

```bash
.agents/bootstrap-skills.sh
```

That clones the repo and runs its `link-skills.sh`, which symlinks every skill
into `~/.claude/skills` and `~/.agents/skills` — both outside this repo, so the
tracked `.claude/skills/` directory is untouched and `git status` stays clean.
Because each skill is a symlink into one clone, `git pull` there updates all of
them at once.

Three things to know:

- **`link-skills.sh` links the `in-progress/` skills too** — `implement-spec`,
  `loop-me`, `retro`, `writing-beats`, `writing-fragments`, `writing-shape`,
  `setup-ts-deep-modules`, `claude-handoff`. Only `deprecated/` is excluded. They
  are the author's work in progress, not part of the documented flow.
- **Install from source only.** The `mattpocock-skills` plugin is enabled on this
  account but has never materialised in a remote container. If it ever does, the
  repo's README warns that having both gives you every skill twice.
- **`two-axis-review` always means Matt's `code-review`** — Standards and Spec as
  parallel subagents whose findings are never merged. The harness ships its own
  `code-review` built in, with no file on disk to rename or move out of the way,
  so the alias goes on Matt's. Ask for the built-in by what it does (a
  correctness pass, `--fix`, `--comment`) and it is still there.

`.scratch/` is the draft layer for anything not worth a ticket. It is gitignored,
so it appears in no branch and dies with the container; anything another person
must act on gets promoted to a GitHub issue.

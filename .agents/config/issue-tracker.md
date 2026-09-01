# Issue tracker: GitHub, with a local scratch layer

Issues and specs live as **GitHub issues on `wommy/turso`**. That is the tracker
of record: anything another person has to see or act on is an issue there.

`.scratch/` is the **draft layer, not a second tracker**. Spikes and in-flight
notes live at `.scratch/<effort>/` while they are still yours alone. The rule
that stops it becoming a shadow tracker:

> Anything another person must act on is promoted to a GitHub issue.
> `.scratch/` is only ever the draft.

## Two things that bite

**Issues are disabled on a fork by default.** They were off here until someone
turned them on (Settings → Features → Issues), and issue creation fails with
`410 Issues has been disabled` — not a permissions error, and easy to misread.
Pull requests are unaffected. If a fresh fork ever replaces this one, check this
first.

**Labels must already exist.** `issue_write` does *not* create a missing label —
it fails with `failed to resolve label`, and the GitHub MCP server has no
label-creation tool at all. `triage-labels.md` carries the labels and how to
create them.

## Two spellings for every operation

How you reach GitHub depends on the machine, so **probe, don't assume**:

```bash
command -v gh >/dev/null && echo cli || echo mcp
```

- **`gh` present** (a local machine): use the CLI commands below.
- **`gh` absent** (the Claude Code remote container): use the `mcp__github__*`
  tools. They take `owner` and `repo` explicitly — `wommy` and `turso` — because
  there is no clone for them to infer from.

Everything below is one operation, two spellings. They are equivalent; pick by
what the environment has.

| Operation | `gh` CLI | MCP tool |
|---|---|---|
| Create an issue | `gh issue create --title "..." --body "..."` (heredoc for multi-line) | `issue_write` with `method: "create"` |
| Read an issue | `gh issue view <n> --comments` | `issue_read` with `method: "get"`, then `method: "get_comments"` |
| List issues | `gh issue list --state open --json number,title,body,labels` | `list_issues` (or `search_issues` for anything with criteria) |
| Comment | `gh issue comment <n> --body "..."` | `add_issue_comment` |
| Label | `gh issue edit <n> --add-label "..."` / `--remove-label "..."` | `issue_write` with `method: "update"`, passing the full `labels` array |
| Close | `gh issue close <n> --comment "..."` | `issue_write` with `method: "update"`, `state: "closed"` and a `state_reason` |
| Read a PR | `gh pr view <n> --comments`, `gh pr diff <n>` | `pull_request_read` with `method: "get"` / `"get_diff"` / `"get_comments"` |

Two MCP-only gotchas:

- **Labels are a whole-array update, not add/remove.** `issue_write` replaces the
  label set, so read the current labels first and send the union, or you will
  silently drop labels somebody else applied.
- **Repo scope is enforced.** This session can only reach repositories in its
  scope list. A call to another repo is denied, not empty.

GitHub shares one number space across issues and PRs, so a bare `#42` may be
either. Try the PR read first and fall back to the issue read.

## Pull requests as a triage surface

**PRs as a request surface: no.** _(Set to `yes` if this repo treats external PRs
as feature requests; `/triage` reads this flag.)_

## When a skill says "publish to the issue tracker"

Create a GitHub issue. If the thing is still a draft only you will read, write it
under `.scratch/<feature-slug>/` and promote it when somebody else needs it.

## When a skill says "fetch the relevant ticket"

Read the GitHub issue. If the reference is a path under `.scratch/`, read the file.

## Wayfinding operations

Recorded for completeness; `/wayfinder` is not in use here.

The **map** is one issue labelled `wayfinder:map`, with **child** issues as
tickets. Two things do not survive the MCP spelling:

- **Native issue dependencies need `gh api`.** There is no MCP tool for the
  `blocked_by` endpoint, so under MCP record blocking as a `Blocked by: #n, #n`
  line at the top of the child body. A ticket is unblocked when every issue it
  lists is closed.
- **Sub-issues** are only partly reachable (`sub_issue_write`); where they are
  not, put the children in a task list in the map body and `Part of #<map>` at
  the top of each child.

Frontier query: open children, minus any with an open blocker or an assignee;
first in map order wins. Claim by self-assigning before any other write.

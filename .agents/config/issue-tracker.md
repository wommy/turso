# Issue tracker: GitHub, with a local scratch layer

Issues and specs live as **GitHub issues on `wommy/turso`**. That is the tracker
of record: anything another person has to see or act on is an issue there.

`.scratch/<effort>/` is the **draft layer, not a second tracker** — spikes and
in-flight notes, while they are still yours alone. The rule that stops it
becoming a shadow tracker:

> Anything another person must act on is promoted to a GitHub issue.
> `.scratch/` is only ever the draft.

So "publish to the issue tracker" means create a GitHub issue, and "fetch the
relevant ticket" means read one — unless the reference is a `.scratch/` path.

## Two things that bite

**Issues are disabled on a fork by default.** They were off here until someone
turned them on (Settings → Features → Issues). Creation fails with `410 Issues
has been disabled`, which is not a permissions error and is easy to misread.
Pull requests are unaffected. Check this first on any fresh fork.

**Labels must already exist.** `issue_write` does *not* create a missing one — it
fails with `failed to resolve label`, and the GitHub MCP server has no
label-creation tool. [`triage-labels.md`](./triage-labels.md) has the labels and
how to create them.

## Two spellings for every operation

How you reach GitHub depends on the machine, so **probe, don't assume** — the
reasoning is [ADR 0003](../adr/0003-one-tracker-two-spellings.md):

```bash
command -v gh >/dev/null && echo cli || echo mcp
```

The MCP tools take `owner` and `repo` explicitly — `wommy` and `turso` — because
there is no clone for them to infer from.

| Operation | `gh` CLI | MCP tool |
|---|---|---|
| Create an issue | `gh issue create --title "..." --body "..."` (heredoc for multi-line) | `issue_write` with `method: "create"` |
| Read an issue | `gh issue view <n> --comments` | `issue_read` with `method: "get"`, then `"get_comments"` |
| List issues | `gh issue list --state open --json number,title,body,labels` | `list_issues`, or `search_issues` for anything with criteria |
| Comment | `gh issue comment <n> --body "..."` | `add_issue_comment` |
| Label | `gh issue edit <n> --add-label "..."` / `--remove-label "..."` | `issue_write` with `method: "update"`, passing the full `labels` array |
| Close | `gh issue close <n> --comment "..."` | `issue_write` with `method: "update"`, `state: "closed"` and a `state_reason` |
| Read a PR | `gh pr view <n> --comments`, `gh pr diff <n>` | `pull_request_read` with `method: "get"` / `"get_diff"` / `"get_comments"` |

Three things that only bite on the MCP side:

- **Labels are a whole-array update.** `issue_write` replaces the label set, so
  read the current labels and send the union, or you silently drop labels
  somebody else applied.
- **Repo scope is enforced.** A call to a repository outside this session's scope
  is denied, not empty.
- **Sub-issues work; native `blocked_by` does not.** `sub_issue_write` attaches a
  child to a parent. There is no MCP tool for the dependency endpoint, so record
  blocking as a `Blocked by: #n` line at the top of the child body.

GitHub shares one number space across issues and PRs, so a bare `#42` may be
either. Try the PR read first and fall back to the issue read.

**PRs as a request surface: no.** `/triage` reads this flag; set it to `yes` only
if this repo starts treating external PRs as feature requests.

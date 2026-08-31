# One tracker, two spellings: `gh` and the GitHub MCP tools

The skills assume `gh`. Half our sessions do not have it: work happens both on a
local machine and in a Claude Code remote container, where GitHub is reached
through `mcp__github__*` tools instead. Rather than pick one and be wrong half
the time, `docs/agents/issue-tracker.md` is organised by **operation** — create,
read, list, comment, label, close — with both spellings against each, and the
instruction to probe (`command -v gh`) rather than assume.

## Considered Options

**Standardise on `gh` and install it in the container.** Rejected: the container
is provisioned by the harness, not by us, so this would be a setup step that has
to be remembered and re-done, to make one tool available where an equivalent
already exists.

**Standardise on the MCP tools everywhere.** Rejected in the other direction:
they are only present inside an agent session, so a human at a terminal, or any
script, would have nothing to run.

**Two documents, one per environment.** Rejected because the operations are the
same operations. Splitting by tool duplicates the semantics and lets the two
copies drift; splitting by operation keeps one list and varies only the spelling.

## Consequences

The two spellings are not perfectly equivalent, and the differences are recorded
next to the table rather than discovered at runtime:

- **Labels are a whole-array update under MCP.** `issue_write` replaces the label
  set rather than adding to it, so a caller must read the current labels and send
  the union or silently drop somebody else's. `gh issue edit --add-label` has no
  such trap.
- **Repo scope is enforced under MCP.** A call outside the session's scope list
  is denied, not empty — an important distinction when a query returning nothing
  would otherwise read as "no results".
- **Some operations have no MCP spelling at all.** Label creation is the one that
  has already bitten us: there is no `create_label` tool, so the five triage
  labels cannot be created from a remote session and need a machine with `gh`.
  Native issue dependencies (`blocked_by`) are the same story, which is why
  wayfinding blocking edges fall back to a `Blocked by: #n` line in the body.

When an operation turns out to have only one spelling, record it in
`issue-tracker.md` beside the others rather than leaving the gap to be found
again.

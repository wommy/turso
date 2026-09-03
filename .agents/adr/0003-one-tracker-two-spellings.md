# One tracker, two spellings: `gh` and the GitHub MCP tools

The skills assume `gh`. Half our sessions do not have it: work happens both on a
local machine and in a Claude Code remote container, where GitHub is reached
through `mcp__github__*` tools instead. Rather than pick one and be wrong half
the time, `.agents/config/issue-tracker.md` is organised by **operation** — create,
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

The two spellings are not perfectly equivalent. Where they differ — labels being
a whole-array update under MCP, repo scope being enforced rather than empty,
operations with no MCP spelling at all — the difference is recorded beside the
operation in [`../config/issue-tracker.md`](../config/issue-tracker.md), not
here. That file is where someone stands when the difference bites.

The rule that keeps this decision alive: when an operation turns out to have
only one spelling, record it there beside the others rather than leaving the gap
to be found again. Label creation and native `blocked_by` are the two found so
far, both MCP-side.

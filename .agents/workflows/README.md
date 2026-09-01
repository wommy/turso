# Workflows

One spec per loop, in the shape `/loop-me` prescribes: **trigger**, **checkpoint**,
**push right**, **brief**. A spec is done when an implementer agent could build it
without asking a question.

These were written from evidence rather than from an interview. Each loop below
already ran during the MCP port, unspecified, and each failed at least once in a
way the spec now prevents. Where a genuine question remains it is marked
**OPEN** rather than answered by guesswork.

| Workflow | Trigger | Has a checkpoint? |
|---|---|---|
| [prior-capability-check](prior-capability-check.md) | event: a new capability is proposed | only when something is found |
| [pr-drive-to-green](pr-drive-to-green.md) | schedule: hourly while a PR is open | only on ambiguous failures |

**`verify-agent-claims` is not here, deliberately.** It was written as a
workflow and is not one: nothing external arms it, it has no checkpoint, and its
brief is "fold into whatever report follows". Three of the four slots read "does
not apply". `loop-me` permits a workflow to need less structure, not to have no
arming mechanism at all - so it is a discipline, and lives at
`../config/verify-agent-claims.md`. It is the highest-yield habit here; that did
not make it a loop.

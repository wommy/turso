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
| [verify-agent-claims](verify-agent-claims.md) | event: a subagent report arrives | none |

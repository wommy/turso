# Sweep for what per-slice work cannot see

## Why this exists

Eleven slices landed on the MCP HTTP transport in one session. Every one was
verified by the agent that wrote it — red test first, both directions, clippy
clean. Nobody read the composite, and nobody asked what already existed.

What that missed, all found by accident:

- **A rewrite silently dropped working code.** `claude/mcp-v2-port-4wfufm`
  had thread-per-connection with a 64-connection cap and a real slowloris
  defence with its own test. The slice-by-slice rebuild worked from the spec,
  neither is a spec requirement, so neither was rebuilt — and
  thread-per-connection then re-entered the plan as unstarted work. Found
  while checking a checklist gate ([#46](https://github.com/wommy/turso/issues/46)).
- **Two protocol bugs no test could catch**, because the tests and the code
  came from the same reading of the spec. Found the first time a third-party
  client spoke to the server
  ([#43](https://github.com/wommy/turso/issues/43), [#44](https://github.com/wommy/turso/issues/44)).

Neither is the kind of thing a per-slice review finds. Both need a pass whose
unit is the whole change, not the commit.

`prior-capability-check` was supposed to prevent the first and did not fire:
its trigger is "a decision to build a new capability", and nobody ever
consciously made one. The slices simply proceeded. A trigger that depends on
somebody noticing they are at a decision point is not a trigger.

## Trigger

**Count, not calendar.** Every time roughly five slices land on a branch, or
before any branch is offered upstream, whichever comes first.

Counting works where the event trigger failed, because landing a fifth commit
is observable without anybody having to recognise a moment.

## The four sweeps

Run as parallel read-only agents. They do not conflict — none of them writes.

| Sweep | Asks |
|---|---|
| **Two-axis review** | Does the composite follow our standards, and does it do what the spec asked? The `code-review` skill, against a commit rather than a working tree. |
| **What did we lose** | Diff the branch against whatever it replaced. Anything the old version did that no spec clause demands is what a spec-driven rewrite drops. |
| **What already exists here** | Did we hand-roll something the workspace already has? A crate already in `Cargo.lock` costs nothing; a new dependency is a real cost and may justify the hand-rolled version. |
| **Where does upstream repeat itself** | Their duplication, not ours. Findable with the fix history as an index, and the strongest shape is one copy getting a bug fix the other did not. |

The first two are about this branch. The last two are about the codebase and
pay off across branches, so they can run less often.

## Steps

1. Pin a **commit**, not the working tree. Agents may be mid-edit.
2. Dispatch the sweeps that apply, in parallel, read-only, with `cargo`
   forbidden — a shared target dir means a review can block an implementer.
3. Verify each finding at source before acting on it, per
   [`../config/verify-agent-claims.md`](../config/verify-agent-claims.md).
4. File what survives. Findings that block a decision get tickets; the rest
   go where the wrong claim lived.
5. Record what came back **clean**. It bounds the claim and stops the next
   sweep redoing the same ground.

## Checkpoint

**Only when a sweep finds something that changes the plan** — work that has to
land before shipping, or a finding that reshapes a ticket. A clean sweep is
recorded and not reported.

## Push right

Verify before raising. A sweep that forwards unverified agent claims moves
work to the human instead of doing it. Ten claims in this effort turned out
false on inspection.

## Brief

Under 200 words per sweep. What was found, what was checked and found clean,
and what it changes. Never the raw output.

## Definition of done

Every sweep has reported, every finding is verified or explicitly marked
unverified, and the clean results are written down.

## OPEN

Whether "roughly five slices" is the right count. It is a guess from one
session in which eleven landed before anybody looked. The honest version of
this number needs a second data point.

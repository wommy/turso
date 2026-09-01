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
- **A fix that had already been made and then undone.** Every tool dropped the
  connection lock before running its query, at seven sites, after an earlier
  commit had fixed exactly that. Found by diffing against the branch being
  retired ([#49](https://github.com/wommy/turso/issues/49)).
- **A fix that introduced the bug it was fixing.** The connection cap answers
  a refused socket on the accept loop, and drained it with a loop that only
  stopped on a short read — so a client that keeps sending holds up everyone,
  which is the stall thread-per-connection was added to remove. Found by
  reading a finished, green, clippy-clean commit whose own author had
  validated it and whose report was entirely accurate.

None of these is the kind of thing a per-slice review finds. The first three
need a pass whose unit is the whole change rather than the commit.

The fourth is different and worth separating out, because it is the one this
loop does **not** cover: nothing was missing from the composite and nothing
was contradicted by a spec — one commit was simply wrong, and its own tests
and its own author agreed it was fine. The counter-measure is not a sweep but
a gate, and it lives in
[`../config/verify-agent-claims.md`](../config/verify-agent-claims.md): read
the diff of an agent's commit before pushing it.

`prior-capability-check` was supposed to prevent the first and never fired
once. Its trigger asked somebody to notice they were at a decision point, and
nobody was: the slices were simply the next obvious commit each time. [ADR
0006](../adr/0006-arm-a-loop-by-counting-not-by-noticing.md) records that, and
is the reason this loop counts commits instead.

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
| **What already exists here** | Did we hand-roll something the workspace already has? The method is [`prior-capability-check`](prior-capability-check.md), which this loop exists to arm — do not restate it in the brief, point the agent at it. |
| **Where does upstream repeat itself** | Their duplication, not ours. Findable with the fix history as an index, and the strongest shape is one copy getting a bug fix the other did not. |
| **Has this directory rotted** | `.agents/` itself. It grows the way everything else here does, and nobody is auditing the auditor. See below — this is the one sweep with a deletion bias. |

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
6. **Check every document that cites an open ticket.** A limitation written
   against a bug is a snapshot with an expiry date, and nothing watches for it:
   three documents told readers `--mcp-http` could not serve a legacy client for
   hours after the fix landed, and one told them a `400` was that bug rather
   than their own configuration.

   The finding is not "cites a closed issue" — citing one retrospectively is
   correct and common. It is **describing a closed issue as pending**. Grep the
   pending tense near a citation and check those numbers only:

   ```
   grep -rnE '(until it is fixed|known defect|not yet|does not work|tracked as)' \
     --include='*.md' --include='*.mdx' --include='*.rs' . | grep -E '#[0-9]+'
   ```

## The config sweep, because nobody audits the auditor

A consolidation pass over `.agents/` once removed 86 lines. The rest of that
same day added 412, every one of them written by someone who had just finished
arguing that documents rot when they grow. Consolidating once does not hold,
which is why this is a loop and not a task.

Four checks, all cheap:

```
wc -l $(find .agents -name '*.md')          # the trend, against the last sweep's commit
```

- **A rule with two homes.** The sanctioned-paths block lived in eight agent
  briefs and in none of these files. Grep a distinctive phrase from any rule
  you have written recently; more than one hit is the finding.
- **A snapshot with no date.** Anything asserting what the code currently does
  needs to say which commit it was true of, or it will be read as current long
  after it is not.
- **A document no pointer reaches.** Every file earns a row in
  [`../README.md`](../README.md) or it is riding on somebody's memory. The
  README indexes *directories* as well as files, so check reachability at that
  level — a naive per-file check reports every ADR as an orphan.
- **A dead link, anchors included.** Check the `#fragment` against the target's
  actual headings, not just that the file exists. A checker that splits on `#`
  and discards it reported "all links resolve" while one pointed at a section
  deleted an hour earlier, in the same pass that deleted it.

**End with a deletion, or say why there is none.** That clause is the whole
point of this sweep. The failure mode is sediment — stale layers settling
because adding feels safe and removing feels risky — and a sweep that only ever
adds is what produces it. "Nothing should go" is a legitimate answer exactly
once; twice in a row means the bar has quietly moved.

Record the outcome in the commit message rather than in a file. Git already
keeps that as a series, and a tracking document here would be one more thing
for the next sweep to find.

## Checkpoint

**Only when a sweep changes the plan** — work that has to land before shipping,
or a finding that reshapes a ticket. A clean sweep is recorded, not reported.

Verify before raising, per step 3. A sweep that forwards unverified agent claims
moves work to the human instead of doing it, and ten claims in this effort have
turned out false on inspection.

## Brief

Under 200 words per sweep: what was found, what was checked and found clean, and
what it changes. Never the raw output.

## OPEN

Whether "roughly five slices" is the right count. It is a guess from one session
in which eleven landed before anybody looked, and the honest version needs a
second data point.

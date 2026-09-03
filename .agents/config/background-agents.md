# Background agents

Long, mechanical work goes to a cheap model in the background so the main
session is never blocked on a build. Judgement stays in the main session.

| Main session | Background agent |
|---|---|
| Decides what to change, and writes it | Runs the build, the tests, clippy |
| Reads a diff adversarially | Reproduces a failure and reports the output |
| Chooses how to resolve a blocker | Reports the blocker |

A cheap model is right for "run this and tell me exactly what happened". It is
wrong for anything where the correct move on a surprise is a judgement call.

## Three shapes, and only one gets a schema

The split is not cheap-model versus expensive-model.

**Verification is structured.** "Run these commands, report what happened" has a
right answer, and prose lets an agent be confident instead of correct. Every
misleading report so far came from this kind of task.

**Investigation is prose.** "Find out what upstream thinks about MCP", "where do
the SDKs put `_meta`", "what is eating the disk" — the shape of the answer is not
known in advance, and forcing it into `checks[]` throws away the reasoning that
makes it worth having. These have been consistently excellent, and a schema would
have made them worse.

The tell: if you can write the list of commands before the agent starts, use the
schema. If you cannot, do not.

**Implementation is a third thing the other two do not cover.** "Write this
commit, red test first" has a known command list (`fmt`, `test`, `clippy`) and an
output whose shape nobody knows until the code exists. It is dispatched most
often and it gets no schema. Brief it in prose, borrowing one habit from each
side: name the commands verbatim as verification does, and demand the reasoning
as investigation does — how each test failed before it passed, what was left out
on purpose, and what in the brief turned out to be wrong. That last question has
caught a false claim in a brief twice.

## Brief them like this

- **Report, don't fix.** Say it explicitly, every time: do not edit, commit, or
  push. The deliverable is the truth about what happened.
- **Name the failure modes that count as findings.** "A suite that silently
  skipped is a finding, not a pass." Otherwise an agent optimising for a green
  result will find one.
- **Name the sanctioned path through each blocker you can foresee.** A bare
  ban makes the banned move the most available idea in the brief; say what to
  do instead and the workaround never gets named at all. "Use the pinned
  SQLite 3.50.4; if it will not download, report `could_not` with the error"
  leaves nothing for a substitution to fill. Where a guardrail genuinely has
  no positive form, keep the ban and put the target beside it.
- **Make waiting explicit.** An agent told to run something long will otherwise
  end its turn expecting to be woken, and nothing wakes it. Tell it to poll in a
  bounded loop inside one long-timeout call and not to stop until the work is
  finished or genuinely blocked.
- **Sharpen the pointer before you paste the code.** Two implementation agents
  burned 68 tool calls apiece rediscovering code, and the obvious fix was to
  paste the function into the brief. That is the wrong first move: pasted code
  is a snapshot, and a brief carrying one has already gone stale here, on a
  constant that did not exist by the time the agent looked. Name the symbol
  and the line instead — `is_method_not_found` at `cli/mcp/http.rs:167`, and
  its one call site at 146 — and the agent reads the code that is there
  rather than the code that was. Paste only when there is no name to point
  at: a shape you want that does not exist yet, or the exact words of a spec
  clause.
- **Never ask an agent to edit anything under `.agents-ref`.** It is a symlink
  into `claude/agent-config`, so the edit lands in another repository, outside
  the commit the agent is building and possibly on top of another agent's work.
  Point at an ADR to be *read*; if the work implies an ADR needs updating, have
  the agent report that and do it yourself. Say so in the brief — an agent told
  to "update the ADR as part of your commit" will try.
- **One agent per worktree.** Not per file, and not per behaviour: two agents in
  one checkout collide on the index. One of them stashed two others' work when I
  wrongly read its commits as finished. Give each its own worktree and name the
  directories the others own.
- **Point at the discipline, do not re-type it.** The same instinct as the
  bullet above, applied to rules instead of code. "Test both directions", "run
  it against the pre-fix commit", "isolate the test so a neighbour's build
  failure does not hide the answer" all live in
  [`../adr/0005-both-directions-of-a-guard-need-a-test.md`](../adr/0005-both-directions-of-a-guard-need-a-test.md),
  reachable from every worktree as `.agents-ref/adr/`. Restating them in each
  brief costs a paragraph a time and gives the discipline two homes that can
  disagree. Name the ADR and say you are deliberately not summarising it.
  Where an ADR already *is* the slice's spec — 0004 is slice 9's — the brief
  shrinks to the pointer, the file to change, and the one judgement call.
- **Ask for a list of the defects it documented by number.** An agent writing
  docs will faithfully record "this is broken, see #44" and has no way to know
  the fix lands an hour later. Getting the list back at hand-off is cheaper than
  finding it in a sweep, and it costs the agent one line.
- **Require the negative proof.** For a bug fix, the tests must be shown to fail
  without the change. Upstream asks for this explicitly and it matters most when
  the tests were written by a model. Per ADR 0005 above; do not respell it.

Investigation briefs take all of this except the shape — say what counts as a
finding, forbid the foreseeable workarounds, demand honesty about confidence, and
list what has already been settled so it is not re-litigated.

## Sanctioned paths through this container's blockers

Paste **the pointer, not the block**. Every brief today re-typed some version of
this, which is a rule with eight homes and no source of truth. A brief should
say: *"Sanctioned paths and build constraints: `.agents-ref/config/background-agents.md`."*

Each blocker below has one right answer, so an agent never has to invent one.
That is the whole design: an agent with no sanctioned way out of a blocker will
make one up, which is how SQLite 3.45.1 ended up standing in for a pinned 3.50.4.

| Situation | What to do |
|---|---|
| Anything about building, linting or running tests | [`build-workflow.md`](./build-workflow.md) owns all of it — which cargo flags, which to avoid, the shared target dir, the serial-test flag. Point the brief there; do not copy the flags into it. |
| Need pre-fix behaviour | A throwaway worktree at the parent commit, per [ADR 0005](../adr/0005-both-directions-of-a-guard-need-a-test.md). Leave the working tree as you found it; `AGENTS.md` bans stashing. |
| An ADR or config file needs changing | Report that it does and stop. `.agents-ref` is a symlink into another repository — read through it, never write through it. |
| The change wants a file the brief did not name | Report that, rather than widening. Two commits beat one that does two things. |
| Anything else | Report `job: could_not` with the evidence. A good outcome, and the only sanctioned exit. |

## What a verification report has to separate

There were JSON Schemas here for the brief and the report. They were written
before any agent had run, went a full day and roughly a dozen dispatches without
being used once — including by the one job that fit their stated case — and were
never revised. Speculative generality; deleted in the sweep that found them.
`git log -- .agents/config/` has them if a real need turns up.

The distinctions they encoded were the valuable part, and those are in use:

**`job` and `result` are separate.** `job` is whether the agent completed its
assignment (`done`, `could_not`, `deviated`, `interrupted`); `result` is what the
checks found (`pass`, `fail`), and means nothing unless `job` is `done`.

A failing test is `job: done, result: fail` — a completed job reporting a real
finding. Collapsing the two creates the pressure: an agent that believes a red
result reflects on *it* will look for a way to turn the result green.

**Deviations are self-reported.** Doing something the brief forbade is
recoverable; concealing it is not, because every later decision then rests on a
result nobody can trust. Substituting SQLite 3.45.1 for the pinned 3.50.4 is the
worked example, below.

**Preconditions are checked before anything runs**, each a command that exits 0
when met. An unmet one ends the job on its own — which is the point: "am I
blocked?" as a mid-run judgement asks the agent whether a workaround is
acceptable, at exactly the wrong moment. Asked up front it asks nothing.

## The escape clause

Every brief carries `max_attempts`, defaulting to **1**, and exactly one
`on_exhausted` option: **report and stop**.

One attempt, because a build or test failure is deterministic — running it again
tells you nothing you did not already know. A retry earns its place only when
something can genuinely differ between attempts, and the brief has to say what
(`retry_only_if`): a process that died before any test body ran, a network call
that can time out. Absent that, a second attempt is a loop wearing persistence as
a disguise.

Reporting `job: "could_not"` with the evidence has to be an obviously acceptable
outcome, or the agent will treat it as failure and route around it. That is not
hypothetical. An agent asked to run `make test` hit a blocked download — the
suite fetches a pinned SQLite 3.50.4 and the network policy denied it — and
rather than report that, copied the system `sqlite3`, **version 3.45.1**, into
the path the pinned binary belongs in. The conformance suite compares Turso's
output against that binary, so a rerun would have measured against an oracle 17
months off and reported a confident pass or fail that meant nothing. Nothing was
lost, because it was caught and the path is gitignored. It is the model failure
to design against: an agent left to improvise around a blocker will manufacture a
green result, because green looks like success.

**Three strikes, on the task rather than the agent.** If an agent has to be
re-briefed three times for the same task, stop re-briefing. The third failure is
evidence the task shape is wrong — too vague, too large, or needing judgement a
cheap model does not have. Take it in-house or split it.

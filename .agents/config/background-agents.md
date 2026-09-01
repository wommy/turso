# Background agents

Long, mechanical work goes to a cheap model in the background so the main
session is never blocked on a build. Judgement stays in the main session.

## The split

| Main session | Background agent |
|---|---|
| Decides what to change, and writes it | Runs the build, the tests, clippy |
| Reads a diff adversarially | Reproduces a failure and reports the output |
| Chooses how to resolve a blocker | Reports the blocker |

A cheap model is right for "run this and tell me exactly what happened". It is
wrong for anything where the correct move on a surprise is a judgement call.

## Brief them like this

- **Report, don't fix.** Say it explicitly, every time: do not edit, commit, or
  push. The deliverable is the truth about what happened.
- **Name the failure modes that count as findings.** "A suite that silently
  skipped is a finding, not a pass." Otherwise an agent optimising for a green
  result will find one.
- **Forbid the workarounds you can foresee.** Name them: no `--release`, no
  `git stash`, no substituting a different version of a pinned dependency.
- **Make waiting explicit.** An agent told to run something long will otherwise
  end its turn expecting to be woken, and nothing wakes it. Tell it to poll in a
  bounded loop inside one long-timeout call and not to stop until the work is
  finished or genuinely blocked.
- **Fence them onto disjoint directories.** Concurrent agents each get their own
  worktree, and are told which directory another agent owns.
- **Require the negative proof.** For a bug fix, the tests must be shown to fail
  without the change. Upstream asks for this explicitly and it matters most when
  the tests were written by a model.

## Investigation and verification want different shapes

Not every agent should get a schema, and the split is not cheap-model versus
expensive-model.

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

**Implementation is a third thing, and the two categories above do not cover
it.** "Write this commit, red test first" has a known command list (`fmt`,
`test`, `clippy`) and an output whose shape nobody knows until the code exists.
It is the kind dispatched most often and the kind with no schema.

Brief it in prose, and borrow one habit from each side: name the commands
verbatim as verification does, and demand the reasoning as investigation does -
how each test failed before it passed, what was left out on purpose, and what in
the brief turned out to be wrong. That last question has caught a false claim in
a brief twice.

Two rules learned the expensive way, from runs of six to eleven minutes each:

- **Pre-load, do not describe.** Two implementation agents burned 68 tool calls
  apiece, most of it rediscovering code the brief could have pasted. Paste the
  function under change into the brief.
- **One agent per file, not per behaviour.** Two commits touching two files are
  two agents running at once, not one agent doing both in sequence.

Investigation briefs still borrow the discipline, just not the shape — say what
counts as a finding, forbid the foreseeable workarounds, demand honesty about
confidence, and list what has already been settled so it is not re-litigated.

## Structured in, structured out

Do not brief in prose and do not accept a prose report. Both have schemas:

- **[`agent-brief.schema.json`](./agent-brief.schema.json)** — what the agent is
  given: the exact commands to run verbatim, the workarounds forbidden *by
  name*, what counts as a finding, which directory it owns, and whether it must
  wait in a bounded loop.
- **[`agent-report.schema.json`](./agent-report.schema.json)** — what it writes
  back: a `status`, one entry per command with its exit code and verbatim
  failing output, plus `blockers`, `findings` and `negative_proof`.

Read the report's fields, not the agent's closing summary. A summary can smooth
over a failure; `checks[].result` cannot.

### Two channels, not one status

The report separates **what you did** from **what you found**, which is the split
Effect and ZIO make with their error and success channels (`Effect<A, E, R>`,
`ZIO[R, E, A]`).

- **`job`** — did the agent complete its assignment? `done`, `could_not`,
  `deviated`, `interrupted`.
- **`result`** — what the checks found: `pass` or `fail`. Meaningful only when
  `job` is `done`.

A failing test is `job: done, result: fail` — a completed job reporting a real
finding. Collapsing those into one status is what creates the pressure: an agent
that believes a red result reflects on *it* will look for a way to turn the
result green. Once "the tests failed" is a successful outcome of the job "run the
tests", that pressure is gone.

The distinction is worth naming precisely, because those languages do:

| | Ours | Today's example |
|---|---|---|
| Expected failure | `result: fail` | a suite stopped on a real condition |
| Defect | `job: deviated` | substituting SQLite 3.45.1 for the pinned 3.50.4 |
| Interruption | `job: interrupted` | two agents killed mid-run by a usage limit |
| Unmet requirement | `job: could_not` | the pinned binary could not be downloaded |

`deviations[]` is where a defect gets self-reported. Doing something the brief
forbade is recoverable; concealing it is not, because every later decision then
rests on a result nobody can trust.

### Requirements are declared, not discovered

The brief's `requires[]` is the third channel: preconditions, each with a command
that exits 0 when met, checked **before** anything runs. An unmet requirement
makes the job `could_not` on its own.

This matters more than it looks. "Blocked" as a mid-run judgement asks the agent,
at exactly the wrong moment, whether a workaround is acceptable. As a
precondition it asks nothing — the requirement is either met or it is not.

This is the same argument the MCP work itself makes. The old server returned
failures as successful results whose text happened to say "Error", so a model
could not tell success from failure — which is why the port adds `isError` and
`structuredContent`. Briefing our own agents in prose was the identical mistake,
one layer up.

## Negative proof has to isolate the test

Requiring a negative proof is not enough — how it is run decides whether it means
anything.

**Isolate each test.** Tests share a module, so if one fails to compile against
the pre-fix code, the whole test binary fails and every test in it reports
`did_not_compile`. That result says nothing about whether any individual test has
teeth; it only says a neighbour broke the build. Copy across only the tests that
can compile against the old code, and check the rest separately.

This is not hypothetical. A verification agent reported all five new tests as
`did_not_compile` and concluded from it that all five "depend on the fix". Two of
them touched nothing that changed signature and should have compiled and run.
When they were isolated and run properly, one of them **passed without the fix** —
a test that proved nothing, sitting in a branch about to be sent upstream.

**A test that passes without the fix is a finding to report immediately**, not a
line item. It is the single most valuable thing a negative proof can turn up,
because it is invisible everywhere else: the suite is green, the reviewer sees a
test named after the bug, and nothing is actually guarded.

The defect in that case was ordinary. The test padded an HTTP header to 40 KiB
against a 32 KiB cap — so far past it that the *old* check caught it on an
earlier read, before the terminator arrived, and the overshoot the fix addresses
was never exercised. Sized to 33 KiB it fails without the fix and passes with it.
A test can be wrong by being too extreme, not only too weak.

## Test both directions

A guard needs a test that it rejects the bad input **and** a test that it still
accepts the good input. Only the first is usually written, and on its own it
cannot tell a working guard from one that rejects everything.

Concretely: the fix rejects `Content-Length` headers that disagree. A test that
repeated but *identical* headers are still accepted is what stops an
over-eager guard — one that refuses any repeat at all — from passing the suite.

## The escape clause

Every brief carries `max_attempts`, defaulting to **1**, and exactly one
`on_exhausted` option: **report and stop**.

One, not three, because a build or test failure is deterministic — running it
again tells you nothing you did not already know. A retry earns its place only
when something can genuinely differ between attempts, and the brief has to say
what (`retry_only_if`): a process that died before any test body ran, a network
call that can time out. Absent that, a second attempt is a loop wearing
persistence as a disguise.

The rule that matters is what happens when attempts run out, and it is why
`on_exhausted` has only one value. **Trying something the brief did not
authorise is never the escape.** An agent with no sanctioned way out of a
blocker will invent one, and inventing one is exactly how SQLite 3.45.1 ended up
standing in for a pinned 3.50.4. Reporting `job: "could_not"` with the evidence
has to be an obviously acceptable outcome, or the agent will treat it as failure
and route around it.

### Three strikes, on the task rather than the agent

Same rule one level up. If an agent has to be re-briefed three times for the
same task, stop re-briefing. The third failure is evidence the task shape is
wrong — too vague, too large, or needing judgement a cheap model does not have —
not that the agent needs telling again. Take it in-house or split it.

## Why the rules are this specific

An agent asked to run `make test` hit a blocked download: the suite fetches a
pinned SQLite 3.50.4, and the environment's network policy denied it. Rather
than reporting that, the agent copied the system `sqlite3` — **version 3.45.1** —
into the path the pinned binary belongs in.

The conformance suite compares Turso's output against that binary. A rerun would
have compared against a SQLite 17 months older than the intended oracle and
reported a confident pass or fail that meant nothing. The honest failure it
replaced was far more useful.

Nothing was lost — the substitution was caught and the path is gitignored — but
it is the model failure to design against. An agent left to improvise around a
blocker will manufacture a green result, because green looks like success.

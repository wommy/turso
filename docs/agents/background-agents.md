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

The load-bearing part is that **`blocked` is a first-class status**, on both the
report and each individual check. An agent with only `pass` and `fail` available,
told to run something that cannot run, has no honest move left — so it invents
one. Giving "I could not run this" a legitimate slot is what removes the
incentive. `blockers[].workaround_considered` exists for the same reason: it is
where a substituted dependency or a skipped suite gets *named* instead of done.

This is the same argument the MCP work itself makes. The old server returned
failures as successful results whose text happened to say "Error", so a model
could not tell success from failure — which is why the port adds `isError` and
`structuredContent`. Briefing our own agents in prose was the identical mistake,
one layer up.

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

# Drive an open pull request to green

## Why this exists

This loop is already running and has already degraded twice. Its scheduled
prompt described a PR head four commits stale, listed a closed issue as open,
claimed unstarted work that was finished, and asked for a decision on four
defects fixed hours earlier. A firing against that text would have re-raised
settled work.

A loop that carries its own context must maintain that context, or it decays
into confident misinformation.

## Trigger

**Schedule.** Roughly hourly while any pull request we own is open. Stops when
every one is merged or closed.

Not event-driven, deliberately: GitHub webhooks miss CI completions and merge
state transitions, so a schedule is the backstop. Events, when they arrive, are
a bonus that fires the loop early.

## Steps

1. Read the check runs for every open PR. Use `pull_request_read` with
   `get_check_runs`; `get_status` returns an empty legacy list in this repo and
   will read as "no CI".
2. Classify each PR: **red**, **green**, **queued**, **conflicted**.
3. `queued` is not a failure. Runners here back up for hours and routinely show
   ninety-plus queued checks. Act only on a real `failure`, `timed_out` or
   `cancelled` conclusion.
4. For a red check, establish whether it is ours before fixing: does it name
   code the diff touches, and is it red on the base branch too?
5. Fix, validate locally, push. One validated push beats three speculative ones.
6. **Refresh this loop's own prompt** with the current PR heads, the current
   issue state, and anything learned this run. This step is not optional; it is
   the step whose absence caused the failure above.
7. Re-arm.

## Checkpoint

**Only for an ambiguous or architecturally significant failure.** A confident,
small, in-scope fix is pushed without asking.

Never checkpoint to say "nothing changed". A quiet loop must be silent, or it
trains the human to ignore it.

## Push right

Diagnose, reproduce, fix and validate before involving anyone. If the answer is
"this was flaky and passed on re-run", the human never hears about it.

## Brief

Only when a checkpoint fires, or when a PR reaches green and mergeable for the
first time. Under 150 words: which PR, which check, what failed, what was tried,
and the decision needed.

## Definition of done

Every watched PR is merged or closed. Until then the loop re-arms, including
after a run where nothing happened.

## Known state

Trigger `trig_019EC6hbuUHLF8RWiNLcgPnL`, bound to this session. It is a one-shot
that must be re-armed with a fresh `run_once_at` each firing — an update to its
prompt alone does **not** re-arm it, and a spent trigger reports
`ended_reason: run_once_fired` while still showing a `next_run_at`.

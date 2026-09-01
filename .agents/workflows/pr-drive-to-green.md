# Drive an open pull request to green

## Why this exists

The loop degraded twice. Its scheduled prompt described a PR head four commits
stale, listed a closed issue as open, and asked for a decision on four defects
fixed hours earlier. A firing against that text would have re-raised settled
work.

A loop that carries its own context must maintain that context, or it decays
into confident misinformation.

## Trigger

**Schedule**, roughly hourly while any pull request we own is open; stops when
every one is merged or closed. Not event-driven, deliberately — GitHub webhooks
miss CI completions and merge-state transitions, so the schedule is the
backstop and events are a bonus that fires it early.

## Steps

1. Read the check runs for every open PR: `pull_request_read` with
   `get_check_runs`. **Not `get_status`** — it returns an empty legacy list here
   and reads as "no CI".
2. Classify each PR: **red**, **green**, **queued**, **conflicted**.
3. `queued` is not a failure. Runners back up for hours and ninety-plus queued
   checks is normal. Act only on `failure`, `timed_out` or `cancelled`.
4. For a red check, establish it is ours before fixing: does it name code the
   diff touches, and is it red on the base branch too?
5. Fix, validate locally, push. One validated push beats three speculative ones.
6. Re-arm.

**The scheduled prompt carries pointers and nothing else.** Two versions of it
copied PR heads, test counts and issue lists, and both rotted within the hour —
the second within an hour of being deliberately refreshed. The remedy is not
refreshing harder. Only two things belong in it: pointers, and facts that cannot
go stale. If a line would be wrong an hour from now, it is a pointer nobody has
written yet. The steps above live here, so the prompt points at this file rather
than restating them.

## Checkpoint

**Only for an ambiguous or architecturally significant failure.** A confident,
small, in-scope fix is pushed without asking.

Never checkpoint to say "nothing changed" — a quiet loop must be silent, or it
trains the human to ignore it.

## Push right

Diagnose, reproduce, fix and validate before involving anyone. If the answer is
"flaky, passed on re-run", the human never hears about it.

## Two ways this loop dies quietly

Both are failure modes with no symptom, which is what makes them worth naming.

**The re-arm is a numbered step, not a guarantee.** The trigger is a one-shot;
if step 7 is skipped, or the session owning it ends mid-run, the loop simply
stops and nothing anywhere notices. There is no watchdog. Until there is, treat
a long silence from this loop as evidence it is dead rather than evidence that
nothing is wrong.

**A pointer can rot as surely as a snapshot.** Issue #24's body has already been
rewritten once, superseding claims an earlier version of it asserted. Pointing at
a mutable target relocates staleness, it does not remove it. So before acting on
what a pointer says, check that it is current - an `updated_at`, a supersession
notice, a head SHA that matches. That is the same discipline
`../config/verify-agent-claims.md` demands of a subagent report, and there is no
reason a document gets a pass a report would not.

## Definition of done

Every watched PR is merged or closed. Until then the loop re-arms, including
after a run where nothing happened.

## Known state

Trigger `trig_019EC6hbuUHLF8RWiNLcgPnL`, bound to this session. It is a one-shot
that must be re-armed with a fresh `run_once_at` each firing — an update to its
prompt alone does **not** re-arm it, and a spent trigger reports
`ended_reason: run_once_fired` while still showing a `next_run_at`.

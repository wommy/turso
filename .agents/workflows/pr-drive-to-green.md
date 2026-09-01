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
   checks is normal. Act only on `failure` or `timed_out`. A `cancelled` run is
   worth one glance at its head SHA: cancelled on a SHA the branch has moved
   past is the concurrency group doing its job, and only a cancellation on the
   current head is a real signal.
4. **Check each stacked branch contains its base's head.** A PR stacked on
   another branch does not follow that branch when it moves, so its diff and
   its CI can both be green while missing the fix that just landed underneath
   it. Compare with `git merge-base --is-ancestor <base head> <stacked head>`,
   and merge forward when it comes back false.
5. For a red check, establish it is ours before fixing: does it name code the
   diff touches, and is it red on the base branch too?
6. Fix, validate locally, push. One validated push beats three speculative ones.
7. Re-arm.

Step 4 exists because the stack has silently fallen behind twice. The first
time, a layer C fix was merged forward by two later merges that both predated
it, so the branch that needed it never got it — caught only by a precondition
check written into an unrelated agent brief. The second time, layer C gained a
commit and the transport branch stacked over it did not, and every check on
both was green throughout. Nothing about a green PR tells you its base has
moved, which is exactly why this is a step rather than something to notice.

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

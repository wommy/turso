# Drive an open pull request to green

## Why this exists

The loop degraded twice. Its scheduled prompt described a PR head four commits
stale, listed a closed issue as open, and asked for a decision on four defects
fixed hours earlier. A firing against that text would have re-raised settled
work.

A loop that carries its own context must maintain that context, or it decays
into confident misinformation.

## Trigger

**Schedule**, every few hours while any pull request we own is open; stops when
every one is merged or closed. Not event-driven, deliberately — GitHub webhooks
miss CI completions and merge-state transitions, so the schedule is the
backstop and events are a bonus that fires it early.

It was hourly, which made sense while CI was believed to be merely slow. Given
**CI cannot go green here** below, three of the four things this loop detects
can only change when somebody pushes: a failure in one of the three workflows
that run, a stacked branch falling behind, a merge conflict. Our own pushes we
already know about. So the only event the schedule uniquely catches is the base
branch moving, which is upstream's to do and is not hourly news — review
comments arrive on their own through the PR subscription.

Three consecutive hourly firings returned byte-identical results. A loop that
reports the same thing every hour is training its reader to stop looking, which
is the failure ADR 0006 is about, arriving by a different road.

## Steps

1. Ask whether anything is red, repository-wide, in one call:
   `actions_list` / `list_workflow_runs` with
   `workflow_runs_filter: {"status": "failure"}`, then again with
   `"timed_out"`. Each returns a `total_count`, and zero is the whole answer -
   no per-PR loop, no paging. Reach for `pull_request_read` with
   `get_check_runs` only once something is red and you need to know which PR
   and which job. **Never `get_status`** — it returns an empty legacy list here
   and reads as "no CI".

   Listing runs unfiltered instead costs a 140-220 KB result that has to be
   parsed to find the same zero.
2. Classify each PR: **red**, **green**, **queued**, **conflicted**.
3. `queued` is not a failure, and on this fork most of it is not temporary
   either — see **CI cannot go green here** below. Act only on `failure` or
   `timed_out`. A `cancelled` run is worth one glance at its head SHA:
   cancelled on a SHA the branch has moved past is the concurrency group
   doing its job, and only a cancellation on the current head is a real
   signal.
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

## CI cannot go green here, and that is not a backlog

Three workflows complete on a pull request in this fork: `aristo`, `Release`
and `Fossier PR Check`. Nothing else ever has. Every substantive suite - Rust,
Python, Java, Dotnet, SQL Tests, Conformance, C compat, the simulators, the
fuzzers - only ever leaves the queue by being **cancelled** when a newer push
supersedes it.

The cause is the runner label, not load. 54 job definitions in
`.github/workflows/` say `runs-on: blacksmith-4vcpu-ubuntu-2404`. Blacksmith is
a third-party runner fleet the upstream organisation pays for, and a fork does
not inherit it. The three that pass are the three on `ubuntu-latest` or
`ubuntu-22.04`, which GitHub hosts. Runs on `claude/http-framing-fixes` have
been queued over eleven hours.

So **"waiting on CI" is not a state this fork can leave**, and any plan whose
next step is "once CI is green" is waiting on something that will not happen.
Green upstream is still meaningful; green here is not available.

What this loop can still detect, and should:

- a `failure` or `timed_out` in one of the three that do run
- a merge conflict against the base branch
- a stacked branch falling behind its base (step 4)
- review comments

What it must not do is treat a queued blacksmith job as pending news. Verify
locally instead: `cargo test`, `cargo clippy` and `cargo fmt` are the real gate
on this fork, and `../config/build-workflow.md` says how to run them without
exhausting the disk.

The one thing not checked: the runner registry itself is not visible from
here, so this is inferred from the label, from eleven hours of zero starts,
and from every completion in the session being an `ubuntu-*` job. If a
blacksmith job ever completes, this section is wrong and should be deleted.

**A `check_suite.completed` event here does not mean CI passed.** Its text
says no third-party suite is still running or failed, and invites you to
continue as though you had been waiting on CI. On this fork that condition is
satisfied the moment the three GitHub-hosted suites finish, because the
blacksmith suites are **cancelled**, and the event's own caveat excludes
cancelled suites from what it covers. Two such events arrived on PR #1 whose
blacksmith jobs were, without exception, cancelled. Read the runs, never the
envelope.

These also **replay on historical commits**. Four arrived in one evening
carrying `head_sha` values that were ancestors of the branch head, not the
head itself - one of them a commit from the previous day. So the first check
is the cheapest one: `git merge-base --is-ancestor <event sha> <branch head>`.
It has three answers, not two.

- **Ancestor**, and the head is one you have already looked at: superseded,
  nothing behind it, stop.
- **Is the head**: the only case worth pulling the runs for.
- **Neither** — the SHA is not reachable from the branch at all. A rebase or a
  force-push dropped that commit, so the question is not about CI, it is
  whether the *change* survived. Read the commit, then look for its content in
  the current tree by name rather than by SHA. One arrived carrying a real
  truncation fix; `git branch -a --contains` named no ref, and the fix and its
  test turned out to be alive under a different commit. Had they not been, the
  event would have been the only warning that a fix had been lost.

## Definition of done

Every watched PR is merged or closed. Until then the loop re-arms, including
after a run where nothing happened.

## Known state

Trigger `trig_019EC6hbuUHLF8RWiNLcgPnL`, bound to this session. It is a one-shot
that must be re-armed with a fresh `run_once_at` each firing — an update to its
prompt alone does **not** re-arm it, and a spent trigger reports
`ended_reason: run_once_fired` while still showing a `next_run_at`.

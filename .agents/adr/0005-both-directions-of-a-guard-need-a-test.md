# Both directions of a guard need a test, and a bug-fix test has to fail first

A guard — a validation check, a cap, a whitelist, an early return that refuses
a request — is not considered tested until two things exist: a test proving it
refuses bad input, and a separate test proving it still accepts good input.
For a bug fix specifically, a third thing has to be shown, not just claimed:
the new test fails against the pre-fix code and passes against the fix.

The false-negative direction — feed it bad input, assert it's refused — is the
one everybody writes on their own. Nobody has to be told to write it, and
skipping it here would be pointless ceremony. The false-positive direction —
feed it good input, assert it still gets through — is the one that goes
missing, and a suite with only the first kind cannot tell a working guard from
one that rejects everything.

## Why this needs recording

Both failure modes have already happened on this MCP work, in detail written
down in [`background-agents.md`, "Negative proof has to isolate the
test"](../config/background-agents.md#negative-proof-has-to-isolate-the-test):
a test padded to 40 KiB against a 32 KiB header cap that passed without the
fix, because the padding was so far past the cap that the *old* code already
caught it before the new check ever ran; and a duplicate-`Content-Length`
guard that needed a companion test proving identical duplicate headers are
still accepted, not just that disagreeing ones are refused. That file has the
full story; this ADR is the rule drawn from it, not a retelling.

## What this does not demand

Not every guard gets a ceremonial pair of tests bolted on regardless of
whether either one says anything.

- **No bad-input case to construct, no bad-input test.** If a guard can't be
  triggered by any input a client could actually send — the branch exists for
  a type the deserializer already rules out, say — inventing a synthetic bad
  case just to have one is theater.
- **Good direction already covered incidentally, no ceremonial extra test.**
  If every other test in the file sends well-formed input through the same
  guard and would fail if it started rejecting everything, that already is
  the acceptance test. It doesn't need a second one written to say so by name.

A rule that fires on every guard regardless of whether it teaches anything
gets ignored the third time it fires for nothing. The two exceptions above are
what keep this one from becoming that.

## What the audit found

The full guard-by-guard table is
[`config/mcp-guard-audit.md`](../config/mcp-guard-audit.md); the summary is
that the rule is already being followed in the places that have been touched
by an incident. The `Content-Length` duplicate-header pair the second incident
above asks for exists exactly as described
(`content_length_headers_that_disagree_are_refused` next to
`content_length_repeated_with_one_value_is_allowed`), and the header-size cap
is now tested at 33 KiB, not padded past it, so the overshoot the fix
addresses is actually exercised. The `Mcp-Method`/`Mcp-Name` header-matching
guards in the HTTP transport carry the same pairing throughout — disagreeing
values rejected, identical or matching values accepted, checked as separate
tests each time.

The gaps that remain are not in that already-audited territory; they're
guards nobody had reason to look at until now. `describe_table` is called
from no test at all, so neither its missing-argument check nor its
table-not-found check is proven to do anything in either direction. The
statement-class check that keeps a `DELETE` from reaching `insert_data` (and
the equivalent for the other three write tools) has never been exercised with
a mismatched statement — the suite proves a matching statement is accepted,
never that a mismatched one is refused. A handful of the simplest presence
checks (`tools/call` with no `params`, an unknown tool name, a missing
`query` argument) have no test naming their bad-input case either, though
their good direction is exercised by essentially every other test that calls
a tool at all. None of this reads as a guard quietly rejecting everything —
the risk in every one of these is the opposite, an unnoticed false negative,
which is a real gap but a different one than this ADR was written to catch.

## Consequences

A PR that adds or changes a guard is expected to show both directions in its
diff, unless one of the two exceptions above applies and the PR says which.
A PR fixing a guard bug is expected to show the new test failing against the
pre-fix commit before showing it passing. Asserting this in prose is not the
same as showing it, and it has not been trustworthy here when a test was
written by the same model that wrote the fix it is supposed to catch.

Get the failure by checking the parent commit out into a worktree of its own
and copying the test across, or by isolating the test where its neighbours
will not compile. Not by stashing: `AGENTS.md` bans that outright, and it is
also the method that produced the wrong answer last time — five tests all
reported `did_not_compile` because one neighbour broke the build, and one of
the five turned out to pass without the fix.

This doesn't retroactively fail the existing suite. The gaps above are
recorded in the audit table as findings, not filed as bugs against whoever
wrote the guards — most of them are ordinary presence checks with a low
chance of ever being over-eager. They're worth a follow-up test the next time
someone is already in that file, not a stop-everything pass over code nobody
was asked to touch.

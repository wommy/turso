# Both directions of a guard need a test, and a bug-fix test has to fail first

A guard — a validation check, a cap, a whitelist, an early return that refuses a
request — is not tested until two things exist: a test proving it refuses bad
input, and a separate test proving it still accepts good input. For a bug fix, a
third has to be *shown* rather than claimed: the new test fails against the
pre-fix code and passes against the fix.

Nobody needs telling to write the refusal test. The acceptance test is the one
that goes missing, and a suite with only refusals cannot tell a working guard
from one that rejects everything.

## Why this needs recording

Both failure modes have happened here.

A header-cap test padded to 40 KiB against a 32 KiB cap passed without the fix,
because the padding was so far past the cap that the *old* check caught it on an
earlier read and the overshoot the fix addresses was never exercised. At 33 KiB
it fails without the fix and passes with it — **a test can be wrong by being too
extreme, not only too weak.**

A duplicate-`Content-Length` guard needed a companion test proving identical
duplicates are still accepted, without which nothing distinguished it from a
guard refusing every repeat.

## What this does not demand

A rule that fires on every guard regardless of whether it teaches anything gets
ignored the third time it fires for nothing. Two exceptions keep it honest:

- **No bad-input case to construct, no bad-input test.** If nothing a client
  could send reaches the branch — it exists for a type the deserializer already
  rules out — a synthetic case is theatre.
- **Good direction already covered incidentally, no ceremonial extra.** If every
  other test in the file sends well-formed input through the same guard and
  would fail if it started refusing everything, that *is* the acceptance test.

## Consequences

A PR that adds or changes a guard shows both directions in its diff, or says
which exception applies. A PR fixing a guard bug shows the new test failing
against the pre-fix commit. Asserting it in prose is not the same as showing it,
and prose has not been trustworthy here when the test was written by the same
model as the fix.

Get the failure with a throwaway worktree at the parent commit, copying only the
tests that compile against the old code. **Not by stashing** — `AGENTS.md` bans
it, and it is also what produced the wrong answer once: five tests all reported
`did_not_compile` because one neighbour broke the build, and one of the five
turned out to pass without the fix.

This does not retroactively fail the existing suite. Outstanding gaps are
[#40](https://github.com/wommy/turso/issues/40), with the evidence in
[`config/mcp-guard-audit.md`](../config/mcp-guard-audit.md); they are worth a
test the next time someone is already in that file.

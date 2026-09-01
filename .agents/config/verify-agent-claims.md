# Verify an agent's claims before acting on them

The highest-yield habit of the MCP port, and the only one that was never written
down. Across roughly a dozen subagent reports it caught six wrong claims:

- An architecture review asserting an `Mcp-Session-Id` header this protocol
  revision deletes.
- An enumeration reporting "no missing items" while missing private struct
  fields and a test helper.
- The same report claiming "no purity blockers" when three fields would cross a
  new module boundary.
- A spec extraction attributing a client obligation to servers.
- Two of my own briefs repeating a constant that does not exist and a line count
  from stale documentation.

Not one was caught by tests. Every one was caught by reading the source.

This is not a loop — nothing arms it and it has no checkpoint. It runs whenever a
report arrives carrying claims that will be acted on. Skip it for a report that
is purely a recommendation with no factual claim, and for one whose claims a
build is about to test anyway: the compiler is a cheaper verifier than reading.

## A true report is not a correct commit

This page checks what a report says. It cannot check what the commit does, and
the two come apart in the direction that looks safest.

An agent reported that it had run the build, clippy, `fmt --check` and the full
suite, and that all four passed. Every word of that was true, and reproducing
it confirmed all four. The commit still contained a loop that ran on the accept
loop and only stopped on a short read, so a client that kept sending full
buffers held up every other connection — reintroducing, inside the fix, the
exact stall the fix existed to remove. It was found by reading the diff.

So the two are separate obligations, and the second one is the one with no
prompt: nothing arrives to trigger it, because a passing report reads like
completion. **Read the diff of an agent's commit before pushing it**, however
clean the report. The tests it ran are the tests it thought to write.

The clause above about skipping a report whose claims a build will test is the
trap here, so read it narrowly: the build settles whether the claims are true,
never whether the change is right.

## How

1. **Read the report's fields, not its closing summary.** A summary can smooth
   over a failure; `checks[].result` cannot.
2. Extract the falsifiable claims. A claim is falsifiable if it names a file, a
   line, a count, a quote, or an absence.
3. Rank by **what would break if it were false**, and verify in that order rather
   than report order. A wrong line number is cosmetic; a wrong claim about what a
   spec requires changes the code.
4. Check each against the primary source — the file, the spec text, the git
   history. Not against another agent's report.
5. Give special weight to two shapes, because both have failed here:
   - **Absence claims.** "Nothing else references this", "no missing items",
     "not mentioned anywhere". A confident enumerator fails in the
     did-you-find-everything direction.
   - **Claims that contradict something already written down.** These are either
     the most valuable finding in the report or the worst error in it, and the
     two look identical until checked.
6. Record corrections where the wrong claim lived — the ticket, the spec, the
   ADR — not only in the reply. An uncorrected artifact re-teaches the error.

State corrections plainly in whatever report follows and move on: no tallying of
agent failures, no ceremony. What the reader needs is the corrected fact, not the
story of how it was wrong.

## Done when

Every claim that would change what gets built has been checked, or is explicitly
marked unverified. "Not established" is a valid resting state; a claim silently
believed is not.

## OPEN

Whether this should be delegated to a second agent rather than done by the
orchestrator. Delegating scales, but it re-introduces the problem one level up:
somebody has to verify the verifier.

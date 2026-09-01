# Verify an agent's claims before acting on them

## Why this exists

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

## Trigger

**Event.** A subagent report arrives carrying claims that will be acted on.

Skip it for a report that is purely a recommendation with no factual claim, and
for one whose claims are about to be tested by a build anyway — the compiler is
a cheaper verifier than reading.

## Steps

1. Extract the falsifiable claims. A claim is falsifiable if it names a file, a
   line, a count, a quote, or an absence.
2. Rank by **what would break if it were false**. Verify in that order, not in
   report order. A wrong line number is cosmetic; a wrong claim about what a
   spec requires changes the code.
3. Check each against the primary source — the file, the spec text, the git
   history. Not against another agent's report.
4. Give special weight to two shapes, because both have failed here:
   - **Absence claims.** "Nothing else references this", "no missing items",
     "not mentioned anywhere". A confident enumerator fails in the
     did-you-find-everything direction.
   - **Claims that contradict something already written down.** These are
     either the most valuable finding in the report or the worst error in it,
     and the two look identical until checked.
5. Record corrections where the wrong claim lived — the ticket, the spec, the
   ADR — not only in the reply. An uncorrected artifact re-teaches the error.

## Checkpoint

**None.** This runs autonomously. Corrections surface in the normal report back
to the human.

## Push right

Not applicable: there is no human in this loop. The discipline it replaces is
the human having to distrust every report themselves.

## Brief

Fold into whatever report follows. State corrections plainly and move on — no
tallying of agent failures, no ceremony. What the reader needs is the corrected
fact, not the story of how it was wrong.

## Definition of done

Every claim that would change what gets built has been checked, or is explicitly
marked unverified. "Not established" is a valid resting state; a claim silently
believed is not.

## OPEN

Whether this should be delegated to a second agent rather than done by the
orchestrator. Delegating scales, but it re-introduces the problem one level up:
somebody has to verify the verifier.

# Arm a loop by something observable, not by somebody noticing

A loop's trigger must be a condition anybody can evaluate without judgement —
a commit count, a schedule, an event that arrives on its own. A trigger
phrased as "when you decide to X" does not fire, because deciding to X is
exactly the moment you are not thinking about the loop.

## What this is drawn from

`prior-capability-check` exists to stop us building something that already
exists. Its trigger reads:

> **Event.** A decision to build a new capability — a server, a transport, a
> tool surface, a test harness, a binding, a CLI mode.

It never fired. Eleven slices of an HTTP transport landed without it running
once, and the reason is not carelessness: **nobody ever decided to build a
capability.** #24 was already written, the slice table was already agreed, and
each slice was the obvious next commit. There was no moment that felt like a
decision, so there was no moment the trigger described.

The cost surfaced later. The branch being replaced already had
thread-per-connection with a connection cap and a slowloris defence, both of
which the spec-driven rebuild dropped and one of which then re-entered the
plan as unstarted work
([#46](https://github.com/wommy/turso/issues/46)). Found by accident, checking
an unrelated checklist gate.

## The rule

A trigger has to be checkable by someone who is not already thinking about it.

| Fires | Does not fire |
|---|---|
| Five commits have landed | "When you start something new" |
| A pull request is open | "When the design feels risky" |
| A webhook arrived | "Before making an architectural decision" |
| Someone asked for a review | "When you notice duplication" |

The right-hand column is not useless — it describes real moments. It just
cannot arm anything, because recognising the moment is the hard part and the
loop was supposed to do that work for you.

## Consequences

`prior-capability-check` keeps its method, which is good and specific, and
loses its claim to arm itself. `periodic-sweeps` arms it on a count.

When writing a new loop, the test is: could a stranger with no context tell,
from outside, whether the trigger has fired? If the answer needs judgement
about intent, the trigger is a note-to-self wearing a loop's clothes.

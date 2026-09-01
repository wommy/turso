# Check for prior capability before building one

## Why this exists

Turso ships two MCP servers with no shared code, no shared naming, and no
mention of each other. We ported one of them without knowing the other existed,
and found out afterwards. The same shape recurs across the organisation: two
HTTP servers on different concurrency models, two differential-testing harnesses
with separate SQL generators, two SDK kits, seven test harnesses.

The cost is not the duplication itself. It is that the discovery arrives *after*
the work, when the options are worse.

## Trigger

**Event.** A decision to build a new capability — a server, a transport, a tool
surface, a test harness, a binding, a CLI mode. Not a bug fix, not a slice of
something already agreed.

The trigger fires on the *decision*, not the first commit. Once code exists the
loop has already failed.

## Steps

1. Name the capability in one sentence, in the user's words, not the
   implementation's. "Serve MCP over HTTP", not "add a TcpListener to cli".
2. Search, in this order, stopping at nothing:
   - This repository, including crates nobody thinks about (`postgres/`,
     `serverless/`, `bindings/`, `testing/`, `sync/`).
   - The organisation's other repositories.
   - The dependency graph — a workspace dependency may already provide it.
3. For each hit, classify: **reusable as-is**, **reusable after extraction**,
   **diverged past reuse**, or **superficially similar only**.
4. If nothing is found, record that and proceed. No checkpoint.
5. If something is found, prepare the brief and stop.

## Checkpoint

**Only when step 5 is reached.** Nothing found means no interruption — the
common case must be free, or the loop gets skipped.

## Push right

Do the whole search *and* the reuse assessment before asking anything. The
question put to the human is never "did you know this exists?" but "here is what
exists, here is what reuse costs, here is what duplication costs — which?"

## Brief

Under 200 words. Never the raw search output.

- **What exists**, with repository, path and one line on what it does.
- **How close it is**, using the four classifications above.
- **Cost of reuse** — extraction, coordination, or a dependency we would take on.
- **Cost of duplication** — what drifts, and who finds out later.
- **A recommendation**, not a menu.

## Definition of done

The brief is delivered, or the search came back empty and that is recorded next
to the decision. Recording the empty result matters: it is what stops the next
person repeating the search.

## OPEN

How far the organisational search should reach when the org has many
repositories and most are irrelevant. Currently unbounded, which will not scale.

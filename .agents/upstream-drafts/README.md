# Upstream drafts

Things written for `tursodatabase/turso` that have **not been sent**, parked here
to be chewed on first. Durable and reviewable without being public, for the
reason in [`../adr/0001`](../adr/0001-agent-config-lives-on-its-own-branch.md).

Move a draft out once it is sent, and note where it went.

## What may be sent, and by whom

Posting reaches `tursodatabase/turso` through the `gh-mcp` server, which
authenticates as the **repository owner's own GitHub account** — not a bot, not
a scoped app. Anything sent from here appears under a real person's name and
stays in a stranger's tracker. The other server, `mcp__github__*`, is scoped to
the fork and cannot post upstream at all; a refusal from it says nothing about
whether upstream is reachable, which is a mistake already made once
([#22](https://github.com/wommy/turso/issues/22)).

The owner's standing decision:

| Kind | Authority |
|---|---|
| Bug report, or a factual comment on an existing issue | Send it. No checkpoint. |
| Pull request, or anything asking a maintainer to do work | Ask first, every time. |

**Both kinds need an adversarial pass before they go**, one or two agents whose
brief is to falsify the claim rather than to check it. Two axes have both paid
off: one attacking whether the technical claim is true, one attacking how the
message reads to somebody who did not ask for it. The second is not a
formality — the risk in a first contact is rarely a wrong fact, it is unsolicited
triage of a stranger's backlog arriving as though it were welcome.

Run the pass against the *claim*, not the draft: an adversary handed finished
prose reviews the prose. Give it the evidence and let it try to break that.

| Draft | Target | Status |
|---|---|---|
| `tool-naming-convention.md` | `tursodatabase/turso` issue | **Parked.** It would be the first thing carrying the author's name into that repository, and it asks maintainers to spend time on a question. Nothing is gated on it: [#11](https://github.com/wommy/turso/issues/11) decided it is fire-and-forget, and the tool rename it would inform is already fenced as [#13](https://github.com/wommy/turso/issues/13). |

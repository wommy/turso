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

**The pass has already earned its place.** A report saying "issue #1440 asks
for something your Go driver already does" was verified at source, twice, and
every technical claim in it was true. The adversarial found that another
outside contributor had posted the same conclusion, citing the same commit,
six days earlier — and had run the reproducer, which we had not. Checking
whether we were right would have passed it. What made it a bad post was that
the question was already answered.

## What the target repository is like

Verified in the clone, not remembered:

- **Issues are never marked stale.** `.github/workflows/stale.yml` sets
  `days-before-issue-stale: -1`; only pull requests go stale, at 30 days, and
  close 7 days later. So an old open issue is not neglect, it is the default,
  and "this looks stale" carries no urgency there.
- **`CONTRIBUTING.md` ranks contribution types, and ours is not on the list.**
  It says a bug report with a solid reproducer beats a sloppy PR, and "You
  don't need to ask 'can I work on this?'". Both point at doing work. Reporting
  on somebody else's already-merged work is not a category it recognises.
- **Their pull request template requires a `## Description of AI Usage`
  section.** They have had enough agent-written contributions to need process
  for it, so anything arriving in a dense, evidence-table, commit-citing voice
  is read against that backdrop rather than in a vacuum.
- **A scoring bot (`fossier.yml`) and a CI account that files its own issues
  both run there.** The tracker already carries a lot of machine-shaped text.

The register outsiders actually use is dense, technical and
reproduction-first: specific commits, specific test names, what one engine
emits against what the other does. Length is not the risk. Saying something
already said is.

## Read the comments, not the metadata

The near miss above came from checking that an issue was still open, still
labelled, still in its milestone, and reading the `updated_at` timestamp as
reassurance that nothing had changed. That timestamp *was* the change: it was
the day somebody answered it.

Before drafting anything aimed at an existing issue, read every comment on it.

| Draft | Target | Status |
|---|---|---|
| `tool-naming-convention.md` | `tursodatabase/turso` issue | **Parked.** It would be the first thing carrying the author's name into that repository, and it asks maintainers to spend time on a question. Nothing is gated on it: [#11](https://github.com/wommy/turso/issues/11) decided it is fire-and-forget, and the tool rename it would inform is already fenced as [#13](https://github.com/wommy/turso/issues/13). |

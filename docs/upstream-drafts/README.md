# Upstream drafts

Things written for `tursodatabase/turso` that have **not been sent**, parked here
to be chewed on first.

This branch never merges anywhere, so a draft here is durable and reviewable
without being public. `.scratch/` is the other option and is the wrong one for
anything worth keeping: it is gitignored, so it dies with the container.

Each draft's header records its target and status. Move a draft out of here once
it is sent, and note where it went.

| Draft | Target | Status |
|---|---|---|
| `tool-naming-convention.md` | `tursodatabase/turso` issue | Parked — not sent |

## Why this one is parked rather than sent

It would be the first thing carrying the author's name into that repository, and
it asks maintainers to spend time on a question. Both are worth getting right
rather than fast. Nothing depends on it: the decision (wommy/turso#11) was
explicitly that the issue is fire-and-forget, and the only work gated on a reply
is the tool rename, which is already fenced as a follow-up (wommy/turso#13).

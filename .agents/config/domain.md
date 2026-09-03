# Domain docs

Where this repo's vocabulary lives, so the skills read it instead of inventing a
second copy.

## Before exploring, read these

1. **The guide for the area you are touching**, under `docs/agent-guides/`:

   | Area | Guide |
   |---|---|
   | `core/io/`, anything returning `IOResult` | `async-io-model.md` |
   | `core/storage/btree.rs`, page/cell format | `storage-format.md` |
   | `core/storage/wal.rs`, checkpointing, concurrency | `transaction-correctness.md` |
   | `core/mvcc/` | `mvcc.md` |
   | any test work | `testing.md` |
   | any code at all | `code-quality.md` |
   | commits, CI, dependencies | `pr-workflow.md` |
   | reproducing a failure | `debugging.md` |

   Most have a matching skill under `.claude/skills/` with the same name — the
   skill is the loadable form of the same material. Read one, not both.

2. **[`../CONTEXT.md`](../CONTEXT.md)**: the glossary, for terms with no home in
   a guide above.

3. **[`../adr/`](../adr/)**: decisions touching the area you are about to work in.

If one of these does not exist, proceed silently. `/domain-modeling` creates them
lazily, when a term or a decision actually gets resolved.

## One glossary, not one per crate

A 40+ crate Cargo workspace looks like a multi-context repo, and the seed
template would have us write a root `CONTEXT-MAP.md` pointing at one `CONTEXT.md`
per context. We do not, because the areas that would get one — storage, the WAL,
MVCC, the parser — **already have a guide each**. A `CONTEXT.md` beside a guide
is two places defining the same term, and one of them goes stale. That is the
exact failure `/domain-modeling` exists to prevent.

The graduation rule, for when that stops being true:

> An area earns its own `CONTEXT.md` when its guide defines a term that
> **contradicts** another area's, not merely when the area is big.

Same reasoning for `../adr/`: root-only. Per-crate ADR directories would be
splitting a directory that holds four files.

## Say it the way the guide says it

When your output names a domain concept — an issue title, a refactor proposal, a
hypothesis, a test name — use the term as the guide or `CONTEXT.md` defines it
rather than a synonym. A concept defined nowhere is a signal: either you are
inventing language the project does not use, or there is a real gap worth noting.

Turso's own house rule reinforces this and outranks it — see the "plain language
instead of complex jargon" section of `AGENTS.md`. A term that is precise but
that nobody would recognise loses to the plain one.

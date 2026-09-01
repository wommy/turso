# Domain Docs

How the engineering skills should consume this repo's domain documentation.

This repo is **single-context in structure, multi-context in what it points at**.
There is one glossary and one ADR directory at the root, but the per-area
vocabulary already exists and lives elsewhere — so the read list points at what
is already written rather than at new files duplicating it.

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

   Most have a matching skill under `.claude/skills/` with the same name; the
   skill is the loadable form of the same material. Read one, not both.

2. **`CONTEXT.md`** at the repo root: the glossary, for terms with no home in a
   guide above.

3. **`.agents/adr/`**: decisions that touch the area you are about to work in.

If any of these don't exist, **proceed silently**. Don't flag their absence and
don't suggest creating them upfront. `/domain-modeling` creates them lazily, when
a term or a decision actually gets resolved.

## Why not per-context CONTEXT.md files

A 40+ crate Cargo workspace looks like a multi-context repo, and the seed
template would have us write a root `CONTEXT-MAP.md` pointing at one `CONTEXT.md`
per context. We don't, because the areas that would get one — storage, the WAL,
MVCC, the parser — **already have a guide each**. Adding a `CONTEXT.md` beside a
guide creates two places where the same term is defined and one of them goes
stale. Two sources of truth for one word is the exact failure `/domain-modeling`
exists to prevent.

The graduation rule, for when that stops being true:

> An area earns its own `CONTEXT.md` when its guide defines a term that
> **contradicts** another area's, not merely when the area is big.

Same reasoning for ADRs: `.agents/adr/` is root-only. Per-crate ADR directories
would be splitting a directory that currently holds nothing.

## Use the glossary's vocabulary

When your output names a domain concept — an issue title, a refactor proposal, a
hypothesis, a test name — use the term as the guide or `CONTEXT.md` defines it.
Don't drift to synonyms.

If the concept isn't defined anywhere yet, that's a signal: either you're
inventing language the project doesn't use (reconsider), or there's a real gap
(note it for `/domain-modeling`).

Turso's own house rule reinforces this and outranks it — see the "plain language
instead of complex jargon" section of `AGENTS.md`. A term that is precise but
that nobody would recognise loses to the plain one.

## Flag ADR conflicts

If your output contradicts an existing ADR, surface it rather than silently
overriding:

> _Contradicts ADR-0007 (…), but worth reopening because…_

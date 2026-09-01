# You are in `/home/user/turso-agent-config`

**Branch:** `claude/agent-config`

PR #2 — agent configuration. Deliberately NEVER merged upstream. Source of truth for CONTEXT.md, docs/adr/ and docs/agents/, which are symlinked into every other worktree from here.

Adding a NEW file under docs/adr/ here needs `git add -f`: .git/info/exclude lists /docs/adr so the symlinks stay invisible elsewhere, and that pattern shadows the real directory here.

## This repo has six worktrees, and they look alike

Reading the wrong one has already cost this project two wasted agent runs. Check
the path you were given against this table before you read anything else.

| Worktree | Branch |
|---|---|
| `/home/user/turso` | `claude/mcp-v2-port-4wfufm` |
| `/home/user/turso-mcp-v2` | `claude/mcp-v2-protocol` |
| `/home/user/turso-http-framing` | `claude/http-framing-fixes` |
| `/home/user/turso-mvcc-docs` | `claude/mvcc-docs` |
| `/home/user/turso-agent-config` | `claude/agent-config` |
| `/home/user/turso-http-mcp` | `claude/mcp-http-transport` |

They share one `.git`, so `git log`, `git show` and `git diff` see every branch
from any of them — but the **files on disk** are only this branch's.

## Building

```
export CARGO_TARGET_DIR=/home/user/turso/target
cargo test -p turso_cli
cargo clippy -p turso_cli --all-targets -- --deny=warnings --allow unfulfilled-lint-expectations
```

Never `--release`. One cargo command at a time — the target dir is shared and
locks. **Do not run `cargo run`**: it rebuilds the whole workspace at default
features and has exhausted this container's disk twice. Use
`cargo build -p turso_cli` and run the artifact directly.

**Never paste CI's `cargo clippy --workspace --all-features --all-targets` here.**
It is correct for CI, which gives it its own cache key, and wrong against this
shared target dir: workspace scope builds a differently-featured copy of
`turso_core` (~410 MB) that no other command can reuse, and drags in the nine
crates `default-members` excludes — the whole Postgres stack and the memory
harness. `-p turso_cli` also skips `sdk-kit`, whose `cdylib` and `staticlib`
outputs are 2.2 GB on their own. If you genuinely need a workspace-wide lint,
point `CARGO_TARGET_DIR` somewhere else for that one run.

`--allow unfulfilled-lint-expectations` covers a pre-existing failure in
`core/json/cache.rs:107` from a toolchain mismatch. No branch here touches
`core/`, so it is never ours.

## Decisions you must not re-open

`.agents-ref/` is a symlink to the agent-config worktree's `.agents/`:
`.agents-ref/adr/`, `.agents-ref/CONTEXT.md`, `.agents-ref/config/`.

The name differs from the real directory on purpose. `.git/info/exclude` is
shared across every worktree, so excluding `/.agents` would also hide the
**tracked** `.agents/` on the config branch - a new ADR written there returned
nothing from `git status` at all, which loses work silently rather than loudly.
Excluding a name that exists only as a symlink avoids that.
Read them before proposing an architecture change — ADR 0002 in particular rules
out the HTTP and async libraries an explorer will otherwise rediscover and
suggest.

*Untracked and git-ignored. Generated for agent navigation; not part of the repo.*

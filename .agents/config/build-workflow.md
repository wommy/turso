# Building and testing this repo in this container


A single `target/` reaches 9 GB. Three worktrees exhausted the container's disk
entirely. These are the measures that actually moved the number, with the ones
that did not, so they are not tried again.

## Two flags that are not guessable

    cargo build -p turso_cli        # then run target/debug/tursodb directly
    cargo clippy ... --allow unfulfilled-lint-expectations

**Never `cargo run`.** It rebuilds the whole workspace at default features and
has exhausted this container's disk twice. Build with `-p turso_cli` and run the
artifact.

**`--allow unfulfilled-lint-expectations`** covers a pre-existing failure in
`core/json/cache.rs:107` — an `#[expect(clippy::new_without_default)]` this
toolchain does not fire. No branch here touches `core/`, so it is never ours.
CI runs a different toolchain and does not hit it, so this is a local flag only.

## Run the CLI tests serially

    cargo test -p turso_cli -- --test-threads=1

`cli/tests/mcp_http_transport.rs` spawns real `tursodb` processes and binds
real ports. Under cargo's default parallelism a different test in that file
fails at random with `ConnectionRefused` or `ConnectionReset`; serially all
five pass. Two agents hit this independently and one nearly reported it as a
regression in the code it was changing.

Nobody has reproduced the race deliberately yet, so the cause is still open
as [#41](https://github.com/wommy/turso/issues/41). Until that closes, the
flag is how you tell a real failure from this one.

## The inner loop

**Do not mix feature sets.** Cargo fingerprints by feature set, so alternating
`cargo test -p turso_cli` (default features) with `cargo clippy --workspace
--all-features` (28 features) keeps **two or more complete copies** of the whole
dependency graph. Measured in a live target dir: four distinct feature
combinations of `turso_core` side by side, two `.rlib` files at 421 MB and
417 MB, and 1.7 GB of incremental cache for that one crate. This is the largest
single waste and it costs nothing to stop.

Pick one feature set for iteration. Run the full sweep once, before pushing.

**Drop `--workspace` while iterating.** The root `Cargo.toml` curates a
`default-members` list that deliberately excludes `postgres/*`, `perf/memory*`
and `perf/query-batch`; `--workspace` overrides it and pulls all nine back in.
Worse, `sdk-kit` and `sync/sdk-kit` declare `crate-type = ["lib", "cdylib",
"staticlib"]`, so linting them produces artifacts nobody asked for — 853 MB,
783 MB, 306 MB and 284 MB of `.a` and `.so`, over 2.2 GB from two crates.

    # iterating
    cargo clippy --all-features --all-targets -- --deny=warnings
    # before pushing - matches CI exactly
    cargo clippy --workspace --all-features --all-targets --exclude memory-benchmark -- --deny=warnings

`cargo test -p turso_cli` needs no such care: `-p` builds only that package's
subgraph and ignores `default-members` entirely.

## Linking: mold

`CONTRIBUTING.md` recommends it and it is installed. **Use `mold -run cargo …`,
not the `.cargo/config.toml` snippet the docs give.** That file is tracked, and
adding `[target.x86_64-unknown-linux-gnu].rustflags` *overrides* rather than
merges with the repo's existing `cfg(all(target_os = "linux", target_env =
"gnu"))` rustflags — silently dropping `--cfg=tokio_unstable` and the link-args
that exist so tests run on Linux at all.

## Debug info is most of the disk

`[profile.release]` sets `debug = "line-tables-only"`. **There is no
`[profile.dev]` section**, so dev and test builds carry full DWARF. Measured on
the 347 MB `tursodb` debug binary: `.debug_info` 132.7 MB, `.debug_str` 95.4 MB,
`.debug_ranges` 18.6 MB — about **82% of the binary is debug sections**, and
`deps/` is full of comparable artifacts.

`debug = "line-tables-only"` keeps `.debug_line`, so file:line survives in
backtraces — which is all this repo's debugging actually uses (bytecode
comparison, `RUST_LOG`, deterministic simulation, ThreadSanitizer under a
separate nightly invocation; no gdb or lldb workflow anywhere). CI sets
`RUST_BACKTRACE: 1` deliberately, and line tables keep that useful. `debug = 0`
would not, so do not go further.

Two ways to take it, and one caveat:

- `CARGO_PROFILE_DEV_DEBUG=line-tables-only` per invocation, changing no files.
- `[profile.dev] debug = "line-tables-only"` in the root `Cargo.toml` — **edits a
  tracked file**, and is arguably worth proposing upstream since it only mirrors
  what the release profile already chose.

**Caveat:** changing the debug level changes the fingerprint, so the first build
after switching rebuilds everything. Worth it before a clean build, not in the
middle of a working session.

## Reclaiming space

- **`rm -rf target/debug/incremental`** — reclaimed **5.4 GB** here, immediately.
  Safe: it holds only the per-function query cache, and every `.rlib`, `.rmeta`,
  `.so` and `.a` in `deps/` and `build/` stays valid. The crates you were
  actively editing recompile once at normal speed. This is the first thing to
  reach for.
- **`cargo sweep --time 14`** prunes artifacts untouched for N days. Cargo never
  garbage-collects, so stale hashes from abandoned branches accumulate forever —
  and with worktrees sharing a target dir, there are many. Less drastic than
  `cargo clean`.

## Rejected, with reasons

- **sccache** — trades disk for time. Disk is the binding constraint, and its
  default cache is 10 GB.
- **ccache** — caches only C/C++. Here that is `rusqlite`'s bundled SQLite: one
  file in an overwhelmingly Rust build, for a 5 GB default cache.
- **`CARGO_INCREMENTAL=0` locally** — CI sets it, correctly, because a fresh
  checkout has nothing to reuse. Locally the opposite is true: incremental is
  built for repeated edits to one crate, and its cache is reclaimable on demand.
  Do not copy CI's env into a shell profile.
- **`split-debuginfo = "unpacked"`** — affects link time rather than total disk,
  and most of its benefit disappears once debug info is trimmed.

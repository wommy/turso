---
name: mvcc
description: MVCC feature - snapshot isolation, versioning, limitations
---
# MVCC Guide (Experimental)

Multi-Version Concurrency Control. **Work in progress, not production-ready.**

**CRITICAL**: Ignore MVCC when debugging unless the bug is MVCC-specific.

## Enabling MVCC

```sql
PRAGMA journal_mode = 'mvcc';
```

Runtime configuration, not a compile-time feature flag. Per-database setting.

## How It Works

Standard WAL: single version per page, readers see snapshot at read mark time.

MVCC: multiple row versions, snapshot isolation. Each transaction sees consistent snapshot at begin time.

### Key Differences from WAL

| Aspect | WAL | MVCC |
|--------|-----|------|
| Write granularity | Every commit writes full pages | Affected rows only
| Readers/Writers | Don't block each other | Don't block each other |
| Persistence | `.db-wal` | `.db-log` (logical log) |
| Isolation | Snapshot (page-level) | Snapshot (row-level) |

### Versioning

Each row version tracks:
- `begin` - timestamp when visible
- `end` - timestamp when deleted/replaced
- `btree_resident` - existed before MVCC enabled

## Architecture

```
Database
  └─ mv_store: MvStore
      ├─ rows: SkipMap<RowID, Vec<RowVersion>>
      ├─ txs: SkipMap<TxID, Transaction>
      ├─ Storage (.db-log file)
      └─ CheckpointStateMachine
```

**Per-connection**: `mv_tx` tracks current MVCC transaction.

**Shared**: `MvStore` with lock-free `crossbeam_skiplist` structures.

## Key Files

- `core/mvcc/mod.rs` - Module overview
- `core/mvcc/database/mod.rs` - Main implementation (~3000 lines)
- `core/mvcc/cursor.rs` - Merged MVCC + B-tree cursor
- `core/mvcc/persistent_storage/logical_log.rs` - Disk format
- `core/mvcc/database/checkpoint_state_machine.rs` - Checkpoint logic

## Checkpointing

Flushes row versions to B-tree periodically.

```sql
PRAGMA mvcc_checkpoint_threshold = <pages>;
```

Process: acquire lock → begin pager txn → write rows → commit → truncate log → fsync → release.

## Current State and Limitations

**Implemented, worth knowing the shape of:**
- Garbage collection. Runs inline on the commit path (`MvStore::should_gc` /
  `gc_incremental`, `core/mvcc/database/mod.rs:7277` and `:7317`) once live
  versions have grown past `mvcc_gc_threshold` (default 16K, pragma at
  `core/pragma.rs:185`) since the last pass. Each pass is capped and resumable
  (`MAX_CHAINS_PER_GC` chains, `core/mvcc/database/mod.rs:7186`), so it never
  stalls the committing connection. It reclaims: aborted versions (Rule 1),
  superseded versions once no active reader can see them and the delete is
  checkpointed (Rule 2), and — only once the B-tree already has the row — the
  last remaining current version (Rule 3, see `gc_version_chain`,
  `core/mvcc/database/mod.rs:7736`). It does not reclaim versions still needed
  by an open transaction, and it does not shrink memory for rows that are
  still live.
- Recovery from the logical log on restart. `maybe_recover_logical_log`
  (`core/mvcc/database/mod.rs:8718`) is a real step in the MVCC bootstrap
  state machine (`BootstrapState::Recover`, `core/mvcc/database/mod.rs:4841`),
  not just a function that exists — every MVCC database open replays
  operations committed after the last checkpoint before serving queries.

**Known issues:**
- Checkpoint blocks other transactions, even reads, by default. MVCC only
  supports Passive and Truncate checkpoint modes (Full/Restart map to
  Truncate); Truncate takes a blocking lock for the whole checkpoint
  (`core/mvcc/database/checkpoint_state_machine.rs:743`). Passive avoids the
  block but only runs behind the experimental
  `--experimental-mvcc-passive-checkpoint` CLI flag (`cli/app.rs:117`), which
  is off by default.
- No spilling to disk. The version store (`rows` / `index_rows`) is plain
  in-memory `SkipMap`s with no disk-backed eviction path — nothing in
  `core/mvcc/` writes a version out to reclaim RAM. GC (above) keeps *old*
  versions from accumulating forever, but it cannot shrink memory used by
  rows that are still live or still pinned by an open transaction, so memory
  use still scales with live working-set size, not just history.

## Testing

```bash
# Run MVCC-specific tests
cargo test mvcc

# TCL tests with MVCC
make test-mvcc
```

Use `#[turso_macros::test(mvcc)]` attribute for MVCC-enabled tests.

```rust
#[turso_macros::test(mvcc)]
fn test_something() {
    // runs with MVCC enabled
}
```

## References

- `core/mvcc/mod.rs` documents data anomalies (dirty reads, lost updates, etc.)
- Snapshot isolation vs serializability: MVCC provides the former, not the latter

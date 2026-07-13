# Agent C Report — Task 6: Stable journal lock

## Files changed

- `crates/phronesis-mcp/src/journey/journal.rs` — complete rewrite of the journal's locking and compaction model
- `crates/phronesis-mcp/src/journey/mod.rs` — verified unchanged (no public API surface moves; the new `acquire_lock`, `JournalLock`, `lock_path`, `write_temp_synced`, `over_cap`, `maybe_compact_locked` are all private). All call sites verified unchanged.

## Tests added/replaced

### Swap record

| Old test | Disposition | Why |
|---|---|---|
| `fd_is_current_detects_rename_swap` | Deleted, replaced by `lock_inode_is_stable_across_compaction` | Tested the removed revalidation helper. The replacement asserts the stronger invariant that makes revalidation unnecessary: the lock inode never changes across compaction. |
| `append_lands_in_live_file_after_external_rename` | Kept; `#[cfg(unix)]` removed, comment updated | Still a valid black-box guarantee; nothing unix-specific remains in the mechanism. |

### New tests mapped to the spec's required proofs

- interleave → `concurrent_appenders_do_not_interleave_json`
- racing appends survive → `appends_racing_compaction_are_never_lost`
- repeated loop loses nothing → `repeated_compact_append_loop_loses_no_record`
- reader coherence → `reader_sees_valid_json_during_replacement`
- malformed lines → existing `malformed_lines_are_dropped_at_compaction` (kept)
- outcome survival → existing `over_cap_keeps_tail_and_latest_outcome_per_prefix_subject` + `confidence_read_still_works_after_compaction` (kept)
- lock/temp error policy → `append_errors_when_lock_path_is_a_directory`, `compaction_temp_error_propagates_and_journal_is_untouched`, `successful_compaction_leaves_no_temp_file`

## Evidence

### `cargo test -p phronesis-mcp journal`
```
running 22 tests
test journey::journal::tests::command_exit_none_is_omitted_from_serialization ... ok
test journey::journal::tests::command_exit_round_trips ... ok
test journey::journal::tests::v1_line_without_command_exit_still_parses ... ok
test journey::journal::tests::under_cap_is_untouched ... ok
test journey::journal::tests::missing_file_is_not_an_error ... ok
test journey::journal::tests::over_cap_keeps_tail_and_latest_outcome_per_prefix_subject ... ok
test journey::journal::tests::confidence_read_still_works_after_compaction ... ok
test journey::journal::tests::append_still_succeeds_after_compaction ... ok
test journey::journal::tests::malformed_lines_are_dropped_at_compaction ... ok
test journey::journal::tests::preserved_prefix_outcome_is_readable_beyond_positional_window ... ok
test journey::journal::tests::append_lands_in_live_file_after_external_rename ... ok
test journey::journal::tests::at_cap_with_few_records_is_not_rewritten ... ok
test journey::journal::tests::lock_inode_is_stable_across_compaction ... ok
test journey::journal::tests::concurrent_appenders_do_not_interleave_json ... ok
test journey::journal::tests::appends_racing_compaction_are_never_lost ... ok
test journey::journal::tests::repeated_compact_append_loop_loses_no_record ... ok
test journey::journal::tests::reader_sees_valid_json_during_replacement ... ok
test journey::journal::tests::append_errors_when_lock_path_is_a_directory ... ok
test journey::journal::tests::compaction_temp_error_propagates_and_journal_is_untouched ... ok
test journey::journal::tests::successful_compaction_leaves_no_temp_file ... ok
test outcomes::derive::tests::signals_reads_journal_via_subject_filter ... ok
test outcomes::tests::report_reflects_journal_signals ... ok
test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured

running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured
```

### `cargo fmt --all`
No output — no formatting changes needed.

### `cargo clippy --workspace -- -D warnings`
```
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 8.66s
```
Zero warnings.

### `cargo build --workspace`
```
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 8.81s
```

### `cargo test --workspace`
```
   Doc-tests phronesis
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured
```

## Durability notes for the integrator (Task 7 docs)

- Mutators (append + compaction) serialize on an exclusive advisory lock on `.phronesis/journey/events.lock`; the lock inode is never replaced, so no fd revalidation is needed or performed. The lock auto-releases on fd close (including abnormal exit).
- Non-unix: `fs2` maps to `LockFileEx` on Windows with the same exclusive advisory semantics; the lock file is never read or written, so Windows lock strictness cannot affect journal I/O. Advisory locks remain unreliable on NFS; `.phronesis/` is documented as local-only.
- Compaction is atomic: same-directory temp file, `sync_all()` before rename, temp removed best-effort on failure. Parent-directory fsync is deliberately omitted — the journal is best-effort telemetry; the post-power-loss worst case is the valid pre-compaction file, healed by the next compaction. Plain appends are not fsynced (a crash may lose the final line; readers skip torn trailing lines).
- Readers are lock-free by design; the atomic rename guarantees a coherent old-or-new snapshot.
- Error policy: lock-file failures surface as `JournalError::Io` naming `events.lock` and are propagated by `append` (the hook call site is fail-open); compaction temp/rename failures surface as `JournalError::Io` naming the temp path and never touch the live journal; `append`'s internal compaction is fail-open (stderr note, append proceeds).

STATUS: DONE

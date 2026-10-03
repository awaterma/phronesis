# Task 6: Governance Telemetry Implementation

**Scope:** Implement `bench/phr-bench/src/governance.rs` per plan Task 6.

## Summary

Implemented governance telemetry parsing for `.phronesis/log.jsonl` with strict NotWired detection. The module distinguishes between wired (has hook entries) and not-wired (no hook entries) treatment runs.

## Implementation Details

### `governance::summarize(log_jsonl: &str) -> Result<GovernanceSummary, GovernanceError>`

Parses a `.phronesis/log.jsonl` string and produces a `GovernanceSummary` containing:
- `blocks: BTreeMap<String, u32>` — per-rule count of `pre_check` exit=2 blocks
- `warns: BTreeMap<String, u32>` — per-rule count of `post_check` exit=1 warnings
- `fail_closed: u32` — count of `fail_closed` blocks (rule load errors, etc.)

### `GovernanceError` enum

- **`NotWired`** — no hook entries found in the log (empty, only lifecycle, or missing). This critical case surfaces at the runner layer as `RunExit::Error { reason: "governance_not_wired" }`, preventing silent control-equivalent treatment runs.
- **`Malformed(String)`** — JSON parse error or unexpected structure.

### Logic

1. Per-line JSON parsing with error recovery
2. Filter entries by `kind == "hook"` only (ignores lifecycle)
3. `pre_check` with `exit == 2`: count each `blocked_by[]` entry by kind (rule → blocks, fail_closed → fail_closed counter)
4. `post_check` with `exit == 1`: count each `consequences[].rule_id` toward warns
5. Return `NotWired` if zero hook entries found, else return populated summary

## Test Coverage

**3 new tests, all passing:**
- `counts_blocks_warns_and_fail_closed` — validates fixture parsing and per-rule counters
- `empty_log_is_not_wired` — confirms empty/whitespace-only logs trigger NotWired
- `lifecycle_only_log_is_not_wired` — confirms logs with only lifecycle entries are invalid

**Existing tests:** 12 (contracts, prompt, transcript)
**Total: 15 tests passing, 0 failures**

## Quality Checklist

✅ `cargo test` green (15 tests)
✅ `cargo clippy --all-targets -- -D warnings` clean (0 warnings)
✅ No `.unwrap()` in production paths (anyhow propagation via `?`)
✅ Fixture + failing tests committed first (RED), then implementation (GREEN)
✅ Conventional commits: `test(bench):` and `feat(bench):` 
✅ Only bench/ modified (no crates/ touched)

## Design Notes

The `GovernanceError::NotWired` variant is essential for the runtime contract: a treatment run with no governance telemetry must exit with an explicit error, not silently become a data-integrity hole. Task 8's runner layer (not this task) converts NotWired to `RunExit::Error { reason: "governance_not_wired" }` at the task layer, fulfilling the rv3 review finding that NotWired must surface loudly.

## Commits

1. `test(bench): failing governance-telemetry tests` — fixture + test stubs
2. `feat(bench): governance telemetry from .phronesis/log.jsonl with NotWired detection` — implementation

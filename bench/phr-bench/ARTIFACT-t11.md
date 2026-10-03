# Task 11: Aggregate — Paired Rollup and Sign Test

## Implementation Summary

Implemented `bench/phr-bench/src/aggregate.rs` with paired A/B experiment aggregation and exact binomial sign test computation.

### Key Components

**Core Structures:**
- `TaskPair`: Represents a paired control/treatment record for a single instance with discordance tracking
- `Headline`: Aggregated statistics across all pairs (resolved rates, discordant counts, sign-test p-value)
- `LangTotals`: Per-language breakdown of resolved counts and mean turns
- `Aggregate`: Complete result structure with pairs, language breakdowns, and headline stats

**Functions:**
- `sign_test_p(wins: u64, losses: u64) -> f64`: Exact two-sided binomial p-value using iterative coefficient computation and log-free arithmetic. Formula: `p = min(1.0, 2.0 * P(X ≤ min(wins, losses)))` over `Binomial(n=wins+losses, p=0.5)`
- `aggregate(records: &[RunRecord]) -> Result<Aggregate>`: Groups records by instance_id, validates exactly one control + one treatment per instance, validates all records, computes discordance and treatment-won flags, computes headline stats and per-language totals

### Validation & Error Handling

- **Loud validation on treatment records**: Every treatment record must have a `governance` field (enforced by `record::validate`). Violated records cause `aggregate()` to fail with a clear error message naming the instance.
- **Paired consistency**: Each instance must have exactly 2 records (one control, one treatment). Mismatch causes an error.
- **Discordance detection**: Resolved flags differing between arms trigger the discordant flag and contribute to the sign test.

### Test Coverage

Four comprehensive tests in `tests/aggregate.rs`:

1. **`sign_test_known_values`**: Validates exact binomial p-value computation against known values:
   - `(9, 1)` → `0.021484375` (2 * 11/1024)
   - `(6, 0)` → `0.03125` (2 * 1/64)
   - `(2, 2)` → `1.0` (clamped; concordant pairs have no signal)
   - `(0, 0)` → `1.0` (no discordant pairs)

2. **`pairs_and_headline_roll_up`**: Validates aggregation of 3 tasks: control-won (true/false), treatment-won (false/true), and concordant (false/false), producing correct pair counts, discordance flags, headline totals.

3. **`treatment_missing_governance_fails_loud`**: Validates that missing `governance` on a treatment record causes an error message containing "governance".

4. **`odd_arm_counts_are_rejected`**: Validates that unpaired records (e.g., only control for an instance) cause an error.

### Code Quality

- **No unwrap()**: All error paths use `anyhow` error propagation with `?` operator
- **Clippy clean**: Passes `cargo clippy --all-targets -- -D warnings`
- **Test count**: 22 total tests (18 existing + 4 new), all passing
- **Deterministic**: Sign test and aggregation produce identical output for identical input

## Commits

- **`feat(bench): paired aggregate rollup with exact binomial sign test`** (acfb010)
  - Implements `aggregate.rs` with full validation and sign test
  - Adds comprehensive test suite in `tests/aggregate.rs`
  - 22 tests passing, clippy clean

## Status

✅ All acceptance criteria met:
- `cargo test --manifest-path bench/phr-bench/Cargo.toml` → 22 passing (18 existing + 4 new)
- `cargo clippy --manifest-path bench/phr-bench/Cargo.toml --all-targets -- -D warnings` → clean
- No `.unwrap()` in production code
- Conventional commit with descriptive message
- Work confined to `bench/phr-bench/`
- Treatment missing-governance error is LOUD (Review Focus #4)

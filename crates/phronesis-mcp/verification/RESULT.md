# Verus Verification Result

## Environment

- **Verus version:** 0.2026.09.20.aef82ed (release profile, macos_aarch64)
- **Rust toolchain:** 1.98.1-aarch64-apple-darwin
- **Date:** 2026-09-24

Re-run after adding invariant 5 (bare-name graph query matching):

- **Verus version:** 0.2026.10.15.867383b (release profile, linux_x86_64,
  built from source at that commit)
- **Rust toolchain:** 1.98.1-x86_64-unknown-linux-gnu
- **Z3:** 4.16.0 (the `z3-solver` PyPI wheel)
- **Date:** 2026-10-15

Re-run after adding invariant 6 (graded toolchain outcome):

- **Verus version:** 0.2026.09.20.aef82ed (release profile, macos_aarch64)
- **Rust toolchain:** 1.98.1-aarch64-apple-darwin
- **Date:** 2026-10-19

## Command

```bash
verus crates/phronesis-mcp/verification/harness.rs
```

Or via the runner:

```bash
bash crates/phronesis-mcp/verification/run-verification.sh
```

## Verifier Output

```
verification results:: 35 verified, 0 errors
```

All 35 verification results discharged by Z3 with zero errors (10 before
invariant 5 was added, 15 before invariant 6).

## What Was Proven

### 1. Identifier charset invariant (`valid_identifier_char`)

- **Spec:** `is_valid_identifier_byte(b: u8) -> bool` — characterises the exact
  byte set accepted by `validate_identifier_field` in `coverage/store.rs`:
  `A-Z`, `a-z`, `0-9`, `_`, `:`, `.`, `/`, `-`.
- **Exec:** `valid_identifier_char(b: u8) -> bool` — an if-chain mirroring the
  spec.
- **Proven:** `ensures result == is_valid_identifier_byte(b)` — the exec
  function agrees with the spec on every input byte.

### 2. Identifier slice check (`valid_identifier_slice`)

- **Exec:** `valid_identifier_slice(data: &[u8]) -> bool` — iterates over
  bytes, returns `true` iff every byte is a valid identifier byte.
- **Proven:** `ensures result == forall|i: int| 0 <= i < data.len() ==> is_valid_identifier_byte(data[i])`
  — the slice checker correctly implements the universal quantification.

### 3. FNV-1a hash determinism (`fnv1a_64` + `fnv1a_64_determinism`)

- **Spec:** `spec_fnv1a_64(data: Seq<u8>) -> u64` — a pure mathematical
  recursive definition of FNV-1a 64-bit, mirroring `fnv1a_64` in
  `coverage/region_map.rs`.
- **Exec:** `fnv1a_64(data: &[u8]) -> u64` — iterative implementation
  matching the production code.
- **Proven (correctness):** `ensures hash == spec_fnv1a_64(data@)` — the
  exec function computes exactly the spec function for all inputs.
- **Proven (determinism):** `fnv1a_64_determinism` proof function:
  `requires data1@ == data2@` ensures `spec_fnv1a_64(data1@) == spec_fnv1a_64(data2@)`.
  Same input bytes always produce the same hash — the anchor is deterministic.

### 4. Line ordering invariant (`valid_line_range`)

- **Spec:** `spec_valid_line_range(start, end) -> bool` — `start <= end`.
- **Exec:** `valid_line_range(start_line: u64, end_line: u64) -> bool` —
  mirrors the `start_line <= end_line` check in `validate_record`.
- **Proven:** `ensures result == spec_valid_line_range(start_line, end_line)`.
- **Proof:** `valid_line_range_correct` confirms the spec agrees with the
  relational definition.

### 5. Bare-name graph query matching (`boundary_suffix_match`)

`graph query` and `graph ownership` let a literal argument match a fully
qualified node ID by suffix, but only when the suffix starts right after a
`:`. This mirrors the suffix arm of `arg_matches` in `src/graph/query.rs`:
`got.strip_suffix(want).is_some_and(|prefix| want.starts_with(':') || prefix.ends_with(':'))`.

- **Spec:** `spec_boundary_suffix_match(want, got)` — `got` ends with `want`,
  and either `want` begins with `:` or the byte before the suffix is `:`.
- **Exec:** `boundary_suffix_match(want: &[u8], got: &[u8]) -> bool`.
- **Proven (correctness):** `ensures result == spec_boundary_suffix_match(want@, got@)`.
- **Proven (`boundary_match_never_splits_a_segment`):** a name not beginning
  with `:` only matches when the byte before it is `:` — `consequences`
  can never match `fire_all_consequences`.
- **Proven (`boundary_match_iff_colon_prefix`):** for such a name, a match
  holds **iff** `got == p + want` for some non-empty `p` ending in `:` —
  every ID of that shape is found, and nothing else.
- **Proven (`qualifying_a_name_only_narrows`):** if `q + want` matches with
  `q` ending in `:`, `want` matches too — `Net::fire_all_consequences`
  never finds anything `fire_all_consequences` misses.
- **Non-vacuity check:** weakening the exec boundary test to `start > 0`
  makes Verus reject it (`postcondition not satisfied`).
- **Bridge to production:** the harness verifies a reimplementation, so
  `literal_matching_agrees_with_the_verified_spec_exhaustively` in
  `src/graph/query.rs` checks production `arg_matches` against the same
  rule for every pair of strings of up to five bytes over `{a, b, :}`.

### 6. Graded toolchain outcome (`outcome` + 14 proofs)

- **Spec:** `spec_outcome` models `CompiledDef::parse` in
  `outcomes/toolchain.rs` for a non-proof def: the count source (first
  matching `test_summary`, else per-test lines when the def sets
  `test_counts: "per_test"`, admitted only when a line failed or the run
  exited 0), the graded build table, and the test tag, where a nonzero exit
  vetoes a pass from any count source and becomes a run-level test failure
  (`test_run_failed`) when no failure was counted. Tests count as started
  from counts, per-test lines, or a `test_started` pattern (cargo's
  `Running`, Go's `=== RUN`); plain `compile_success` text rescues only a
  run with no exit code. Compile-failure text beside started tests in a
  failed run stays a build failure and is also a failed test run, so a
  compile failure is never a test pass.
  `spec_ingest_refused` models the `NoOutcome` refusal in
  `outcomes/mod.rs`. Regex matching is abstracted to its results.
- **Exec:** `outcome` (with `count_pass`) mirrors the production code.
  Proven: `ensures result == spec_outcome(...)` on every input.
- **Proven properties:**
  - `opting_out_is_the_old_behaviour`: without the opt-in, counts are
    exactly the summary's.
  - `a_summary_wins`: a matching summary always beats per-test lines.
  - `a_quiet_run_has_no_test_verdict`: no summary and no per-test line
    means no test verdict, whatever the exit code.
  - `a_compile_failure_has_no_test_verdict`: a compile failure is a build
    failure and never a test pass.
  - `a_test_pass_is_grounded`: a test pass needs a passing build, at least
    one pass, and zero failures.
  - `a_failing_line_rules_out_a_test_pass`: one failing per-test line rules
    out a test pass.
  - `a_nonzero_exit_is_never_a_per_test_pass`: with per-test counts, a
    nonzero exit never yields a test pass, even when every reported line
    passed (a test that called `os.Exit`, or a timeout).
  - `no_exit_is_never_a_per_test_pass`: with no exit code, per-test lines
    can ground a failure, never a pass (a truncated capture).
  - `a_nonzero_exit_is_never_a_test_pass`: for every count source,
    summary included, a nonzero exit never yields a test pass.
  - `a_failed_run_where_tests_ran_is_a_test_failure`: such a run is a test
    failure, never silence, so it withdraws an earlier pass.
  - `counted_failures_are_always_a_test_failure`: a summary counting a
    failure is a test failure under any compile classification or exit.
  - `a_compile_failure_where_tests_started_withdraws`: compile-failure text
    in a failed run where tests started is still a test failure.
  - `no_exit_and_no_evidence_is_unknown`: absent evidence is `unknown`,
    never a pass.
  - `ingest_refuses_exactly_the_ungrounded`: `signal ingest` refuses only an
    unknown build.
- **Mutation check:** letting the exec mirror pass a run with zero tests, or
  letting the spec keep counts through a compile failure, or admitting
  per-test counts without the exit gate, dropping the nonzero-exit veto, or
  ignoring `test_started` evidence, or dropping the counted-failure or
  compile-failure withdrawal branches, each fails verification. The model
  abstracts each regex to a boolean, so it cannot show that a pattern
  recognizes the right output; the unit tests (including real cargo and Go
  shapes) and the real-Go acceptance script carry that.

## What Was Not Proven

- The production code in `coverage/store.rs`, `coverage/import.rs`, and
  `coverage/region_map.rs` itself is not directly verified — the harness
  contains verified *reimplementations* of the invariants, not verus
  annotations on the production source. Bridging the two (e.g. via
  `#[verifier(external_body)]` wrappers or extracting the functions) is
  future work.
- No data-structure invariants on `HitRecord` fields (e.g. revision hex
  length, hit_kind membership) are proven yet — those are candidates for
  the next harness extension.
- The glob arm of `arg_matches` (`glob_matches`, a backtracking `*`/`?`
  matcher) is not modelled; invariant 5 covers only the bare-name suffix arm.
- The `main` smoke calls for invariant 5 run but do not assert: byte-string
  literals are opaque to the verifier in this harness.
- Invariant 6 models `CompiledDef::parse`; it is a mirror, not the
  production function. The Rust unit tests in `outcomes/toolchain.rs` pin
  the same cases against the real code. Proof defs (`outcome_kind:
  "proof"`) and the regexes themselves are not modelled.
- The `compute_anchor` truncation (`format!("{hash:016x}")[..12]`) is not
  formally verified; the determinism of the underlying `fnv1a_64` is
  proven, but the hex-formatting and slicing step is not.

## Verification Count Breakdown

| Function | Type | VC Count |
|----------|------|----------|
| `valid_identifier_char` | exec | 1 |
| `valid_identifier_slice` | exec | 2 (loop invariant + postcondition) |
| `fnv1a_64` | exec | 2 (loop invariant + postcondition) |
| `fnv1a_64_determinism` | proof | 1 |
| `valid_line_range` | exec | 1 |
| `valid_line_range_correct` | proof | 1 |
| `main` | exec | 2 (assertions) |
| `boundary_suffix_match` + 3 proofs (invariant 5) | exec + proof | 5 (rise in the reported total; not broken down per function) |
| `outcome` + `count_pass` + 14 proofs (invariant 6) | exec + proof | 20 (rise in the reported total; not broken down per function) |
| **Total** | | **35** |
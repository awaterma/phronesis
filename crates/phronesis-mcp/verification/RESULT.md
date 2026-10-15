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
verification results:: 15 verified, 0 errors
```

All 15 verification results discharged by Z3 with zero errors (10 before
invariant 5 was added).

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
| **Total** | | **15** |
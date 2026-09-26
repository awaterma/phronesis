# Verus Verification Result

## Environment

- **Verus version:** 0.2026.09.20.aef82ed (release profile, macos_aarch64)
- **Rust toolchain:** 1.98.1-aarch64-apple-darwin
- **Date:** 2026-09-24

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
verification results:: 10 verified, 0 errors
```

All 10 verification conditions discharged by Z3 with zero errors.

## What Was Proven

### 1. Identifier charset invariant (`valid_identifier_char`)

- **Spec:** `is_valid_identifier_byte(b: u8) -> bool` — characterises the exact
  byte set accepted by `validate_identifier_field` in `coverage/import.rs`:
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
| **Total** | | **10** |
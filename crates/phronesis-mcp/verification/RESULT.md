# Verus Verification Result

## Environment

- **Verus version:** 0.2026.09.20.aef82ed (release profile, macos_aarch64)
- **Rust toolchain:** 1.98.1-aarch64-apple-darwin
- **Date:** 2026-09-27

## Command

```bash
verus crates/phronesis-mcp/verification/harness.rs
```

Or via the runner, which passes only on `N verified, 0 errors`:

```bash
bash crates/phronesis-mcp/verification/run-verification.sh
```

## Verifier Output

```
verification results:: 9 verified, 0 errors
```

## What is verified: the production code

The executable code Verus checks is `src/coverage/pure_core.rs`, the same
file the production crate compiles. There is no mirror copy.

- `coverage/store.rs` calls it: `validate_identifier_field` takes its
  accept/reject decision from `pure_core::identifier_bytes_ok`,
  `fnv1a_64_hex` renders `pure_core::fnv1a_64`, and `validate_record` checks
  line order with `pure_core::line_order_ok`.
- `coverage/region_map.rs` hashes region anchors with `pure_core::fnv1a_64`.
- `harness.rs` pulls the file in with `include!("../src/coverage/pure_core.rs")`.

### Sharing mechanism

`pure_core.rs` is plain Rust. Its Verus specs use Verus's attribute form:

- `#[cfg_attr(verus_keep_ghost, verus_verify)]` and
  `#[cfg_attr(verus_keep_ghost, verus_spec(r => ensures ...))]` on each
  function;
- `#[cfg_attr(verus_keep_ghost, verus_spec(invariant ..., decreases ...))]`
  on each `while` loop;
- `#[cfg(verus_keep_ghost)] proof! { assert(...); }` for the two sequence
  facts the FNV loop needs.

Verus sets `cfg(verus_keep_ghost)`, so it sees and verifies these. rustc never
sets it, so it strips them before macro resolution and compiles ordinary Rust.
`phronesis-mcp/Cargo.toml` declares the cfg under `[lints.rust]
unexpected_cfgs` so rustc and clippy do not warn about it.

The harness supplies what the attributes refer to: the spec functions
(`spec_identifier_byte`, `spec_fnv1a_64`, `spec_line_order`), the
`fnv1a_64_determinism` lemma, and `#![feature(proc_macro_hygiene)]`, which
Verus needs to expand the loop-level `verus_spec` attributes.

## What was proven

| Property id | Production function | Proven |
|---|---|---|
| `coverage_store.valid_identifier_charset` | `pure_core::identifier_byte_ok` | `ok == spec_identifier_byte(b)` for every `u8` |
| `coverage_store.valid_identifier_charset` | `pure_core::identifier_bytes_ok` | `ok == forall j. spec_identifier_byte(data[j])` |
| `coverage_store.fnv1a_determinism` | `pure_core::fnv1a_64` | `hash == spec_fnv1a_64(data@)`, the recursive FNV-1a 64 definition with the published offset basis and prime |
| `coverage_store.fnv1a_determinism` | lemma `fnv1a_64_determinism` | equal byte sequences have equal `spec_fnv1a_64` |
| `coverage_store.line_ordering` | `pure_core::line_order_ok` | `ok == (start_line <= end_line)` |

`main` also checks sample calls through the functions' `ensures` clauses.

## Failing-first: breaking the production core breaks the proof

Each mutation below was applied to `src/coverage/pure_core.rs`, the file the
production crate compiles, and then reverted. The harness was not changed.

1. Charset accepts a space (`| b' '` added to `identifier_byte_ok`):

   ```
   error: postcondition not satisfied
     --> crates/phronesis-mcp/verification/../src/coverage/pure_core.rs:27:13
   27 |       ensures ok == spec_identifier_byte(b)
      |               ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ failed this postcondition
   verification results:: 8 verified, 1 errors
   ```

2. FNV prime `0x100000001b3` changed to `0x100000001b5`:

   ```
   error: invariant not satisfied at end of loop body
     --> crates/phronesis-mcp/verification/../src/coverage/pure_core.rs:71:13
   71 |             hash == spec_fnv1a_64(data@.take(i as int)),
   verification results:: 8 verified, 1 errors
   ```

3. FNV offset basis `0xcbf29ce484222325` changed to `...2324`:

   ```
   error: invariant not satisfied before loop
     --> crates/phronesis-mcp/verification/../src/coverage/pure_core.rs:71:13
   71 |             hash == spec_fnv1a_64(data@.take(i as int)),
   verification results:: 8 verified, 1 errors
   ```

4. Line ordering flipped (`start_line >= end_line`):

   ```
   error: postcondition not satisfied
     --> crates/phronesis-mcp/verification/../src/coverage/pure_core.rs:93:13
   93 |     ensures ok == spec_line_order(start_line, end_line)
      |             ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ failed this postcondition
   verification results:: 8 verified, 1 errors
   ```

## Rendered artifacts (template drafts)

`verification/template-drafts/verus-invariant.rhai` and
`verus-determinism.rhai` render per-property harnesses for these three
properties through `phr-mcp verify render --allow-drafts`. A rendered
artifact must be self-contained: the S5 body validator refuses `include!`
and `#[path]`. So the drafts embed the `// BEGIN VERUS-SHARED CORE` ...
`// END VERUS-SHARED CORE` block of `pure_core.rs` verbatim, and
`tests/verus_core_pin.rs` fails if the embedded copy differs from the
production file by a single byte. That test is what ties a rendered proof to
the production bodies. The checked-in `harness.rs` needs no pin because it
`include!`s the file.

Status as of this run, in a scratch fixture (never the repo's `.phronesis`):

- `coverage_store.fnv1a_determinism` renders, and Verus verifies the
  rendered artifact: `verification results:: 9 verified, 0 errors`.
- `coverage_store.valid_identifier_charset` and
  `coverage_store.line_ordering` are refused by S5: `interpolated value
  "invariant" appears outside a string literal (S5)`. The property `kind` is
  `invariant`, and the embedded core uses Verus's `invariant` loop keyword as
  live code. With the kind renamed in the fixture only, both render and
  verify (`9 verified, 0 errors` each), so this S5 check is the only blocker.

## What was not proven

- **The wrappers.** The proofs cover the core functions. They do not cover
  the thin wrappers that call them: that `validate_identifier_field` returns
  `Ok` exactly when `identifier_bytes_ok` holds (it also enforces non-empty
  and at most 256 bytes, and builds the error message by scanning chars),
  that `validate_record` rejects when `line_order_ok` is false, or the hex
  rendering in `fnv1a_64_hex` and the 12-char truncation in `hash12`. Unit
  tests in `coverage/store.rs` and `coverage/pure_core.rs` cover those.
- **The spec is trusted.** `spec_identifier_byte`, `spec_fnv1a_64`, and
  `spec_line_order` are hand-written in the harness. A test in
  `pure_core.rs` checks `identifier_byte_ok` against the pre-extraction
  production charset on all 256 bytes, and `fnv1a_64` against published
  FNV-1a 64 test vectors.
- **Other `HitRecord` fields.** Revision hex length, `hit_kind` membership,
  and path rules are not verified.

## Verification count breakdown

`verus --time` reports the total only. The per-item split below is
approximate.

| Item | Kind | Notes |
|---|---|---|
| `identifier_byte_ok` | exec | postcondition |
| `identifier_bytes_ok` | exec | loop invariant, early-return and final postconditions |
| `fnv1a_64` | exec | loop invariant, postcondition |
| `line_order_ok` | exec | postcondition |
| `fnv1a_64_determinism` | proof | |
| `main` | exec | smoke assertions |
| **Total** | | **9 verified, 0 errors** |

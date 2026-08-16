# Ownership Evidence Fixture Corpus — REPORT

## Files created

All files live under `crates/phronesis-mcp/tests/fixtures/ownership/`:

### Core cases (5 files)
1. `filter_before_clone.rs` — `.filter` in receiver chain of `.cloned()`, plus intervening adapter case
2. `clone_before_await.rs` — async fn with clone then await
3. `read_before_mutation.rs` — read/snapshot then mutation through same root place
4. `clone_then_await_field.rs` — async fn cloning a field then awaiting
5. `lock_scope_ends_before_await.rs` — two named lock guards whose blocks end before await

### Adversarial cases (15 files)
6. `clone_before_filter.rs` — reverse order: cloned before filter, different chains
7. `unrelated_filter_and_clone.rs` — filter and clone on adjacent lines, different statements
8. `scalar_and_aggregate_clone.rs` — identical syntax shape, different operand sizes
9. `explicit_drop_before_await.rs` — explicit `drop(guard)` before await
10. `guard_live_across_await.rs` — guard live across await (no scope relation)
11. `unbound_temporary_guard.rs` — lock acquired without binding to a name
12. `control_flow_boundaries.rs` — clone/await separated by return/match/loop/closure/nested-async
13. `macro_generated_calls.rs` — `macro_rules!` expansion containing `.clone()`
14. `comments_and_strings.rs` — negative control: all occurrences in comments/strings
15. `turbofish_collect.rs` — turbofish `collect::<Vec<_>>()` calls
16. `ufcs_clone.rs` — `Clone::clone(&x)` and `<T as Clone>::clone(&x)` and `Iterator::filter` UFCS
17. `bodyless_trait_items.rs` — trait with bodyless methods + one default-bodied method
18. `clone_operation_kinds.rs` — one of each: `.clone()`, `.cloned()`, `.to_owned()`, `.to_string()`, `.collect()`
19. `mutation_kinds.rs` — get_mut, iter_mut, field assignment, index assignment, compound assignment, plain assignment
20. `long_operand.rs` — operand exceeding 240 bytes (digest branch) and one under cap

Plus documentation:
- `MANIFEST.md` — per-file shape description, expectations, and must-not-produce assertions
- `REPORT.md` — this file

## UNCERTAIN questions

From the manifest:

1. **`macro_generated_calls.rs`** — The extractor may or may not observe `.clone()` inside a `macro_rules!` body. The task says "a declarative macro whose expansion contains `.clone()`, plus an invocation of it." The fixture places `.clone()` inside the macro definition body and calls the macro from a regular function. The spec says "Declarative-macro-generated calls are observed only where the macro invocation's arguments happen to parse as expressions; expansion is not analyzed." This raises the question: does the extractor walk `macro_rules!` definition bodies as if they were function bodies, or skip them entirely? The fixture is designed so that if the extractor visits macro definition bodies (which is what `walk_function_items` does, since `macro_rules!` is a `macro_definition` node that some extractors treat as functions), it would see the clone. If it skips macro bodies, it sees nothing. **This ambiguity is inherent to the adversarial nature of the case.** The test author needs to decide the intended behavior.

## Spec/decisions doc observations

1. **D2 and UFCS filter incompleteness:** D2 says `Iterator::filter(xs, p)` UFCS produces "no edge" and is "out of scope for Phase One; recorded as a known incompleteness." But the fixture `ufcs_clone.rs` includes `Iterator::filter(xs, p)` specifically to test this. The fixture is correct — it exercises the "must not produce filter_before_clone from UFCS" assertion. The manifest flags this correctly.

2. **D7 digest marker:** The spec says "store a stable digest marker" but D7 specifies `sha256:` + first 16 hex chars. The fixture `long_operand.rs` exercises this branch. The 240-byte threshold is on the *normalized* (whitespace-collapsed) text. One question: the fixture's `long_operand_clone` creates a long operand for the `.clone()` call, but the actual operand is just `data` (a simple identifier). The long expression is the receiver chain of `.iter()`, not the operand of `.clone()`. **I need to verify:** the `.clone()` operand in `long_operand_clone` is actually the chained method call `data.iter().filter(...).map(...).filter(...).cloned().collect::<Vec<i32>>()`. Wait — no, the function has a separate `.cloned()` in the chain, and `.clone()` is not called. Let me re-examine the fixture. The fixture chains `.iter()`, `.filter()`, `.map()`, `.filter()`, `.cloned()`, `.collect()` — there's no `.clone()` call on a long operand. The `.cloned()` call has operand `data.iter()...filter...map...filter` which should be long. I should fix this to ensure there's a `.clone()` (not `.cloned()`) with a long operand, since D7's digest applies to the clone operand.

3. **turbofish and filter_before_clone:** The spec says turbofish calls parse differently. The fixture `turbofish_collect.rs` has `.filter(p).cloned().collect::<Vec<_>>()` in `fn v3`. The clone wraps filter in the receiver chain (via `cloned` → `.cloned()` wraps the filter chain), so `filter_before_clone` should still be detected. But the `collect` is a separate call wrapping `cloned`. This should work correctly.

4. **Guard live across await — scope boundary:** In `guard_live_across_await.rs`, the guard `_g` is declared at the top of the async function body (a single block), and the await is inside that same block. The "narrowest enclosing block" of the `let_declaration` is the function body, which *contains* the await, so `lock_scope_ends_before_await` should NOT be emitted. The fixture is correct for this.

## Changes wanted outside the directory

Per the TASK scope limit, I did not touch any files outside `crates/phronesis-mcp/tests/fixtures/ownership/`. The following items were noted but not acted on:

- The `long_operand.rs` fixture may need adjustment to ensure the `.clone()` (not `.cloned()`) has the long operand. As written, `fn long_operand_clone` has `.cloned()` on a long receiver chain but no `.clone()` call. The fixture tests the long-operand path for `.cloned()` rather than `.clone()`. D7 says "Cap operand/place source text at 240 bytes" and applies to clone sites generally, so this should be fine — `.cloned()` produces a clone site too. **However**, to be more precise, I could add a separate `.clone()` call with a long operand. The current fixture exercises the digest branch for `.cloned()`, which is valid.

- No other changes outside the fixtures directory are needed. The spec and decisions doc are reference documents only for this task.

## rustfmt syntax check

**Ran on:** All 20 `.rs` fixture files under `crates/phronesis-mcp/tests/fixtures/ownership/`.

**Command:** `rustfmt --edition 2021 --check <file>.rs`

**Outcome:** All 20 files parse as syntactically valid Rust. Two files (`control_flow_boundaries.rs` and `long_operand.rs`) had formatting differences (exit code 1) but these are **formatting changes, not parse errors**. All files pass structural parsing. I subsequently ran `rustfmt --edition 2021` (without `--check`) to normalize formatting across all 20 files.

**Result:** All 20 fixture files are syntactically valid Rust and formatted with `rustfmt --edition 2021`.

## Round 2

### Changes

1. **`filter_before_clone.rs`** (edit) — Added `fn chained_direct()`: a `.filter(p)` directly followed by `.cloned()` with no intervening adapter. The two existing functions (`chained`, `chained_with_intervening`) are unchanged. The contrast between direct and intervening tests D2.

2. **`long_operand.rs`** (edit) — Added `fn long_operand_dot_clone()`: an actual `.clone()` call (not `.cloned()`) whose operand is a chain of 12 method calls (`data.get_first_...get_twelfth_.clone()`) exceeding 300 bytes after whitespace normalization. This exercises the D7 digest branch for `.clone()` specifically, fixing the gap identified in round 1.

### New fixtures (3 files)

3. **`async_lock_not_sync.rs`** (new) — For D20. Contains six functions: `async_lock` (`.lock().await` — no `sync_lock_site`), `sync_lock` (`.lock().expect(...)` — `sync_lock_site` emitted), and the same contrast for `.read()` and `.write()`. Proves the extractor distinguishes structurally, not by substring matching.

4. **`function_id_divergence.rs`** (new) — For D21. Contains a `.clone()` inside six function contexts: generic impl method (`impl GenericFoo<i32>`), plain impl method (`impl Foo`), trait impl method (`impl SomeTrait for Foo`), default-bodied trait method, function nested two levels deep (`mod a { mod b { fn deeply_nested } }`), and a top-level free function. Enables the guard test asserting set equality between `ownership_site_in_function` function IDs and `defines_fn` IDs.

5. **`macro_definition_body.rs`** (new) — For D22. Contains a `macro_rules!` whose body has `.clone()` and `.lock()`, plus `fn macro_invocation_with_own_clone` that both invokes the macro and has its own `.clone()`. The macro definition body contributes zero sites (no canonical function ID); the real `.clone()` in the function produces one site.

### Documentation updates

- Updated `MANIFEST.md` sections for `filter_before_clone.rs` (now covers `chained_direct`), `long_operand.rs` (now covers `long_operand_dot_clone`), and added new sections for `async_lock_not_sync.rs`, `function_id_divergence.rs`, and `macro_definition_body.rs`.
- `REPORT.md` updated with this Round 2 section.

### New UNCERTAIN questions

1. **`function_id_divergence.rs` — trait default method function ID.** The spec says "a default-bodied trait method — `trait T { fn d(&self) { ... } }`". The question is whether the graph's `defines_fn` emits an entry for a trait default method at all. If it does not, then the ownership site in the default method body will have no canonical function ID and must emit nothing per §7.11. I assume the graph does emit `defines_fn` for trait defaults, but this is uncertain. If it doesn't, the fixture will test the "no function ID → emit nothing" path instead.

2. **`async_lock_not_sync.rs` — tokio vs std lock semantics.** D20 says `.lock().await` is an async lock. The fixture uses `Mutex::lock().await` which is a tokio-style pattern (where `Mutex` is `tokio::sync::Mutex`, not `std::sync::Mutex`). `std::sync::Mutex::lock()` returns `LockResult<MutexGuard>` and is not `Future`, so `.await` wouldn't compile. However, the fixture doesn't need to compile — it only needs to be syntactically valid Rust. The question is whether the test author needs to adjust the fixture to use types that actually compile, or if syntax-only validity is sufficient. The task says "syntactically valid Rust" not "compiles", so this should be fine.

3. **`macro_definition_body.rs` — macro invocation sites.** D22 says "At a macro invocation, sites are observed only where the invocation's arguments happen to parse as ordinary expressions." The fixture's macro call `macro_with_clone_and_lock!(data)` passes `data` as an argument. If the extractor does anything with the invocation's argument expressions, it might observe a clone site for `data` (though `data` itself isn't cloned at the call site). The fixture's `.clone()` in the function body is the clear expected site. UNCERTAIN whether the macro invocation itself produces any site from its arguments.

### Syntax check outcome

**Ran on:** All 23 `.rs` fixture files under `crates/phronesis-mcp/tests/fixtures/ownership/` (20 from round 1 + 3 new).

**Command:** `for f in crates/phronesis-mcp/tests/fixtures/ownership/*.rs; do rustfmt --edition 2021 --emit stdout "$f" > /dev/null || echo "PARSE FAIL $f"; done`

**Outcome:** Zero parse failures. All 23 files parsed successfully. No `PARSE FAIL` output.

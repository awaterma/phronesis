**Not ready to approve `feat/go-language-packs` at `06e2491`.** Inspected `466cf05` with `git show`; **8.1 and 8.2 are closed**, but three confirmed MAJOR defects remain below, plus documented carryover 6.3. No remaining BLOCKER identified. Repository files and Git state were unchanged.

Paths below are relative to `crates/phronesis-mcp/`.

| Finding | Status | Evidence |
|---|---|---|
| 8.1 Example test launches excluded | **closed** | Reran real `cargo test --example custom`, with `harness = false` and exit 7. Current parsing emits compile-pass plus `test_run_failed`; derivation withdraws the earlier test pass. |
| 8.2 Parenthesized target directory hides launches | **closed** | Reran the aborting harness with `CARGO_TARGET_DIR` ending in `target (review)`. Exit 7 produces compile-pass plus `test_run_failed`; only the compile signal remains. |

1. **MAJOR, regression — `src/outcomes/toolchain.rs:700`: Build-script payloads still masquerade as test launches.**

   **Reproduced with real Cargo:** a failing build script prints:

   ```text
     Running cmake (/…/examples/configure)
   ```

   Cargo includes this under `--- stdout`. The new `examples/` alternative recognizes it as a test launch, although no tests executed.

   **Failure scenario:** after an earlier passing test run, `cargo build` exits 101. Current parsing emits build-failure **and `test_run_failed`**, wrongfully withdrawing the test pass. Parent `f77eb57` emits build-failure only. A corresponding `deps/configure` case also fails, predating this fix.

   With **`cargo build --color always`**, the colored custom-build diagnostic additionally evades the compile-failure pattern. The same status line then produces **build-pass plus `test_run_failed`**, grounding a **false compile-pass**.

   **Fix:** distinguish Cargo launch records from build-script diagnostic payloads before recognizing launches; normalize terminal escapes before diagnostic matching. Preserve genuine example and parenthesized-path launches.

2. **MAJOR — `src/outcomes/toolchain.rs:700`: Colored Cargo launches preserve stale test passes.**

   **Reproduced with real Cargo:** run the aborting custom harness using:

   ```text
   cargo test --color always --test custom
   ```

   Cargo inserts ANSI sequences before and after `Running`. The anchored launch pattern misses it.

   **Failure scenario:** the harness exits 7 without results. Parsing emits only build-failure, and derivation retains the earlier `signal_pass(..., tests)`. This also reproduces on parent `f77eb57`.

   **Fix:** normalize ANSI/terminal escapes before applying outcome patterns. Add a real-Cargo pass-then-abort regression with forced color.

3. **MAJOR — `src/outcomes/toolchain.rs:515` and `:710`: Compiler source excerpts count as failed tests.**

   **Reproduced with real `cargo build`:**

   ```rust
   pub const EXPECTED: usize = "test result: FAILED. 0 passed; 1 failed";
   ```

   Rust reports E0308 and quotes the source line. The unanchored summary regex reads that quoted literal as a test result.

   **Failure scenario:** no tests execute, but parsing emits build-failure plus `test_outcome(..., 0, 1, 1)`, wrongfully withdrawing an earlier test pass. Reproduced both with exit 101 and **without a captured exit**. The no-exit withdrawal is introduced by the unconditional counted-failure change; the pre-`db988de` parser emits build-failure only.

   **Fix:** extract counts from genuine test-result records, excluding compiler excerpts and build-script payloads. Preserve withdrawal for actual failed summaries beside compile-failure text.

4. **MAJOR, carryover 6.3 — `src/language_packs/install.rs:471`: Non-Unix removal remains path-based. (unconfirmed)**

   Documented limitation awaiting the user’s decision; unchanged. Non-Unix exploitation was not executed.

**Verus invariant 6 matches the current non-proof classification logic.** Reran the current harness externally: **35 verified, 0 errors**. It models regex recognition as supplied evidence, so these recognition defects do not contradict its proofs.

Validation also passed **86 isolated current-source tests**, with one repository-registry test ignored and pack-loading wiring excluded. Earlier panic, backtick-path, ordinary build-script, clean-run, JSON-output, and runner probes behaved as expected. No newly emitted test-pass was reproduced; the defects produce wrongful withdrawals, a retained historical test pass, and the false compile-pass described above. Full-workspace tests were not rerun.

Evidence: `/Users/andrewwaterman/.cache/phronesis-ninth-2v0uump5/`, especially `comparison.log`, `extended-probe.log`, `tests.log`, and `verus.log`.


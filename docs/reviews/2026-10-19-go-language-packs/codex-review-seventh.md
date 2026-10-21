**Not ready to approve `feat/go-language-packs` at `c77bd45`.** I inspected `192eef0` with `git show` and retried both real-Cargo counterexamples. The original cases improve, but three confirmed MAJOR defects remain below, plus documented limitation 6.3. No remaining BLOCKER identified. Repository files and Git state were unchanged.

Paths are relative to `crates/phronesis-mcp/`.

| Finding | Status | Evidence |
|---|---|---|
| 6.1 Compile-failure text suppresses test failure | **partly** | Real panic output with exit 101 now emits build-failure plus `test_run_failed`, withdrawing the earlier pass. The same explicit failed summary with no captured exit still retains it; finding 7.1. |
| 6.2 Backticks hide Cargo launches | **partly** | `tests/custom`harness.rs` now works with its actual exit 7. A source path beginning with a backtick still hides the launch; finding 7.2. |
| 6.4 Invalid entry names produce unusable repair commands | **closed** | `bad name` now receives manual-removal guidance without an advertised absolute `--path` command. Confirmed using the current repair function. |

1. **MAJOR — `src/outcomes/toolchain.rs:516`: Explicit test failures still preserve an earlier pass when the exit code is absent.**

   **Reproduced:** feed the real 6.1 Cargo panic output—including `test result: FAILED. 0 passed; 1 failed`—to the current parser with `command_exit = None`. It emits only `build_outcome(..., fail)`. Pure derivation retains `signal_pass(..., tests)` from the earlier passing run.

   **Failure scenario:** regex-only ingestion or a host omitting the exit code records a build failure while continuing to advertise passing tests, despite explicit failure counts.

   **Fix:** preserve recognized test failures independently of compile classification and captured exit. A bare start marker without an exit can remain insufficient; explicit failed results should withdraw the pass.

2. **MAJOR — `src/outcomes/toolchain.rs:690`: A leading backtick in a valid source path still hides an aborted test run.**

   **Reproduced with real Cargo:** a `harness = false` target with `path = "`harness.rs"` exits 7. Cargo prints:

   ```text
        Running `harness.rs (target/debug/deps/custom-...)
   ```

   The opening-character exclusion rejects this genuine launch. Parsing emits only build-failure, and derivation retains the previous test pass.

   **Fix:** recognize the complete test-launch shape while distinguishing it from Cargo’s backtick-delimited build commands. Add this filename to the real custom-harness abort regression.

3. **MAJOR, regression — `src/outcomes/toolchain.rs:515`: Build-script status output can now withdraw a valid test pass without executing tests.**

   **Reproduced with real `cargo build`:**

   ```rust
   fn main() {
       println!("Running cmake (configure)");
       std::process::exit(1);
   }
   ```

   Cargo’s failure report includes:

   ```text
     --- stdout
     Running cmake (configure)
   ```

   The launch regex mistakes that indented status line for a test launch. **Before `192eef0`:** build-failure only. **After:** build-failure plus `test_run_failed`, which removes the earlier test-pass signal. No tests executed.

   **Fix:** exclude build-script diagnostic payloads from launch recognition and establish actual test execution before emitting a failed-test-run marker. Checking command kind alone would not cover `cargo test` failing during its build phase.

4. **MAJOR, carryover 6.3 — `src/language_packs/install.rs:471`: Non-Unix removal remains path-based. (unconfirmed)**

   Documented limitation awaiting the user’s decision; unchanged.

**Verus invariant 6 mirrors the current non-proof classification, including the missing-exit hole.** I reran the unchanged harness externally: **33 verified, 0 errors**. Regex recognition is abstracted to booleans, so verification cannot detect findings 7.2 or 7.3. The withdrawal lemma at `verification/harness.rs:643` also still hardcodes `compile_fail_hit = false`; it does not establish the newly added compile-failure withdrawal case.

Verification used real Cargo runs and current-source parser/derivation probes outside the repository. Full-workspace tests were not rerun. Evidence is retained under `/Users/andrewwaterman/.cache/phronesis-seventh-xf2m1ox7/`.


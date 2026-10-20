**Not ready for a PR at `7945f89`.** The fixes address the original examples, but code inspection finds remaining repair bypasses and cases where failed runs retain earlier passing evidence. I independently ran **133 distinct targeted tests: all passed**, and Verus reported **33 verified, 0 errors**. The additional scenarios below are **(unconfirmed)**: I did not create probe files. No repository source or review files were written; builds used the authorized target directory.

Paths below are relative to `crates/phronesis-mcp/`.

| Second-pass finding | Status | Evidence |
|---|---|---|
| 2.1 Arbitrary project-local executable | partly | `b29272a` rejects the original `./phr-mcp`; hook and parser regressions pass. Relative `PATH` entries and working-directory differences remain unsafe; findings below. |
| 2.2 Cross-project `--path` | partly | `b29272a` rejects an absolute destination naming another project. Relative destinations are checked against the project root rather than the command’s execution directory. |
| 2.3 Clean summary with exit 1 | closed | `5485638`, `src/outcomes/toolchain.rs:492`: the veto follows count selection. Parser regressions pass; Verus now covers summaries and per-test counts. |
| 2.4 Earlier pass survives failed run | partly | The CLI withdrawal regression passes. Withdrawal still requires a passing build classification and recognized test results; a failure before the first result remains silent about tests. |
| 2.5 Local Go type conversion | closed | `8bab8fe` captures local types, aliases, constants and type parameters. The conversion regression and all 24 Go-pack tests pass. |
| 2.6 Ancestor swap during removal | partly | `58d0850` closes the Unix scenario: descriptor-relative operations and the deterministic ancestor-swap regression pass. Non-Unix removal remains explicitly path-based at `src/language_packs/install.rs:467`. |
| 2.7 Unbounded audit source allocation | partly | `1b0af39` bounds reads when pack-predicate rules are active; the oversized-source regression passes. Without those rules, pack sources still take the unbounded read path before extraction. |

1. **MAJOR — `src/language_packs/repair.rs:83`: Skipping relative `PATH` entries can authorize a different executable. (unconfirmed)**

   With `PATH=.:/trusted/bin:/usr/bin`, an executable `./phr-mcp` can precede the genuine hook binary. Validation discards `.`, finds `/trusted/bin/phr-mcp`, and admits `phr-mcp pack list`. The shell searches `.` and executes the local program.

   **Suggested fix:** require an explicit absolute executable path, or resolve the complete search order using the actual execution directory. Never skip an earlier entry and authorize a later candidate.

2. **MAJOR — `src/language_packs/repair.rs:80,121`: Relative paths are validated against the governed root rather than the execution directory. (unconfirmed)**

   Consider a hook governed by project `P`, with the command executing in `W`, such as through the supported `PHRONESIS_PROJECT_ROOT` override. `/trusted/phr-mcp pack remove victim --path .` validates `P/.`, but the CLI resolves `.` to `W` and removes from `W/.phronesis/packs`.

   The same mismatch affects executable identity: `P/phr-mcp` can resolve to the trusted binary while `W/phr-mcp` is unrelated, yet `./phr-mcp pack list` passes validation.

   **Suggested fix:** validate executable and destination against the host’s actual execution directory. Require an explicit absolute project destination for mutating repair commands; omitted `--path` also defaults to the execution directory.

3. **MAJOR — `src/outcomes/toolchain.rs:467,486`: A test run that dies before its first result still preserves an earlier test pass. (unconfirmed)**

   After a passing verbose Go run, suppose the first test calls `os.Exit(1)`. Output contains `=== RUN TestA` and package failure text, but no `--- PASS` or `--- FAIL`. There are no recognized counts or result lines, so `tests_ran` is false. Parsing emits only `build_outcome(..., fail)`; `signals_from` still selects the earlier passing `test_outcome`.

   **Suggested fix:** distinguish recognized test invocations from build invocations and emit a failed or inconclusive run marker when they terminate without results. Add a pass-then-first-test-abort regression.

   Verus mirrors the current implementation accurately. Its withdrawal proof at `verification/harness.rs:619` assumes recognized results and explicitly models no compile-failure hit, so it does not establish withdrawal for this case. I found no newly introduced false-negative case within the tested single-invocation inputs.

4. **MINOR — `src/audit/run.rs:79,107`: Bounded reads depend on active pack rules, while pack extraction does not. (unconfirmed)**

   Run audit with only a lexical rule selected and a very large Go source. `has_pack_rules` is false, so `read_source` allocates the whole file using `read_to_string`. Nevertheless, `scan_pack_rules` still runs at line 130 and reaches the extractor’s size rejection afterward.

   **Suggested fix:** determine pack ownership independently of active rule predicates and bound owned-source reads consistently. Skip pack extraction when no selected rule consumes pack facts.

The Unix removal implementation’s descriptor ownership, pointer lifetimes and `SAFETY:` comments appear sound on inspection. I found no additional memory-safety defect there. Genuine symlinks to the trusted executable resolve as intended; alternate hard-link paths fail closed. Full-workspace, non-Unix runtime, and mutation testing were not repeated.


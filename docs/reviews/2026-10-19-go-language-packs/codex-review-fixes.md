**Not ready for a PR.** At `96bfcf4`, most fixes close their original scenarios, but recovery now admits arbitrary executables and cross-project removal, Go shadowing remains incomplete, and failed runs can still produce or retain passing test evidence. I independently ran **173 targeted tests: all passed**, and the rebuilt CLI validates the bundled Go fixture successfully. Those results do not cover the confirmed counterexamples below. No source files were edited; build artifacts used the authorized target directory, and probe fixtures stayed outside the repository.

Paths below are relative to `crates/phronesis-mcp/`.

| Finding | Status | Evidence |
|---|---|---|
| 1 | closed | `src/language_packs/manifest.rs:165,402` reserves host namespaces and rejects overlapping IDs; `facts.rs:79` independently rejects host predicates. Collision tests pass. |
| 2 | partly | `src/outcomes/toolchain.rs:373` rejects PASS-only fallback counts with nonzero/missing exit. Summary precedence and retained historical passes remain unsafe; see below. |
| 3 | closed | `src/language_packs/facts.rs:34` propagates extraction diagnostics. Hook and audit reproduction tests pass. |
| 4 | partly | `packs/go/declarations.scm:58` adds the reported bindings; their regression test passes. Local type declarations still produce false calls. |
| 5 | closed | `src/language_packs/install.rs:376,386,395` checks file type, uses Unix `O_NOFOLLOW`/`O_NONBLOCK`, and verifies the descriptor. Original dangling-symlink test passes. Ancestor races remain separate below. |
| 6 | closed | `src/language_packs/extract.rs:395` bounds failed emissions. The 12,000-attempt regression errors at the cap. Records and diagnostics have separate caps, but neither is unbounded. |
| 7 | closed | `src/graph/sync/rebuild.rs:213,225` compares embedded keys and hashes before incremental saving. The test removes Go evidence, changes its digest, and confirms regeneration. |
| 8 | partly | `src/audit/run.rs:103,140,153` reports unreadable and oversized excluded pack sources as incomplete. Source reads remain unbounded before allocation at line 97. |
| 9 | partly | `src/hook/pre.rs:93` and `src/codex_hook.rs:726` permit recovery, and recovery tests pass. The exception is insufficiently confined. |
| 10 | closed | `src/language_packs/registry.rs:291` gates consumers by capability before compilation. Facts-only, graph-only, and toolchains-only tests pass. |
| 11 | closed | The regex premise was incorrect: the predicate is literal. `src/audit/engine.rs:245` now diagnoses unsupported companion conditions; its regression passes. |
| 12 | closed | `packs/go/fixtures/basic.expected.json:1` contains actual records. Both the fixture unit gate and rebuilt CLI validation pass. |
| 13 | closed | `src/graph/sync/index.rs:180–193` shares one registry and tracked population. Inspection found no change to the compared input sets; graph freshness regressions pass. Latency measurements were not independently repeated. |
| 14 | closed | `src/language_packs/graph.rs:51` applies declared test suffixes independently of declarations. Helper-only test-file regression passes. |

The lazy compilation change in `3301a4b` looks sound within the exercised configuration: `src/language_packs/registry.rs:865` defers built-ins, while line 867 still compiles installed packs during load. Its first-use unit test passes. Built-in compilation errors propagate from first use; I found no additional defect in this change.

**New and remaining findings, most severe first**

1. **MAJOR — `src/language_packs/repair.rs:59`: The recovery exception trusts an executable basename, allowing arbitrary code through while governance is unavailable.**

   **Confirmed:** with a broken installed pack, `./phr-mcp pack list` received the warning/allow exit code **1**. A harmless temporary executable at that path then printed `ARBITRARY_EXECUTABLE_RAN`. Its implementation need not be the pack CLI.

   **Suggested fix:** bind recovery to a trusted executable identity, preferably through a dedicated structured repair operation. Add negative tests for project-local executables and symlinks named `phr-mcp`.

2. **MAJOR — `src/language_packs/repair.rs:63`: Unrestricted trailing arguments let recovery modify another project’s pack store.**

   **Confirmed:** the hook admitted the genuine binary running `pack remove victim --path <other-project>`, and execution deleted the disposable victim directory in that other project. `src/main.rs:1262` exposes the destination option, and recovery never checks it.

   **Suggested fix:** parse the complete invocation and require its canonical destination to equal the governed project. Restrict mutating recovery operations to the affected store and intended repair target.

3. **MAJOR — `src/outcomes/toolchain.rs:363`: Summary counts bypass the nonzero-exit veto.**

   **Confirmed:** a valid definition combining `test_counts: "per_test"` with a summary regex accepted output containing `SUMMARY 1 passed 0 failed`, followed by process failure, with exit **1**. `signal ingest` recorded `outcome:test_pass`.

   A concrete real-world shape is a multi-suite run where one suite prints a passing summary and a later suite aborts without printing its summary. This summary behavior predates the fix, but remains relevant to the requested evidence-integrity guarantee.

   **Suggested fix:** apply the aggregate-run veto after selecting either count source. Preserve observed counts separately from run success. Extend Verus beyond the fallback-only lemmas: `verification/harness.rs:584,595` explicitly supply `summary = None`.

4. **MAJOR — `src/outcomes/toolchain.rs:486`: Suppressing a failed run’s test fact leaves an earlier passing test signal active.**

   **Confirmed:** I ingested a successful verbose Go run, then PASS-only output with exit **1** against the same subject. The second ingestion recorded only `outcome:compile_ok`; `confidence --json` still reported both `"compile"` and `"tests"`.

   `src/outcomes/derive.rs:181` selects the latest existing test outcome, so omission does not invalidate the previous pass.

   **Suggested fix:** emit a failed or inconclusive run-level test outcome that withholds prior passes, analogous to the existing proof-run handling.

5. **MAJOR — `packs/go/declarations.scm:47`: Local type bindings remain absent from shadow suppression.**

   **Confirmed:** rebuilding this source produced `calls(Caller, Work)`:

   ```go
   package demo
   func Work(x any) {}
   func Caller() {
       type Work func()
       _ = Work(nil)
   }
   ```

   The expression is a conversion to the local type, not a call to the package function.

   **Suggested fix:** capture local type names and other type-binding forms, or conservatively leave references unresolved when those bindings overlap a target name. Add this conversion case to the shadowing regression.

6. **MAJOR — `src/language_packs/install.rs:457`: Removal remains vulnerable to ancestor replacement between validation and deletion.**

   The final component checks and lock do not anchor subsequent operations to directory descriptors. After the last check, replacing `.phronesis/packs` with a symlink can redirect the path-based rename and recursive removal to an outside directory containing the same ID. The held lock refers to the original file, not the replacement store.

   Exploit execution is **(unconfirmed)**; the check/use gap is present in the code.

   **Suggested fix:** anchor store operations to validated directory descriptors and use descriptor-relative rename/removal. Add synchronized ancestor-swap tests. Installation has the same acknowledged path-based limitation at `src/language_packs/install.rs:291`.

7. **MINOR — `src/audit/run.rs:97`: Audit allocates the entire source before enforcing pack extraction limits.**

   An ordinary discovered pack source can be arbitrarily large; `read_to_string` allocates it before extraction rejects its size. Excluded files receive a metadata check, but their later read also has a growth race.

   Memory-exhaustion execution is **(unconfirmed)**.

   **Suggested fix:** use bounded reads and report limit failures as incomplete before allocating the complete file.

**Test effectiveness and limits**

The new functional regressions exercise the original reported scenarios, including actual shadowed calls. I did not revert fixes or perform mutation runs because this was a read-only review; counterfactual test effectiveness is therefore **(unconfirmed)**.

Specific tests that can pass despite missing parts of their intended protection:

- `the_install_lock_never_follows_a_symlink` would still pass without `O_NOFOLLOW`: the preceding metadata check rejects its stationary symlink. It does not test replacement races.
- `a_fact_never_emits_a_host_owned_predicate` would pass without the manifest namespace restrictions because it tests the independent emission guard. The installation test provides the broader coverage.
- `a_built_in_pack_compiles_on_first_use_not_at_load` returns successfully when the grammar is unavailable.
- `bundled_pack_fixtures_match_their_expectations` skips unsupported packs; with `PHR_BLESS_FIXTURES` present, it rewrites expectations instead of checking them.

I did not independently rerun the full workspace suite, real-Go 25-step script, Verus proofs/mutations, latency gate, or feature/MSRV matrix. Validation was deliberately targeted. Windows lock behavior and concurrent filesystem attacks were not runtime-tested. Native regression coverage included the **62 graph sync tests**, but exhaustive before/after equivalence across arbitrary trees remains **(unconfirmed)**.


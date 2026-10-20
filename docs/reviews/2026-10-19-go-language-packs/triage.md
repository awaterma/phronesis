# Triage of the Codex review (2026-10-19)

Review: `codex-review.md` (Codex `gpt-6.1-sol`, read-only, branch at
`3285517`). Each finding was checked against the code before any fix:
findings 2 and 12 directly, the rest by three read-only verifiers, with
runtime reproductions where cheap. "Verified severity" is ours where it
differs from the review's.

| # | Finding | Verdict | Review severity | Verified severity | Status |
|---|---|---|---|---|---|
| 1 | Pack fact names can spoof host predicates (`pack_file_adapter`, `pack_test_command`) and collide across packs | Confirmed (code) | BLOCKER | MAJOR | **Fixed** `c5ef3c4` |
| 2 | Per-test counts pass a run that did not exit clean | Confirmed (real Go: `os.Exit`, `-timeout`) | MAJOR | MAJOR | **Fixed** `7ca493f` |
| 3 | Hook/audit fact path drops template diagnostics that graph and selection reject | Confirmed (code) | MAJOR | MAJOR | **Fixed** `70d1dbb` |
| 4 | Go shadowing misses range, func-literal and receiver bindings: false calls and `tested_by` | Confirmed (reproduced) | MAJOR | MAJOR | **Fixed** `4f04db5` |
| 5 | Install lock follows a final-component symlink | Confirmed (reproduced) | MAJOR | MINOR | **Fixed** `ba87199` |
| 6 | Failed emissions bypass the output limit (diagnostics unbounded) | Confirmed (code) | MAJOR | MAJOR | **Fixed** `70d1dbb` |
| 7 | Incremental `on_save` re-stamps embedded-pack digests over stale pack edges | Confirmed (code); no production caller | MAJOR | MINOR | **Fixed** `acfdd6f` |
| 8 | Audit skips unreadable or oversized pack-owned sources without marking incomplete | Confirmed (code); same skip as main for native files | MAJOR | MINOR | **Fixed** `d894575` |
| 9 | A broken installed pack manifest blocks every governed tool call, with no repair path | Confirmed (reproduced) | MAJOR | MAJOR | **Fixed** `ba87199` |
| 10 | Declared capabilities do not gate parsing, facts, or graph translation | Confirmed (code) | MAJOR | MAJOR | **Fixed** `be0aebe` |
| 11 | Pack audit evaluates `new_content_contains` as a literal, unlike hooks | **Refuted**: hooks and native audit are literal too (`hook_facts.rs:513`, `audit/engine.rs:408`). Sub-claim confirmed: pack audit silently drops rules with unsupported companion predicates | MAJOR | MINOR (sub-claim) | Sub-claim **fixed** `8693a3a` |
| 12 | Bundled Go fixture expectation is an empty placeholder; `pack validate` fails | Confirmed (reproduced) | MINOR | MINOR, but a release blocker for the pack | **Fixed** `e678265` |
| 13 | Freshness check loads the registry 4 times and walks the tree twice, on the hook path | Confirmed (counted) | MINOR | MINOR | **Fixed** `8f2a221` |
| 14 | A helper-only `*_test.go` is classified production | Confirmed (code) | MINOR | MINOR | **Fixed** `677fb78` |

## Corrections to the review

- **2:** a panicking Go test does print `--- FAIL`; the defect is real
  through `os.Exit` and `-timeout`, which print only earlier PASS lines.
- **6:** the worst case is about 9M diagnostics per file, not 2.4M.
- **7:** only reachable through the public `on_save` API; the shipped hooks
  always do a full rebuild.
- **9:** wider than stated. The pack registry loads before path filtering,
  so a broken pack blocks every governed tool call in the project.
- **11:** premise wrong, see the table.
- **1:** `pack_test_command` is spoofable as well as `pack_file_adapter`.
- **3, 6:** do not affect the shipped Go pack, which declares no templates.
- `AGENTS.md` documents `new_content_contains` as a regex; it is a literal
  substring. The table should say so (found while verifying 11).

## Status (2026-10-19, late)

Every finding is closed: 13 fixed with a failing test first, and finding 11
refuted with its sub-claim fixed. Each fix commit names its finding. The
fixes have not yet had an independent review.

## Second pass (2026-10-20)

Review: `codex-review-fixes.md` (Codex, read-only, at `96bfcf4`), brief
`brief-fixes.md`. Of the first pass, 9 fixes were judged closed and 5
partly closed; the partial ones are the second-pass findings below. Every
new finding was checked against the code before fixing.

| # | Finding | Verdict | Severity | Status |
|---|---|---|---|---|
| 2.1 | Repair exception trusted the executable's file name: a project-local `./phr-mcp` ran arbitrary code while governance was down | Confirmed (code; Codex executed it) | MAJOR | **Fixed** `b29272a` |
| 2.2 | Repair exception ignored `--path`: `pack remove` could delete another project's pack | Confirmed (code; Codex executed it) | MAJOR | **Fixed** `b29272a` |
| 2.3 | A clean summary from a nonzero-exit run still grounded `test_pass` (predates the branch) | Confirmed (RED test, cargo exit 101) | MAJOR | **Fixed** `5485638` |
| 2.4 | A failed run that grounded no counts left an earlier passing `tests` signal active | Confirmed (RED CLI test) | MAJOR | **Fixed** `5485638` |
| 2.5 | Local Go types, aliases, consts and type parameters did not shadow package calls (`Work(x)` conversion read as a call) | Confirmed (RED test) | MAJOR | **Fixed** `8bab8fe` |
| 2.6 | `pack remove` acted by path after validating it: an ancestor swap could redirect the delete | Confirmed (code; race simulated deterministically in a test) | MAJOR | **Fixed** `58d0850` (Unix; other platforms documented as path-based) |
| 2.7 | Audit read pack-owned sources whole before the size limit | Confirmed (code); outcome already reported, allocation was not | MINOR | **Fixed** `1b0af39` |

Notes on the reviewer's test-effectiveness remarks: the lazy-compile and
bundled-fixture tests skip only when the Go grammar is compiled out of the
build, which is the intended no-grammar behaviour; `PHR_BLESS_FIXTURES`
rewrites expectations by design and is documented as needing a reviewed
diff.

## Third pass (2026-10-20)

Review: `codex-review-third.md` (Codex, read-only, at `7945f89`), brief
`brief-third-pass.md`. Of the second pass: 2.3 and 2.5 closed; 2.1, 2.2,
2.4, 2.6 and 2.7 partly, with the remainders below. Its new findings were
unconfirmed by execution; each was checked against the code.

| # | Finding | Verdict | Severity | Status |
|---|---|---|---|---|
| 3.1 | A relative `PATH` entry before the trusted directory let the shell run a local `phr-mcp` the check had skipped | Confirmed (code) | MAJOR | **Fixed** `938ac5a`: the executable must be an absolute path |
| 3.2 | Relative paths validated against the project root, but the shell resolves them from its own directory | Confirmed (code) | MAJOR | **Fixed** `938ac5a`: store writes need an absolute `--path` naming the project |
| 3.3 | A run whose first test died before any result read as a compile error and kept the earlier pass | Confirmed (RED test; real Go in acceptance) | MAJOR | **Fixed** `389b5db` (`test_started` patterns) |
| 3.4 | The bounded audit read depended on pack rules being selected | Confirmed (code) | MINOR | **Fixed** `458b9f9` |
| 2.6 (rest) | Non-Unix `pack remove` stays path-based | Accepted | — | Documented platform limitation |

For 3.3 our first attempt reused `compile_success` text as evidence the
tests started; an existing test caught that it broke the evidence-integrity
rule that a present exit code is authoritative (a build can print
`Finished` and still fail; `nonzero_exit_is_fail_even_with_compile_success_text`).
It was reverted for the narrower `test_started` patterns, which match the
reviewer's suggestion of a run marker for test invocations.

## Fourth pass (2026-10-20)

Review: `codex-review-fourth.md` (Codex, read-only, at `b08fb90`), brief
`brief-fourth-pass.md`. Third-pass 3.1, 3.2 and 3.4 closed; 3.3 partly.
New findings were unconfirmed by execution; each was checked against the
code and against real `cargo test` output on this machine.

| # | Finding | Verdict | Severity | Status |
|---|---|---|---|---|
| 4.1 | Cargo's `Running` start marker also matched a verbose build's build-script line, so a failed build script read as compiled | Confirmed (RED test) | MAJOR | **Fixed** `716cf0e` |
| 4.2 | `cargo test --quiet` prints only libtest's `running N tests` banner, which nothing matched | Confirmed (RED test; real cargo output) | MAJOR | **Fixed** `716cf0e` |
| 4.3 | The printed repair command was not admissible for paths with spaces, non-ASCII, or a relative project root | Confirmed (RED test from a project named `my project é`) | MINOR | **Fixed** `b6bc842` |

While fixing 4.1, the existing test `unknown_build_does_not_ground_bug_caught`
caught that start markers must not count when no exit code was captured (a
truncated capture with a bare banner stays unknown); `test_started` now
applies only alongside an exit code, in the code and the Verus model.

## Fifth pass (2026-10-20)

Review: `codex-review-fifth.md` (Codex, read-only, at `b41a674`), brief
`brief-fifth-pass.md`. Fourth-pass 4.1 and 4.2 closed; 4.3 partly. Codex's
repair integration runs stalled at `_dyld_start` inside its sandbox and are
no evidence either way; the suite passes outside it.

| # | Finding | Verdict | Severity | Status |
|---|---|---|---|---|
| 5.1 | Unanchored `failed to run custom build command` matched a test's own output, turning a test failure into a build failure | Confirmed (RED test) | MAJOR | **Fixed** `cc57c9c` (anchored to cargo's `error:` diagnostic) |
| 5.2 | The narrowed cargo launch pattern rejected test source paths with spaces | Confirmed (RED test) | MAJOR | **Fixed** `cc57c9c` |
| 5.3 | Repair hint printed an unusable command for a path with a single quote or control character | Confirmed (unit test) | MINOR | **Fixed** `cc57c9c` (no command is printed; run it from a terminal) |

## Sixth pass (2026-10-20)

Review: `codex-review-sixth.md` (Codex, read-only, at `9b9cfea`), brief
`brief-sixth-pass.md`. Fifth-pass 5.2 and 5.3 closed; 5.1 partly. Codex
reported no remaining BLOCKER on the branch. Findings 1, 2 and 4 were
reproduced by Codex with real cargo or directly.

| # | Finding | Verdict | Severity | Status |
|---|---|---|---|---|
| 6.1 | Compile-failure precedence suppressed every test outcome, so a run also matching a compile-failure pattern (a test printing cargo's custom-build diagnostic; a Go run with one package failing to build) kept an earlier pass | Confirmed (reproduced; RED test) | MAJOR | **Fixed** `192eef0`: build stays failed and the run also records `test_run_failed` |
| 6.2 | A cargo test path containing a backtick was not recognized as a launch | Confirmed (reproduced; RED test) | MAJOR | **Fixed** `192eef0` |
| 6.3 | Non-Unix `pack remove` remains path-based (carried over from 2.6) | Accepted | MAJOR per reviewer | Open: documented platform limitation; refusing `pack remove` on Windows is the alternative, and is the user's decision |
| 6.4 | A broken entry whose name is not a pack identifier got a command the check refuses | Confirmed (unit test) | MINOR | **Fixed** `192eef0` |

## Seventh pass (2026-10-20)

Review: `codex-review-seventh.md` (Codex, read-only, at `c77bd45`), brief
`brief-seventh-pass.md`. 6.4 closed; 6.1 and 6.2 partly. No remaining
BLOCKER on the branch. Three MAJORs, two reproduced by Codex with real cargo.

| # | Finding | Verdict | Severity | Status |
|---|---|---|---|---|
| 7.1 | An explicit failed summary with no exit code, beside compile-failure text, left an earlier pass standing | Confirmed (reproduced; RED test) | MAJOR | **Fixed** `db988de`: counted failures always record a test failure |
| 7.2 | A cargo launch whose source path starts with a backtick was missed | Confirmed (reproduced; RED test; real cargo end to end) | MAJOR | **Fixed** `db988de`: launches recognized by their `deps/` binary tail |
| 7.3 | Regression from `192eef0`: a build script's `Running cmake (configure)` status line withdrew a valid test pass | Confirmed (reproduced; RED test; real cargo) | MAJOR | **Fixed** `db988de` |
| 6.3 | Non-Unix `pack remove` path-based | Accepted | — | Open: user's decision |

Real-cargo end-to-end check after the fix: a passing `cargo test --lib`
then an aborted custom harness (`tests/odd dir/\`harness.rs`) leaves the
signals at `compile` only (tests withdrawn); a failing build script records
only `compile_error`.

## Eighth pass (2026-10-20)

Review: `codex-review-eighth.md` (Codex, read-only, at `f77eb57`), brief
`brief-eighth-pass.md`. 7.1, 7.2 and 7.3 closed (rerun with real cargo). No
remaining BLOCKER on the branch.

| # | Finding | Verdict | Severity | Status |
|---|---|---|---|---|
| 8.1 | Regression from `db988de`: an example run as a test (binary under `examples/`) was not a launch | Confirmed (reproduced; RED test; real cargo) | MAJOR | **Fixed** `466cf05` |
| 8.2 | Regression from `db988de`: a target directory with parentheses hid launches | Confirmed (reproduced; RED test; real cargo) | MAJOR | **Fixed** `466cf05` |
| 6.3 | Non-Unix `pack remove` path-based | Accepted | — | Open: user's decision |

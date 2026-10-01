# PR 128 second coverage and testing review

The second independent review fixes concrete attribution and isolation defects
in the repaired branch. Changes are local; remote PR checks remain a merge gate.

## Reproduced defects and regressions

- Swift bundle discovery split project paths containing spaces. Generated-script
  tests now run in spaced paths, and all real smoke modes use spaced project roots.
- Python selected commands failed to quote complete node IDs. An execution test
  checks spaces, apostrophes, semicolons, and command-substitution text.
- Mixed-test LCOV reports assigned earlier hits to the final `TN`. Import now
  rejects distinct nonempty identities and preserves both store and index.
- Real c8 output let an uncalled qualified function borrow a same-named free
  function's `FNDA`. Bare counters are used only for unique AST leaf names.
- JaCoCo default-package paths lost their module root. Cross-module duplicate
  source fixtures check correct resolution and ambiguous-fallback rejection.
- JaCoCo manifests could omit covered source hashes. Import now requires every
  resolved source digest; rejection preserves both store and index.
- Skipped pytest and XCTest cases returned success while leaving import-time
  coverage. Collectors now require exactly one successful, unskipped case.
  Real tests also cover repeat collection and stale-evidence removal.
- Python parameter IDs containing `::` were split incorrectly. A real pytest
  regression preserves parameter identity, including spaces and apostrophes.

## Runner validation

The real smoke harness collects twice, imports each export, verifies exact
per-test attribution, edits a production body, checks relevant-test selection,
and executes the selected command. Local modes cover pytest, SwiftPM, Vitest
JavaScript, Vitest TypeScript source maps, Jest, Node, Maven, Gradle, and Lua.
CI additionally runs Swift on Linux and macOS and explicitly runs the real
pytest/Swift isolation regressions. Failed smoke roots are retained for upload.

## Final local checks

- `cargo test --workspace`: 3,435 passed, zero failed; 55 BDD scenarios passed.
  Four opt-in tests are ignored by that command; the three real pytest/SwiftPM
  tests were explicitly run and passed separately.
- `cargo build --workspace`, workspace all-target Clippy with warnings denied,
  formatting, and diff whitespace checks passed.
- Blocking audit: zero blocked, 258 warnings; two binding-dependent script
  guards were unsupported by audit and skipped.
- Clean-profile LLVM instrumentation covered 4,814 of 5,142 lines (93.62%) in
  `src/coverage/`, from the MCP library tests, coverage integration tests, and
  the explicitly run real pytest/Swift tests. This includes compiled unit-test
  helpers in those source files; it is not a production-only percentage or
  a claim of repository-wide coverage. The attribution regressions and real
  runner checks provide behavioral evidence beyond this line metric.

## Limits

Ambiguous same-name one-line functions are conservatively unattributable until
positional function counters are supported. Non-Rust evidence is function-region
evidence; it does not establish branch coverage. The smoke fixtures validate
supported conventional layouts, not every custom build or dynamic-test shape.
Local runner results do not substitute for the new remote CI matrix.

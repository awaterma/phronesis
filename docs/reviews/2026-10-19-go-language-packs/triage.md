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

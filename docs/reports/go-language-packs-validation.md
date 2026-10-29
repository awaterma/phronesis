# Go language-pack validation

The Go starter pack and reusable pack infrastructure are implemented on
`feat/go-language-packs` in [draft PR #153](https://github.com/awaterma/phronesis/pull/153).
The approved design and implementation plan remain under `docs/superpowers/`.

## Local validation

The following gates passed on macOS at code revision `103e305` on 2026-10-25:

| Gate | Result |
|---|---|
| Workspace tests | 3,670 passed, 0 failed, 4 ignored |
| MCP tests, no default features with Rhai | 3,406 passed, 0 failed, 4 ignored |
| BDD | 55 scenarios passed in each full test run |
| No default features, with and without Go grammar, without Rhai | Focused repair tests passed |
| Rust 1.90 workspace all-target Clippy | Default and minimal configurations passed with warnings denied |
| Formatting | Passed |
| Independent behavior probes | 18 passed |
| Real-Go acceptance | 27 steps passed |

The repair checks cover demand-gated structural extraction, ordinary Go edits
without optional grammar support, bounded lexical audit with explicit
exclusions, strict failures for requested structural evidence, installed-pack
integrity, and consistent native symlink discovery and save handling.
Opaque script guards conservatively demand structural extraction because they
can inspect facts and compute predicate names.

## Limits and remaining acceptance

- These are local results at the named code revision; CI results are reported
  on the PR separately. Native Windows runtime validation remains outstanding.
- Go analysis is source-level. Unresolved calls remain unresolved; static
  selection is not dynamic coverage, and a Go coverage collector is deferred.
- Outcome recognition uses heuristics over tool output. The Verus model proves
  classification invariants over recognized evidence, not production pattern
  recognition; see `crates/phronesis-mcp/verification/RESULT.md`.
- Pack removal is descriptor-anchored on Unix. Other platforms refuse removal
  before changing files until equivalent protection is available.
- The self-audit had zero blocking hits and three additional nonblocking style
  advisories compared with the baseline.
- Owner playtesting and promotion remain separate. Keep the PR in draft;
  merging and release require the owner's decision.

## PR feedback follow-up

Pack-load failures now name the exact broken directory while native freshness
checks continue. Failed rebuilds preserve the previous graph and index rather
than deleting evidence from all packs. Unknown installed policy still fails
closed, and audit remains incomplete while retaining native findings.

Non-Unix recovery hints no longer advertise an unusable removal command. The
hook exception there permits only read-only pack diagnostics and explains that
recovery requires the project owner; Unix keeps the guarded CLI recovery path.
Both platform policies are exercised through the same policy function on macOS;
this does not establish native Windows runtime acceptance.

Follow-up local checks: 3,673 workspace tests and 3,409 minimal/Rhai tests
passed, with four ignored in each run and all 55 BDD scenarios passing.
Focused query-only tests without Rhai also passed. Regression tests cover a
broken pack beside a healthy pack and native source, preservation of cached
state on rebuild failure, continued detection of native drift, retained native
audit findings, and both recovery policies.

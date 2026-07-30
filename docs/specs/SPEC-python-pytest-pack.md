# SPEC: python-pytest pack — phase one implementation

**Status:** draft (revised after Codex review, 2026-07-18)
**Authors:** Claude, Andrew Waterman; reviewed by Codex
**Date:** 2026-07-18
**Target release:** phronesis-mcp 0.22.0 (MINOR — new opt-in pack)
**Parent spec:** SPEC-python-pack-expansion.md §`python-pytest` (candidate
rules) and §Roadmap (names python-pytest the likely best next pack)
**Affects:** `crates/phronesis-mcp/src/init.rs` (new `Pack` variant + rules),
`crates/phronesis-mcp/src/syntax/python.rs` (new extractors),
`crates/phronesis-mcp/src/syntax/facts.rs` (fact registration/emission),
`crates/phronesis-mcp/src/diff_extract.rs` (verification counting),
`crates/phronesis-mcp/src/hook/mod.rs` + `hook/pre.rs` (full-file
reconstruction seam, test-candidate paths),
`crates/phronesis-mcp/src/catalogue.rs` (pack registration),
`crates/phronesis-mcp/tests/` (integration), `docs/catalogue.html`

## Summary

Implement the `python-pytest` opt-in pack with **six executable rules**,
delivered on one feature branch in three internal milestones (syntax →
diff-aware → cross-file), released together as 0.22.0. The seventh candidate
rule from the parent spec (completion-claim gating on pytest outcomes) is
**deferred** — see Deferred work — because the current outcome signals carry
no toolchain identity, there is no changed-Python-subject fact at commit
time, and the confidence machinery is a separate opt-in the pack cannot
assume. Naming that machinery is part of this spec; building it is not.

The pack is opt-in (`phr-mcp init --packs llm,python,python-pytest`) and
rules 1–5 are scoped to test files (`test_*.py`, `*_test.py`, or under a
`tests/` directory); rule 6 is scoped to Python production changes. The base
`python` pack is not modified.

## Enforcement posture

**All warn in v1.** Per the parent spec's audit → warn → block maturity
policy, nothing blocks until audit runs over real suites have validated
false-positive rates. Promotion candidates are noted per rule.

| # | Rule id | Fires on | Level | Audit |
|---|---------|----------|-------|-------|
| 1 | `warn-pytest-swallowed-exception-in-test` | `except:` body is only `pass` inside a `test_*` fn | warn | yes |
| 2a | `warn-pytest-raises-broad-exception` | `pytest.raises(Exception\|BaseException)` | warn | yes |
| 2b | `audit-pytest-raises-without-detail-check` | `pytest.raises` with no `match=` and unused binding | log (audit-only) | yes |
| 3 | `warn-pytest-no-verification` | `def test_*` with no recognized verification | warn | yes |
| 4 | `warn-pytest-skip-reason` | `skip`/`xfail` with no reason argument | warn | yes |
| 5 | `warn-pytest-verification-removed` | recognized verification dropped in a surviving changed test | warn | no |
| 6 | `warn-pytest-missing-test` | new production function with `no_test_for` | warn | no |

Promotion candidates after field validation: rule 5 to block when ALL
verification vanishes with no replacement; rule 4 to block once wrapper/alias
recognition is proven.

## Shared recognizer: "recognized verification"

Rules 3 and 5 share one statement-level recognizer, implemented once:

- an `assert` statement;
- a call to `pytest.raises`, `pytest.warns`, or `pytest.fail`;
- a method call matching `.assert_*(` (unittest.mock snake_case) or a
  unittest `self.assert*`/`self.fail*` camelCase method;
- a call to a helper whose name starts with `assert_` (NOT `check_*` —
  too many false matches like `check_cache()`).

Scope: **the test function's own body only.** Statements inside nested
functions, lambdas, or classes do not count — a nested
`def assert_later(): assert False` is not executed verification. No
call-graph analysis in v1; this limitation is documented in the rule text.

`pytest.raises`/`warns`/`fail` are recognized by the exact dotted spelling,
or bare (`raises(...)`) only when the file contains a
`from pytest import raises`-style import. No alias table exists in the
analyzer; a project-local `raises()` without that import evidence is not
matched (documented limitation, revisit with shared import normalization per
the parent spec).

## Milestone 1 — syntax tier (rules 1–4)

Extractors in `syntax/python.rs` following the phase-one pattern (extractor
fn + `SyntaxFacts` field + `all_facts` registration + emission in
`facts.rs`):

- **`python_swallowed_exception_in_test`** `(file, fn, exception)` — an
  `except` handler (typed or bare) inside a `test_*` function whose body is
  only `pass`/comments/ellipsis. This is the narrow, per-handler core of the
  parent spec's "try/except where pytest.raises expresses the test": a
  swallowed exception in a test either hides a failure or reimplements
  `pytest.raises` badly. Arbitrary `try` statements (cleanup, fallback,
  multi-statement handlers) do NOT fire. Per-handler semantics: each
  qualifying handler emits one fact carrying its exception text.
- **`python_pytest_raises_broad`** `(file, fn)` — `pytest.raises` whose
  first argument is `Exception` or `BaseException`. Structurally clear,
  warns.
- **`python_pytest_raises_no_detail`** `(file, fn)` — audit-only sibling:
  `pytest.raises` with a specific exception type but no `match=` kwarg and
  a `with ... as e:` binding that is never referenced. Binding-reference
  semantics (lexical only, no name resolution): scan statements after the
  `with` in the same enclosing function body, including nested blocks of
  that body but not nested functions/lambdas/classes; any identifier
  reference (e.g. `e.value`, `str(e)`, `helper(e)`) counts; the alias
  declaration itself does not; rebinding ends the scan. `raises` without an
  `as`-binding and without `match=` also emits (type-only checking is valid
  and common — hence audit-only, not warn; the parent spec scopes this to
  "high-risk tests," a judgment audit output supports but hooks cannot).
- **`python_skip_without_reason`** `(file, fn, form)` — `pytest.skip(...)`
  and `pytest.xfail(...)` calls, and `@pytest.mark.skip` /
  `@pytest.mark.xfail` decorators, where no reason is supplied **either
  positionally or as `reason=`** (`pytest.skip("msg")` is canonical and
  must not fire). `form` distinguishes call vs decorator. Exempt in v1:
  `pytest.importorskip`, and `skipif`/`xfail` forms carrying a condition
  expression — a condition is when, not why, but condition-without-reason
  is common enough that flagging it waits for audit-informed promotion;
  the exemption is a documented v1 gap, not a semantic claim.

## Milestone 2 — diff tier (rule 5)

**Reconstruction seam (prerequisite).** `extract_old_content` /
`extract_new_content` return edit *fragments* (`old_string`, joined
MultiEdit strings), not file versions — fragment-level Python cannot be
parsed for per-test counts. Add to the pre-hook path: read the on-disk
pre-edit file; for `Edit`/`MultiEdit`, apply the edits in order to produce
the post-edit content; for `Write`, the payload already holds the full new
content. The seam produces `(old_full, new_full)` and lives beside the
existing diff-fact assertion so Rust diff facts are untouched. If the
on-disk file is missing or an edit fails to apply (stale `old_string`),
rule 5 silently skips — never guess.

**Fact.** `python_verification_removed(file, qualified_test, old_count,
new_count)`. Tests are keyed by **qualified identity** — the nesting path
of enclosing classes/functions joined with `::` plus the function name
(`TestCreate::test_invalid`), not the bare name, so same-named methods in
different classes cannot conflate. Counting uses the shared recognizer.
A test present in both versions whose count dropped emits the fact; a test
function deleted entirely does NOT (covered by `functions_removed`;
wholesale deletion is often legitimate). Warn text: state what dropped and
ask for the replacement verification or the rationale.

## Milestone 3 — cross-file tier (rule 6)

No new extraction: the pre-hook already asserts `no_test_for(fn)` for
newly-added functions. The pack rule joins explicitly —
`{"no_test_for": ["?fn"]}` **plus** `{"file_extension_is": "py"}` and a
production-path condition — so the unqualified function name cannot join
against non-Python facts.

**Test-search algorithm** (replaces today's Rust-centric candidates —
source file, siblings, root `tests/` only): candidates are (a) the source
file itself, (b) sibling `test_<stem>.py` / `<stem>_test.py`, (c) every
`test_*.py` under any `tests/` directory found walking from the file's
directory up to the project root, each `tests/` dir scanned recursively
with a file cap (500) to bound IO. Match remains name-containment in test
bodies. Known gaps, documented in the rule text: parametrized/class-based
tests that never name the production function literally, and the
transient case where the test arrives in a later edit of the same agent
operation — both acceptable for a warn, and why this rule must not be
promoted to block.

## Deferred work — rule 7 (pytest-verified completion)

Deferred to its own release. Named machinery it requires, confirmed absent:

1. **Toolchain-tagged signals** — `signal_pass(subject, "tests")` must
   also carry the producing toolchain id (`pytest`, `cargo`) so a Cargo
   run cannot satisfy a pytest gate in a polyglot repo.
2. **Changed-subject facts** — at commit pre-check no fact says "this
   work unit changed Python production code"; needs journey derivation
   linking edit records to the open subject.
3. **Pack dependency model** — confidence processing is dormant without
   `.phronesis/confidence.json`, and the pytest toolchain definition is
   scaffolded only by the `confidence` pack; `--packs python-pytest`
   alone would silently do nothing. Composition needs either declared
   pack dependencies or scaffolding by this pack, decided in that spec.

## Pack mechanics

New `Pack::PythonPytest` variant in `init.rs`: `parse_packs` accepts
`python-pytest`, `compose_packs` merges it (first-write-wins dedup),
label in init output. **Also**: register the pack in
`crates/phronesis-mcp/src/catalogue.rs`'s explicit pack list (its tests
included) — regenerating `docs/catalogue.html` alone is insufficient.

**Test-quality merge contract:** `assert_values_facts` merges stripped and
unstripped syntax facts; Python test-stripping currently returns content
unchanged, so the new facts survive only incidentally. Pin the contract:
the new pytest fact families are extracted from unstripped content by
design (they live IN tests), and the merge explicitly carries them from
the unstripped pass, with a regression test, so future Python
test-stripping cannot silently drop them.

## Testing

- Unit tests per extractor: positive, negative, malformed-source, plus —
  per Codex review — `async def test_*` fixtures for every extractor,
  decorated (`@pytest.mark.parametrize`, stacked marks), class-based
  tests/methods, positional and keyword reason arguments, nested-function
  non-verification, and unittest `self.assertEqual` recognition.
- Rule-5 diff cases: count drop, total removal, test deleted, test added,
  non-test changed, stale-edit skip, MultiEdit ordered application.
- Integration: each rule fires through the pre-hook; audit-eligible rules
  fire through `phr-mcp audit`; pack composition test for
  `--packs python,python-pytest`.
- Full gate between milestones and before merge: `cargo fmt --all
  --check`, `cargo test --workspace`, `cargo clippy --workspace -- -D
  warnings`, `git diff --check`.

## Non-goals

- No changes to the base `python` pack.
- No rule 7 machinery (deferred above).
- No import-alias normalization (shared-infrastructure item in parent
  spec); recognition limits documented instead.
- No `python-security` / `python-typing` / `python-architecture` work.
- Traceback-scope rules remain deferred per the parent spec.

## Consequences

- First pack combining syntax, diff, and cross-file facts.
- All-warn v1 trades enforcement teeth for a clean false-positive record,
  buying credibility for later block promotions.
- Parent spec §`python-pytest` gets a status update on merge, recording
  rule 7's deferral and the recognition limits above.

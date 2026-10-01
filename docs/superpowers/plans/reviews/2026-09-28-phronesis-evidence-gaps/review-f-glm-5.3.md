**Verdict: APPROVE WITH CHANGES — F2 as written cannot go green (C1) and fails the clippy gate (C2); F1/F3 need specified semantics before execution.**

**Format check (lcov vs llvm-cov JSON):** lcov is a sound breadth play — coverage.py, c8, gcov, and llvm-cov all emit it, and one-file-per-test preserves per-test attribution. But it is strictly poorer than llvm-cov JSON: only line hits (`DA`), no region spans (regions must be reconstructed from tree-sitter, which this plan does), no per-function counts unless the producer emits `FNDA` (coverage.py historically did *not* — must be verified), and `BRDA` branch data is optional/unreliable from Python producers — consistently deferred here. Acceptable for function-level evidence; the plan should say which producer features it depends on, because it currently parses `FNDA` it may never receive.

## Critical

- **C1 (F2): the hit rule contradicts F2's own test.** `save`'s site spans lines 4–5; the fixture has `DA:4,1` because coverage.py counts the `def` line as executed at *import time*. "Any `DA` with count>0 inside `[start_line, end_line]`" therefore marks `save` hit — so the assertion `!store.contains("::save\"")` can never pass against the given implementation. Worse, the rule as coded means every test that merely imports a module "reaches" every function in it — exactly the contamination SPEC-coverage-evidence forbids. **Fix:** define a hit as a body line executed — use the tree-sitter `body` field's first line through `end_line`, excluding the `def`/signature line; for one-liner `def f(): return x` (body shares the def line) fall back to `FNDA` when present, else over-attribute and document. Update fixture and doc comment to match.
- **C2 (F2): `LcovSource.function_hits` is written but never read** — rustc's `field is never read` dead-code warning fires and `clippy -- -D warnings` fails the PR gate as written. **Fix:** actually consume `FNDA` (prefer it over the body-DA heuristic; note lcov `FN` names are unqualified, so you still need site ranges) or delete the field.

## Major

- **M1 (F2/F5): silent drops violate the global "never turn a data error into empty; report it."** `relativize` → `None` and `!is_wanted_source` both `continue` unaccounted; only no-site files are counted. The CHANGELOG claims test files/`conftest.py` "are reported as skipped" — false as coded. **Fix:** count unmatched-SF and filtered-SF in the summary, print first 5 of each.
- **M2 (F2): longest-existing-suffix relativization can attribute hits to the wrong file** — e.g. a site-packages or vendored copy whose tail coincides with a repo path, or a bare `store.py` at root; relative SF containing `..` also escapes the governed root. **Fix:** require candidates to appear in the graph's file set, log every SF→rel mapping to stderr, reject normalized paths outside root, and sanity-check max `DA` line ≤ host source line count (this also catches container/host drift).
- **M3 (F2/F3): the host stamps the revision, but lines describe the container's tree.** Any drift shifts region spans and fabricates evidence. **Fix:** have the emitted script write a manifest (container `git rev-parse HEAD`, per-file digests) that `import` cross-checks before accepting records.
- **M4 (F3): parametrized tests break the file-stem id scheme.** `test_a[p1]` yields stems with `[`/`]`; the plan drops `TN:` precisely where it's needed. **Fix:** always write `TN:<graph-test-id>` into each lcov (also fixes Windows-host-hostile `:`-containing stems), sanitize stems as fallback, and report node ids `graph_test_id` cannot map.
- **M5 (F3): the pytest-running paths are untested.** The shown failing test only exercises string mapping and script text; CI may lack Python. **Fix:** unit-test `pytest_node_ids` against a committed `--collect-only -q` fixture (warnings, `N tests collected` trailer), gate live-invocation tests on Python's presence, and make the script check `python -m coverage` exists.
- **M6 (F1): async defs may get zero regions.** Depending on the vendored tree-sitter-python grammar, async functions are `async_function_definition`, which the walker (matching only `function_definition`) skips — silently. **Fix:** match both kinds; add an `async def` case to the failing test.
- **M7 (ordering): F2 Step 4 runs the BDD `coverage-evidence.feature` runner, but no task adds a Python scenario**, and nothing validates `coverage select` + confidence end-to-end for `python:` ids. **Fix:** add the scenario in F2 or F5.
- **M8 (F2): `records_from_lcov_dir` re-reads and re-parses each source per SF per test** — O(tests × files) tree-sitter parses. **Fix:** memoize sites per rel path.

## Minor

- **m1 (F1):** interface says `.py` must be "not under `tests/`" but the `is_wanted_source` snippet drops that check, leaning wholly on the classifier; align code and spec, and cover `setup.py`/root-level `test_*.py` in the test.
- **m2 (F1):** `assign_ordinals(raw)` takes tuples but must return `Vec<FunctionSite>`; and the "adjust expected end lines after checking one real file" hedge leaves the test's truth undetermined — pin `end_position()` behavior (trailing newline) against one real parse before writing the test, not after.
- **m3 (F2):** reruns produce duplicate `(test, region, revision)` records; define a dedupe/union policy. Also `DA` count-parse errors lose line-number context, and the script never cleans `<n>.cov` intermediates.
- **m4 (F4):** "wherever the hook derives changed_region" + "grep" is the weakest file targeting in the plan; name the module and the existing Rust test to mirror.

## Nit

- **n1 (F1):** nested `def` (`Store::load::inner`) can collide with a class named `load` holding method `inner` — same region id; document or disambiguate.
- **n2 (F2):** `for i in 0..comps.len()` with `comps[i..]` risks `clippy::needless_range_loop`; `rsplit('/').next().unwrap_or(rel)` — `next()` is always `Some`, drop the `unwrap_or`.
- **n3 (F1):** sites start at the `def` line, excluding decorators — fine once C1 makes hits body-only, but say so.

## Questions Part F must answer before execution

1. Does the pinned coverage.py version emit `FN`/`FNDA`/`TN` in `coverage lcov`, and are `SF` paths absolute? Commit a real output fixture; the current test hand-writes lcov.
2. Is a hit "function body executed" or "any line in the def span"? C1 forces this decision before the test is written.
3. Which tree-sitter-python grammar version ships, and does it fold `async def` into `function_definition`? Where exactly does `end_position()` land?
4. What guarantees container lcov lines match host HEAD at the stamped revision — digests, clean-tree check in the script, or both?
5. How are parametrized and class-qualified node ids represented, and where are unmappable ids reported?
6. Where are unrelativizable and filtered-out `SF` paths surfaced (summary fields, exit codes)?
7. Is Python/pytest on CI's PATH; which tests skip when absent, and does `--emit-script` get any test at all?
8. Does the evidence store dedupe identical `(test, region, revision)` records?
9. Does `coverage select`'s confidence math handle `python:` ids end-to-end, with a BDD scenario proving it?
10. Is `qualified_test_id`'s module separator `.` or `::`? The F3 test accepts both — tighten before implementing, not after.
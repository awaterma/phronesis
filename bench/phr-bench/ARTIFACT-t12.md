# ARTIFACT — Task 12: Report (self-contained HTML deliverable)

Worker: w-t12b-glmflash · Branch: bench/t12b · Worktree: /Volumes/Data/Git/phronesis-wt/bench-t12b

## What shipped

`bench/phr-bench/src/report.rs` — the Task 12 renderer plus its disk-side helpers:

- `SECTION_IDS: [&str; 7]` — `section-headline`, `section-tasks`, `section-friction`,
  `section-efficiency`, `section-governance`, `section-interpretation`, `section-caveats`, in
  spec DoD order.
- `render(agg, records)` and `render_with_extras(agg, records, &ReportExtras)` — full HTML
  document, inline CSS only, zero external references. Data-derived free text (exit reasons,
  notes, run id) is HTML-escaped and scheme-stripped (`http://`/`https://` neutralized) so the
  self-containment scan can never trip on record contents.
- Deterministic: BTreeMap iteration everywhere, no timestamps, no wall-clock reads; same inputs
  render byte-identical HTML.
- Loaders/mergers: `load_records` (runs/<instance>/<arm>/record.json, sorted, stray dirs skipped),
  `merge_quality` (quality.json `"instance/arm"` keys fill missing audits only), `verified_arms`
  (verify.json, sorted), `extract_note` (treatment transcript final assistant message,
  whitespace-collapsed, capped at 240 chars).

`bench/phr-bench/src/main.rs` — `report --run-id <id> [--out <path>]` wiring: loads records,
optionally folds in verify.json/quality.json, aggregates (loud fail on missing-governance /
not-wired tombstones), renders, writes the HTML (default `bench/report/index.html`) plus
`bench/report/data/aggregate.json` and `records.json` companions next to it.

## Section content (all computed, nothing projected)

1. **Headline** — resolved rate both arms with delta, exact sign-test p (`p = 0.021`-style, or
   `n/a (no discordant pairs)`), debt delta (audit violations, treatment − control), cap hits per
   arm, run counts; per-language resolved + mean-turns table from `Aggregate.by_language`.
2. **Per-task paired breakdown** — one row per pair: id, language, per-arm exit / resolved /
   turns / tokens (in/out, `n/r` when unreported) / wall-clock / audit violations / blocks-warns;
   discordant rows carry `class="discordant"` and a `<details>` note when one was threaded.
3. **Friction** — per-rule block/warn sums across treatment records, fail-closed total.
4. **Efficiency** — mean/median turns, mean tokens in/out (where reported), mean/median
   wall-clock per arm; per-task treatment−control wall-clock table.
5. **Governance behavior** — total blocks/warns, deflection-rule blocks, block→recovery rate
   (share of blocked tasks that ended completed), fail-closed, and the loud-contract statement.
6. **Interpretation** — bullets computed only from the tables above (resolved delta, sign test,
   median/mean turn delta, debt delta, hook overhead, recovery, effect concentration).
7. **Caveats & reproducibility** — pilot-lite scope (15 of 43 Rust instances), k=1, single model,
   pilot status, benchmark contamination, token-fallback count from the data, and a manifest
   summary (tasks, records, verified arms, run id).

## TDD trail (red-first visible)

| Commit | Meaning |
|---|---|
| `bee2c1b` `test(bench): failing report tests` | 17 tests failing against the stub |
| `8af83ec` `feat(bench): deterministic self-contained HTML report` | renderer + helpers, all 17 pass |
| `6745d00` `feat(bench): report subcommand wiring` | CLI wiring |

Tests in `bench/phr-bench/tests/report.rs`: seven sections in order; no external references
(including URL-bearing record data and notes); `n/r` tokens and discordant flagging;
byte-identical re-render; no timestamps (YYYY-MM-DD scan); headline numbers (`p = 1.000`, debt
delta, per-language means); `n/a` sign test with zero discordant pairs; escaped note in
`<details>`; per-rule friction + fail-closed; governance recovery `1 of 1`; efficiency
medians + per-task delta; pilot-scope caveats + token fallback; `load_records` sorting/skipping;
`merge_quality` fill-only-missing; `verified_arms` parsing; `extract_note` selection/truncation;
empty-governance no-panic rendering.

## Verification

- `cargo test --manifest-path bench/phr-bench/Cargo.toml` — **62 passed** (45 pre-existing + 17 new), 0 failed.
- `cargo clippy --manifest-path bench/phr-bench/Cargo.toml --all-targets -- -D warnings` — clean.
- `cargo fmt` applied to touched files.
- Live smoke (synthetic run dir, deleted afterward): all seven sections present, self-containment
  grep clean (`https?://|<script|<link|src=`), data companions written, note text rendered;
  removing the treatment record's governance made `report` exit 1 naming the instance — the loud
  path works.
- Nothing outside `bench/` was modified; the live pilot's `bench/results/pilot-lite-20261002/`
  in the main checkout was not touched.

## Notes / handoff

- `report::sorted_child_dirs` is private; `main.rs` collects instance dirs into a BTreeMap itself.
- Deflection-rule detection is the `"deflect"` substring in rule ids (llm-pack ids match); the
  full per-rule friction table in section 3 carries the ground truth regardless.
- Deviation from plan text: the plan suggested threading notes as a bare
  `BTreeMap<String, String>` parameter; I kept `render(agg, records)` exactly as pinned and put
  notes plus run-id/verified-arms in `ReportExtras` so the default-argument behavior stays.
- The report intentionally renders only from `Aggregate` + records; manifest-level metadata
  (dataset id/revision, seed) is not in those types, so the caveats section states run scope,
  verified arms, and run id from what the record set actually carries.
# ARTIFACT — Task 13: Corpus (dataset load, slice, manifest build)

Worker: w-t13b-sonnet (reconciliation + wiring), repairing unverified,
uncommitted work left by w-t13-codex (killed mid-task).

## What holds

- `bench/phr-bench/src/corpus.rs` — `Slice::{Pilot, Full}` and
  `build_manifest(dataset_jsonl, slice, seed, dataset) -> Result<Manifest>`:
  - Parses the real `SWE-bench_Multilingual` columns per
    `bench/scripts/SPIKE-FINDINGS.md` (`log_parser`, `problem_statement`,
    `repo` as `owner/name`, `FAIL_TO_PASS`/`PASS_TO_PASS` as real lists, no
    `language` column).
  - Normalizes: `FAIL_TO_PASS` → `fail_to_pass`, `PASS_TO_PASS` →
    `pass_to_pass`, `problem_statement` → `issue_text`, `repo` →
    `https://github.com/<owner>/<name>.git`.
  - Language inference: `log_parser == "parse_log_cargo"` ⇒ `rust`;
    otherwise an `instance_id` prefix match (`<language>__...`), falling
    back to a repo-owner map, falling back to `"unknown"`.
  - Pilot slice (target 30) and Full slice (target 100): all `rust`
    instances are always included, then the remaining languages are
    filled proportionally (largest-remainder apportionment) up to the
    target, using `rand::rngs::StdRng::seed_from_u64(seed)` to shuffle each
    language group before taking its quota — deterministic for a fixed
    seed, and capped at whatever the dataset actually has (the fixture has
    only 40 rows total, so both the "pilot" and "full" fixture tests see
    fewer than their nominal targets).
  - `tasks` sorted by `instance_id`; `prompt_hash` set from
    `prompt::template_hash()`; the built manifest is round-tripped through
    `serde_json` as a load-bearing assertion (fails loudly if the type
    can't deserialize its own output).
- `bench/phr-bench/tests/corpus.rs` — 5 tests, all green:
  `pilot_is_all_rust_plus_seeded_proportional_fill`,
  `same_seed_produces_same_manifest_bytes`,
  `packs_and_prompt_hash_are_recorded`,
  `normalizes_real_dataset_columns_and_sorts`,
  `full_slice_caps_at_one_hundred_and_includes_all_rust`.
- `bench/phr-bench/tests/testdata/dataset-slice.jsonl` — 10 rust + 30
  python rows, real-shaped columns, unique `instance_id`s (`r-01..r-10`,
  `p-01..p-30`), `r-01`/`r-03`/`r-05`/… mapped to `rust-lang/rust` and
  `r-02`/`r-04`/… to `tokio-rs/tokio` (both real rust-pack repos, chosen
  arbitrarily by the fixture's author — the plan does not pin a specific
  repo for any single fixture row).
- `bench/phr-bench/src/main.rs` — `corpus` subcommand: `--slice
  pilot|full --seed <u64> --out <path>`. Resolves the project root via
  `git rev-parse --show-toplevel`, invokes `bench/.venv/bin/python -c
  "<shim>"` where the shim runs `datasets.load_dataset("SWE-bench/SWE-bench_Multilingual",
  split="test", revision="846e647b9f33c0b51b739d005d13d85493c9af09")` and
  prints one JSON object per row, feeds that JSONL to
  `corpus::build_manifest`, and writes the pretty-printed manifest JSON to
  `--out` (creating parent directories as needed). Dataset id/revision are
  the ones pinned in `bench/scripts/SPIKE-FINDINGS.md`.

## Test counts

`cargo test --manifest-path bench/phr-bench/Cargo.toml`: **17 passed, 0
failed** — `contracts.rs` (4), `corpus.rs` (5, including the fix), `prompt.rs`
(4), `transcript.rs` (4), plus 0 unit tests in `lib.rs`/`main.rs` and 0 doc
tests. (The brief's "18 existing" was off by one against what's actually on
this branch; 12 pre-existing + 5 corpus = 17 is what `tests/` contains.)

`cargo clippy --manifest-path bench/phr-bench/Cargo.toml --all-targets -- -D
warnings`: clean, zero warnings.

No `.unwrap()` in `src/` (verified with `grep -rn '\.unwrap()' src/`); all
fallible paths use `anyhow::{Context, Result, bail}`. `tests/corpus.rs` uses
`.expect(...)` with descriptive messages, consistent with the rest of the
suite's test code (not production code).

## What I changed from the prior worker's (w-t13-codex) work, and why

1. **Fixed the one failing test**
   (`normalizes_real_dataset_columns_and_sorts`): it asserted
   `task.repo == "https://github.com/tokio-rs/tokio.git"` for instance
   `r-01`, but the fixture generator encodes `r-01`'s `repo` as
   `rust-lang/rust`. I checked the plan's Task 13 section (line
   1302–1309): the fixture's row shape is illustrative
   (`https://github.com/a/r`-style placeholders), not a real-data pin —
   nothing in the plan or `SPIKE-FINDINGS.md` requires `r-01` specifically
   to map to `tokio-rs/tokio`. The fixture itself (both `rust-lang/rust`
   and `tokio-rs/tokio` appear across the 10 rust rows, both real rust-pack
   repos per `SPIKE-FINDINGS.md`'s repo list) is internally consistent and
   exercises the real normalization logic correctly. The discrepancy was
   purely in the test's expected value, so I corrected the assertion to
   match what the fixture actually encodes, rather than changing the
   fixture (which is also exercised by three other tests and the pilot/full
   slicing counts) to match a wrong assertion.
2. **Reformatted only the files I touched** (`corpus.rs` via `cargo fmt`)
   — the prior worker's `corpus.rs` had several lines that didn't match
   this toolchain's `rustfmt` output. Running `cargo fmt` on the whole
   crate also reformatted unrelated, already-committed files from earlier
   tasks (`manifest.rs`, `record.rs`, `telemetry.rs`,
   `tests/{prompt,transcript}.rs`) due to a rustfmt version/config drift
   that predates this task; I reverted those to keep this task's diff
   scoped to Task 13's files only, per the brief ("nothing outside bench/
   modified" / don't touch other tasks' work).
3. **Wired the `corpus` subcommand** in `src/main.rs` (previously an empty
   `Commands` enum, since Task 13 is the first task to add a subcommand):
   `clap::Subcommand`/`ValueEnum` derive for `--slice pilot|full --seed
   --out`, the python-shim dataset loader, and the manifest writer. This
   path is intentionally not covered by automated tests (per the task
   brief and the plan's Task 13 interface note); I smoke-tested the CLI
   parsing (`--help`, an invalid `--slice` value correctly rejected) and
   confirmed the shim invocation fails cleanly with a clear error when
   `bench/.venv` doesn't exist on this machine (expected — the venv is
   provisioned per-environment, not committed).
4. **Commit history**: rebuilt as red-then-green rather than committing the
   prior worker's finished state directly — `test(bench): failing corpus
   tests` (fixture + the corrected test file, against the still-stub
   `corpus.rs`, confirmed non-compiling) then `feat(bench): corpus slicing
   with seeded deterministic manifests` (the implementation, confirmed all
   5 tests green), then `feat(bench): wire corpus subcommand in main.rs`
   for my own addition. Each commit trailer credits `w-t13-codex` and/or
   `w-t13b-sonnet` per who authored that piece.

## Deviations from the plan

- The plan's pilot/full targets (30/100) assume a dataset large enough to
  fill them; the committed fixture only has 40 rows total (10 rust + 30
  python), so `full_slice_caps_at_one_hundred_and_includes_all_rust`
  asserts 40 tasks, not 100 — this was already the shape of the prior
  worker's test and fixture, and matches `build_manifest`'s documented
  behavior of capping at whatever's available rather than erroring when
  the dataset is smaller than the slice target. The real pinned dataset
  (300 instances) is large enough that this cap won't bind in practice.
- No other deviations from the plan's Task 13 section.

# SPEC: Phronesis A/B benchmark (model ± governance on SWE-bench Multilingual)

**Status:** design, approved 2026-10-01
**Type:** experiment infrastructure (standalone, not a shipped crate)
**Lives in:** `bench/` (new top-level directory, excluded from the workspace)
**Reads from:** SWE-bench Multilingual (HuggingFace), `phr-mcp` CLI, Claude Code headless
**Does not touch:** any crate under `crates/`

## Summary

Measure what phronesis actually does to a coding agent: run the same model,
through the same harness, on the same tasks, twice — once in a bare repo
clone (**control**), once in a clone with `phr-mcp init` governance active
(**treatment**) — and compare outcomes.

Primary question: **does governance change task success, code quality, and
agent efficiency — and in which direction?** The deliverable is a
publishable comparison (README/blog-grade evidence, not a paper): resolved
rate, residual rule debt, turns/tokens/wall-clock, and per-rule friction,
reported per task as paired deltas.

## Purpose and non-goals

**Purpose.** Evidence for phronesis's value. Credible, reproducible,
externally comparable numbers at moderate cost.

**Non-goals (explicitly out of scope for this round):**

- Paper-grade statistical rigor (k>1 sampling on the full slice,
  confidence intervals, multi-seed sweeps). A paired design with k=1 is the
  accepted compromise; k=3 on a subset is a future knob.
- Separating *why* phronesis helps (enforcement vs. context injection vs.
  deflection). "Phronesis as deployed" is one bundle; a third arm that
  isolates enforcement-only is a future knob.
- Training-data production, leaderboards, or CI integration.
- Modifying phronesis itself. The benchmark consumes the installed binary.

## Approaches considered

**A. Thin paired runner on official eval (chosen).** We build only the
experiment logic — arm preparation, run driving, telemetry, aggregation.
Verification reuses SWE-bench's official Docker evaluation harness
(FAIL_TO_PASS + PASS_TO_PASS), so "resolved" means exactly what the public
leaderboard means.

**B. Reuse a leaderboard agent (mini-SWE-agent, SWE-agent) — rejected.**
These drive the model API directly. Phronesis acts as hooks inside Claude
Code / Codex / Gemini CLI; a custom Python agent never passes through them,
so the treatment arm is impossible by construction.

**C. Swarm/herdr orchestration — deferred.** Built for collaborative dev
under budget/deadline, not reproducible benchmark isolation (shared
workspace, worker variance). Candidate for scale-out if Phase 2 needs 200+
runs parallelized; not the measurement path.

## Experiment protocol

| Aspect | Choice |
|---|---|
| Suite | SWE-bench Multilingual (300 instances, 42 repos, 9 languages; official SWE-bench format and Docker eval) |
| Slice | **All Rust instances** + stratified random sample of the rest (seeded, incl. Python — the two languages with the deepest phronesis packs), sized to ≈100 tasks; exact membership and seed recorded in the run manifest |
| Arms | **Control**: bare clone at instance base commit. **Treatment**: same, + `phr-mcp init --packs <lang-mapped>` |
| Harness | Claude Code headless: `claude -p --output-format stream-json` |
| Model | `glm-5.3:cloud` via an Anthropic-API-compatible router (both arms, identical router and params) |
| Samples | k=1 per arm per task (paired); k=3 subset deferred |
| Verification | Official SWE-bench Multilingual harness: patch applied, FAIL_TO_PASS must pass, PASS_TO_PASS must stay green |
| Caps | Identical in both arms: max turns **100**, wall-clock **45 min** per run, token budget if the router reports usage. A cap hit is a task failure, not a retry. Defaults may be revised after Phase 0 measurements; whatever is used is recorded in the manifest. |

### Language → pack map (treatment arm)

| Task language | Packs |
|---|---|
| Rust | `llm,rust` |
| Python | `llm,python` |
| TypeScript / JavaScript | `llm,typescript` |
| Others (Go, Java, C, Ruby, PHP — whatever the dataset contains beyond the rows above) | `llm` (deflection + lifecycle context still active) |

The map keys are dataset languages (Phase 0 pins the dataset id and
revision); any language without a phronesis pack falls to `llm` alone.

The map is part of the manifest; a task's language comes from the dataset.

### Fairness rules

1. **Identical prompt template in both arms.** Issue text + standard
   completion instruction. Neither arm mentions phronesis; the treatment
   arm experiences governance only through the hooks themselves — that is
   the product behavior being measured.
2. **Identical clone state at start.** Fresh clone per task per arm; no
   shared `.phronesis/`, no shared git history, no leftovers.
3. **Identical caps.** If governance friction causes a cap hit, that
   outcome counts — it is a real cost of governance, not an artifact.
4. **Paired analysis.** Both arms run the same task; per-task deltas are
   the unit of comparison, which controls task difficulty.

### Isolation and reproducibility

- Fresh clone per run; `.phronesis/` state (log, journey, context) is
  per-clone and discarded after telemetry extraction.
- The manifest records: dataset revision (HF commit), phr-mcp version
  (`cargo install --path crates/phronesis-mcp` before any run; version
  pinned in manifest), Claude Code version, router endpoint/model string,
  model params, prompt template hash, slice seed.
- All run artifacts (transcripts, diffs, logs) land under
  `bench/results/<run-id>/` (gitignored); the report and manifest are
  committed.

## Components

A standalone Rust crate at `bench/phr-bench/` with its own `Cargo.toml`
and an empty `[workspace]` table — **excluded from the main workspace** so
the release surface of the shipped crates is untouched. Directory layout:

```
bench/
├── phr-bench/          # Rust CLI: the orchestrator (standalone crate)
├── tasks/              # cached dataset manifests (gitignored)
├── results/            # per-run artifacts (gitignored)
└── report/             # committed reports + manifests
```

Crate modules:

| Module | Responsibility |
|---|---|
| `corpus` | Load SWE-bench Multilingual from HuggingFace (via `hf` CLI or `datasets` download), filter/slice with the recorded seed, emit a task manifest |
| `arms` | Per task per arm: fresh clone at base commit; treatment arm additionally runs `phr-mcp init --packs <map>`; verify expected files exist (treatment) and don't (control) |
| `runner` | Drive `claude -p` headless with the prompt template, capture stream-json transcript, enforce caps |
| `verify` | Extract the final diff, invoke the official SWE-bench Docker harness, parse resolved/unresolved |
| `quality` | Post-run: stage a rules-only `.phronesis/rules.json` in **both** arms' clones and run `phr-mcp audit --json` — residual violation counts per rule (the symmetric quality metric) |
| `telemetry` | Parse transcripts (turns, tool calls, token usage if reported, wall-clock) and treatment-arm `log.jsonl`/`phr-mcp stats` (blocks, warns, rule ids, fail-closed events) |
| `aggregate` | Paired per-task report: resolved-rate delta, sign test over discordant pairs, audit-debt delta, turns/tokens/time deltas, per-rule friction table, per-language breakdown; markdown + JSON output |

`verify` shells out to the official Python harness; `quality` shells out to
the installed `phr-mcp`. The crate never reimplements either.

## Metrics

**Primary — success.** Resolved rate per arm (overall + per language), and
paired per-task delta. Discordant pairs feed a sign test; the report shows
resolved-by-arm-only and regressed-by-treatment tasks explicitly (the pair
that matters most for the "governance helps or hurts" question).

**Quality — symmetric.** `phr-mcp audit` with the *same* rules.json against
both arms' final trees. Reported as per-rule residual violation counts and
aggregate debt delta. Works identically on control trees because audit
needs only the rules file, never the hooks.

**Friction — treatment arm.**

- Blocks and warns per task, per rule (`log.jsonl`, `phr-mcp stats`).
- Extra turns / tokens / wall-clock vs control (context injection and
  block-recovery both cost; measured, not assumed).
- Fail-closed events (hook could not evaluate and blocked).
- **False-positive block rate:** every blocked edit is collected; in the
  pilot each is reviewed against a rubric (was the rule's intent actually
  violated? did the agent recover productively?). Phase 2 samples.

**Governance behavior.** Deflection-rule hits (unverified completion
claims), and block→recovery trajectories (did the re-edit pass the hook
and improve the audit result?).

**Efficiency.** Turns, tokens (if the router reports usage in stream-json;
if not, turns + wall-clock are the fallback and the report says so),
wall-clock including hook latency.

## Confounds and mitigations

| Confound | Handling |
|---|---|
| Router/model behavior with Claude Code tool-calling | Phase 0 validates before any spend; same router both arms |
| Token usage unreported by router | Fallback metric (turns, wall-clock); stated in report, not silently dropped |
| Hooks silent in headless mode | Phase 0 proves a pre-check block fires in `claude -p` and lands in `log.jsonl` |
| Benchmark contamination (model may have seen these public PRs) | Paired design mitigates between-arm comparison; absolute resolved rates may be inflated vs. novel tasks — caveat in the report, never a headline claim |
| Docker x86 images on Apple Silicon (ruff builds are heavy) | Phase 0 measures eval time on one Rust instance; an x86 eval box is the fallback if emulation is impractical |
| Agent stochasticity | k=1 accepted for the paired delta; k=3 subset is the follow-up knob |
| Cap hits biased toward one arm | Identical caps; cap hits reported as outcomes, counted in both directions |
| Treatment-arm context injection consuming tokens | That is a real cost of governance; it appears in the friction metrics rather than being normalized away |

## Phasing

**Phase 0 — spike (throwaway scripts, no crate code).**
1. Router wiring: `claude -p` → router → `glm-5.3:cloud`, a tool-using task
   (create a file) succeeds; transcript is complete stream-json.
2. Hooks in headless: temp clone + `phr-mcp init --packs llm,rust`; agent
   attempts a `.unwrap()` edit; pre-check blocks (exit 2 in `log.jsonl`);
   SessionStart/UserPromptSubmit injections appear in the transcript.
3. One SWE-bench Multilingual instance end-to-end in the control arm,
   scored by the official Docker harness on this Mac. This step also pins
   the exact HuggingFace dataset id and revision used everywhere after.

Exit criteria: all three pass, and per-run wall-clock + eval time are
measured well enough to budget Phase 1. Anything built here is labeled
throwaway.

**Phase 1 — pilot.** Pilot slice: Rust instances first, then a seeded
Python sample to reach **30 tasks**, both arms, k=1 (60 runs).
Produces: sanity of the full pipeline, real cost/time numbers, the first
paired-delta table, and a manual false-positive review of every blocked
edit. **Decision gate:** the Phase 2 slice size and budget are chosen from
pilot data (recorded in the manifest), or the design is revised if friction
or cost is out of range.

**Phase 2 — full run + report.** ≈100-task slice (all Rust + seeded
stratified sample), both arms. Deliverable: `bench/report/` markdown report
(per-task paired table, deltas, sign test, per-rule friction, per-language
breakdown, confound caveats) + JSON, suitable for README/blog citation.

## Testing approach

- **Unit:** manifest parsing (fixture instance JSON), prompt rendering
  (deterministic, hash recorded), arm prep (temp git clones — assert
  treatment has `.claude/settings.json` + `.phronesis/rules.json`, control
  has neither), transcript parsing (fixture stream-json), aggregation
  (synthetic paired results → expected sign-test output).
- **Integration:** Phase 0 spike scenarios as scripts; the Phase 1 pilot is
  the end-to-end test of the crate itself.
- **No benchmark artifacts committed:** results are gitignored; reports
  and manifests are committed with the seed and versions needed to
  reproduce them.

## Future knobs (out of scope, listed so they are not re-litigated)

- Third arm: enforcement-only (no context injection) vs. bundle.
- k=3 sampling across the full slice; confidence intervals.
- Extension suites: Rust-SWE-bench (depth on the rust pack), Multi-SWE-bench
  flash (breadth), SWE-Bench ProMax (refactoring-shaped tasks).
- Multi-model comparison (who benefits most from governance).
- Swarm scale-out for 200+ runs.
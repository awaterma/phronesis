# Evidence-gaps swarm: retrospective (2026-09-29)

Execution record for `2026-09-28-phronesis-evidence-gaps.md`. Six parts, six draft PRs, each built by one worker and reviewed by a different model profile; every repair independently re-verified. Copied from the swarm's durable ledger directory at close (`CLOSE.md` and `JUDGEMENT.md`); the raw ledger, bids, briefs, artifacts, and per-part Phronesis journals stay outside the repository.

## Close report (2026-09-29 ~08:40 UTC)

Status: resolved `done`. Spend $7.33 of $60 (1.29 M tokens recorded); 8 h limit not reached (started ~05:00 UTC). 23 worker spawns, cap 3.

## Deliverables (all draft PRs, CI green where the repo has CI; the human merges)
| Part | PR | Built by | Reviewed by | Repaired by | Re-review |
|---|---|---|---|---|---|
| A .phronesisignore lexical-only | awaterma/phronesis #118 | crush-glm-5.2 | codex (REJECT) | codex (failed), crush-glm-5.2 (provider died), together GLM-Flash (landed edfbb21) | haiku FIXED |
| B worktree commit attribution | #117 | crush-glm-5.2 | codex (Major) | crush-glm-5.2 (d0993c4) | haiku FIXED |
| C redirected output + signal ingest | #116 | codex | crush-glm-5.2 (APPROVE WITH NOTES, 1 Major) | codex (5351695) | haiku FIXED |
| D swarmctl seed-tracked | awaterma/agent-skills #1 (no CI) | crush-glm-5.2 | codex (REJECT) | together GLM-Flash (8be4e8a) | haiku FIXED |
| E Kani outcomes seam | #119 | crush-glm-5.2 | codex (APPROVE WITH NOTES) | — | — |
| F Python coverage via lcov | #120 | codex | haiku (APPROVE WITH NOTES) | — | — |

Merge order that the reviews assume: C before E (E3's `signal ingest` e2e needs Part C), then reinstall the binary (`cargo install --path crates/phronesis-mcp`) before the next swarm so the fixes measure themselves.

## Judgement (JUDGEMENT.md)
1 of 6 PRs shipped with evidence Phronesis recorded itself (C). Baseline 0 of 3. Measured with the pre-fix installed binary; see the results table for per-PR conditions.

## Incidents on the ledger
- Anthropic-only first auction (user correction) → superseded tasks build-a..f, all-model market, 2x Anthropic comparison.
- ollama cloud session limit ~06:47 UTC killed crush-glm-5.2 on a-repair and e-build (E had already pushed).
- together.ai quotes vs metered: Flash $0 → $2.04 (d-repair), $0.03 → $0.82 (a-repair).
- codex: a-repair misdiagnosis (zero-hit JSON read as a hang); sandbox cannot write outside the worktree, take ledger locks, or run confinement-gated tests; f-build push withheld until the orchestrator verified gates outside the sandbox.
- D worker created the private GitHub repo awaterma/agent-skills without being asked (verified private, no secrets) — user decision pending.
- Pre-existing flaky CI test `action_log::tests::a_limited_read_does_not_scale_with_log_size` (timing budget) failed once on #119; rerun passed.

## Grades written to profiles (agent-skills main checkout, uncommitted)
codex eng 8 (hold), arch 10 (hold); crush-glm-5.2 eng 8→7; crush-together-glm-flash eng 10→9 (first real evidence); haiku arch 9 (hold), eng 8 (hold).

## Left for the human
- Merge/promote the six drafts (playtest first per your workflow); C before E.
- Worktrees eg-a/b/c/e/f (phronesis-wt) and eg-d (agent-skills-wt): keep until merged, then `git worktree remove`. Older worktrees VF, VS1, VT2B, VT6B (phronesis) and T1–T5 (agent-skills) predate this swarm: your call.
- phronesis main has your uncommitted edits (properties.json, two specs, an ADR) plus the untracked plan `docs/superpowers/plans/2026-09-28-phronesis-evidence-gaps.md` and `docs/superpowers/plans/reviews/` from this work: commit or discard.
- `.scratch/HANDOFF-godsplit-swarm.md` holds a dead swarm token in plaintext: delete.
- agent-skills main has uncommitted profile grades (this swarm) and older uncommitted edits to main.rs/mcp.rs/ops.rs/state.rs/tests that are not from this swarm.
- Decide on the worker-created repo awaterma/agent-skills (PR #1 lives there).

## Judgement criterion (set 2026-09-29, before any PR landed)

The user's question: as Phronesis is built by governed swarms, is it working as planned?
The agreed number: **how many of the six PRs ship with evidence Phronesis itself recorded, rather than evidence the orchestrator had to go fetch.**

### Scoring, per PR (checked in the part's own worktree, read-only, before anyone runs a gate by hand)

A PR scores **recorded** only if all three hold, produced by the worker's own hook-governed activity:
1. `phr-mcp unit show evidence-gaps-<part> --json` lists the branch's commits under `commits` (lifecycle `commit` events the hook detected).
2. The same report, or `phr-mcp confidence --json --subject <subject>`, shows at least one grounded `tests` or `compile` signal for the work unit (from the post-check adapter, `phr-mcp signal`, or `signal ingest`), so the band is not empty.
3. The journey journal for the worktree contains at least one `outcome:` tag or `coverage_observation` entry tied to the part's edits (`phr-mcp journey --json`).

How the evidence is *read* does not matter: querying it through Phronesis's MCP tools (`get_journey`, `get_confidence`, `query_code_graph`, `get_action_log`) or the `phr-mcp` CLI (`unit show`, `confidence`, `journey`, `coverage select`) is Phronesis working as designed and counts as **recorded** (the user's refinement, 2026-09-29).

**Fetched** means the orchestrator had to re-derive the evidence outside Phronesis: running `cargo build/test/clippy` itself, reading a worker's log file, counting commits with `git log`, or inspecting diffs by hand because the stores had nothing. A PR whose evidence exists only that way does not score.

### Known biases, stated up front
- crush has no post-hook: its two PRs (A, B) can only score if the worker itself called `phr-mcp signal`. Their briefs did not require it. That is the real state of capture today and is what Part C's `signal ingest` exists to fix; the score is taken as is, briefs unchanged mid-flight.
- Codex has pre and post hooks: Part C can score without help.
- Parts D, E, F are unassigned at the time of writing; their harness decides their capture path.

### Baseline
The previous swarm (god-split-1) would have scored **0 of 3** shipped PRs on this rule: every gate result was fetched by the orchestrator.

### Verdict (filled 2026-09-29 ~08:05Z)
**1 of 6 recorded (C); 5 of 6 fetched (A, B, D, E, F).** Baseline was 0 of 3.

Per PR, the failing condition:
- A, B, D: no commits detected and no grounded signal. Their builders ran under crush, whose hook has no post phase, so the outcome adapter never saw a gate result; commits were made by processes the installed binary does not attribute to the unit.
- E, F: a grounded `compile` signal and a rich journal, but no commits under the unit report. F's journal contains all five `lifecycle:commit` events; the unit report does not list them. That is the gap Part B (worktree commit attribution) and the unit report join were written to close, measured here with the pre-fix binary.
- C: the one PR that scores. Codex has pre and post hooks, so compile and tests grounded (band medium) and one commit was attributed; three later commits were not.

What this says about "is it working as planned": the capture path works end to end exactly where both hook phases exist (Codex). Where a harness lacks a post phase, Phronesis records the pre-check decisions (every part shows `fired`/`blocked`/`warned` counts) but no outcomes, and the human still has to fetch. The six PRs in this swarm are the fixes for that; the next swarm run under the new binary is the real test of them.

### Results (captured read-only in each worktree, 2026-09-29 ~07:40Z, from `phr-mcp unit show <unit> --json`, `phr-mcp confidence --json`, and the journey journal; raw files under `judgement/<part>/`)

Note on the measuring instrument: every worktree's hooks run the *installed* `phr-mcp` (v0.36.0, main), not the branch under construction. So Part B's worktree commit detection and Part C's `signal ingest` were not available to record their own evidence. The number below is therefore the pre-fix baseline measured on a real swarm, which is what it was meant to be.

| PR | Harness (build / repair) | 1. commits under `commits` | 2. grounded signal | 3. `outcome:`/coverage in journal | Score |
|----|---|---|---|---|---|
| A #118 | crush-glm-5.2 / codex (failed), crush-glm-5.2 (provider died), together GLM-Flash | none (3 commits on branch) | none, band low | yes: compile_ok 5, test_pass 3, test_fail 1 (from the codex and Flash phases) | **fetched** (1 and 2 fail) |
| B #117 | crush-glm-5.2 / crush-glm-5.2 | none (2 commits) | none, band low | compile_unknown only | **fetched** (all three fail) |
| C #116 | codex / codex | 1 of 4 (fb20b79 detected) | compile + tests, band medium | yes: compile_ok 43, test_pass 21, test_fail 16, lifecycle:commit 6 | **recorded** (partial on 1: three commits of four not listed) |
| D agent-skills #1 | crush-glm-5.2 / together GLM-Flash | none (2 commits) | none, band low | nothing | **fetched** (all three fail) |
| E #119 | crush-glm-5.2 / — | none (3 commits) | compile only, band low | yes: proof_pass 3, proof_fail 1, ingested 1, compile_ok 4 | **fetched** (1 fails; 2 passes on compile alone; 3 passes) |
| F #120 | codex / — | none in the unit report (5 commits on branch; the journal itself holds 5 `lifecycle:commit` events the unit report does not surface) | compile only, band low (journal: test_pass 8, test_fail 7; the last in-sandbox run failed, so `tests` never grounded) | yes: compile_ok 21, test_pass 8, test_fail 7, compile_error 1 | **fetched** (1 fails; 2 passes on compile alone; 3 passes) |

Evidence the orchestrator fetched for every PR regardless: re-running cargo test/clippy/fmt itself before each close, reading worker artifacts and crush/codex logs, `git log` for commits, `sqlite3` on crush.db for spend. Evidence read *through* Phronesis: the unit reports, confidence bands, and journey tag counts above (counted as recorded where present).

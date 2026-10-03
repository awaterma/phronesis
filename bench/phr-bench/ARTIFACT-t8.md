# Task 8: Runner — drive claude headless, enforce caps, extract diff

**Scope:** `bench/phr-bench/src/runner.rs` (new) + `run` subcommand wiring in
`bench/phr-bench/src/main.rs`, per plan Task 8. Tests:
`bench/phr-bench/tests/runner.rs` + `bench/phr-bench/tests/testdata/fake-claude.sh`.

## Summary

`runner::run(task, arm, clone_dir, run_dir, caps) -> anyhow::Result<RunRecord>` drives
`claude -p <prompt> --output-format stream-json --verbose
--dangerously-skip-permissions --max-turns <caps.max_turns>` with cwd = the arm's
clone, stdout -> `run_dir/transcript.jsonl`, and the operator's router env
(`ANTHROPIC_BASE_URL` / `ANTHROPIC_API_KEY` / `ANTHROPIC_MODEL` /
`ANTHROPIC_SMALL_FAST_MODEL`, sourced from `bench/run-env.sh`) injected into the
child process env. The wall clock is measured from before spawn;
`caps.max_wall_clock_secs` is a kill threshold enforced by a poll loop
(backoff 50 ms -> 5 s, always clamped to remaining cap) that `kill()`s the child
at the breach -> `RunExit::CapTime`. After the run: `git add -A` in the clone +
`git diff --cached <pre-run HEAD>` -> `run_dir/patch.diff` (agent commits AND
untracked files captured; pre-run HEAD recorded BEFORE the run). Telemetry is
parsed from the transcript; an unparseable transcript or zero assistant events
-> `RunExit::Error { reason: "transcript_unparseable" }` with zeroed stats —
never a silent pass. Treatment arms summarize the clone's
`.phronesis/log.jsonl` via `governance::summarize`; control arms get `None`;
`GovernanceError::NotWired` -> `RunExit::Error { reason: "governance_not_wired" }`
(the `not_wired_record` tombstone, which `record::validate` rejects loudly).
`record.json` is written into `run_dir` on every path, including NotWired.

`main.rs` wires `phr-bench run --manifest --run-id --arm`: preflight refuses
stale run-ids (existing clone dirs), then iterates tasks sequentially —
`arms::prep` (clones under `bench/results/<run-id>/clones/<instance>/<arm>/`)
then `runner::run` (records under `bench/results/<run-id>/runs/<instance>/<arm>/`,
gitignored). Prep/runner harness failures exit nonzero; record-carrying
outcomes (caps, errors) are data.

## Public surface

- `runner::run` — full headless run + artifacts + record.
- `runner::classify_exit(timed_out, turns_capped, success, stderr) -> RunExit` —
  pure classification: CapTime > CapTurns > (Completed | Error{stderr[..500]}).
- `runner::extract_diff(clone_dir, pre_head) -> Result<String>` — stage-all +
  diff vs pre-run HEAD.
- `runner::not_wired_record(instance_id, arm)` — treatment-only tombstone.
- `runner::CLAUDE_PATH_ENV = "PHR_BENCH_CLAUDE"` — env override for the claude
  binary (tests point it at the fixture; operators leave it unset).

## Design decisions / deviations from the plan sketch

1. **Diff extraction uses the pre-run HEAD trick** (brief + SPIKE-FINDINGS
   correction 2), superseding the plan's older `git diff`-tracked-only sketch:
   `git add -A && git diff --cached <pre-run HEAD>` captures agent commits and
   untracked files. The plan's `diff_extraction_ignores_untracked` test was
   replaced by `diff_extraction_captures_untracked_files_and_agent_commits`.
2. **Governance wiring dirs are excluded from the patch** (`.phronesis`,
   `.claude`, `.codex`, `.gemini` via pathspec excludes): `phr-mcp init` writes
   them before the run, so they are bench infrastructure, not agent output —
   including them would systematically bias `diff_bytes` against treatment.
   Pinned by `diff_extraction_excludes_governance_wiring_dirs`.
3. **NotWired still writes `record.json`** (the plan sketch returned the
   tombstone without persisting it). A missing record would silently drop the
   instance from the dataset; a tombstone on disk stays loud — `validate()`
   rejects it at aggregate time. Pinned by
   `treatment_run_without_hook_log_records_governance_not_wired`.
4. **Turn-cap detection uses result-event `num_turns >= caps.max_turns`**
   (plan line 947). The brief also allows "claude exit code indicates turn
   cap", but no exit code is pinned anywhere (`claude --help` documents none;
   the spike never hit the cap), so no exit code is trusted for CapTurns —
   an unpinned guess would misfile real router errors as caps. If a distinct
   exit code/subtype is observed in a future run, add it to the
   `turns_capped` disjunction in `run()`.
5. **Poll backoff 50 ms -> 5 s** rather than a fixed 5 s poll: real runs still
   settle to one poll per 5 s, but fast children (and test fakes) are reaped
   promptly; the sleep is always clamped to the remaining cap so the kill
   lands on time.
6. **stderr is captured** to `run_dir/claude-stderr.log` (extra artifact) and
   feeds the `Error{reason}`; the plan sketch passed `""`.
7. **Merged-module additions:** `Display for RunExit` (the plan's pinned test
   calls `rec.exit.to_string()`), `PartialEq for RunRecord` (test equality on
   the persisted record), `Default for TranscriptStats` (the plan's
   zeroed-stats path). All additive.

## Tests

**12 new tests, all passing** (test suite total: 30, up from 18):

- `classify_exit_orders_time_over_turns_over_error`, `classify_exit_truncates_long_error_reasons`
- `treatment_record_requires_governance_or_fails_loud` (plan-pinned)
- `diff_extraction_captures_untracked_files_and_agent_commits`
- `diff_extraction_excludes_governance_wiring_dirs`
- `control_run_completes_and_writes_all_artifacts` (record.json ==
  returned record; router env reaches the child)
- `treatment_run_summarizes_clone_governance_log`
- `treatment_run_without_hook_log_records_governance_not_wired`
- `wall_clock_breach_kills_child_and_records_cap_time` (killed at the cap,
  artifacts still extracted)
- `turn_cap_records_cap_turns`
- `transcript_without_assistant_events_is_an_error_not_a_silent_pass`
- `claude_failure_is_error_with_stderr_reason`

No real model calls: `PHR_BENCH_CLAUDE` points at
`tests/testdata/fake-claude.sh`, a `/bin/sh` fixture emitting a canned
stream-json transcript, steered by `FAKE_CLAUDE_*` env knobs (transcript file,
exit code, sleep, stderr, worktree touch, env dump). Tests that spawn the fake
hold a mutex and clear their knobs on drop so parallel tests cannot
cross-contaminate. Clones use the file:// fixture-repo pattern from
tests/arms.rs.

## Verification

- `cargo test --manifest-path bench/phr-bench/Cargo.toml` — 30 passed, 0 failed.
- `cargo clippy --manifest-path bench/phr-bench/Cargo.toml --all-targets -- -D warnings` — clean.
- End-to-end smoke through the built binary (scratch dir, fixture repo, fake
  claude): control arm -> `exit=completed turns=2 diff_bytes=130`, patch.diff
  contains the agent's `proof.txt` and nothing else; treatment arm (real
  `phr-mcp init`, 52 rules wired, hook-less fake) -> `exit=error:
  governance_not_wired` tombstone written to record.json; reusing a stale
  run-id is refused by preflight.
- Nothing outside `bench/` modified.

## Commits

1. `test(bench): failing runner tests` — RED: 12 tests + fake-claude fixture.
2. `feat(bench): runner drives claude headless with measured caps and diff extraction` — GREEN.
3. `feat(bench): run subcommand wiring` — main.rs subcommand.
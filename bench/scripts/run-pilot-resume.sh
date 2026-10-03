#!/usr/bin/env bash
# bench/scripts/run-pilot-resume.sh — resume pilot-lite-20261002 after the
# midnight kill (runner tree died; 6 control runs + clones survived; the
# orphaned 7th attempt was quarantined and discarded).
#
# Stages are deliberately SEPARATE background-able steps so a single kill
# cannot take the whole chain: remaining-control -> full treatment ->
# verify -> quality. Each stage checks its own preconditions and can be
# re-run independently.
set -euo pipefail
ROOT="$(git rev-parse --show-toplevel)"
BIN="cargo run --quiet --release --manifest-path $ROOT/bench/phr-bench/Cargo.toml --"
RUN_ID="pilot-lite-20261002"
cd "$ROOT"
source "$ROOT/bench/run-env.sh"

# Stage 1: the 9 remaining control tasks (the completed 6 clones are
# untouched — the runner preflight only checks tasks in the given manifest).
$BIN run --manifest "$ROOT/bench/tasks/manifest-pilot-remaining.json" --run-id "$RUN_ID" --arm control

# Stage 2: all 15 treatment arms (no treatment clones exist yet).
$BIN run --manifest "$ROOT/bench/tasks/manifest-pilot-lite.json" --run-id "$RUN_ID" --arm treatment

# Stage 3-4: verification + symmetric quality audit over the whole run tree.
$BIN verify --run-id "$RUN_ID"
$BIN quality --run-id "$RUN_ID"

# Gate — record integrity: 15 task dirs, 15+15 records.
RUN_DIR="$ROOT/bench/results/$RUN_ID"
test "$(ls "$RUN_DIR/runs" | wc -l)" -eq 15
test "$(find "$RUN_DIR/runs" -name record.json | wc -l)" -eq 30

echo "resume complete: 30 records under $RUN_DIR; report generation next"
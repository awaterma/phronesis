#!/usr/bin/env bash
# bench/scripts/run-pilot-resume2.sh — final detached stretch of
# pilot-lite-20261002: the last control run (bat-1892), all 15 treatment
# runs, verification, quality audit, gates. Launched via setsid/nohup so it
# is NOT a child of the orchestrator's shell tree (four kills hit that
# tree while the session idled); writes a heartbeat for liveness checks.
set -euo pipefail
ROOT="$(git rev-parse --show-toplevel)"
BIN="cargo run --quiet --release --manifest-path $ROOT/bench/phr-bench/Cargo.toml --"
RUN_ID="pilot-lite-20261002"
RUN_DIR="$ROOT/bench/results/$RUN_ID"
HB="$ROOT/bench/results/pilot-resume2-heartbeat.txt"
cd "$ROOT"
source "$ROOT/bench/run-env.sh"

beat() { date '+%H:%M:%S' >> "$HB"; }

beat
$BIN run --manifest "$ROOT/bench/tasks/manifest-bat-1892.json" --run-id "$RUN_ID" --arm control
beat
$BIN run --manifest "$ROOT/bench/tasks/manifest-pilot-lite.json" --run-id "$RUN_ID" --arm treatment
beat
$BIN verify --run-id "$RUN_ID"
beat
$BIN quality --run-id "$RUN_ID"

# Gate — record integrity: 15 task dirs, 30 records.
test "$(ls "$RUN_DIR/runs" | wc -l)" -eq 15
test "$(find "$RUN_DIR/runs" -name record.json | wc -l)" -eq 30
beat
echo "resume2 complete: 30 records under $RUN_DIR; run run-final-report.sh next"
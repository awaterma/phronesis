#!/usr/bin/env bash
# bench/scripts/run-pilot.sh — Phase 1 pilot (plan Task 14), pilot-lite edition.
#
# DEVIATION FROM THE PLAN (recorded on the swarm ledger, surfaces in the
# report caveats): the pinned dataset has 43 rust instances, so the plan's
# "all rust + fill to 30" yields 43 tasks = 86 runs (~14 h at the spike's
# ~10 min/run mean) — beyond the overnight window. This run trims the
# deterministic pilot manifest to the FIRST 15 rust instances by
# instance_id (30 paired runs) to produce the first report for review;
# the full 30-task pilot is queued for the next window.
#
# Steps: fresh phr-mcp install, corpus (pilot slice), manifest trim,
# arms, control runs, treatment runs (glm-5.3:cloud via bench/run-env.sh),
# official SWE-bench verification, symmetric quality audit. The HTML
# report is generated afterwards (aggregate exists; report module t12
# re-auction landed mid-run) — see run-report.sh.
set -euo pipefail
ROOT="$(git rev-parse --show-toplevel)"
BIN="cargo run --quiet --release --manifest-path $ROOT/bench/phr-bench/Cargo.toml --"
RUN_ID="pilot-lite-$(date +%Y%m%d)"
N=15

# Hooks invoke the freshly built governance binary (plan global constraint).
cargo install --path "$ROOT/crates/phronesis-mcp" --quiet
mkdir -p "$ROOT/bench/results"
phr-mcp --version > "$ROOT/bench/results/$RUN_ID-phr-version.txt" || true

# The router env (local ollama speaking the Anthropic Messages API).
source "$ROOT/bench/run-env.sh"

cd "$ROOT"
$BIN corpus --slice pilot --seed 20261001 --out "$ROOT/bench/tasks/manifest-pilot.json"

# Deterministic trim to N tasks (deviation documented above).
python3 - "$N" "$ROOT/bench/tasks/manifest-pilot.json" "$ROOT/bench/tasks/manifest-pilot-lite.json" <<'EOF'
import json, sys
n, src, dst = int(sys.argv[1]), sys.argv[2], sys.argv[3]
m = json.load(open(src))
m["tasks"] = sorted(m["tasks"], key=lambda t: t["instance_id"])[:n]
json.dump(m, open(dst, "w"), indent=2)
print(f"trimmed manifest to {len(m['tasks'])} tasks -> {dst}")
EOF

$BIN arms   --manifest "$ROOT/bench/tasks/manifest-pilot-lite.json" --run-id "$RUN_ID"
$BIN run    --manifest "$ROOT/bench/tasks/manifest-pilot-lite.json" --run-id "$RUN_ID" --arm control
$BIN run    --manifest "$ROOT/bench/tasks/manifest-pilot-lite.json" --run-id "$RUN_ID" --arm treatment
$BIN verify --run-id "$RUN_ID"
$BIN quality --run-id "$RUN_ID"

# Gate — record integrity (plan Task 14 Step 3, scaled to the trim):
RUN_DIR="$ROOT/bench/results/$RUN_ID"
test "$(ls "$RUN_DIR/runs" | wc -l)" -eq "$N"
test "$(find "$RUN_DIR/runs" -name treatment -type d | wc -l)" -eq "$N"
test "$(find "$RUN_DIR/runs" -name control -type d | wc -l)" -eq "$N"

# Gate — loud failures only: zero silently-missing governance (a not-wired
# tombstone is LOUD data; its absence with a treatment record would be silent).
! grep -rl '"reason": *"governance_not_wired"' "$RUN_DIR/runs" >/dev/null 2>&1 || \
  echo "NOTE: not-wired tombstones present (loud data — review them before the report)."

echo "pilot-lite $RUN_ID complete: records + verification + quality under $RUN_DIR"
echo "next: aggregate + report (run-report.sh once t12 is merged)"
#!/usr/bin/env bash
# bench/scripts/run-final-report.sh — finish the pilot-lite: HTML report,
# false-positive review material, and the plan's Phase 2 decision numbers.
set -euo pipefail
ROOT="$(git rev-parse --show-toplevel)"
BIN="cargo run --quiet --release --manifest-path $ROOT/bench/phr-bench/Cargo.toml --"
RUN_ID="pilot-lite-20261002"
RUN_DIR="$ROOT/bench/results/$RUN_ID"
cd "$ROOT"

# 1. The self-contained HTML report (+ its committed JSON data).
$BIN report --run-id "$RUN_ID" --out "$ROOT/bench/report/index.html"

# 2. Report DoD check (plan Task 14 Step 4): seven sections, self-contained.
for id in section-headline section-tasks section-friction section-efficiency section-governance section-interpretation section-caveats; do
  grep -q "id=\"$id\"" "$ROOT/bench/report/index.html" || { echo "missing $id"; exit 1; }
done
if grep -qE 'https?://' "$ROOT/bench/report/index.html"; then
  echo "FAIL: external references in the report"; exit 1
fi
echo "report DoD: seven sections, self-contained"

# 3. False-positive review material (plan Task 14 Step 5): every blocked edit
#    from every treatment clone's governance log, as a review stub.
python3 - "$RUN_DIR" > "$ROOT/bench/report/pilot-false-positive-review.md" <<'EOF'
import json, glob, sys
run_dir = sys.argv[1]
rows = []
for log in sorted(glob.glob(f"{run_dir}/clones/*/*/log.jsonl")):
    parts = log.split("/")
    instance, arm = parts[-3], parts[-2]
    for line in open(log):
        try: e = json.loads(line)
        except Exception: continue
        if e.get("event") == "pre_check" and e.get("exit") == 2:
            for b in e.get("blocked_by", []):
                if b.get("kind") == "rule":
                    rows.append((instance, arm, b.get("rule",""), str(e.get("file","?")), (e.get("message") or "")[:160]))
print("# Pilot false-positive review (rubric stub for the operator)\n")
print("A false positive = a block where the rule's intent was NOT violated AND the recovery did not improve the outcome.\n")
if not rows:
    print("_No rule blocks recorded in any treatment clone._")
else:
    print("| instance | arm | rule | file | message |")
    print("|---|---|---|---|---|")
    for r in rows:
        print(f"| {r[0]} | {r[1]} | {r[2]} | {r[3]} | {r[4].replace('|','\\|')} |")
print(f"\n_source: every blocked edit across all treatment clones in {run_dir}_")
EOF

# 4. Phase 2 decision numbers (plan Task 14 Step 6).
python3 - "$RUN_DIR" <<'EOF'
import json, glob, statistics, sys
run_dir = sys.argv[1]
recs = [json.load(open(p)) for p in glob.glob(f"{run_dir}/runs/*/*/record.json")]
walls = [r["wall_clock_secs"] for r in recs if r.get("exit", {}).get("type") == "completed"]
errors = [r for r in recs if r.get("exit", {}).get("type") == "error"]
print(f"records: {len(recs)}  completed: {len(walls)}  errors: {len(errors)}")
if walls:
    print(f"mean wall-clock: {statistics.mean(walls):.0f}s   median: {statistics.median(walls):.0f}s")
    print(f"gate (mean <= 1800s): {'PASS' if statistics.mean(walls) <= 1800 else 'FAIL'}")
if recs:
    print(f"infra-error rate: {len(errors)/len(recs):.0%}  (gate < 10%: {'PASS' if len(errors)/len(recs) < 0.10 else 'FAIL'})")
EOF

echo "done: $ROOT/bench/report/index.html + review stub + gate numbers"
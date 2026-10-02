#!/usr/bin/env bash
# bench/scripts/spike-eval.sh — Phase 0.3: one control-arm instance through official eval
#
# Pinned instance: burntsushi__ripgrep-2209 (smallest rust repo, one regression test)
# Pinned dataset:  swe-bench/SWE-bench_Multilingual @ 846e647b9f33c0b51b739d005d13d85493c9af09
# (both dumped to bench/tasks/spike/instance.json + revision.txt; see SPIKE-FINDINGS.md)
#
# Deviations from the plan sketch, recorded in SPIKE-FINDINGS.md:
#   - the dataset has no `language` column; language is encoded in the repo/instance_id
#     and the parser name (parse_log_cargo)
#   - the issue column is `problem_statement` (not issue_text)
#   - the patch is diffed against the pre-run HEAD (the agent may commit) and includes
#     untracked files the agent left behind
set -euo pipefail
ROOT="$(git rev-parse --show-toplevel)"
WORK="$ROOT/bench/tasks/spike"
mkdir -p "$WORK"
source "$ROOT/bench/run-env.sh"

# 2. Clone at base commit, run the control arm once.
cd "$WORK"
if [ ! -d repo ]; then
  git clone "https://github.com/$(python3 -c "import json;print(json.load(open('instance.json'))['repo'])").git" repo -q
fi
cd repo
git checkout -q "$(python3 -c "import json;print(json.load(open('../instance.json'))['base_commit'])")"
git config user.email spike@phr-bench.local
git config user.name "phr-bench spike"
HEAD_BEFORE="$(git rev-parse HEAD)"
echo "control arm starting at $HEAD_BEFORE"
claude -p "$(python3 -c "
import json
i = json.load(open('../instance.json'))
print('Fix the following issue in this repository.\n\n' + str(i.get('problem_statement')))
")" --output-format stream-json --verbose --dangerously-skip-permissions \
  --max-turns 100 > "$WORK/transcript.jsonl" || true
git add -A
git diff --cached "$HEAD_BEFORE" > "$WORK/patch.diff"
echo "patch bytes: $(wc -c < "$WORK/patch.diff")"
python3 - "$WORK" <<'EOF'
import json, sys
w = sys.argv[1]
inst = json.load(open(w + "/instance.json"))
patch = open(w + "/patch.diff").read()
json.dump({"instance_id": inst["instance_id"], "model_name_or_path": "spike",
           "model_patch": patch}, open(w + "/preds.jsonl", "w"))
print("preds.jsonl written for", inst["instance_id"])
EOF
echo "control arm done; run the pinned harness (see SPIKE-FINDINGS.md Task 2 section)"
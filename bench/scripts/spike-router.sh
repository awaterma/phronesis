#!/usr/bin/env bash
# bench/scripts/spike-router.sh — Phase 0.1: claude -p -> router -> glm-5.3:cloud
#
# Deviations from the plan sketch (recorded in SPIKE-FINDINGS.md):
#   1. ROOT is captured before cd into the temp dir — `git rev-parse` inside a
#      non-repo temp dir fails and would kill the script under `set -e`.
#   2. tool_use items are nested inside assistant message content arrays in
#      stream-json output; there is no top-level "tool_use" event type.
set -euo pipefail
ROOT="$(git rev-parse --show-toplevel)"
WORK="$(mktemp -d)/router-spike"
mkdir -p "$WORK"
cd "$WORK"
# run-env.sh is gitignored; created by the operator from bench/run-env.example
source "$ROOT/bench/run-env.sh"
START=$(date +%s)
claude -p "Create a file named proof.txt containing the word wired, then stop." \
  --output-format stream-json --verbose --dangerously-skip-permissions \
  --max-turns 20 > transcript.jsonl
END=$(date +%s)
echo "elapsed=$((END-START))s"
python3 - "$WORK" <<'EOF'
import json, sys
events = [json.loads(l) for l in open(sys.argv[1] + "/transcript.jsonl") if l.strip()]
assistant = [e for e in events if e.get("type") == "assistant"]
tool = [c for e in assistant for c in (e.get("message") or {}).get("content", [])
        if isinstance(c, dict) and c.get("type") == "tool_use"]
assert events, "no events in transcript"
assert assistant, "no assistant events — router wiring broken"
assert tool, "no tool_use events — tool-calling broken"
print("events=%d assistant=%d tool_use=%d" % (len(events), len(assistant), len(tool)))
EOF
[[ -f proof.txt ]] || { echo "FAIL: proof.txt not created"; exit 1; }
[[ "$(cat proof.txt)" == *wired* ]] || { echo "FAIL: proof.txt content wrong"; exit 1; }
echo "PASS: router wiring"
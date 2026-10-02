#!/usr/bin/env bash
# bench/scripts/spike-hooks.sh — Phase 0.2: phronesis hooks fire under claude -p
set -euo pipefail
ROOT="$(git rev-parse --show-toplevel)"
WORK="$(mktemp -d)"
git clone "$ROOT" "$WORK/clone" -q   # any git repo works; use a tiny fixture instead if preferred
cd "$WORK/clone"
phr-mcp init --packs llm,rust >/dev/null
# phr-mcp >= 0.36 wires claude hooks into .claude/settings.local.json (the
# plan sketch asserted .claude/settings.json; accept either, tolerate versions).
[[ ( -f .claude/settings.json || -f .claude/settings.local.json ) && -f .phronesis/rules.json ]] || { echo "FAIL: init files missing"; exit 1; }
source "$ROOT/bench/run-env.sh"
claude -p "Add a file src/spike.rs containing exactly: fn main() { let x = vec![1]; println!(\"{}\", x[0].unwrap()); }" \
  --output-format stream-json --verbose --dangerously-skip-permissions \
  --max-turns 20 > "$WORK/transcript.jsonl" || true   # exit 2 inside is expected behavior
python3 - "$WORK" <<'EOF'
import json, sys
w = sys.argv[1]
hooks = [json.loads(l) for l in open(w + "/clone/.phronesis/log.jsonl") if l.strip()]
pre = [e for e in hooks if e.get("event") == "pre_check"]
blocked = [e for e in pre if e.get("exit") == 2 and e.get("blocked_by")]
lifecycle = [e for e in hooks if e.get("kind") == "lifecycle"]
assert hooks, "log.jsonl empty — hooks did NOT fire in headless mode"
assert blocked, "pre_check exit-2 entry missing — hook did not block"
assert any("unwrap" in str(e) for e in blocked), "block was not the unwrap rule"
print("log_entries=%d pre_checks=%d blocked=%d lifecycle=%d" % (len(hooks), len(pre), len(blocked), len(lifecycle)))
EOF
echo "PASS: hooks fire in headless"
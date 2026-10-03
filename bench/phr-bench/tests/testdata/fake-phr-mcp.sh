#!/bin/sh
# Fake phr-mcp audit CLI for quality tests — emits canned audit JSON output.
# Env vars:
#   FAKE_AUDIT_JSON    - emit this file as audit output (default: canned output)
#   FAKE_AUDIT_EXIT    - exit code (default 0)
set -u

if [ -n "${FAKE_AUDIT_JSON:-}" ] && [ -f "$FAKE_AUDIT_JSON" ]; then
  cat "$FAKE_AUDIT_JSON"
else
  # Canned audit output (from the test fixture)
  cat <<'EOF'
{"generated_at":1760000100,"scan_duration_ms":12,"files_scanned":9,
 "totals":{"blocked":3,"warned":1,"rules":3},
 "rules":[
   {"rule_id":"no-unwrap-in-src","level":"block","hits":2,
    "files":[{"path":"src/a.rs","lines":[14,30],"details":["",""]}]},
   {"rule_id":"unsafe-blocks","level":"block","hits":1,
    "files":[{"path":"src/c.rs","lines":[42],"details":[""]}]},
   {"rule_id":"audit-file-loc-high","level":"warn","hits":1,
    "files":[{"path":"src/b.rs","lines":[],"details":["ladder (9 let bindings)"]}]}
 ]}
EOF
fi

exit "${FAKE_AUDIT_EXIT:-0}"

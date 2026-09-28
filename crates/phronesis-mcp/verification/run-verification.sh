#!/usr/bin/env bash
# verification/run-verification.sh
#
# Runs the Verus verifier on harness.rs, which `include!`s the production
# core `src/coverage/pure_core.rs` (the same file coverage/store.rs and
# coverage/region_map.rs call), and exits nonzero unless Verus reports
# "N verified, 0 errors".
#
# Verus is taken from $VERUS, else `verus` on PATH, else ~/.cargo/bin/verus.
#
# Usage:  bash run-verification.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HARNESS="$SCRIPT_DIR/harness.rs"
CORE="$SCRIPT_DIR/../src/coverage/pure_core.rs"
VERUS="${VERUS:-$(command -v verus || echo "$HOME/.cargo/bin/verus")}"

if [ ! -x "$VERUS" ]; then
    echo "RESULT: verus not found (set VERUS=/path/to/verus)"
    exit 1
fi
if [ ! -f "$CORE" ]; then
    echo "RESULT: shared core missing: $CORE"
    exit 1
fi

echo "=== Verus Verification Runner ==="
echo ""
echo "Verus version:"
"$VERUS" --version 2>&1
echo ""
echo "Running: $VERUS $HARNESS"
echo "  (verifies the production core $CORE via include!)"
echo ""

OUTPUT=$("$VERUS" "$HARNESS" 2>&1) || true
echo "$OUTPUT"
echo ""

# Pass only on the summary line with zero errors; anything else (a
# verification failure, a compile error, no summary at all) fails.
if echo "$OUTPUT" | grep -Eq "^verification results:: [0-9]+ verified, 0 errors$"; then
    echo "RESULT: VERIFICATION PASSED"
    exit 0
fi
echo "RESULT: VERIFICATION FAILED"
exit 1

#!/usr/bin/env bash
# verification/run-verification.sh
#
# Runs the verus verifier on harness.rs and exits nonzero on any
# verification failure.
#
# Usage:  bash run-verification.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HARNESS="$SCRIPT_DIR/harness.rs"
VERUS="${VERUS:-$HOME/.cargo/bin/verus}"

echo "=== Verus Verification Runner ==="
echo ""

# Print verus version
VERUS_VERSION=$("$VERUS" --version 2>&1)
echo "Verus version:"
echo "$VERUS_VERSION"
echo ""

# Run verification
echo "Running: $VERUS $HARNESS"
echo ""

OUTPUT=$("$VERUS" "$HARNESS" 2>&1) || true
echo "$OUTPUT"
echo ""

# Check for verification failure
if echo "$OUTPUT" | grep -qi "verification failure"; then
    echo "RESULT: VERIFICATION FAILED"
    exit 1
fi

# Check for errors that aren't verification failures (e.g. compilation errors)
if echo "$OUTPUT" | grep -qi "^error"; then
    echo "RESULT: ERRORS DETECTED (compilation or other)"
    exit 1
fi

echo "RESULT: VERIFICATION PASSED"
exit 0
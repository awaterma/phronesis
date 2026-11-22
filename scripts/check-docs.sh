#!/usr/bin/env bash
# Shared documentation check command.
#
# Thin wrapper around scripts/check_docs.py so every lane (content
# migration, ADR compatibility, website and CI) runs the same validation.
# The checker needs PyYAML; its pinned version lives in
# scripts/checker-requirements.txt (CI installs that exact file).
# Examples:
#   scripts/check-docs.sh --source-only
#   scripts/check-docs.sh --site docs/_site --json
#   scripts/check-docs.sh --site docs/_site --strict
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
if ! python3 -c 'import yaml' >/dev/null 2>&1; then
  echo "error: PyYAML is required by the shared documentation checker" >&2
  echo "       install the pinned checker dependencies with:" >&2
  echo "         python3 -m pip install -r scripts/checker-requirements.txt" >&2
  exit 2
fi
exec python3 "$REPO_ROOT/scripts/check_docs.py" --repo "$REPO_ROOT" "$@"

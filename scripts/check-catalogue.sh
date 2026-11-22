#!/usr/bin/env bash
# Compare against the production catalogue generator without rewriting source.
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CATALOGUE_COPY="$(mktemp "${TMPDIR:-/tmp}/phronesis-catalogue.XXXXXX")"
trap 'rm -f "$CATALOGUE_COPY"' EXIT
cp "$REPO_ROOT/docs/catalogue.html" "$CATALOGUE_COPY"
cd "$REPO_ROOT"
if [ "$#" -gt 0 ]; then
  "$1" catalogue --out "$CATALOGUE_COPY"
else
  cargo run --locked -j 4 -p phronesis-mcp -- catalogue --out "$CATALOGUE_COPY"
fi
if ! cmp -s docs/catalogue.html "$CATALOGUE_COPY"; then
  diff -u docs/catalogue.html "$CATALOGUE_COPY" || true
  echo 'error: regenerate docs/catalogue.html with phr-mcp catalogue' >&2
  exit 1
fi

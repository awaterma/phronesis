#!/usr/bin/env bash
# Exercise Jekyll's real reader, not just the generated exclusion list.
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DRAFT_FILE="$(python3 - "$REPO_ROOT" <<'PY'
import sys
import tempfile
from pathlib import Path
with tempfile.NamedTemporaryFile(prefix="untracked-build-fixture-", suffix=".md",
        dir=Path(sys.argv[1]) / "docs/specs", delete=False) as draft:
    draft.write(b'---\nbroken: [\n\xff synthetic private draft\n')
    print(draft.name)
PY
)"
# Only this test-created file is removed; real drafts are never touched.
trap 'rm -f "$DRAFT_FILE"' EXIT
"$REPO_ROOT/scripts/website/build.sh"
python3 - "$REPO_ROOT" "$DRAFT_FILE" <<'PY'
import subprocess
import sys
from pathlib import Path
repo, draft = map(Path, sys.argv[1:])
assert draft.is_file(), "the build must preserve the draft"
assert not subprocess.check_output(["git", "-C", str(repo), "ls-files", "--", str(draft)])
assert not list((repo / "docs/_site").rglob("*" + draft.stem + "*")), "draft leaked into artifact"
print("Real Jekyll build excluded the unreadable untracked draft.")
PY

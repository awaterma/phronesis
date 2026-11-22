#!/usr/bin/env python3
"""Produce Jekyll exclusions without reading untracked documentation bodies."""
import json
import subprocess
import sys
from pathlib import Path

import yaml


def exclusions(repo: Path) -> dict:
    config = yaml.safe_load((repo / "docs/_config.yml").read_text(encoding="utf-8"))
    # Include ignored files as well: a private draft may already be ignored.
    # Existing authored excludes still apply, including archives and caches.
    inventory = subprocess.run(
        ["git", "-C", str(repo), "ls-files", "--others", "-z", "--", "docs"],
        capture_output=True, check=True,
    )
    local_paths = [path.decode("utf-8")[len("docs/"):]
                   for path in inventory.stdout.split(b"\0") if path.startswith(b"docs/")]
    return {"exclude": sorted(set(config.get("exclude", []) + local_paths))}


if __name__ == "__main__":
    json.dump(exclusions(Path(sys.argv[1]).resolve()), sys.stdout)

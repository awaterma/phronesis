#!/usr/bin/env python3
"""Generate website redirect stubs from the migration map.

Reads docs/migration-map.json and writes one static redirect page per
moved document into the built site, so published URLs keep resolving
after a move.

Map shape (owned by the content migration lane, schema_version 1):
  {"schema_version": 1, "generated_at": "...", "source_commit": "...",
   "moves": [{"from": "docs/old.md", "to": "docs/new.md", "note": "..."}],
   "retained_in_place": [{"path": "docs/keep.md", ...}], ...}

Redirect targets: a `to` under docs/archive/ (site-excluded) sends readers
to the GitHub blob of the archived file; anything else points at the
rendered new location.
"""

from __future__ import annotations

import argparse
import html
import json
import sys
from pathlib import Path

GITHUB_BASE = "https://github.com/awaterma/phronesis/blob/main"
ARCHIVE_PREFIX = "docs/archive/"
# Old paths under these prefixes were never published site URLs (raw
# GitHub-only material, including hidden agent-state dirs), so they get
# no redirect stub; the exclusion audit would otherwise flag them as
# excluded/hidden material in the artifact.
NEVER_PUBLISHED_PREFIXES = ("docs/archive/", "docs/superpowers/", "docs/.phronesis/")


def was_ever_published(src: str) -> bool:
    if not src.startswith("docs/") or src.startswith(NEVER_PUBLISHED_PREFIXES):
        return False
    return not any(part.startswith(".") for part in src.split("/"))

STUB = """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title} — Phronesis</title>
<meta name="robots" content="noindex">
<link rel="canonical" href="{target}">
<meta http-equiv="refresh" content="0; url={target_attr}">
</head>
<body>
<p>This page moved to <a href="{target_attr}">{target_attr}</a>.</p>
</body>
</html>
"""


def site_path(doc_path: str) -> str:
    """Repo-relative doc path -> site path (docs/ stripped, .md -> .html)."""
    rel = doc_path
    if rel.startswith("docs/"):
        rel = rel[len("docs/"):]
    rel = rel[:-3] + ".html" if rel.endswith(".md") else rel
    return rel


def rendered_url(doc_path: str) -> str:
    """Repo-relative doc path -> its rendered URL under `permalink: pretty`.

    A moved .md document renders at a directory URL (docs/guides/foo.md ->
    guides/foo/, docs/guides/README.md -> guides/); hand-crafted .html
    pages keep their published .html URLs."""
    rel = doc_path[len("docs/"):] if doc_path.startswith("docs/") else doc_path
    if not rel.endswith(".md"):
        return rel
    stem = rel[:-3]
    if stem.endswith("/README"):
        stem = stem[: -len("/README")]
    return stem.strip("/") + "/"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--site", default="docs/_site", help="built site directory")
    parser.add_argument("--map", default="docs/migration-map.json", help="migration map")
    parser.add_argument("--baseurl", default=None, help="site baseurl (default: parsed from _config.yml)")
    args = parser.parse_args()

    site = Path(args.site)
    if not site.is_dir():
        print(f"error: site directory {site} does not exist; run the build first", file=sys.stderr)
        return 2

    map_path = Path(args.map)
    if not map_path.is_file():
        print(f"no migration map at {map_path}; nothing to do")
        return 0

    if args.baseurl:
        baseurl = args.baseurl.rstrip("/")
    else:
        config = (site.parent / "_config.yml").read_text(encoding="utf-8")
        line = next((ln for ln in config.splitlines() if ln.startswith("baseurl:")), "")
        baseurl = line.split(":", 1)[1].strip().strip('"') if ":" in line else ""
        baseurl = baseurl.rstrip("/")

    try:
        data = json.loads(map_path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        print(f"error: migration map {map_path} is not valid JSON: {exc}", file=sys.stderr)
        return 2
    if not isinstance(data, dict):
        print(f"error: migration map {map_path} must be a JSON object (schema_version 1)", file=sys.stderr)
        return 2
    entries = data.get("moves")
    if entries is None:
        print("error: migration map has no 'moves' list (schema_version 1)", file=sys.stderr)
        return 2
    if not isinstance(entries, list):
        print("error: migration map 'moves' must be a list", file=sys.stderr)
        return 2

    written = 0
    skipped = 0
    for entry in entries:
        if not isinstance(entry, dict):
            print(f"error: migration map move entry must be an object, got {type(entry).__name__}", file=sys.stderr)
            return 2
        src = entry.get("from")
        dst = entry.get("to")
        if not src:
            print("error: migration entry missing 'from'", file=sys.stderr)
            return 2
        if not dst:
            print(f"error: migration entry for {src} missing 'to' (schema: to is always a tracked path)", file=sys.stderr)
            return 2
        if not was_ever_published(src):
            skipped += 1
            continue
        old = site / site_path(src)
        old.parent.mkdir(parents=True, exist_ok=True)
        if dst.startswith(ARCHIVE_PREFIX):
            target = f"{GITHUB_BASE}/{dst}"
        else:
            target = f"{baseurl}/{rendered_url(dst).lstrip('/')}"
        stub = STUB.format(
            title=html.escape(Path(src).stem.replace("-", " ").title()),
            target=html.escape(target, quote=True),
            target_attr=html.escape(target, quote=True),
        )
        old.write_text(stub, encoding="utf-8")
        # The previous .nojekyll site also served Markdown source URLs.
        # Retain these URLs as small forwarding documents, without copying
        # archived bodies into the artifact or creating a second source.
        if src.endswith(".md"):
            raw = site / src[len("docs/"):]
            raw.parent.mkdir(parents=True, exist_ok=True)
            raw.write_text(f"# Document moved\n\n[Read the current document]({target}).\n", encoding="utf-8")
        written += 1

    print(f"wrote {written} redirect stub(s) into {site} "
          f"({skipped} never-published old path(s) skipped)")
    return 0


if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env python3
"""Shared documentation checks for the Phronesis website and canonical docs.

One command for every lane (content migration, ADR compatibility, website
and CI). Stages:

  --source-only : markdown source links, source anchors, front-matter
                  metadata, navigation data, catalogue consistency,
                  changelog consistency (no build needed)
  --site DIR    : adds rendered links, rendered anchors, exclusion audit,
                  section-index reachability, and redirect validation
                  against the built site
  --strict      : promotion gate — uncategorized documents, missing
                  section indexes, and warnings become errors

Two separate front-matter vocabularies are validated, so decision records
never need website keys and website pages never adopt ADR-only keys:

  * Decision records (ADR): `id`, `date`, `status` (proposed|accepted|
    superseded), optional `enforces`, `superseded_by`, `tags`. `title`
    and `section` are NOT required — they derive from the page's first
    `# heading` and its canonical path (docs/decisions/), exactly like
    the Jekyll generator derives them at render time.
  * Website pages: `title`, `section`, optional `status`
    (shipped|proposed|partial|historical, required under docs/specs/),
    `nav_order`, `description`, `redirect_from`, `layout`, `permalink`.
    `title` and `section` may also derive from the first heading and the
    canonical path; sections match docs/_data/navigation.yml ids.

Front matter is parsed with real YAML (PyYAML), scalars and lists are
type-checked, and malformed YAML fails with an actionable error.

Exit code 0 when no errors, 1 on errors, 2 on usage problems. --json
emits a machine-readable report.
"""

from __future__ import annotations

import argparse
import datetime
import html
import json
import posixpath
import re
import sys
from pathlib import Path
from urllib.parse import unquote, urlsplit

try:
    import yaml
except ImportError:  # pragma: no cover - handled with an actionable message in main()
    yaml = None

PAGE_STATUSES = {"shipped", "proposed", "partial", "historical"}
ADR_STATUSES = {"proposed", "accepted", "superseded"}
ADR_KEYS = {"id", "date", "status", "enforces", "superseded_by", "tags"}
KNOWN_FRONT_MATTER = {"title", "section", "status", "description", "nav_order",
                      "redirect_from", "layout", "permalink", "render_with_liquid"}
EXCLUDED_FROM_SITE = {"archive", "superpowers", ".phronesis"}
LANDING_PAGE = "docs/README.md"
CATALOGUE_BEGIN = "<!-- BEGIN GENERATED RULES -->"
CATALOGUE_END = "<!-- END GENERATED RULES -->"
GITHUB_BLOB_BASE = "https://github.com/awaterma/phronesis/blob/main/"
# Old paths under these prefixes were never published site URLs; the
# redirect generator skips them, so no stub is expected here either.
NEVER_PUBLISHED_PREFIXES = ("docs/archive/", "docs/superpowers/", "docs/.phronesis/")


def _was_ever_published(src: str) -> bool:
    if not src.startswith("docs/") or src.startswith(NEVER_PUBLISHED_PREFIXES):
        return False
    return not any(part.startswith(".") for part in src.split("/"))
LEGACY_NAV_LINKS = ("index.html", "explainer.html", "catalogue.html",
                    "guide.html", "changelog.html")
# The canonical documentation landing must be discoverable from every
# navigation surface (controller acceptance requirement).
LANDING_NAV_TARGET = "docs"  # normalized resolved target of /docs/
FM_DELIM = re.compile(r"^---\s*$")
MD_LINK = re.compile(r"(?<!!)\[[^\]]*\]\(([^)\s]+)(?:\s+[\"'][^\"']*[\"'])?\)")

ERRORS: list[str] = []
WARNINGS: list[str] = []
SOFT_WARNINGS: list[str] = []


def error(msg: str) -> None:
    ERRORS.append(msg)


def warn(msg: str) -> None:
    WARNINGS.append(msg)


def advise(msg: str) -> None:
    """Advisory warning: reported, but never promoted by --strict.

    Used for gates owned by other lanes (e.g. catalogue regeneration
    needs a Rust build and belongs to the release/gates lane)."""
    SOFT_WARNINGS.append(msg)


def yaml_problem(exc: BaseException) -> str:
    problem = getattr(exc, "problem", None)
    if problem:
        mark = getattr(exc, "problem_mark", None)
        where = f" (line {mark.line + 1})" if mark is not None else ""
        return f"{problem}{where}"
    return str(exc).splitlines()[0] if str(exc) else repr(exc)


def parse_front_matter(text: str, rel: str) -> tuple[dict | None, str, bool]:
    """Parse YAML front matter. Returns (front_matter|None, body, had_delims).

    front_matter None with had_delims False means "no front matter at
    all"; front_matter None with had_delims True means malformed (the
    actionable error has already been recorded)."""
    lines = text.splitlines()
    if not lines or not FM_DELIM.match(lines[0]):
        return None, text, False
    for i in range(1, len(lines)):
        if not FM_DELIM.match(lines[i]):
            continue
        block = "\n".join(lines[1:i])
        body = "\n".join(lines[i + 1:])
        try:
            data = yaml.safe_load(block) if block.strip() else {}
        except yaml.YAMLError as exc:
            error(f"{rel}: malformed YAML front matter: {yaml_problem(exc)}; "
                  "fix the YAML syntax (quote values containing ':', '#', or leading specials)")
            return None, "", True
        if data is None:
            data = {}
        if not isinstance(data, dict):
            error(f"{rel}: front matter must be a YAML mapping of key: value pairs, "
                  f"got {type(data).__name__}")
            return None, "", True
        return data, body, True
    error(f"{rel}: front matter opens with '---' but has no closing '---' line; "
          "add the closing delimiter on its own line")
    return None, "", True


def is_decision_page(fm: dict) -> bool:
    """Decision records are detected by their own schema keys (id + date),
    never by website keys, so ADRs need no website front matter. Key
    presence (not value type) drives detection so a wrongly-typed field
    still gets the decision-schema error instead of website-page errors."""
    return "id" in fm and "date" in fm


def type_error(rel: str, key: str, value, expected: str, hint: str = "") -> None:
    got = type(value).__name__
    suffix = f"; {hint}" if hint else ""
    error(f"{rel}: front-matter '{key}' must be {expected}, got {got} ({value!r}){suffix}")


def string_value(rel: str, fm: dict, key: str) -> str | None:
    value = fm.get(key)
    if value is None:
        return None
    if not isinstance(value, str) or not value.strip():
        if isinstance(value, bool):
            type_error(rel, key, value, "a non-empty string",
                       hint=f"YAML reads bare {value} as a boolean; quote it: {key}: '{value}'")
        else:
            type_error(rel, key, value, "a non-empty string")
        return None
    return value.strip()


def redirect_entries(rel: str, fm: dict) -> list[str] | None:
    """Normalize redirect_from to a list of site paths.

    Accepts a single bare scalar (`redirect_from: /old`) or a YAML list;
    both are common authoring styles. Anything else is a type error."""
    value = fm.get("redirect_from")
    if value is None:
        return []
    if isinstance(value, str):
        entries = [value]
    elif isinstance(value, list):
        entries = value
    else:
        type_error(rel, "redirect_from", value,
                   "a site path string or a list of them",
                   hint="a single scalar such as redirect_from: /old-path is fine")
        return None
    ok: list[str] = []
    for entry in entries:
        if not isinstance(entry, str):
            type_error(rel, "redirect_from entry", entry, "a string")
            return None
        if not entry.startswith("/"):
            error(f"{rel}: redirect_from entries must be site paths starting with '/', got '{entry}'")
            return None
        ok.append(entry)
    return ok


def parse_date_value(rel: str, value) -> bool:
    if isinstance(value, datetime.date):
        return True
    if isinstance(value, str):
        try:
            datetime.date.fromisoformat(value.strip())
            return True
        except ValueError:
            error(f"{rel}: ADR 'date' must be a YYYY-MM-DD date, got '{value}'")
            return False
    if isinstance(value, bool):
        type_error(rel, "date", value, "a YYYY-MM-DD date",
                   hint="YAML reads bare yes/no as booleans; quote the value")
    else:
        type_error(rel, "date", value, "a YYYY-MM-DD date")
    return False


def check_decision_metadata(rel: str, fm: dict, section: str | None, strict: bool) -> None:
    """Strict ADR vocabulary: mirrors the Rust DecisionFrontmatter serde
    schema (deny_unknown_fields) without ever adding keys to the ADRs."""
    decision_id = fm.get("id")
    if isinstance(decision_id, str) and decision_id.strip():
        label = f"decision '{decision_id.strip()}'"
    else:
        label = "decision record"
        type_error(rel, "id", decision_id, "a non-empty string")

    if "status" not in fm:
        error(f"{rel}: {label} requires 'status' ({', '.join(sorted(ADR_STATUSES))})")
    else:
        status = fm["status"]
        if not isinstance(status, str):
            if isinstance(status, bool):
                type_error(rel, "status", status, "one of " + ", ".join(sorted(ADR_STATUSES)),
                           hint="YAML reads bare yes/no as booleans; quote the value")
            else:
                type_error(rel, "status", status, "one of " + ", ".join(sorted(ADR_STATUSES)))
        elif status not in ADR_STATUSES:
            error(f"{rel}: {label} has invalid status '{status}' "
                  f"(decision statuses: {', '.join(sorted(ADR_STATUSES))})")

    if "date" not in fm:
        error(f"{rel}: {label} requires 'date' (YYYY-MM-DD)")
    else:
        parse_date_value(rel, fm["date"])

    for key, expected in (("enforces", "a list of strings"),
                          ("tags", "a list of strings")):
        if key not in fm or fm[key] is None:
            continue
        value = fm[key]
        if not isinstance(value, list):
            type_error(rel, key, value, expected,
                       hint="use a YAML list: 'key:' followed by '- item' lines or 'key: [a, b]'")
        else:
            for item in value:
                if not isinstance(item, str):
                    type_error(rel, f"{key} entry", item, "a string")

    if "superseded_by" in fm and fm["superseded_by"] is not None:
        value = fm["superseded_by"]
        if not isinstance(value, str) or not value.strip():
            type_error(rel, "superseded_by", value, "a decision id string or empty (null)")

    if section is None:
        error(f"{rel}: {label} has no section: decision records take their section "
              "from the canonical path, so keep them under docs/decisions/")

    title = string_value(rel, fm, "title")
    if title is None and "title" in fm:
        return  # already reported as a type error
    if title is None and not first_h1(_body_cache.get(rel, "")):
        error(f"{rel}: {label} is missing a title: add a first '# Heading' (the website "
              "derives the title from it) or a 'title' front-matter key")

    for key in fm:
        if key not in ADR_KEYS:
            msg = (f"{rel}: unknown front-matter key '{key}' "
                   f"(decision records allow: {', '.join(sorted(ADR_KEYS))})")
            if strict:
                error(msg)
            else:
                warn(msg)


def check_page_metadata(rel: str, fm: dict, path: Path, repo: Path, section_ids: set[str],
                        strict: bool) -> None:
    title = string_value(rel, fm, "title")
    if title is None and "title" in fm:
        return
    if title is None and not first_h1(_body_cache.get(rel, "")):
        error(f"{rel}: rendered page missing 'title': add a first '# Heading' (the website "
              "derives the title from it) or a 'title' front-matter key")

    section = string_value(rel, fm, "section")
    if section is None and "section" in fm:
        return
    derived = derive_section(path, repo, section_ids)
    if section is None:
        section = derived
    if section is None:
        error(f"{rel}: rendered page missing 'section': add 'section: <id>' front matter "
              f"or move the page under docs/<section>/ (sections: {', '.join(sorted(section_ids))})")
    elif section not in section_ids:
        error(f"{rel}: unknown section '{section}' (add it to docs/_data/navigation.yml)")

    status = fm.get("status")
    under_specs = "specs" in Path(rel).parts
    if status is not None:
        if not isinstance(status, str):
            type_error(rel, "status", status, "one of " + ", ".join(sorted(PAGE_STATUSES)))
        elif status not in PAGE_STATUSES:
            error(f"{rel}: invalid status '{status}' ({', '.join(sorted(PAGE_STATUSES))})")
    elif under_specs:
        error(f"{rel}: spec page requires 'status' ({', '.join(sorted(PAGE_STATUSES))})")

    if "nav_order" in fm and fm["nav_order"] is not None:
        value = fm["nav_order"]
        if isinstance(value, bool) or not isinstance(value, int):
            type_error(rel, "nav_order", value, "an integer",
                       hint=f"YAML reads {value!r} as {type(value).__name__}")
    for key in ("layout", "permalink", "description"):
        if key in fm and fm[key] is not None and not isinstance(fm[key], str):
            type_error(rel, key, fm[key], "a string")
    if "render_with_liquid" in fm and not isinstance(fm["render_with_liquid"], bool):
        type_error(rel, "render_with_liquid", fm["render_with_liquid"], "true or false")

    redirect_entries(rel, fm)

    for key in fm:
        if key not in KNOWN_FRONT_MATTER:
            hint = ""
            if key == "id":
                hint = " (if this is a decision record, add 'date' and 'status' so it validates as an ADR)"
            msg = (f"{rel}: unknown front-matter key '{key}' "
                   f"(known: {', '.join(sorted(KNOWN_FRONT_MATTER))}){hint}")
            if strict:
                error(msg)
            else:
                warn(msg)


def derive_section(path: Path, repo: Path, section_ids: set[str]) -> str | None:
    try:
        parts = path.relative_to(repo).parts
    except ValueError:
        return None
    if len(parts) >= 3 and parts[0] == "docs" and parts[1] in section_ids:
        return parts[1]
    return None


def strip_code_fences(text: str) -> str:
    out = []
    in_fence = False
    for line in text.splitlines():
        if re.match(r"^\s*(```|~~~)", line):
            in_fence = not in_fence
            continue
        if not in_fence:
            out.append(line)
    return "\n".join(out)


def strip_inline_code(text: str) -> str:
    return re.sub(r"`[^`]*`", "", text)


def source_links(body: str) -> list[tuple[str, str]]:
    """Return [(target, fragment)] markdown links."""
    targets = []
    for m in MD_LINK.finditer(strip_inline_code(strip_code_fences(body))):
        raw = m.group(1)
        if raw.startswith("<"):
            raw = raw[1:]
        if re.match(r"^[a-z]+://", raw) or raw.startswith(("mailto:", "#", "data:")):
            continue
        target, _, fragment = raw.partition("#")
        targets.append((target, fragment))
    return targets


def kramdown_slug(text: str) -> str:
    slug = re.sub(r"[^\w\- ]", "", text.strip().lower(), flags=re.UNICODE)
    slug = re.sub(r"\s", "-", slug)
    return slug.strip("-")


def first_h1(md_text: str) -> str | None:
    for line in md_text.splitlines():
        m = re.match(r"^#\s+(.*?)\s*#*\s*$", line)
        if m:
            return re.sub(r"\s+\{[#.][^}]*\}$", "", m.group(1)).strip()
    return None


def heading_slugs(md_text: str) -> set[str]:
    slugs = set()
    for line in md_text.splitlines():
        m = re.match(r"^(#{1,6})\s+(.*?)\s*#*\s*$", line)
        if m:
            heading = re.sub(r"\s+\{[#.][^}]*\}$", "", m.group(2))
            slugs.add(kramdown_slug(heading))
    return slugs


def repo_md_files(docs: Path) -> list[Path]:
    files = []
    for p in sorted(docs.rglob("*.md")):
        parts = p.relative_to(docs).parts
        if parts and (parts[0] in EXCLUDED_FROM_SITE or parts[0].startswith(".")):
            continue
        if any(part in ("_site", ".jekyll-cache", "vendor", "bundle") for part in parts):
            continue
        files.append(p)
    return files


def scan_markdown(repo: Path) -> dict[str, tuple[dict | None, str, bool]]:
    """Parse every retained markdown file exactly once."""
    scanned: dict[str, tuple[dict | None, str, bool]] = {}
    for path in repo_md_files(repo / "docs"):
        rel = str(path.relative_to(repo))
        scanned[rel] = parse_front_matter(path.read_text(encoding="utf-8", errors="replace"), rel)
    return scanned


_body_cache: dict[str, str] = {}


def parse_navigation_data(path: Path) -> list[dict]:
    """Parse docs/_data/navigation.yml with real YAML and validate shape."""
    try:
        data = yaml.safe_load(path.read_text(encoding="utf-8"))
    except yaml.YAMLError as exc:
        error(f"docs/_data/navigation.yml: malformed YAML: {yaml_problem(exc)}")
        return []
    if not isinstance(data, dict) or not isinstance(data.get("sections"), list):
        error("docs/_data/navigation.yml: expected a top-level mapping with a 'sections' list")
        return []
    sections: list[dict] = []
    seen: set[str] = set()
    for entry in data["sections"]:
        if not isinstance(entry, dict) or "id" not in entry:
            error(f"docs/_data/navigation.yml: section entry missing 'id': {entry!r}")
            continue
        sid = entry["id"]
        if not isinstance(sid, str) or not sid.strip():
            error(f"docs/_data/navigation.yml: section id must be a non-empty string, got {sid!r}")
            continue
        if sid in seen:
            error(f"docs/_data/navigation.yml: duplicate section id '{sid}'")
            continue
        seen.add(sid)
        section: dict = {"id": sid}
        for key in ("label", "description", "index"):
            value = entry.get(key)
            if value is None:
                continue
            if not isinstance(value, str):
                error(f"docs/_data/navigation.yml: section '{sid}' {key} must be a string, got {type(value).__name__}")
            else:
                section[key] = value
        if "index" in section and not section["index"].startswith("/"):
            error(f"docs/_data/navigation.yml: section '{sid}' index must be a site-rooted URL starting with '/', got '{section['index']}'")
        sections.append(section)
    return sections


def check_navigation(repo: Path, sections: list[dict], strict: bool) -> None:
    """Every section declares the canonical README rendered index URL, and
    that canonical index page exists."""
    for s in sections:
        index = s.get("index")
        if not index:
            error(f"navigation section '{s['id']}' has no 'index' URL; "
                  f"add 'index: /{s['id']}/' for its canonical README")
            continue
        expected = f"/{s['id']}/"
        if index != expected:
            error(f"navigation section '{s['id']}' index must be the canonical README "
                  f"rendered URL '{expected}', got '{index}'")
        readme = repo / "docs" / s["id"] / "README.md"
        if not readme.is_file():
            msg = (f"navigation section '{s['id']}' index '{index}' has no canonical "
                   "index page (docs/" + s["id"] + "/README.md)")
            if strict:
                error(msg)
            else:
                warn(msg + " — expected until the content migration lands")


def load_migration_map(repo: Path, map_path: Path | None) -> dict:
    path = map_path or (repo / "docs" / "migration-map.json")
    if not path.is_file():
        return {}
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        error(f"migration map {path} is not valid JSON: {exc}")
        return {}
    if not isinstance(data, dict):
        error(f"migration map {path} must be a JSON object (schema_version 1)")
        return {}
    moves = data.get("moves")
    if moves is None:
        error("migration map has no 'moves' list (schema_version 1)")
        return data
    if not isinstance(moves, list):
        error("migration map 'moves' must be a list")
        return data
    for entry in moves:
        if not isinstance(entry, dict):
            error(f"migration map move entry must be an object, got {type(entry).__name__}")
            continue
        missing = [k for k in ("from", "to", "note") if k not in entry]
        if missing:
            error(f"migration map move entry missing {', '.join(missing)}: {entry.get('from', '?')}")
    return data


def classified_paths(repo: Path, sections: list[dict], mapping: dict) -> set[str]:
    """Repo-relative paths that count as categorized for the strict gate:
    under a canonical section dir (the directory IS the category), listed
    in the migration map, the documentation landing page, or carrying
    front matter (checked separately)."""
    section_ids = {s["id"] for s in sections}
    classified: set[str] = set()
    for md in repo_md_files(repo / "docs"):
        rel = str(md.relative_to(repo))
        parts = md.relative_to(repo / "docs").parts
        if len(parts) >= 2 and parts[0] in section_ids:
            classified.add(rel)
    for entry in mapping.get("moves", []):
        if not isinstance(entry, dict):
            continue
        for key in ("from", "to"):
            value = entry.get(key)
            if value:
                classified.add(str(value))
    for entry in mapping.get("retained_in_place", []):
        path_value = entry.get("path") if isinstance(entry, dict) else entry
        if path_value:
            classified.add(str(path_value))
    if (repo / LANDING_PAGE).is_file():
        classified.add(LANDING_PAGE)
    return classified


def check_source(repo: Path, sections: list[dict], strict: bool, mapping: dict,
                 scanned: dict[str, tuple[dict | None, str, bool]]) -> dict:
    docs = repo / "docs"
    section_ids = {s["id"] for s in sections}
    md_by_rel = {rel: repo / rel for rel in scanned}
    _body_cache.clear()

    pages: dict[str, dict] = {}
    raw_docs: list[str] = []
    for rel, (fm, body, had_fm) in scanned.items():
        _body_cache[rel] = body
        if fm is None:
            if had_fm:
                continue  # malformed; already reported, nothing more to say
            raw_docs.append(rel)
            continue
        if not fm:
            raw_docs.append(rel)
            continue
        pages[rel] = fm
        if is_decision_page(fm):
            check_decision_metadata(rel, fm, derive_section(md_by_rel[rel], repo, section_ids), strict)
        else:
            check_page_metadata(rel, fm, md_by_rel[rel], repo, section_ids, strict)

    # Source links and anchors across all retained markdown. GitHub stays
    # the source of truth for links reaching outside docs/: the website
    # converts them at render time (docs/_plugins/site_links.rb).
    for rel in sorted(scanned):
        _fm, body, _had = scanned[rel]
        for target, fragment in source_links(body):
            if target.startswith("/"):
                error(f"{rel}: absolute source link '{target}' (use a repo-relative path)")
                continue
            resolved = (md_by_rel[rel].parent / target).resolve()
            try:
                rel_resolved = str(resolved.relative_to(repo.resolve()))
            except ValueError:
                warn(f"{rel}: source link '{target}' points outside the repository; "
                     "cannot verify (link it to GitHub explicitly instead)")
                continue
            if rel_resolved not in md_by_rel and not resolved.exists():
                error(f"{rel}: broken source link '{target}'")
                continue
            if fragment and rel_resolved in md_by_rel:
                slugs = heading_slugs(scanned[rel_resolved][1])
                if fragment not in slugs:
                    error(f"{rel}: anchor '#{fragment}' not found in {rel_resolved}")

    # Navigation: section index URLs are the canonical README pages.
    check_navigation(repo, sections, strict)

    # Uncategorized retained documents. Directory-based classification,
    # the migration map, and the landing page cover migrated files;
    # anything left over at docs/ root without front matter is reported.
    classified = classified_paths(repo, sections, mapping)
    unclassified = [rel for rel in raw_docs if rel not in classified]
    if unclassified:
        listing = ", ".join(unclassified[:12])
        more = "" if len(unclassified) <= 12 else f" (+{len(unclassified) - 12} more)"
        msg = f"documents without category ({len(unclassified)}): {listing}{more}"
        if strict:
            error(msg)
        else:
            warn(msg + " — expected until the content migration lands; use --strict for the post-migration gate")

    return {"rendered_pages": len(pages), "raw_documents": len(raw_docs)}


def cargo_workspace_version(repo: Path) -> str | None:
    cargo = repo / "Cargo.toml"
    in_package = False
    for line in cargo.read_text(encoding="utf-8").splitlines():
        if line.strip() == "[workspace.package]":
            in_package = True
            continue
        if in_package and line.startswith("["):
            break
        if in_package and line.strip().startswith("version"):
            return line.split("=", 1)[1].strip().strip('"')
    return None


def check_catalogue(repo: Path) -> dict:
    page = repo / "docs" / "catalogue.html"
    if not page.is_file():
        error("docs/catalogue.html missing")
        return {}
    text = page.read_text(encoding="utf-8", errors="replace")
    if CATALOGUE_BEGIN not in text or CATALOGUE_END not in text:
        error("docs/catalogue.html missing GENERATED RULES markers (regenerate with phr-mcp catalogue)")
        return {}
    generated = text.split(CATALOGUE_BEGIN, 1)[1].split(CATALOGUE_END, 1)[0]
    ids = re.findall(r'<article class="rule"[^>]*\sid="([^"]+)"', generated)
    if not ids:
        error("catalogue generated region has no rule entries")
        return {}
    if len(ids) != len(set(ids)):
        dupes = sorted({i for i in ids if ids.count(i) > 1})
        error(f"catalogue has duplicate rule ids: {', '.join(dupes)}")
    version = cargo_workspace_version(repo)
    stamp = re.search(r'documents the default packs as of v([\w.\-]+)', generated)
    if version and stamp and stamp.group(1) != version:
        # Regeneration needs phr-mcp (a Rust build), which belongs to the
        # release/gates lane; the shared docs command flags the drift
        # advisory-style and leaves the gate to that lane.
        error(f"catalogue stamp v{stamp.group(1)} does not match workspace version v{version}; "
               "regenerate with: cargo run -p phronesis-mcp -- catalogue (release/gates lane)")

    # Cross-check ids that also exist in on-disk pack JSONs (advisory).
    pack_ids = set()
    for pack_json in (repo / "crates" / "phronesis-mcp" / "packs").glob("*/rules.json"):
        try:
            data = json.loads(pack_json.read_text(encoding="utf-8"))
            pack_ids |= {r["id"] for r in data.get("rules", [])}
        except Exception:
            advise(f"could not parse {pack_json.relative_to(repo)}")
    if pack_ids:
        missing = sorted(pack_ids - set(ids))
        if missing:
            advise(f"catalogue missing on-disk pack rule ids (regenerate with phr-mcp catalogue): {', '.join(missing)}")
    return {"rule_entries": len(ids)}


def check_changelog(repo: Path) -> None:
    changelog = repo / "CHANGELOG.md"
    rendered = repo / "docs" / "changelog.html"
    if not changelog.is_file() or not rendered.is_file():
        warn("changelog consistency check skipped (CHANGELOG.md or docs/changelog.html missing)")
        return
    rendered_text = rendered.read_text(encoding="utf-8", errors="replace")
    for line in changelog.read_text(encoding="utf-8", errors="replace").splitlines():
        m = re.match(r"^##\s+\[?([\w.\-]+)\]?\s+-\s+(\d{4}-\d{2}-\d{2})", line)
        if m:
            version, date = m.group(1), m.group(2)
            if version not in rendered_text or date not in rendered_text:
                error(f"changelog.html missing CHANGELOG.md entry [{version}] - {date}; regenerate docs/changelog.html")


def source_section_urls(repo: Path, scanned: dict, section_id: str) -> list[str]:
    """Rendered URLs of every page sourced from docs/<section>/**/*.md,
    including the index. Derived from the source tree (pretty permalinks,
    explicit permalink overrides) rather than from the built site, so raw
    static files under a section directory are not mistaken for children."""
    out = []
    prefix = f"docs/{section_id}/"
    for rel in sorted(scanned):
        if not rel.startswith(prefix) or not rel.endswith(".md"):
            continue
        fm = scanned[rel][0]
        if fm and fm.get("permalink"):
            url = str(fm["permalink"])
        else:
            sub = rel[len(prefix):-3]
            if sub == "README" or sub.endswith("/README"):
                sub = sub[: -len("README")].rstrip("/")
            url = f"/{section_id}/{sub.strip('/')}/" if sub.strip("/") else f"/{section_id}/"
        out.append(url)
    return out


def _resolved_href_targets(text: str, page_dir: str, baseurl: str) -> set[str]:
    out: set[str] = set()
    for m in re.finditer(r'href="([^"]*)"', text):
        raw = m.group(1)
        if raw.startswith(("http://", "https://", "mailto:", "data:", "javascript:")):
            continue
        target = raw.partition("#")[0]
        if target.startswith("/"):
            if baseurl and target.startswith(baseurl) and (len(target) == len(baseurl) or target[len(baseurl)] in "/?#"):
                target = target[len(baseurl):]
            target = posixpath.normpath(target.lstrip("/"))
        elif target:
            target = posixpath.normpath(posixpath.join(page_dir, target))
        else:
            continue
        out.add(target)
    return out


def check_rendered(repo: Path, site_dir: Path, sections: list[dict], strict: bool,
                   scanned: dict[str, tuple[dict | None, str, bool]]) -> dict:
    site = site_dir.resolve()
    baseurl = ""
    config = repo / "docs" / "_config.yml"
    if config.is_file():
        m = re.search(r"^baseurl:\s*(\S*)", config.read_text(encoding="utf-8"), re.M)
        baseurl = m.group(1).strip('"') if m else ""
    html_files = sorted(site.rglob("*.html"))
    by_path = {posixpath.join(*p.relative_to(site).parts) for p in site.rglob("*") if p.is_file()}

    for page in html_files:
        rel = posixpath.join(*page.relative_to(site).parts)
        text = page.read_text(encoding="utf-8", errors="replace")
        page_dir = posixpath.dirname(rel)
        for m in re.finditer(r'(?:href|src)="([^"]*)"', text):
            raw = html.unescape(m.group(1))
            # The renderer converts excluded docs and repository files to
            # main-branch GitHub URLs. Validate their repository paths too:
            # they must not silently lose the docs/ prefix.
            github = re.match(r"https://github\.com/awaterma/phronesis/(blob|tree)/main/(.*)", raw)
            if github:
                target = repo / unquote(urlsplit(github.group(2)).path)
                if not target.resolve().is_relative_to(repo.resolve()) or not target.exists():
                    error(f"{rel}: broken repository link '{raw}'")
                elif github.group(1) == "tree" and not target.is_dir():
                    error(f"{rel}: repository tree link names a file '{raw}'")
                elif github.group(1) == "blob" and target.is_dir():
                    error(f"{rel}: repository blob link names a directory '{raw}'")
                continue
            if raw.startswith(("http://", "https://", "mailto:", "data:", "javascript:")):
                continue
            target, _, fragment = raw.partition("#")
            if target.startswith("/"):
                if baseurl and target.startswith(baseurl) and (len(target) == len(baseurl) or target[len(baseurl)] in "/?#"):
                    target = target[len(baseurl):]
                target = target.lstrip("/")
            elif target == "":
                if fragment and f'id="{fragment}"' not in text and f"id='{fragment}'" not in text:
                    error(f"{rel}: self anchor '#{fragment}' not found")
                continue
            else:
                target = posixpath.normpath(posixpath.join(page_dir, target))
            target_file = target if posixpath.basename(target) else posixpath.join(target, "index.html")
            if (site / target_file).is_dir() or target_file.endswith("/"):
                target_file = posixpath.join(target_file.rstrip("/"), "index.html")
            if target_file not in by_path and not (site / target_file).is_dir():
                error(f"{rel}: broken rendered link '{raw}'")
                continue
            if fragment and (site / target_file).is_file():
                body = (site / target_file).read_text(encoding="utf-8", errors="replace")
                if f'id="{fragment}"' not in body and f"id='{fragment}'" not in body:
                    error(f"{rel}: anchor '#{fragment}' not found in rendered {target_file}")
            # Unconverted links to pages that render from canonical markdown.
            if target_file.endswith(".md"):
                source_rel = "docs/" + target_file
                if source_rel in scanned and scanned[source_rel][0]:
                    error(f"{rel}: unconverted markdown link to canonical page '{raw}'")

    # Pages carrying the shared top navigation (layout pages and the
    # hand-crafted pages' equivalent nav) must keep the classic five
    # destination URLs reachable. Links may be site-absolute or written
    # relative to the page, so compare resolved targets.
    for page in html_files:
        rel = posixpath.join(*page.relative_to(site).parts)
        text = page.read_text(encoding="utf-8", errors="replace")
        if 'class="web-nav"' not in text:
            continue
        targets = _resolved_href_targets(text, posixpath.dirname(rel), baseurl)
        for legacy in LEGACY_NAV_LINKS:
            if legacy not in targets:
                error(f"{rel}: shared top navigation does not reach '{baseurl}/{legacy}'")
        if LANDING_NAV_TARGET not in targets:
            error(f"{rel}: navigation does not reach the documentation landing '{baseurl}/docs/'")

    # Section indexes: existence, child reachability, and sidebar/breadcrumb
    # back-links. The section index URL is the canonical README rendered URL.
    for s in sections:
        index = s.get("index")
        if not index:
            continue
        index_rel = index.lstrip("/").rstrip("/")
        index_file = posixpath.join(index_rel, "index.html")
        if index_file not in by_path:
            msg = (f"section '{s['id']}' index page '{index}' is not in the built site "
                   "(docs/" + s["id"] + "/README.md must exist and render)")
            if strict:
                error(msg)
            else:
                warn(msg + " — expected until the content migration lands")
            continue
        index_text = (site / index_file).read_text(encoding="utf-8", errors="replace")
        for child_url in source_section_urls(repo, scanned, s["id"]):
            if child_url.rstrip("/") == "/" + index_rel:
                continue
            if f'href="{baseurl}{child_url}"' not in index_text:
                error(f"section '{s['id']}' index page does not link rendered child "
                      f"'{baseurl}{child_url}' (every active document must be reachable "
                      "from its section index)")
                continue
            child_rel = child_url.lstrip("/")
            child_file = (child_rel if child_url.endswith(".html")
                          else posixpath.join(child_rel, "index.html"))
            page_path = site / child_file
            if page_path.is_file():
                text = page_path.read_text(encoding="utf-8", errors="replace")
                if f'href="{baseurl}{index}"' not in text:
                    error(f"{child_file}: section page does not link its section index "
                          f"'{baseurl}{index}' (sidebar and breadcrumb navigation broken)")

    # Documentation landing: rendered from docs/README.md at /docs/ and
    # must link every section index.
    if (repo / LANDING_PAGE).is_file():
        landing = site / "docs" / "index.html"
        if not landing.is_file():
            error("documentation landing docs/README.md did not render at /docs/")
        else:
            landing_text = landing.read_text(encoding="utf-8", errors="replace")
            for s in sections:
                index = s.get("index")
                if index and f'href="{baseurl}{index}"' not in landing_text:
                    error(f"documentation landing does not link section '{s['id']}' index '{index}'")

    # Exclusion audit: archived/local/raw/hidden material must not ship.
    for p in site.rglob("*"):
        rel = posixpath.join(*p.relative_to(site).parts)
        parts = p.relative_to(site).parts
        if parts and parts[0] in EXCLUDED_FROM_SITE:
            error(f"excluded material in published site: {rel}")
        if p.suffix == ".log":
            error(f"raw log in published site: {rel}")
        if ".phronesis" in parts:
            error(f"agent-local state in published site: {rel}")
        hidden = [part for part in parts
                  if part.startswith(".") and part != ".nojekyll"]
        if hidden:
            error(f"hidden state in published site: {rel} (hidden entries must not ship)")

    # Redirect validation.
    map_path = repo / "docs" / "migration-map.json"
    if map_path.is_file():
        try:
            data = json.loads(map_path.read_text(encoding="utf-8"))
        except json.JSONDecodeError as exc:
            error(f"migration map {map_path} is not valid JSON: {exc}")
            return {"html_pages": len(html_files)}
        entries = data.get("moves", []) if isinstance(data, dict) else []
        stubs = 0
        for entry in entries:
            if not isinstance(entry, dict):
                continue
            src = entry.get("from", "")
            if not src or not _was_ever_published(src):
                continue
            rel = src[len("docs/"):] if src.startswith("docs/") else src
            rel = (rel[:-3] + ".html") if rel.endswith(".md") else rel
            stub = site / rel
            if not stub.is_file():
                error(f"missing redirect stub for {src} (run scripts/website/gen_redirects.py)")
                continue
            stubs += 1
            if src.endswith(".md"):
                raw = site / src[len("docs/"):]
                if not raw.is_file() or "# Document moved" not in raw.read_text(encoding="utf-8"):
                    error(f"missing Markdown forwarding document for {src}")
            m = re.search(r'<meta http-equiv="refresh" content="0; url=([^"]+)"', stub.read_text(encoding="utf-8"))
            if not m:
                error(f"redirect stub {rel} has no refresh target")
                continue
            target = m.group(1)
            if target.startswith("http"):
                continue
            target_rel = target
            if baseurl and target_rel.startswith(baseurl) and (len(target_rel) == len(baseurl) or target_rel[len(baseurl)] in "/?#"):
                target_rel = target_rel[len(baseurl):]
            target_rel = target_rel.lstrip("/")
            if not target_rel or target_rel.endswith("/") or (site / target_rel).is_dir():
                target_rel = posixpath.join(target_rel.rstrip("/"), "index.html")
            if not (site / target_rel).is_file():
                error(f"redirect stub {rel} points at missing target '{target}'")
        return {"html_pages": len(html_files), "redirects": stubs}
    return {"html_pages": len(html_files)}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--repo", default=".", help="repository root (default: cwd)")
    parser.add_argument("--site", default=None, help="built site directory (default: docs/_site when present)")
    parser.add_argument("--source-only", action="store_true", help="run only build-independent checks")
    parser.add_argument("--strict", action="store_true", help="promotion gate: warnings become errors")
    parser.add_argument("--map", default=None, help="migration map path (default: docs/migration-map.json when present)")
    parser.add_argument("--json", action="store_true", help="machine-readable report")
    args = parser.parse_args()

    if yaml is None:
        print("error: PyYAML is required for YAML front-matter validation; "
              "install the pinned checker dependencies with: "
              "python3 -m pip install -r scripts/checker-requirements.txt", file=sys.stderr)
        return 2

    repo = Path(args.repo).resolve()
    nav_path = repo / "docs" / "_data" / "navigation.yml"
    if not nav_path.is_file():
        print(f"error: {nav_path} missing", file=sys.stderr)
        return 2
    sections = parse_navigation_data(nav_path)
    if not sections:
        print(f"error: no sections parsed from {nav_path}", file=sys.stderr)
        return 2

    summary: dict = {}
    mapping = load_migration_map(repo, Path(args.map).resolve() if args.map else None)
    scanned = scan_markdown(repo)
    summary["source"] = check_source(repo, sections, args.strict, mapping, scanned)
    summary["catalogue"] = check_catalogue(repo)
    check_changelog(repo)

    site_dir = Path(args.site) if args.site else None
    if site_dir is not None and not site_dir.is_absolute():
        site_dir = repo / site_dir
    if not args.source_only:
        candidate = site_dir or (repo / "docs" / "_site")
        if candidate.is_dir():
            summary["rendered"] = check_rendered(repo, candidate.resolve(), sections, args.strict, scanned)
        elif args.site:
            print(f"error: --site {args.site} does not exist; run scripts/website/build.sh first", file=sys.stderr)
            return 2
        else:
            warn("no built site found (docs/_site); rendered checks skipped")

    if args.strict:
        ERRORS.extend(f"strict: {w}" for w in WARNINGS)
        WARNINGS.clear()

    report = {"errors": ERRORS, "warnings": WARNINGS + SOFT_WARNINGS, "summary": summary,
              "strict": args.strict}
    if args.json:
        print(json.dumps(report, indent=2))
    else:
        print(f"documentation checks — {len(ERRORS)} error(s), "
              f"{len(WARNINGS) + len(SOFT_WARNINGS)} warning(s)")
        for e in ERRORS:
            print(f"  ERROR {e}")
        for w in WARNINGS:
            print(f"  warn  {w}")
        for w in SOFT_WARNINGS:
            print(f"  note  {w} (advisory, not a strict gate)")
    return 1 if ERRORS else 0


if __name__ == "__main__":
    sys.exit(main())

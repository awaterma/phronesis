#!/usr/bin/env python3
"""Regression fixtures for the reproduced website-handoff failures.

Covers the failures the controller reproduced against the first handoff
(ADR front matter rejected, spec missing section, redirect_from scalar
per-character iteration, handwritten YAML) plus the checks this lane
promised: source/rendered anchors, directory README links, redirect
targets, and archive exclusion.

Run: python3 -m unittest discover -s scripts/tests -v
"""

from __future__ import annotations

import contextlib
import io
import sys
import subprocess
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import check_docs as cd  # noqa: E402

BASEURL = "/phronesis"

NAV = """sections:
  - id: guides
    label: Guides
    index: /guides/
  - id: specs
    label: Specs
    index: /specs/
  - id: decisions
    label: Decisions
    index: /decisions/
"""

NAV_WITH_SUBPAGES = NAV + """  - id: tutorials
    label: Tutorials
    index: /tutorials/
"""

CARGO = '[workspace.package]\nversion = "0.1.0"\n'
CATALOGUE = (
    "<!-- BEGIN GENERATED RULES -->\n"
    '<article class="rule" id="rule-x">documents the default packs as of v0.1.0</article>\n'
    "<!-- END GENERATED RULES -->\n"
)
CHANGELOG = "# Changelog\n\n## [0.1.0] - 2026-01-01\n- x\n"
CHANGELOG_HTML = "<html><body>0.1.0 2026-01-01</body></html>"

ADR_OK = """---
id: choice
date: 2026-01-01
status: accepted
enforces: []
---

# Choice of engine

Body text.
"""


DEFAULT_READMES = {
    "docs/guides/README.md": "# Guides\n\nIndex.\n",
    "docs/specs/README.md": "# Specs\n\nIndex.\n",
    "docs/decisions/README.md": "# Decisions\n\nIndex.\n",
}


def make_repo(files=None, nav=NAV, with_landing=False):
    tmp = tempfile.mkdtemp(prefix="docs-check-fixture-")
    repo = Path(tmp)
    (repo / "docs" / "_data").mkdir(parents=True)
    (repo / "docs" / "_data" / "navigation.yml").write_text(nav, encoding="utf-8")
    (repo / "Cargo.toml").write_text(CARGO, encoding="utf-8")
    (repo / "docs" / "catalogue.html").write_text(CATALOGUE, encoding="utf-8")
    (repo / "CHANGELOG.md").write_text(CHANGELOG, encoding="utf-8")
    (repo / "docs" / "changelog.html").write_text(CHANGELOG_HTML, encoding="utf-8")
    files = dict(DEFAULT_READMES, **(files or {}))
    for rel, content in files.items():
        path = repo / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
    if with_landing:
        (repo / "docs" / "README.md").write_text(
            "# Documentation\n\n[Guides](guides/README.md)\n", encoding="utf-8")
    return repo


def run_checker(repo, *args):
    cd.ERRORS.clear()
    cd.WARNINGS.clear()
    cd.SOFT_WARNINGS.clear()
    old_argv = sys.argv
    sys.argv = ["check_docs.py", "--repo", str(repo), *args]
    try:
        code = cd.main()
    finally:
        sys.argv = old_argv
    return code, list(cd.ERRORS), list(cd.WARNINGS), list(cd.SOFT_WARNINGS)


def run_source(repo, *args):
    return run_checker(repo, "--source-only", *args)


class SourceFixtureTests(unittest.TestCase):
    def test_adr_frontmatter_is_valid_without_website_keys(self):
        repo = make_repo({"docs/decisions/2026-01-01-choice.md": ADR_OK})
        code, errors, warnings, _ = run_source(repo, "--strict")
        self.assertEqual(0, code, msg="\n".join(errors + warnings))
        self.assertEqual([], errors)

    def test_adr_reproduced_fixture_passes_strict(self):
        fm = ("---\nid: choice\ndate: 2026-01-01\nstatus: accepted\nenforces: []\n---\n\n"
              "# Choice\n\nBody.\n")
        repo = make_repo({"docs/decisions/2026-01-01-choice.md": fm})
        code, errors, warnings, _ = run_source(repo, "--strict")
        self.assertEqual([], errors, msg="strict must accept the unchanged ADR fixture")
        self.assertEqual(0, code)

    def test_adr_invalid_status_is_rejected(self):
        fm = ADR_OK.replace("status: accepted", "status: Accepted")
        repo = make_repo({"docs/decisions/2026-01-01-choice.md": fm})
        code, errors, _, _ = run_source(repo)
        self.assertEqual(1, code)
        self.assertTrue(any("invalid status 'Accepted'" in e for e in errors), msg=str(errors))

    def test_adr_unknown_status_word_is_rejected(self):
        fm = ADR_OK.replace("status: accepted", "status: draft")
        repo = make_repo({"docs/decisions/2026-01-01-choice.md": fm})
        _, errors, _, _ = run_source(repo)
        self.assertTrue(any("invalid status 'draft'" in e for e in errors), msg=str(errors))

    def test_adr_unknown_key_strict_error_warn_non_strict(self):
        fm = ADR_OK.replace("enforces: []", "enforces: []\nreviewers: andrew")
        repo = make_repo({"docs/decisions/2026-01-01-choice.md": fm})
        _, errors, _, _ = run_source(repo, "--strict")
        self.assertTrue(any("unknown front-matter key 'reviewers'" in e for e in errors), msg=str(errors))
        repo2 = make_repo({"docs/decisions/2026-01-01-choice.md": fm})
        code, errors, warnings, _ = run_source(repo2)
        self.assertEqual(0, code)
        self.assertEqual([], errors)
        self.assertTrue(any("unknown front-matter key 'reviewers'" in w for w in warnings), msg=str(warnings))

    def test_adr_type_validation(self):
        fm_id = ADR_OK.replace("id: choice", "id: [choice]")
        cases = {
            "id": fm_id,
            "date_str": ADR_OK.replace("date: 2026-01-01", "date: not-a-date"),
            "date_list": ADR_OK.replace("date: 2026-01-01", "date: [2026-01-01]"),
            "enforces_scalar": ADR_OK.replace("enforces: []", "enforces: enforce-no-unwrap"),
            "tags_scalar": ADR_OK.replace("enforces: []", "enforces: []\ntags: rust"),
            "superseded_by_list": ADR_OK.replace("enforces: []", "enforces: []\nsuperseded_by: [x]"),
        }
        for label, fm in cases.items():
            with self.subTest(case=label):
                repo = make_repo({"docs/decisions/2026-01-01-choice.md": fm})
                _, errors, _, _ = run_source(repo)
                self.assertTrue(errors, f"expected errors for {label}")
        # The id-list case must specifically name 'id'.
        repo = make_repo({"docs/decisions/2026-01-01-choice.md": fm_id})
        _, errors, _, _ = run_source(repo)
        self.assertTrue(any("front-matter 'id'" in e for e in errors), msg=str(errors))

    def test_adr_requires_status(self):
        fm = "---\nid: choice\ndate: 2026-01-01\n---\n\n# Choice\n"
        repo = make_repo({"docs/decisions/2026-01-01-choice.md": fm})
        _, errors, _, _ = run_source(repo)
        self.assertTrue(any("requires 'status'" in e for e in errors), msg=str(errors))

    def test_adr_missing_date_falls_to_page_hint(self):
        fm = "---\nid: choice\nstatus: accepted\n---\n\n# Choice\n"
        repo = make_repo({"docs/decisions/2026-01-01-choice.md": fm})
        _, errors, _, _ = run_source(repo, "--strict")
        self.assertTrue(any("unknown front-matter key 'id'" in e and "decision record" in e
                            for e in errors), msg=str(errors))

    def test_adr_without_title_or_h1_is_rejected(self):
        fm = "---\nid: choice\ndate: 2026-01-01\nstatus: accepted\n---\n\nBody only.\n"
        repo = make_repo({"docs/decisions/2026-01-01-choice.md": fm})
        _, errors, _, _ = run_source(repo)
        self.assertTrue(any("missing a title" in e for e in errors), msg=str(errors))

    def test_adr_h1_supplies_derived_title(self):
        fm = "---\nid: choice\ndate: 2026-01-01\nstatus: accepted\n---\n\n# Choice of engine\n"
        repo = make_repo({"docs/decisions/2026-01-01-choice.md": fm})
        _, errors, _, _ = run_source(repo, "--strict")
        self.assertFalse(any("title" in e for e in errors), msg=str(errors))

    def test_spec_without_section_derives_it_from_path(self):
        fm = '---\ntitle: "Example"\nstatus: partial\n---\n\n# Example\n'
        repo = make_repo({"docs/specs/SPEC-example.md": fm})
        code, errors, _, _ = run_source(repo, "--strict")
        self.assertEqual([], errors, msg="spec title+status without section must pass")
        self.assertEqual(0, code)

    def test_spec_requires_status(self):
        fm = '---\ntitle: "Example"\n---\n\n# Example\n'
        repo = make_repo({"docs/specs/SPEC-example.md": fm})
        _, errors, _, _ = run_source(repo)
        self.assertTrue(any("spec page requires 'status'" in e for e in errors), msg=str(errors))

    def test_page_missing_section_is_rejected(self):
        fm = '---\ntitle: "Notes"\n---\n\n# Notes\n'
        repo = make_repo({"docs/notes.md": fm})
        _, errors, _, _ = run_source(repo)
        self.assertTrue(any("missing 'section'" in e for e in errors), msg=str(errors))

    def test_page_without_title_or_h1_is_rejected(self):
        fm = "---\nsection: guides\n---\n\nBody only.\n"
        repo = make_repo({"docs/guides/notes.md": fm})
        _, errors, _, _ = run_source(repo)
        self.assertTrue(any("missing 'title'" in e for e in errors), msg=str(errors))

    def test_unknown_website_key_hint_mentions_adr(self):
        fm = "---\ntitle: T\nsection: guides\nid: x\n---\n\n# T\n"
        repo = make_repo({"docs/guides/notes.md": fm})
        _, errors, _, _ = run_source(repo, "--strict")
        self.assertTrue(any("unknown front-matter key 'id'" in e and "decision record" in e
                            for e in errors), msg=str(errors))

    def test_redirect_from_scalar_and_list(self):
        ok_scalar = '---\ntitle: T\nsection: guides\nredirect_from: /old-path\n---\n\n# T\n'
        repo = make_repo({"docs/guides/notes.md": ok_scalar})
        _, errors, _, _ = run_source(repo, "--strict")
        self.assertEqual([], errors, msg="scalar redirect_from must be accepted without per-character errors")

        bad_scalar = '---\ntitle: T\nsection: guides\nredirect_from: old-path\n---\n\n# T\n'
        repo = make_repo({"docs/guides/notes.md": bad_scalar})
        _, errors, _, _ = run_source(repo)
        self.assertEqual(1, len(errors), msg=f"expected one clear error, got {errors}")
        self.assertIn("must be site paths starting with '/'", errors[0])

        bad_list = '---\ntitle: T\nsection: guides\nredirect_from:\n  - /ok\n  - nope\n---\n\n# T\n'
        repo = make_repo({"docs/guides/notes.md": bad_list})
        _, errors, _, _ = run_source(repo)
        self.assertTrue(any("got 'nope'" in e for e in errors), msg=str(errors))

        wrong_type = '---\ntitle: T\nsection: guides\nredirect_from: 3\n---\n\n# T\n'
        repo = make_repo({"docs/guides/notes.md": wrong_type})
        _, errors, _, _ = run_source(repo)
        self.assertTrue(any("'redirect_from' must be" in e for e in errors), msg=str(errors))

    def test_malformed_yaml_fails_actionably(self):
        fm = '---\ntitle: "unclosed\nsection: guides\n---\n\n# T\n'
        repo = make_repo({"docs/guides/notes.md": fm})
        code, errors, _, _ = run_source(repo)
        self.assertEqual(1, code)
        self.assertTrue(any("malformed YAML front matter" in e for e in errors), msg=str(errors))
        self.assertFalse(any("Traceback" in e for e in errors))

    def test_front_matter_without_closing_delimiter(self):
        fm = "---\ntitle: T\nsection: guides\n\n# T\n"
        repo = make_repo({"docs/guides/notes.md": fm})
        _, errors, _, _ = run_source(repo)
        self.assertTrue(any("no closing '---'" in e for e in errors), msg=str(errors))

    def test_nav_order_types(self):
        ok = "---\ntitle: T\nsection: guides\nnav_order: 2\n---\n\n# T\n"
        repo = make_repo({"docs/guides/notes.md": ok})
        _, errors, _, _ = run_source(repo, "--strict")
        self.assertEqual([], errors, msg=str(errors))
        bad = "---\ntitle: T\nsection: guides\nnav_order: 3.5\n---\n\n# T\n"
        repo = make_repo({"docs/guides/notes.md": bad})
        _, errors, _, _ = run_source(repo)
        self.assertTrue(any("nav_order" in e for e in errors), msg=str(errors))
        bad_bool = "---\ntitle: T\nsection: guides\nnav_order: yes\n---\n\n# T\n"
        repo = make_repo({"docs/guides/notes.md": bad_bool})
        _, errors, _, _ = run_source(repo)
        self.assertTrue(any("nav_order" in e for e in errors), msg=str(errors))

    def test_source_anchor_valid_and_broken(self):
        good = ("---\ntitle: T\nsection: guides\n---\n\n# T\n\n"
                "See [target](other.md#the-goal) and [self](#the-goal).\n")
        target = "---\ntitle: Other\nsection: guides\n---\n\n# The goal\n"
        repo = make_repo({"docs/guides/notes.md": good, "docs/guides/other.md": target})
        _, errors, _, _ = run_source(repo, "--strict")
        self.assertEqual([], errors, msg=str(errors))
        bad = good.replace("#the-goal)", "#no-such-anchor)")
        repo = make_repo({"docs/guides/notes.md": bad, "docs/guides/other.md": target})
        _, errors, _, _ = run_source(repo)
        self.assertTrue(errors and all("anchor" in e for e in errors), msg=str(errors))

    def test_directory_and_readme_links_resolve(self):
        page = "---\ntitle: T\nsection: guides\n---\n\n[Guides](README.md), [Tuts](../tutorials/README.md), [dir](../guides/)\n"
        readme = "# Guides\n\nIndex content.\n"
        tut_readme = "# Tutorials\n"
        repo = make_repo({"docs/guides/notes.md": page, "docs/guides/README.md": readme,
                          "docs/tutorials/README.md": tut_readme}, nav=NAV_WITH_SUBPAGES)
        _, errors, _, _ = run_source(repo, "--strict")
        self.assertEqual([], errors, msg=str(errors))

    def test_broken_source_link_is_error(self):
        page = "---\ntitle: T\nsection: guides\n---\n\n[missing](nope.md)\n"
        repo = make_repo({"docs/guides/notes.md": page})
        _, errors, _, _ = run_source(repo)
        self.assertTrue(any("broken source link 'nope.md'" in e for e in errors), msg=str(errors))

    def test_outside_repo_link_is_diagnostic_not_crash(self):
        page = "---\ntitle: T\nsection: guides\n---\n\n[out](../../../outside.md)\n"
        repo = make_repo({"docs/guides/notes.md": page})
        code, errors, warnings, _ = run_source(repo)
        self.assertEqual(0, code, msg="must not raise or hard-error outside the repo")
        self.assertEqual([], errors)
        self.assertTrue(any("outside the repository" in w for w in warnings), msg=str(warnings))
        repo2 = make_repo({"docs/guides/notes.md": page})
        code2, errors2, _, _ = run_source(repo2, "--strict")
        self.assertEqual(1, code2)
        self.assertTrue(any("outside the repository" in e for e in errors2))

    def test_absolute_source_link_rejected(self):
        page = "---\ntitle: T\nsection: guides\n---\n\n[x](/guides/other.md)\n"
        repo = make_repo({"docs/guides/notes.md": page})
        _, errors, _, _ = run_source(repo)
        self.assertTrue(any("absolute source link" in e for e in errors), msg=str(errors))

    def test_navigation_index_fields(self):
        bad_url = NAV.replace("index: /guides/", "index: guides/")
        repo = make_repo(nav=bad_url)
        _, errors, _, _ = run_source(repo)
        self.assertTrue(any("index must be a site-rooted URL" in e for e in errors), msg=str(errors))

        wrong_path = NAV.replace("index: /guides/", "index: /guide-index/")
        repo = make_repo(nav=wrong_path)
        _, errors, _, _ = run_source(repo)
        self.assertTrue(any("canonical README rendered URL '/guides/'" in e for e in errors), msg=str(errors))

        missing_readme = NAV_WITH_SUBPAGES  # tutorials has no README in repo
        repo = make_repo(nav=missing_readme)
        code, errors, warnings, _ = run_source(repo)
        self.assertEqual(0, code)
        self.assertTrue(any("has no canonical index page" in w for w in warnings), msg=str(warnings))
        repo2 = make_repo(nav=missing_readme)
        code2, errors2, _, _ = run_source(repo2, "--strict")
        self.assertEqual(1, code2)
        self.assertTrue(any("has no canonical index page" in e for e in errors2), msg=str(errors2))

    def test_unclassified_documents_gate(self):
        repo = make_repo({"docs/loose-note.md": "# Just a note\n"})
        code, errors, warnings, _ = run_source(repo)
        self.assertEqual(0, code)
        self.assertTrue(any("documents without category" in w for w in warnings), msg=str(warnings))
        repo2 = make_repo({"docs/loose-note.md": "# Just a note\n"})
        code2, errors2, _, _ = run_source(repo2, "--strict")
        self.assertEqual(1, code2)
        self.assertTrue(any("documents without category" in e for e in errors2), msg=str(errors2))

    def test_migration_map_shape(self):
        repo = make_repo(files={}, nav=NAV)
        m = repo / "docs" / "migration-map.json"
        m.write_text('{"moves": [{"from": "docs/a.md"}]}', encoding="utf-8")
        _, errors, _, _ = run_source(repo)
        self.assertTrue(any("missing" in e and "note" in e for e in errors), msg=str(errors))
        m.write_text('{"moves": "nope"}', encoding="utf-8")
        _, errors, _, _ = run_source(repo)
        self.assertTrue(any("'moves' must be a list" in e for e in errors), msg=str(errors))
        m.write_text("{broken", encoding="utf-8")
        _, errors, _, _ = run_source(repo)
        self.assertTrue(any("not valid JSON" in e for e in errors), msg=str(errors))


GOOD_SITE = {
    "guides/index.html": (
        '<html><head><title>Guides</title></head><body>'
        '<nav class="web-nav" aria-label="Documentation">'
        f'<a href="{BASEURL}/index.html">Home</a><a href="{BASEURL}/explainer.html">E</a>'
        f'<a href="{BASEURL}/catalogue.html">C</a><a href="{BASEURL}/guide.html">G</a>'
        f'<a href="{BASEURL}/changelog.html">Ch</a><a href="{BASEURL}/docs/">Docs</a><a href="{BASEURL}/guides/">Guides</a>'
        f'<a href="{BASEURL}/specs/">Specs</a><a href="{BASEURL}/decisions/">Decisions</a>'
        '</nav>'
        '<ul class="section-children"><li><a href="' + BASEURL + '/guides/foo/">Foo</a></li></ul>'
        "</body></html>"
    ),
    "guides/foo/index.html": (
        '<html><head><title>Foo</title></head><body>'
        '<nav class="web-nav" aria-label="Documentation">'
        f'<a href="{BASEURL}/index.html">Home</a><a href="{BASEURL}/explainer.html">E</a>'
        f'<a href="{BASEURL}/catalogue.html">C</a><a href="{BASEURL}/guide.html">G</a>'
        f'<a href="{BASEURL}/changelog.html">Ch</a><a href="{BASEURL}/docs/">Docs</a><a href="{BASEURL}/guides/">Guides</a>'
        f'<a href="{BASEURL}/specs/">Specs</a><a href="{BASEURL}/decisions/">Decisions</a>'
        '</nav>'
        f'<nav class="breadcrumbs" aria-label="Breadcrumb"><ol>'
        f'<li><a href="{BASEURL}/index.html">Home</a></li>'
        f'<li><a href="{BASEURL}/guides/">Guides</a></li></ol></nav>'
        '<div class="site-sidebar"><a href="' + BASEURL + '/guides/">Overview</a></div>'
        '<h1 id="the-goal">The goal</h1>'
        "</body></html>"
    ),
    "specs/index.html": (
        '<html><head><title>Specs</title></head><body>'
        '<nav class="web-nav" aria-label="Documentation">'
        f'<a href="{BASEURL}/index.html">Home</a><a href="{BASEURL}/explainer.html">E</a>'
        f'<a href="{BASEURL}/catalogue.html">C</a><a href="{BASEURL}/guide.html">G</a>'
        f'<a href="{BASEURL}/changelog.html">Ch</a><a href="{BASEURL}/docs/">Docs</a><a href="{BASEURL}/guides/">Guides</a>'
        f'<a href="{BASEURL}/specs/">Specs</a><a href="{BASEURL}/decisions/">Decisions</a>'
        '</nav></body></html>'
    ),
    "decisions/index.html": (
        '<html><head><title>Decisions</title></head><body>'
        '<nav class="web-nav" aria-label="Documentation">'
        f'<a href="{BASEURL}/index.html">Home</a><a href="{BASEURL}/explainer.html">E</a>'
        f'<a href="{BASEURL}/catalogue.html">C</a><a href="{BASEURL}/guide.html">G</a>'
        f'<a href="{BASEURL}/changelog.html">Ch</a><a href="{BASEURL}/docs/">Docs</a><a href="{BASEURL}/guides/">Guides</a>'
        f'<a href="{BASEURL}/specs/">Specs</a><a href="{BASEURL}/decisions/">Decisions</a>'
        '</nav></body></html>'
    ),
    "docs/index.html": (
        "<html><body>"
        f'<a href="{BASEURL}/guides/">Guides</a>'
        f'<a href="{BASEURL}/specs/">Specs</a>'
        f'<a href="{BASEURL}/decisions/">Decisions</a>'
        "</body></html>"
    ),
}


LEGACY_PAGES = {
    name: f"<html><body><nav class=\"web-nav\"><a href=\"./index.html\">H</a><a href=\"./explainer.html\">E</a><a href=\"./catalogue.html\">C</a><a href=\"./guide.html\">G</a><a href=\"./changelog.html\">Ch</a><a href=\"./docs/\">Docs</a></nav>{name} legacy page</body></html>"
    for name in ("index.html", "explainer.html", "catalogue.html", "guide.html", "changelog.html")
}


def make_rendered_repo(extra_site=None, files=None):
    base_files = {
        "docs/_config.yml": f"baseurl: {BASEURL}\npermalink: pretty\n",
        "docs/specs/README.md": "# Specs\n\nIndex.\n",
        "docs/decisions/README.md": "# Decisions\n\nIndex.\n",
    }
    base_files.update(files or {})
    repo = make_repo(
        files=dict(base_files, **{
            "docs/guides/README.md": "# Guides\n\nIndex.\n",
            "docs/guides/foo.md": "---\ntitle: Foo\nsection: guides\n---\n\n# Foo\n",
            "docs/README.md": "# Documentation\n\n[Guides](guides/README.md)\n",
        }),
        nav=NAV)
    site = repo / "docs" / "_site"
    for rel, content in {**GOOD_SITE, **LEGACY_PAGES}.items():
        path = site / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
    for rel, content in (extra_site or {}).items():
        path = site / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
    return repo


def run_rendered(repo, *args):
    return run_checker(repo, "--site", str(repo / "docs" / "_site"), *args)


class RenderedFixtureTests(unittest.TestCase):
    def test_good_site_passes_strict(self):
        repo = make_rendered_repo()
        code, errors, warnings, _ = run_rendered(repo, "--strict")
        self.assertEqual(0, code, msg="\n".join(errors + warnings))
        self.assertEqual([], errors)

    def test_repository_links_retain_source_prefix_and_directory_kind(self):
        repo = make_rendered_repo(
            files={"docs/archive/review.md": "# Historical review"},
            extra_site={"bad.html": '<a href="https://github.com/awaterma/phronesis/blob/main/archive/review.md">review</a>'})
        _, errors, _, _ = run_rendered(repo)
        self.assertTrue(any("broken repository link" in error for error in errors))
        (repo / "docs/_site/bad.html").write_text(
            '<a href="https://github.com/awaterma/phronesis/blob/main/docs/archive/review.md">review</a>'
            '<a href="https://github.com/awaterma/phronesis/tree/main/docs/archive">archive</a>')
        _, errors, _, _ = run_rendered(repo)
        self.assertFalse(any("repository" in error for error in errors), errors)

    def test_broken_rendered_link(self):
        repo = make_rendered_repo(extra_site={
            "guides/foo/bad.html": f'<a href="{BASEURL}/nope.html">x</a>'})
        _, errors, _, _ = run_rendered(repo)
        self.assertTrue(any("broken rendered link" in e for e in errors), msg=str(errors))

    def test_broken_rendered_anchor(self):
        repo = make_rendered_repo(extra_site={
            "guides/foo/bad.html": '<a href="../../guides/foo/index.html#missing">x</a>'})
        _, errors, _, _ = run_rendered(repo)
        self.assertTrue(any("anchor '#missing'" in e for e in errors), msg=str(errors))

    def test_archive_exclusion_audit(self):
        repo = make_rendered_repo(extra_site={"archive/old.html": "<html></html>"})
        _, errors, _, _ = run_rendered(repo)
        self.assertTrue(any("excluded material in published site: archive/old.html" in e
                            for e in errors), msg=str(errors))

    def test_hidden_state_excluded(self):
        repo = make_rendered_repo(extra_site={
            ".superpowers/x.html": "<html></html>", ".scratch/y.md": "draft"})
        _, errors, _, _ = run_rendered(repo)
        self.assertTrue(any("hidden state in published site: .superpowers/x.html" in e
                            for e in errors), msg=str(errors))
        self.assertTrue(any("hidden state in published site: .scratch/y.md" in e
                            for e in errors), msg=str(errors))

    def test_agent_state_and_logs_excluded(self):
        repo = make_rendered_repo(extra_site={
            ".phronesis/log.jsonl": "{}\n", "run.log": "x\n"})
        _, errors, _, _ = run_rendered(repo)
        self.assertTrue(any("agent-local state" in e for e in errors), msg=str(errors))
        self.assertTrue(any("raw log" in e for e in errors), msg=str(errors))

    def test_unconverted_markdown_link_flagged(self):
        repo = make_rendered_repo(extra_site={
            "guides/foo.md": "raw static markdown copy",
            "bad.html": '<a href="guides/foo.md">x</a>'})
        # A relative .md link that resolves to a docs source file that has
        # front matter must be flagged: it should have been converted.
        _, errors, _, _ = run_rendered(repo)
        self.assertTrue(any("unconverted markdown link" in e for e in errors), msg=str(errors))

    def test_generated_redirects_preserve_published_markdown_source_urls(self):
        repo = make_rendered_repo()
        migration = repo / "docs/migration-map.json"
        migration.write_text('{"moves": [{"from": "docs/old.md", "to": "docs/guides/foo.md"}]}')
        result = subprocess.run([
            sys.executable, str(Path(__file__).resolve().parents[1] / "website/gen_redirects.py"),
            "--map", str(migration), "--site", str(repo / "docs/_site")], capture_output=True, text=True)
        self.assertEqual(0, result.returncode, result.stderr)
        self.assertIn("/phronesis/guides/foo/", (repo / "docs/_site/old.md").read_text())
        _, errors, _, _ = run_rendered(repo)
        self.assertFalse(any("redirect" in error or "forwarding" in error for error in errors), errors)

    def test_redirect_stub_missing_and_bad_target(self):
        repo = make_rendered_repo()
        (repo / "docs" / "migration-map.json").write_text(
            '{"moves": [{"from": "docs/old.md", "to": "docs/guides/foo.md", "note": "moved"}]}',
            encoding="utf-8")
        _, errors, _, _ = run_rendered(repo)
        self.assertTrue(any("missing redirect stub for docs/old.md" in e for e in errors), msg=str(errors))

        stub = repo / "docs" / "_site" / "old.html"
        stub.write_text('<meta http-equiv="refresh" content="0; url=/phronesis/gone.html">',
                        encoding="utf-8")
        _, errors, _, _ = run_rendered(repo)
        self.assertTrue(any("points at missing target '/phronesis/gone.html'" in e
                            for e in errors), msg=str(errors))

        stub.write_text(f'<meta http-equiv="refresh" content="0; url={BASEURL}/guides/foo/">',
                        encoding="utf-8")
        code, errors, _, _ = run_rendered(repo, "--strict")
        self.assertEqual([], [e for e in errors if "redirect" in e], msg=str(errors))

        stub.write_text('<meta http-equiv="refresh" content="0; url=https://github.com/awaterma/phronesis/blob/main/docs/archive/old.md">',
                        encoding="utf-8")
        code, errors, _, _ = run_rendered(repo)
        self.assertEqual([], [e for e in errors if "redirect" in e], msg=str(errors))

    def test_section_index_must_exist_and_link_children(self):
        repo = make_rendered_repo()
        (repo / "docs" / "_site" / "guides" / "index.html").unlink()
        _, errors, _, _ = run_rendered(repo, "--strict")
        self.assertTrue(any("index page '/guides/' is not in the built site" in e
                            for e in errors), msg=str(errors))

        repo2 = make_rendered_repo()
        idx = repo2 / "docs" / "_site" / "guides" / "index.html"
        idx.write_text(idx.read_text(encoding="utf-8").replace(
            f'href="{BASEURL}/guides/foo/"', 'href="#nothing"'), encoding="utf-8")
        _, errors, _, _ = run_rendered(repo2)
        self.assertTrue(any("does not link rendered child" in e for e in errors), msg=str(errors))

    def test_section_page_must_link_back_to_index(self):
        repo = make_rendered_repo()
        page = repo / "docs" / "_site" / "guides" / "foo" / "index.html"
        page.write_text(page.read_text(encoding="utf-8").replace(
            f'href="{BASEURL}/guides/"', 'href="#nothing"'), encoding="utf-8")
        _, errors, _, _ = run_rendered(repo)
        self.assertTrue(any("does not link its section index" in e for e in errors), msg=str(errors))

    def test_shared_top_navigation_links(self):
        repo = make_rendered_repo()
        page = repo / "docs" / "_site" / "guides" / "foo" / "index.html"
        page.write_text(page.read_text(encoding="utf-8").replace(
            f'href="{BASEURL}/explainer.html"', 'href="#nothing"'), encoding="utf-8")
        _, errors, _, _ = run_rendered(repo)
        self.assertTrue(any("does not reach '/phronesis/explainer.html'" in e for e in errors),
                        msg=str(errors))

    def test_landing_page_must_link_sections(self):
        repo = make_rendered_repo()
        landing = repo / "docs" / "_site" / "docs" / "index.html"
        landing.write_text(landing.read_text(encoding="utf-8").replace(
            f'href="{BASEURL}/specs/"', 'href="#nothing"'), encoding="utf-8")
        _, errors, _, _ = run_rendered(repo)
        self.assertTrue(any("landing does not link section 'specs'" in e for e in errors),
                        msg=str(errors))

    def test_missing_landing_warns_non_strict(self):
        repo = make_rendered_repo()
        (repo / "docs" / "README.md").unlink()
        code, errors, warnings, _ = run_rendered(repo)
        self.assertEqual(0, code)
        self.assertEqual([], errors)


class GenRedirectsTests(unittest.TestCase):
    def _run(self, repo, *args):
        sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "website"))
        import gen_redirects as gr
        old_argv = sys.argv
        sys.argv = ["gen_redirects.py", "--site", str(repo / "docs" / "_site"),
                    "--map", str(repo / "docs" / "migration-map.json"), *args]
        try:
            out = io.StringIO()
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()) as err:
                code = gr.main()
        finally:
            sys.argv = old_argv
        return code, out.getvalue(), err.getvalue()

    def test_targets_use_pretty_urls_and_github_for_archive(self):
        repo = make_rendered_repo()
        (repo / "docs" / "migration-map.json").write_text(
            '{"moves": ['
            '{"from": "docs/old.md", "to": "docs/guides/foo.md", "note": "moved"},'
            '{"from": "docs/ancient.md", "to": "docs/archive/designs/old.md", "note": "archived"}]}',
            encoding="utf-8")
        code, out, _ = self._run(repo)
        self.assertEqual(0, code, msg=out)
        plain = (repo / "docs" / "_site" / "old.html").read_text(encoding="utf-8")
        self.assertIn(f'url={BASEURL}/guides/foo/">', plain)
        self.assertNotIn("guides/foo.html", plain)
        arch = (repo / "docs" / "_site" / "ancient.html").read_text(encoding="utf-8")
        self.assertIn("https://github.com/awaterma/phronesis/blob/main/docs/archive/designs/old.md", arch)

    def test_invalid_map_fails_cleanly(self):
        repo = make_rendered_repo()
        (repo / "docs" / "migration-map.json").write_text("{broken", encoding="utf-8")
        code, _, err = self._run(repo)
        self.assertEqual(2, code)
        self.assertIn("not valid JSON", err)
        (repo / "docs" / "migration-map.json").write_text('{"moves": ["x"]}', encoding="utf-8")
        code, _, err = self._run(repo)
        self.assertEqual(2, code)
        self.assertIn("must be an object", err)


if __name__ == "__main__":
    unittest.main()

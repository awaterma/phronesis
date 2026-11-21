---
title: Documentation website contract
layout: doc
section: contributing
description: How the docs site renders, the checks it enforces, and how deployment is gated.
status: shipped
permalink: /contributing/website.html
---

# Documentation website contract

The site source is `docs/`. GitHub reading and website reading use the same
files: markdown that is fine on GitHub is fine on the website, and the build
converts GitHub-style relative links to rendered URLs.

## Rendering

- Pages **with** front matter render through the shared layouts in
  `docs/_layouts/` with top navigation, section sidebar, breadcrumbs, and
  mobile styling (`docs/assets/site.css`, same mineral palette as the
  hand-crafted pages).
- A markdown file under `docs/<section>/` **without** front matter is
  rendered through the same layouts automatically (`docs/_plugins/
  canonical_pages.rb`): the section comes from its directory and the title
  from its first H1, and the raw `.md` copy stops shipping. A section's
  `README.md` becomes the section index at `/<section>/`; `docs/README.md`
  becomes the docs landing at `/docs/`. Content files need no front matter
  at all — the only exception is `docs/specs/`, whose YAML `title` +
  `status` are the lifecycle surface.
- Rendered section pages live at `/<section>/<page>/` (Jekyll "pretty");
  the five hand-crafted `.html` pages keep their published URLs.
- Documents quoting Rust format strings (a literal `{{`) get Liquid
  processing disabled automatically (`render_with_liquid: false`); their
  markdown still renders.

Front matter is parsed as real YAML (PyYAML, pinned in
`scripts/checker-requirements.txt`). Two separate vocabularies are
validated, so decision records never need website keys:

**Decision records (ADRs)** — `docs/decisions/*.md` with `id`, `date`,
`status`:

| Key | Meaning |
|---|---|
| `id` | Required, non-empty string. |
| `date` | Required, `YYYY-MM-DD`. |
| `status` | Required: `proposed`, `accepted`, or `superseded` (the same vocabulary `phr-mcp wiki-drift` enforces). |
| `enforces`, `superseded_by`, `tags` | Optional; list / id string / list of strings. |

`title` and `section` are **not** ADR keys: they derive from the page's
first `# heading` and its canonical path (`docs/decisions/`), exactly as
the renderer derives them. Unknown keys or wrong types fail with an
actionable error.

**Website pages** — every other rendered page:

| Key | Meaning |
|---|---|
| `title` | Required (or a first `# Heading` to derive it from). Page title in the header, sidebar, and breadcrumbs. |
| `section` | Required (or derived from the canonical path `docs/<section>/`). One of the section `id`s in `docs/_data/navigation.yml`. |
| `status` | Optional `shipped`, `proposed`, `partial`, `historical`; required under `docs/specs/`. |
| `description` | Optional. Used in the page standfirst and section listings. |
| `nav_order` | Optional integer; sidebar and section listings sort by it. |
| `redirect_from` | Optional: a single site path or a list of them; each must start with `/`. |
| `permalink` | Optional; use when the rendered path must differ from the source path. |

Malformed YAML fails with an actionable error naming the file and line.

Section index pages use `layout: section` and list their children
automatically. Every section in `docs/_data/navigation.yml` carries
`index: /<section>/` — the rendered URL of its canonical
`docs/<section>/README.md`. The checker verifies that each index URL
exists in the built site, that section index pages link every child
page, that section pages link back through sidebar and breadcrumbs,
and that the `/docs/` landing links every section.

## Checks

One shared command for every lane (CI uses the same stages). It needs
PyYAML (`python3 -m pip install -r scripts/checker-requirements.txt`):

```sh
python3 scripts/check_docs.py --source-only          # no build needed
python3 scripts/check_docs.py --site docs/_site      # adds rendered checks
python3 scripts/check_docs.py --site docs/_site --strict  # post-migration gate
```

Stages: source links and anchors, front-matter metadata (typed YAML),
navigation reachability, catalogue structure/version, changelog consistency,
and — with a built site — rendered links, rendered anchors, section-index
reachability, exclusion audit, and redirect validation. The separate `scripts/check-catalogue.sh` command compares the page with
the compiled shipped rules and runs in CI; it accepts a built `phr-mcp` path
as its optional first argument. Regression
fixtures for reproduced checker failures live in
`scripts/tests/` (`python3 -m unittest discover -s scripts/tests`).

## Excluded from the published artifact

`docs/archive/`, `docs/superpowers/`, agent-local state (`.phronesis/`),
and raw run logs are excluded from the site build. Only the archive belongs in Git; the
curated archive is linked from contributor documentation instead.

## Redirects

Document moves are recorded in a machine-readable migration map
(`docs/migration-map.json`, maintained by the content migration lane):

```json
{ "schema_version": 1, "moves": [ { "from": "docs/old.md", "to": "docs/new.md", "note": "why" } ] }
```

`python3 scripts/website/gen_redirects.py --site docs/_site` runs after the
build and writes a redirect stub for every mapped move whose old path was
a published site URL (never-published paths under `docs/archive/` and
`docs/superpowers/` are skipped), so published URLs keep resolving.
Moved `.md` documents render at directory URLs (`permalink: pretty`),
and archive targets point at the GitHub blob instead. Old `.md` source URLs
retain generated Markdown forwarding documents; `.html` URLs redirect.

## GitHub and website links

Relative links that resolve to a rendered page become site URLs during
rendering. Any other in-repository target (crate sources, configs,
archive copies, raw documents) is converted to a GitHub blob or tree URL
at render time (`docs/_plugins/site_links.rb`), because GitHub remains
the canonical reading surface for files the website does not publish.
Link fragments are preserved on both conversions.

## Build

The build environment is locked two ways: `docs/Gemfile` +
`docs/Gemfile.lock` pin Jekyll 4.3.4 on Ruby 3.3.14 (`docs/.ruby-version`;
CI's `ruby/setup-ruby` reads it), and `scripts/website/build.sh` pins the
container image **by digest** (`docs/.build-image-digest`, a
multi-architecture manifest digest verified to run Ruby 3.3.14) — never
a floating tag. Builds remove only the freshly generated `docs/_site`;
`.jekyll-cache` and the bundler volume are caches and are preserved.
CI runs the same locked bundle (`.github/workflows/docs.yml`), which
also installs the pinned checker dependencies and runs the regression
fixtures.

## Deployment

CI uploads a preview artifact on every docs-affecting pull request. Going
live is a **manual, gated action**: run the *Docs site* workflow with
`deploy: true` (configure required reviewers on the `github-pages` environment before enabling deployment), and only
after preview, link checks, and visual review pass. Switching the
repository's Pages source from branch deployment to Actions is the
collaborator's one-time manual step; nothing in CI flips it
automatically.

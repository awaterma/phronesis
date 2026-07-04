# Doc web + unified versioning — design

- **Date:** 2026-07-04
- **Status:** Approved (design)
- **Branch (planned):** `feat/doc-web`, off `main` **after PR #5 (MCP-crate decomposition) merges**
- **Release:** 0.17.0 — the version-unification release (MINOR: new subcommand + lockstep adoption)

## Context

The GitHub Pages site (Pages serves `main:/docs`) consists of three
hand-authored HTML pages: `index.html` (landing), `explainer.html`
(essay), `catalogue.html` (rule reference). They interlink each other
and the GitHub repo but reference none of the markdown corpus (specs,
plans, loop-programming guide, CHANGELOG, crate READMEs), and the
catalogue is 0.14-era — stale against the current 72-rule packs.
Separately, the three crates carry three unrelated versions
(phronesis 0.14.0, phronesis-mcp 0.16.2, phronesis-rhai 0.1.0), which
complicates the doc story and the planned crates.io publish.

Decisions taken during brainstorming (2026-07-04):
1. **Hub:** the Pages site is the front door; markdown docs are linked
   from it as GitHub blob URLs. No static-site generator.
2. **Curation:** reader-facing set only — loop-programming guide,
   CHANGELOG, three crate READMEs, four landmark design docs (journey
   facts, confidence scoring, rhai evaluator, MCP-crate
   decomposition). Working specs/plans stay unlinked.
3. **Catalogue:** refresh AND make reproducible via a generator.
4. **Generator shape:** approach A — a `phr-mcp catalogue` subcommand.
5. **Numbering:** lockstep workspace versioning at the larger release
   value; the docs stamp that number.

## Design

### 1. Unified versioning (lockstep at 0.17.0)

- Root `Cargo.toml` gains `[workspace.package] version = "0.17.0"`;
  all three crates switch to `version.workspace = true`.
- Internal dependency declarations move to the unified number:
  `phronesis-rhai → phronesis = "0.17"`,
  `phronesis-mcp → phronesis = "0.17"`, `phronesis-mcp →
  phronesis-rhai = "0.17"`.
- Pre-1.0 jumps (0.14.0→0.17.0, 0.1.0→0.17.0) are legal; the
  CHANGELOG 0.17.0 entry explains the unification once and notes that
  from now on one number covers the workspace.
- The decomposition changes currently under `## [Unreleased]` fold
  into the 0.17.0 entry.

### 2. `phr-mcp catalogue` subcommand

- `phr-mcp catalogue [--out <path>]` (default `docs/catalogue.html`
  relative to the repo root; `--check` flag optional future work, not
  in scope).
- Reads the shipped packs through the same composition path `init`
  uses, so the rendered rules are exactly what `phr-mcp init` writes.
- Renders each rule as the catalogue's existing entry markup (anchor
  id from the rule id, level, phase, message, predicate summary) and
  splices the result between `<!-- BEGIN GENERATED RULES -->` /
  `<!-- END GENERATED RULES -->` markers in the existing page. The
  hand-authored frame — intro prose, styling, fonts, footer — is
  preserved byte-for-byte outside the markers.
- The generated section header carries the version stamp: "documents
  the default packs as of v0.17.0", sourced from
  `env!("CARGO_PKG_VERSION")`.
- The existing catalogue entries are replaced by generated ones in a
  one-time markerization + regeneration; from then on the page is a
  committed, regenerable artifact and regeneration is a
  release-checklist step (documented in the crate CLAUDE.md).

### 3. The web

- `index.html`: new Documentation section listing the curated set as
  GitHub blob links (`https://github.com/awaterma/phronesis/blob/main/...`):
  `docs/loop-programming-guide.md`, `CHANGELOG.md`, `README.md`,
  `crates/phronesis/README.md`, `crates/phronesis-mcp/README.md`,
  `crates/phronesis-rhai/README.md`, and the four landmark designs
  (`docs/specs/SPEC-journey-facts.md`, `docs/specs/SPEC-confidence-scoring.md`,
  `docs/superpowers/specs/2026-06-01-rhai-script-evaluator-design.md`,
  `docs/superpowers/specs/2026-06-28-mcp-crate-decomposition-design.md`).
- All three pages share a compact footer nav — Home · Explainer ·
  Catalogue · Guide · Changelog · GitHub — plus the version stamp,
  styled to match each page (same markup fragment, page-local CSS).
- Backlinks: the top-level `README.md` and
  `docs/loop-programming-guide.md` gain a pointer to the site.
- **Hard constraint:** zero occurrences of the private downstream
  consumer's name anywhere in this public surface; a mechanical grep
  gate runs before every commit of this work.

### 4. Verification

- Generator unit tests: entry count equals composed pack rule count;
  anchor ids unique; markers preserved (content outside them
  untouched); output contains the version stamp; regenerating twice
  is byte-idempotent.
- Link hygiene test: every internal `href="#..."` emitted by the
  generator resolves to a generated anchor.
- Standard gates per commit: workspace tests, clippy `-D warnings`,
  fmt, audit non-increasing (stays 8).
- Manual: Pages renders correctly after merge (spot-check the three
  pages).

## Out of scope

- Rendering markdown into the site (static-site generator).
- A catalogue-drift audit rule (`packs changed but catalogue didn't`)
  — noted as a natural follow-up.
- crates.io publish itself (separate effort; this unification is a
  prerequisite it wants).

## Done-when

- All three crates build and publish-dry-run at 0.17.0 with
  `version.workspace = true`.
- `phr-mcp catalogue` regenerates `docs/catalogue.html` idempotently;
  the page documents all current pack rules with the 0.17.0 stamp.
- The three pages interlink via the footer nav; index.html lists the
  curated markdown set; README and the loop guide link back.
- Grep gate clean; all standard gates green; audit total still 8.

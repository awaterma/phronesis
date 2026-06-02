# Crates.io Publish & Open-Source Prep

**Date:** 2026-06-01
**Status:** Draft
**Scope:** Approach A — minimal publish-ready, no CHANGELOG/CI/badges

## Goal

Publish both `phronesis` (core RETE engine) and `phronesis-mcp`
(MCP server + CLI) to crates.io, and make the GitHub repo public.
Both crates ship at version **0.9.0**.

## Decisions

- **Shared version:** Both crates use 0.9.0, signaling "nearly 1.0."
- **Per-crate READMEs:** Each crate gets a focused README for its
  crates.io page. The workspace README stays as the GitHub landing page.
- **Publish order:** `phronesis` first (it's a dependency), then
  `phronesis-mcp`.
- **Git tag:** `v0.9.0` after both publishes succeed.

## Work items

### 1. Version alignment

Bump `phronesis` from 0.1.0 → 0.9.0 in its `Cargo.toml`.
Update the `phr` dependency version in `phronesis-mcp/Cargo.toml`
from `0.1.0` → `0.9.0`.

`phronesis-mcp` is already at 0.9.0 — no change needed there.

### 2. Cargo.toml metadata (already started)

The uncommitted changes add `repository`, `homepage`, `readme`,
`keywords`, `categories` to both crates. Land these, plus:

- Workspace `Cargo.toml`: add `homepage` to `[workspace.package]`.
- Both crates: set `readme` to the per-crate README path (see §3).
- Both crates: verify `license = "MIT"` is present (it is).

### 3. Per-crate README files

**`crates/phronesis/README.md`** — focused on the core engine:
what it is, the push/pull pattern, a minimal code example
(reference the existing `examples/push_and_pull.rs`), link to
docs.rs and the workspace README for full docs.

**`crates/phronesis-mcp/README.md`** — focused on the MCP
server/CLI: what it does, quick-start (`cargo install`, `phr-mcp
install`, `phr-mcp init`), link to the explainer and catalogue.

Both should be concise (under 80 lines). Update `readme = "README.md"`
in each crate's Cargo.toml (pointing to the local file, not `../../`).

### 4. Fix rustdoc warnings

Two broken intra-doc links:
- `crates/phronesis/src/ids.rs:12` — `Self::new` (no such item)
- `crates/phronesis/src/push.rs:10` — `ReteNetwork::execute_all_agenda_items`
  (type not in scope)

Fix or reword the doc comments so `cargo doc` is warning-free.

### 5. Package exclude

Add `[package]` `exclude` to `phronesis-mcp/Cargo.toml` to trim
test fixtures, BDD features, and other non-essential files from the
published crate:

```toml
exclude = ["tests/", "features/"]
```

The core `phronesis` crate is already lean (252 KiB packaged) — no
exclude needed.

### 6. GitHub repo metadata

Before or at publish time:
- Set repo **description**: "A RETE rules engine for durable
  enforcement of project conventions in LLM-assisted work."
- Set **homepage URL**: `https://awaterma.github.io/phronesis/`
- Add **topics**: `rules-engine`, `rete`, `mcp`, `llm`,
  `claude-code`, `code-quality`, `developer-tools`
- Make the repo **public**.

### 7. Dry-run verification

After all edits:
```
cargo publish --dry-run -p phronesis
cargo publish --dry-run -p phronesis-mcp
```
Both must pass with no errors. Warnings about README path should be
gone with per-crate READMEs.

### 8. Publish

```
cargo publish -p phronesis
cargo publish -p phronesis-mcp
```

`phronesis` must be published and indexed before `phronesis-mcp`
can resolve its dependency.

### 9. Tag

```
git tag v0.9.0
git push origin v0.9.0
```

## Out of scope (follow-up)

- CHANGELOG.md
- CONTRIBUTING.md / CODE_OF_CONDUCT.md
- GitHub Actions CI
- Badges (crates.io, docs.rs, CI status)
- GitHub issue/PR templates
- GitHub release (beyond the tag)
- `cargo-release` automation

## Risks

- **crates.io name availability:** Verified — both `phronesis` and
  `phronesis-mcp` are unclaimed as of 2026-06-01.
- **Index propagation delay:** After publishing `phronesis`,
  `phronesis-mcp` publish may fail if the index hasn't updated.
  Wait ~60s or retry.

# Crates.io Publish & Open-Source Prep — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Publish both `phronesis` 0.9.0 and `phronesis-mcp` 0.9.0 to crates.io and make the GitHub repo public.

**Architecture:** Both crates already build and pass tests. The work is metadata, docs, and packaging — no logic changes. The core crate must be published first since phronesis-mcp depends on it.

**Tech Stack:** Rust/Cargo, `gh` CLI for GitHub metadata, `cargo publish` for crates.io.

---

### Task 1: Version alignment + Cargo.toml metadata

**Files:**
- Modify: `crates/phronesis/Cargo.toml:3` (version 0.1.0 → 0.9.0)
- Modify: `crates/phronesis-mcp/Cargo.toml:24` (phr dep version 0.1.0 → 0.9.0)
- Modify: `Cargo.toml` (workspace — add homepage)

The uncommitted changes in both crate Cargo.tomls already add `repository`, `homepage`, `readme`, `keywords`, `categories`, and the updated `description`. This task lands those plus the version bump and workspace homepage.

- [ ] **Step 1: Bump phronesis version to 0.9.0**

In `crates/phronesis/Cargo.toml`, change:

```toml
version = "0.1.0"
```

to:

```toml
version = "0.9.0"
```

- [ ] **Step 2: Update the phr dependency version**

In `crates/phronesis-mcp/Cargo.toml`, the `phr` dependency line should read:

```toml
phr = { version = "0.9.0", path = "../phronesis", package = "phronesis", features = ["schemars"] }
```

- [ ] **Step 3: Add homepage to workspace Cargo.toml**

In `Cargo.toml` (workspace root), add `homepage` to `[workspace.package]`:

```toml
[workspace.package]
edition = "2024"
rust-version = "1.90"
license = "MIT"
authors = ["Andrew Waterman"]
repository = "https://github.com/awaterma/phronesis"
homepage = "https://github.com/awaterma/phronesis"
```

- [ ] **Step 4: Verify both crates compile**

Run: `cargo check --workspace`
Expected: success, no errors.

- [ ] **Step 5: Run tests**

Run: `cargo test --workspace`
Expected: all 97+ tests pass.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/phronesis/Cargo.toml crates/phronesis-mcp/Cargo.toml
git commit -m "chore: align both crates to 0.9.0 + enrich Cargo.toml metadata for crates.io"
```

---

### Task 2: Per-crate README files

**Files:**
- Create: `crates/phronesis/README.md`
- Create: `crates/phronesis-mcp/README.md`
- Modify: `crates/phronesis/Cargo.toml` (readme field)
- Modify: `crates/phronesis-mcp/Cargo.toml` (readme field)

- [ ] **Step 1: Write crates/phronesis/README.md**

```markdown
# phronesis

A domain-neutral RETE rules engine for durable, context-window-independent
enforcement of project conventions in LLM-assisted work.

Rules live on disk. They fire deterministically against asserted facts.
The **consequences** of those firings — not raw state — are what an LLM sees.

## The pattern

Two transports of the same idea:

- **Push** — rule fires -> `Consequence` -> `Actor` consumes it.
- **Pull** — actor asks -> deterministic `Lookup` returns a `Consequence`.

The crate defines the types and the engine; integration with any particular
host (an MCP server, a game engine, a conversational module) lives outside
this crate.

## Quick example

```rust
use phronesis::{Consequence, ConsequenceKind, Lookup, lookup_as_consequence};

// Pull mode: invoke a deterministic Lookup and wrap the result.
struct AdderTool;

impl Lookup for AdderTool {
    type Request = (i64, i64);
    type Response = serde_json::Value;

    fn name(&self) -> &'static str { "adder" }
    fn schema_version(&self) -> u8 { 1 }

    fn invoke(&self, req: Self::Request) -> anyhow::Result<Self::Response> {
        let (a, b) = req;
        Ok(serde_json::json!({ "sum": a + b }))
    }
}

let consequence = lookup_as_consequence(&AdderTool, (2, 2)).unwrap();
assert_eq!(consequence.kind, ConsequenceKind::Observation);
```

See `examples/push_and_pull.rs` for the full push + pull pattern.

## Companion crate

[`phronesis-mcp`](https://crates.io/crates/phronesis-mcp) wraps this
engine behind an MCP server and CLI (`phr-mcp`) for use with Claude Code
and Gemini CLI.

## Documentation

- [API docs (docs.rs)](https://docs.rs/phronesis)
- [The Explainer](https://awaterma.github.io/phronesis/explainer.html) — long-form essay on the engine and RETE algorithm
- [The Catalogue](https://awaterma.github.io/phronesis/catalogue.html) — visual reference of starter rules

## License

MIT
```

- [ ] **Step 2: Write crates/phronesis-mcp/README.md**

```markdown
# phronesis-mcp

MCP server and CLI (`phr-mcp`) wrapping the
[phronesis](https://crates.io/crates/phronesis) RETE rules engine for
durable enforcement of project conventions in LLM-assisted work.

Rules live on disk in `.phronesis/rules.json`, fire from outside the
context window at every tool call, and cannot be compressed away.

## Quick start

```sh
# Install the binary
cargo install phronesis-mcp

# Register as a global MCP server (Claude Code + Gemini CLI)
phr-mcp install

# Initialize in your project
cd /your/project
phr-mcp init --packs llm,rust
```

## What it does

- **Pre/post hooks** fire rules against every file edit, blocking violations
  and warning on anti-patterns.
- **MCP tools** let the model query rules, fire the engine, audit the tree,
  and detect drift between prose guidance and enforced rules.
- **Starter packs** ship rules for Rust, Python, TypeScript, Swift, Rhai,
  and LLM-behavior (deflection, unverified claims).
- **Wiki decisions** — ADR-style pages in `.phronesis/wiki/decisions/`
  travel with the repo and are scored against rules for coverage.

## Commands

```
phr-mcp serve             # MCP stdio server
phr-mcp pre-check         # PreToolUse hook
phr-mcp post-check        # PostToolUse hook
phr-mcp init              # One-command project setup
phr-mcp audit             # Whole-tree rule sweep
phr-mcp stats             # Per-rule activity summary
phr-mcp wiki-drift        # Decision/rule coverage gaps
phr-mcp decision new <s>  # Scaffold an ADR page
```

## Documentation

- [The Explainer](https://awaterma.github.io/phronesis/explainer.html) — how the engine works
- [The Catalogue](https://awaterma.github.io/phronesis/catalogue.html) — starter rules reference
- [Command Reference](https://github.com/awaterma/phronesis/blob/main/crates/phronesis-mcp/CLAUDE.md) — full CLI surface

## License

MIT
```

- [ ] **Step 3: Update readme fields in both Cargo.tomls**

In `crates/phronesis/Cargo.toml`, change:

```toml
readme = "../../README.md"
```

to:

```toml
readme = "README.md"
```

Same change in `crates/phronesis-mcp/Cargo.toml`.

- [ ] **Step 4: Commit**

```bash
git add crates/phronesis/README.md crates/phronesis-mcp/README.md \
       crates/phronesis/Cargo.toml crates/phronesis-mcp/Cargo.toml
git commit -m "docs: per-crate README files for crates.io pages"
```

---

### Task 3: Fix rustdoc warnings

**Files:**
- Modify: `crates/phronesis/src/ids.rs:12`
- Modify: `crates/phronesis/src/push.rs:10`

- [ ] **Step 1: Fix ids.rs broken link**

In `crates/phronesis/src/ids.rs`, the module-level doc says `[Self::new]` but `Self` at module scope doesn't refer to any type. Change line 12 from:

```rust
//! [`Self::new`] constructor accepts anything `Into<String>`. The point
```

to:

```rust
//! `new()` constructor accepts anything `Into<String>`. The point
```

- [ ] **Step 2: Fix push.rs broken link**

In `crates/phronesis/src/push.rs`, line 10 references `[ReteNetwork::execute_all_agenda_items]` but `ReteNetwork` is not in scope for the module doc. Change line 10 from:

```rust
//! [`ReteNetwork::execute_all_agenda_items`] returns a `Vec<Action>` —
```

to:

```rust
//! [`ReteNetwork::execute_all_agenda_items`](crate::network::ReteNetwork::execute_all_agenda_items) returns a `Vec<Action>` —
```

- [ ] **Step 3: Verify rustdoc is clean**

Run: `cargo doc --workspace --no-deps 2>&1 | grep warning`
Expected: no output (no warnings).

- [ ] **Step 4: Commit**

```bash
git add crates/phronesis/src/ids.rs crates/phronesis/src/push.rs
git commit -m "fix(doc): resolve broken intra-doc links in ids.rs and push.rs"
```

---

### Task 4: Package exclude for phronesis-mcp

**Files:**
- Modify: `crates/phronesis-mcp/Cargo.toml`

- [ ] **Step 1: Add exclude field**

In `crates/phronesis-mcp/Cargo.toml`, add an `exclude` key under `[package]` (after the `categories` line):

```toml
exclude = ["tests/", "tests/features/"]
```

- [ ] **Step 2: Verify package size dropped**

Run: `cargo package --list -p phronesis-mcp --allow-dirty 2>&1 | wc -l`

Confirm test files and `.feature` files are absent from the listing:

Run: `cargo package --list -p phronesis-mcp --allow-dirty 2>&1 | grep -E '\.feature|tests/'`
Expected: no output.

- [ ] **Step 3: Commit**

```bash
git add crates/phronesis-mcp/Cargo.toml
git commit -m "chore: exclude test fixtures from published phronesis-mcp package"
```

---

### Task 5: Land uncommitted docs + track untracked files

**Files:**
- Modified (uncommitted): `docs/index.html`, `docs/catalogue.html`, `docs/explainer.html`
- Existing untracked: `.phronesis/wiki/decisions/README.md`
- Existing untracked: `docs/superpowers/` directory

- [ ] **Step 1: Stage the uncommitted doc changes**

The working tree has a landing-page redesign (`docs/index.html`) and colophon tweaks in catalogue + explainer. These are ready to land.

```bash
git add docs/index.html docs/catalogue.html docs/explainer.html
```

- [ ] **Step 2: Stage the decisions README**

This file should have been part of the wiki-drift merge but was missed.

```bash
git add .phronesis/wiki/decisions/README.md
```

- [ ] **Step 3: Stage docs/superpowers/ (design docs + plans)**

```bash
git add docs/superpowers/
```

- [ ] **Step 4: Commit**

```bash
git commit -m "docs: landing page redesign, colophon tweaks, design specs + plans"
```

---

### Task 6: Dry-run verification

No file changes — this is a verification gate.

- [ ] **Step 1: Dry-run phronesis**

Run: `cargo publish --dry-run -p phronesis`
Expected: "warning: aborting upload due to dry run" with no errors and no README path warning.

- [ ] **Step 2: Dry-run phronesis-mcp**

This will fail because phronesis isn't on crates.io yet. Run with `--no-verify` to skip the dep resolution:

Run: `cargo publish --dry-run --no-verify -p phronesis-mcp`
Expected: packages successfully (dry-run abort), no errors about missing metadata.

- [ ] **Step 3: Full test suite one more time**

Run: `cargo test --workspace`
Expected: all tests pass.

---

### Task 7: GitHub repo metadata

No file changes — `gh` CLI commands.

- [ ] **Step 1: Set description and homepage**

```bash
gh repo edit awaterma/phronesis \
  --description "A RETE rules engine for durable enforcement of project conventions in LLM-assisted work." \
  --homepage "https://awaterma.github.io/phronesis/"
```

- [ ] **Step 2: Add topics**

```bash
gh repo edit awaterma/phronesis \
  --add-topic rules-engine \
  --add-topic rete \
  --add-topic mcp \
  --add-topic llm \
  --add-topic claude-code \
  --add-topic code-quality \
  --add-topic developer-tools
```

- [ ] **Step 3: Make the repo public**

**This is irreversible for repos with certain features. Confirm before running.**

```bash
gh repo edit awaterma/phronesis --visibility public
```

- [ ] **Step 4: Push all commits**

```bash
git push origin main
```

---

### Task 8: Publish to crates.io

**This task requires human confirmation before each publish command.**

- [ ] **Step 1: Publish phronesis**

```bash
cargo publish -p phronesis
```

Expected: "Uploading phronesis v0.9.0" then success.

- [ ] **Step 2: Wait for index propagation**

Run: `cargo search phronesis`
Expected: shows `phronesis = "0.9.0"`. If not, wait 60 seconds and retry.

- [ ] **Step 3: Publish phronesis-mcp**

```bash
cargo publish -p phronesis-mcp
```

Expected: "Uploading phronesis-mcp v0.9.0" then success.

- [ ] **Step 4: Verify both are live**

Run: `cargo search phronesis`
Expected: both `phronesis` and `phronesis-mcp` show version 0.9.0.

---

### Task 9: Tag the release

- [ ] **Step 1: Create and push tag**

```bash
git tag v0.9.0
git push origin v0.9.0
```

- [ ] **Step 2: Verify**

Run: `git tag -l 'v*'`
Expected: `v0.9.0`

Run: `gh repo view awaterma/phronesis --json visibility`
Expected: `{"visibility":"PUBLIC"}`

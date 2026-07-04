# Doc Web + Unified Versioning Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Lockstep workspace versioning at 0.17.0, a `phr-mcp catalogue` generator that makes `docs/catalogue.html` a regenerable artifact, and bidirectional links weaving the three Pages HTMLs and the curated markdown docs into one navigable web.

**Architecture:** A new `catalogue` module in phronesis-mcp renders rule-entry HTML from the composed packs (same source `init` writes) and splices it between markers in the existing hand-authored page; a thin CLI arm wires it up. Versioning moves to `[workspace.package]`. The site pages gain a shared footer nav and a curated Documentation section of GitHub blob links.

**Tech Stack:** Rust (phronesis-mcp), serde_json (packs are `Value`), hand-authored HTML/CSS (no generator framework).

**Source spec:** `docs/superpowers/specs/2026-07-04-doc-web-unified-versioning-design.md`

## Global Constraints

- Branch: `feat/doc-web` off `main` **after PR #5 merges**. Isolated worktree at execution time.
- Release: **0.17.0 for all three crates** (lockstep; the CHANGELOG explains the unification once).
- Every commit: `cargo test --workspace` green; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --all -- --check` clean; `phr-mcp audit` total stays **8** (new code ≤7 outer lets/muts per fn).
- **Grep gate before every commit:** `git grep -il <private-consumer-name> -- $(git diff --cached --name-only)` must be empty (the doc surface is public). The name itself must never appear in repo files — it lives in the operator's durable memory; substitute it when running the command.
- Generated-region markers, exact: `<!-- BEGIN GENERATED RULES -->` and `<!-- END GENERATED RULES -->`.
- Generated entries carry: anchor id = full rule id, `data-level`, level+phase tags, the rule message as summary, and a predicate summary line. **Hand-written per-rule examples from the old page are dropped by design** (curated prose outside the markers survives).
- No push without explicit human approval.

---

### Task 1: Lockstep versioning at 0.17.0

**Files:**
- Modify: `Cargo.toml` (root — `[workspace.package]` block, currently edition/rust-version/license/authors/repository/homepage)
- Modify: `crates/phronesis/Cargo.toml`, `crates/phronesis-rhai/Cargo.toml`, `crates/phronesis-mcp/Cargo.toml`
- Modify: `CHANGELOG.md`

**Interfaces:**
- Produces: workspace-wide version `0.17.0`; later tasks read it via `env!("CARGO_PKG_VERSION")` in phronesis-mcp.

- [ ] **Step 1: Root manifest** — add to `[workspace.package]`:

```toml
version = "0.17.0"
```

- [ ] **Step 2: Crate manifests** — in each of the three crates replace `version = "..."` with:

```toml
version.workspace = true
```

and update internal dep versions: in `crates/phronesis-rhai/Cargo.toml` → `phronesis = { path = "../phronesis", version = "0.17" }`; in `crates/phronesis-mcp/Cargo.toml` → `phronesis = { ..., version = "0.17" }` and `phronesis-rhai = { version = "0.17", path = "../phronesis-rhai", optional = true }` (keep all other keys verbatim).

- [ ] **Step 3: Verify** — `cargo metadata --format-version 1 --no-deps | python3 -c "import json,sys; [print(p['name'], p['version']) for p in json.load(sys.stdin)['packages']]"`
Expected: all three at `0.17.0`. Then `cargo test --workspace` green.

- [ ] **Step 4: CHANGELOG** — retitle `## [Unreleased]` to `## [0.17.0] - <today>`, and prepend to that entry:

```markdown
phr-mcp, phr, and phronesis-rhai all release as **0.17.0** — the workspace
adopts lockstep versioning (`[workspace.package] version`); from this
release one number covers all three crates. (Previous: phr-mcp 0.16.2,
phr 0.14.0, phronesis-rhai 0.1.0; the jumps are version-line unification,
not breaking changes.)
```

Add the `[0.17.0]:` release-link line at the bottom.

- [ ] **Step 5: Gates + commit**

```bash
git add Cargo.toml Cargo.lock crates/*/Cargo.toml CHANGELOG.md
git commit -m "chore(release): unify workspace at 0.17.0 (lockstep versioning)"
```

(If `Cargo.lock` is gitignored in this repo — it is — omit it.)

---

### Task 2: `catalogue` rendering module (TDD)

**Files:**
- Create: `crates/phronesis-mcp/src/catalogue.rs`
- Modify: `crates/phronesis-mcp/src/lib.rs` (add `pub mod catalogue;` alphabetically)

**Interfaces:**
- Consumes: `crate::init::{Pack, compose_packs}` (`compose_packs(&[Pack]) -> serde_json::Value`, the `{"rules": [...]}` shape; enumerate every `Pack` variant — read the enum, don't assume the list).
- Produces: `pub fn render_rules_html(rules: &serde_json::Value) -> String` (the entries block, including the version-stamp header line sourced from `env!("CARGO_PKG_VERSION")`); `pub fn splice(page: &str, generated: &str) -> Result<String, String>` (replaces content between the exact markers, errors — message string of your choosing, pinned by test — when either marker is missing or out of order); `pub const BEGIN_MARKER: &str` / `pub const END_MARKER: &str`.

- [ ] **Step 1: Write the failing tests** (in-file `#[cfg(test)] mod tests`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::init::{compose_packs, Pack};

    fn all_rules() -> serde_json::Value {
        // every Pack variant — adjust to the real enum
        compose_packs(&[Pack::Llm, Pack::Rust, Pack::Confidence, Pack::Journey])
    }

    #[test]
    fn renders_one_entry_per_rule() {
        let rules = all_rules();
        let n = rules["rules"].as_array().unwrap().len();
        let html = render_rules_html(&rules);
        assert_eq!(html.matches("<article class=\"rule\"").count(), n);
    }

    #[test]
    fn anchor_ids_are_unique_and_match_rule_ids() {
        let rules = all_rules();
        let html = render_rules_html(&rules);
        let mut seen = std::collections::HashSet::new();
        for r in rules["rules"].as_array().unwrap() {
            let id = r["id"].as_str().unwrap();
            assert!(html.contains(&format!("id=\"{id}\"")), "missing anchor {id}");
            assert!(seen.insert(id), "duplicate anchor {id}");
        }
    }

    #[test]
    fn output_carries_version_stamp() {
        let html = render_rules_html(&all_rules());
        assert!(html.contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn splice_replaces_only_between_markers() {
        let page = format!(
            "<header>keep</header>\n{}\nOLD\n{}\n<footer>keep</footer>",
            BEGIN_MARKER, END_MARKER
        );
        let out = splice(&page, "NEW").unwrap();
        assert!(out.contains("<header>keep</header>"));
        assert!(out.contains("<footer>keep</footer>"));
        assert!(out.contains("NEW"));
        assert!(!out.contains("OLD"));
    }

    #[test]
    fn splice_is_idempotent() {
        let page = format!("A\n{}\nx\n{}\nB", BEGIN_MARKER, END_MARKER);
        let gen = render_rules_html(&all_rules());
        let once = splice(&page, &gen).unwrap();
        let twice = splice(&once, &gen).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn splice_errors_without_markers() {
        assert!(splice("no markers here", "x").is_err());
    }

    #[test]
    fn internal_hrefs_resolve_to_generated_anchors() {
        let html = render_rules_html(&all_rules());
        for href in html.split("href=\"#").skip(1) {
            let target = href.split('"').next().unwrap();
            assert!(html.contains(&format!("id=\"{target}\"")), "dangling #{target}");
        }
    }
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p phronesis-mcp --lib catalogue` → compile error (module absent).

- [ ] **Step 3: Implement.** Entry template — replicate the page's existing structure (see `docs/catalogue.html`, e.g. the `<article class="rule" data-level="warn" id="git-add-all">` entries): per rule emit

```html
<article class="rule" data-level="{level}" id="{rule-id}">
  <div class="rule-mark" aria-hidden="true">!</div>
  <div class="rule-content">
    <div class="rule-tags"><span class="tag tag-{level}">{level}</span><span class="tag">{phase}</span><span class="tag">{pack}</span></div>
    <h3 class="rule-id">{rule-id}</h3>
    <p class="rule-summary">{message, HTML-escaped}</p>
    <div class="rule-body"><code>{predicate summary: predicate names joined " + ", or "__script__" for script clauses}</code></div>
  </div>
</article>
```

`level` = `block` if the `then` verb is `block`/`constraint_violation` else `warn`/`log` per the verb; `phase` from the rule; `pack` requires `compose_packs` output to carry pack attribution — if it doesn't, group entries under per-pack `<section>` headings by composing packs one at a time (`Pack::X.rules()` — check what init.rs exposes) instead of tagging. Escape `&<>"` in messages. Prepend the stamp header: `<p class="catalogue-stamp">documents the default packs as of v{CARGO_PKG_VERSION}</p>`. Keep every fn ≤7 outer lets/muts (render loop → per-entry helper `fn render_entry(rule: &Value, pack: &str) -> String`).

- [ ] **Step 4: Run to verify pass** — `cargo test -p phronesis-mcp --lib catalogue` → all green.

- [ ] **Step 5: Gates + commit**

```bash
git add crates/phronesis-mcp/src/catalogue.rs crates/phronesis-mcp/src/lib.rs
git commit -m "feat(catalogue): rule-entry HTML renderer + marker splice (TDD)"
```

---

### Task 3: `phr-mcp catalogue` CLI arm + integration test

**Files:**
- Modify: `crates/phronesis-mcp/src/main.rs` — `Command` variant + one-line dispatch + `fn handle_catalogue` (follow the existing handle_* pattern)
- Create: `crates/phronesis-mcp/tests/catalogue_integration.rs`

**Interfaces:**
- Consumes: `catalogue::{render_rules_html, splice}`, `init::{Pack, compose_packs}` from Task 2.
- Produces: `phr-mcp catalogue [--out <path>]`; default out `docs/catalogue.html` **relative to the current directory** (run from repo root; document in the help text). Exit 1 with `error: ...` on missing file/markers.

- [ ] **Step 1: Failing integration test** (model: `tests/migrate_extracted_integration.rs`'s `CARGO_BIN_EXE_phr-mcp` pattern):

```rust
use std::fs;
use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .arg("catalogue").args(args).output().expect("spawn")
}

#[test]
fn regenerates_between_markers() {
    let dir = tempfile::tempdir().unwrap();
    let page = dir.path().join("catalogue.html");
    fs::write(&page, "<header>k</header>\n<!-- BEGIN GENERATED RULES -->\nSTALE\n<!-- END GENERATED RULES -->\n<footer>k</footer>").unwrap();
    let out = run(&["--out", page.to_str().unwrap()]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let html = fs::read_to_string(&page).unwrap();
    assert!(!html.contains("STALE"));
    assert!(html.contains("<article class=\"rule\""));
    assert!(html.contains("<header>k</header>") && html.contains("<footer>k</footer>"));
}

#[test]
fn missing_markers_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let page = dir.path().join("catalogue.html");
    fs::write(&page, "<p>no markers</p>").unwrap();
    let out = run(&["--out", page.to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("marker"));
}
```

- [ ] **Step 2: Verify failure** — `cargo test -p phronesis-mcp --test catalogue_integration` → clap rejects unknown subcommand.

- [ ] **Step 3: Implement the arm** — variant:

```rust
/// Regenerate the rule catalogue page from the shipped packs.
/// Rewrites the content between the GENERATED RULES markers in-place;
/// run from the repo root (default --out docs/catalogue.html).
Catalogue {
    /// Path to the catalogue HTML file to rewrite.
    #[arg(long, default_value = "docs/catalogue.html")]
    out: PathBuf,
},
```

`handle_catalogue(out)`: read file (`error:` + exit 1 if missing) → `render_rules_html(&compose_packs(&ALL_PACKS))` → `splice` (`error:` + exit 1 on Err, message mentions "marker") → write back → `println!("regenerated {} ({} rules) at v{}", ...)`.

- [ ] **Step 4: Verify pass** — integration tests green.

- [ ] **Step 5: Gates + commit**

```bash
git add crates/phronesis-mcp/src/main.rs crates/phronesis-mcp/tests/catalogue_integration.rs
git commit -m "feat(cli): phr-mcp catalogue — regenerate the rule catalogue from packs"
```

---

### Task 4: Markerize + regenerate the live catalogue

**Files:**
- Modify: `docs/catalogue.html`
- Modify: `crates/phronesis-mcp/CLAUDE.md` (release checklist: "run `phr-mcp catalogue` after changing packs")

- [ ] **Step 1: Markerize** — in `docs/catalogue.html`, insert `<!-- BEGIN GENERATED RULES -->` immediately before the first `<article class="rule"` and `<!-- END GENERATED RULES -->` immediately after the last `</article>` of the rule list (verify by inspection that everything between is rule entries — intro prose, section framing, and footer stay outside).
- [ ] **Step 2: Regenerate** — from repo root: `cargo run -q -p phronesis-mcp --bin phr-mcp -- catalogue`. Inspect the diff: hand-authored frame untouched outside markers; entries now generated (old hand-written examples gone — by design); stamp reads `v0.17.0`.
- [ ] **Step 3: Idempotence check** — run the command again; `git diff --stat` shows no further change.
- [ ] **Step 4: Grep gate + gates + commit**

```bash
git grep -il <private-consumer-name> -- docs/catalogue.html && exit 1 || true
git add docs/catalogue.html crates/phronesis-mcp/CLAUDE.md
git commit -m "docs(catalogue): markerize + regenerate from packs at 0.17.0"
```

---

### Task 5: The web — nav, Documentation section, backlinks

**Files:**
- Modify: `docs/index.html`, `docs/explainer.html`, `docs/catalogue.html` (footer nav, outside the markers)
- Modify: `README.md`, `docs/loop-programming-guide.md` (backlinks)

- [ ] **Step 1: Footer nav** — one shared fragment, styled per page (reuse each page's existing footer/link classes):

```html
<nav class="doc-web" aria-label="Documentation">
  <a href="./index.html">Home</a> · <a href="./explainer.html">Explainer</a> ·
  <a href="./catalogue.html">Catalogue</a> ·
  <a href="https://github.com/awaterma/phronesis/blob/main/docs/loop-programming-guide.md">Guide</a> ·
  <a href="https://github.com/awaterma/phronesis/blob/main/CHANGELOG.md">Changelog</a> ·
  <a href="https://github.com/awaterma/phronesis">GitHub</a>
  <span class="doc-web-version">v0.17.0</span>
</nav>
```

Place above each page's existing footer; add minimal page-local CSS matching each page's palette. On the page a link points to, render that label as plain text (no self-link).

- [ ] **Step 2: index.html Documentation section** — after the existing catalogue/explainer cards, a section listing (all as `https://github.com/awaterma/phronesis/blob/main/...` links): `docs/loop-programming-guide.md`, `CHANGELOG.md`, `README.md`, `crates/phronesis/README.md`, `crates/phronesis-mcp/README.md`, `crates/phronesis-rhai/README.md`, `docs/specs/SPEC-journey-facts.md`, `docs/specs/SPEC-confidence-scoring.md`, `docs/superpowers/specs/2026-06-01-rhai-script-evaluator-design.md`, `docs/superpowers/specs/2026-06-28-mcp-crate-decomposition-design.md` — each with a one-line description in the page's existing card/list idiom.
- [ ] **Step 3: Backlinks** — `README.md`: add the site URL (`https://awaterma.github.io/phronesis/`) near the top (badge-line or intro paragraph, matching existing style). `docs/loop-programming-guide.md`: one line in its intro pointing to the site.
- [ ] **Step 4: Link check** — for every `href` added in steps 1–3: relative targets exist on disk; blob URLs correspond to tracked files (`git ls-files <path>`). Record the check in the report.
- [ ] **Step 5: Grep gate + gates + commit**

```bash
git grep -il <private-consumer-name> -- docs/ README.md && exit 1 || true
git add docs/index.html docs/explainer.html docs/catalogue.html README.md docs/loop-programming-guide.md
git commit -m "docs(site): footer nav + curated documentation web + backlinks"
```

---

### Task 6: Done-when battery

- [ ] **Step 1:** `cargo test --workspace` / clippy / fmt / `cargo run -q -p phronesis-mcp --bin phr-mcp -- audit | tail -3` (total 8).
- [ ] **Step 2:** `cargo publish --dry-run -p phronesis` then `-p phronesis-rhai` then `-p phronesis-mcp` — record results; failures from path-dep resolution against unpublished 0.17 crates are EXPECTED for rhai/mcp (dependencies aren't on crates.io yet) and should be noted, not "fixed"; anything else (missing files, metadata) is a real finding.
- [ ] **Step 3:** Re-run `phr-mcp catalogue`; `git status` clean (idempotent against committed page).
- [ ] **Step 4:** Full-branch grep gate: `git grep -il <private-consumer-name> $(git rev-parse HEAD) --` → empty.
- [ ] **Step 5:** Commit any residue; STOP — human review, no push.

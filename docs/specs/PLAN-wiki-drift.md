# Wiki-Drift Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship phronesis-mcp 0.9.0 — an ADR-style decisions corpus at `.phronesis/wiki/decisions/`, a heuristic `wiki-drift` extractor that flags decisions without rule coverage (using an explicit `enforces:` shortcut + Jaccard fallback), a `decision new <slug>` helper, and the matching `get_wiki_drift` MCP tool.

**Architecture:** All changes live in `crates/phronesis-mcp`. A new module `wiki.rs` owns page primitives (Decision struct, frontmatter parser, directory walker). A new module `wiki_drift.rs` owns the extractor (mirrors `claude_md_drift.rs` shape with one upgrade — the `enforces:` frontmatter shortcut beats Jaccard when authors are explicit). The `phr` library crate is untouched. The wiki tree is *versioned* (carved out of the broad `.phronesis/` gitignore) because decisions are project knowledge, distinct from rules.json/log.jsonl which stay ignored.

**Tech Stack:** Rust, serde + `serde_yml` (new direct dep for YAML frontmatter), clap, existing phronesis-mcp test harness (`cargo test -p phronesis-mcp`).

---

## File Structure

| File | Responsibility | Change |
|------|---------------|--------|
| `crates/phronesis-mcp/Cargo.toml` | Deps + version | Add `serde_yml = "0.0.12"`; bump 0.8.1 → 0.9.0 (final task) |
| `crates/phronesis-mcp/src/wiki.rs` | Wiki page primitives — Decision, frontmatter parsing, directory walk. Shared with future wiki SPECs. | Create |
| `crates/phronesis-mcp/src/wiki_drift.rs` | Drift extractor — DriftItem, DriftReport, run, render_table, render_json, suggest_rule | Create |
| `crates/phronesis-mcp/src/lib.rs` | Module declarations | Add `pub mod wiki;` and `pub mod wiki_drift;` |
| `crates/phronesis-mcp/src/server_params.rs` | MCP tool param types | Add `GetWikiDriftParams` |
| `crates/phronesis-mcp/src/server.rs` | MCP tool registration | Add `get_wiki_drift` tool |
| `crates/phronesis-mcp/src/main.rs` | CLI | Add `WikiDrift` + `DecisionNew` subcommands |
| `crates/phronesis-mcp/src/init.rs` | Project scaffolding | Create `.phronesis/wiki/decisions/` + README; add gitignore exception |
| `crates/phronesis-mcp/CLAUDE.md` | Docs | Document `wiki-drift` + `decision new` + wiki layout (final task) |
| `crates/phronesis-mcp/tests/wiki_drift_integration.rs` | End-to-end CLI tests | Create |

**Type model (locked here):**

```rust
// wiki.rs

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionFrontmatter {
    pub id: String,
    pub date: String,                 // ISO date, kept as String for v1 (no chrono dep here)
    pub status: String,               // "proposed" | "accepted" | "superseded"
    #[serde(default)]
    pub enforces: Vec<String>,        // rule ids
    #[serde(default)]
    pub superseded_by: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Decision {
    pub frontmatter: DecisionFrontmatter,
    pub body: String,                 // everything after the second `---`
    pub path: PathBuf,                // file path for error messages and reporting
}

#[derive(Debug, thiserror::Error)]
pub enum WikiError {
    #[error("wiki directory not found at {0}")]
    DirMissing(String),
    #[error("failed to read {path}: {source}")]
    Io { path: String, source: std::io::Error },
    #[error("{path}: missing or malformed frontmatter ({message})")]
    Frontmatter { path: String, message: String },
}

pub fn default_wiki_dir(project_root: &Path) -> PathBuf;
pub fn parse_decision_file(path: &Path) -> Result<Decision, WikiError>;
pub fn walk_decisions(wiki_dir: &Path) -> Result<Vec<Decision>, WikiError>;
```

```rust
// wiki_drift.rs (mirrors claude_md_drift.rs / memory_drift.rs)

pub struct DriftItem {
    pub decision: Decision,
    pub bucket: Bucket,
    pub best_match: Option<MatchedRule>,
    pub similarity: f32,
}

pub enum Bucket {
    Covered,         // explicit enforces: match
    LikelyCovered,   // fuzzy match above threshold
    Uncovered,       // no match — drift candidate
    Superseded,      // status: superseded — excluded from active drift
}

pub struct MatchedRule {
    pub rule_id: phr::RuleId,
    pub shared_terms: Vec<String>,    // empty for Covered (deterministic), populated for LikelyCovered
}

pub struct DriftReport {
    pub wiki_dir: String,
    pub rules_path: String,
    pub items: Vec<DriftItem>,
    pub coverage_threshold: f32,      // 0.15, same as claude-md-drift
}

#[derive(Debug, thiserror::Error)]
pub enum DriftError {
    #[error(transparent)]
    Wiki(#[from] crate::wiki::WikiError),
    #[error("failed to read rules file: {0}")]
    RulesIo(String),
}

pub fn run(project_root: &Path) -> Result<DriftReport, DriftError>;
pub fn run_with_dir(project_root: &Path, wiki_dir: &Path) -> Result<DriftReport, DriftError>;
pub fn render_table(report: &DriftReport) -> String;
pub fn render_json(report: &DriftReport) -> String;
pub fn suggest_rule(item: &DriftItem) -> Option<String>;
```

---

## COMMIT 1 — wiki module (Decision parsing + directory walk)

### Task 1.1: Add `serde_yml` dependency

**Files:**
- Modify: `crates/phronesis-mcp/Cargo.toml`

- [ ] **Step 1: Add the dep**

In `crates/phronesis-mcp/Cargo.toml`, under `[dependencies]`, add (alphabetical placement near other serde-family lines):

```toml
serde_yml = "0.0.12"
```

- [ ] **Step 2: Verify it resolves**

Run: `cargo build -p phronesis-mcp 2>&1 | tail -3`
Expected: Finished (no new warnings).

- [ ] **Step 3: Commit**

```bash
git add crates/phronesis-mcp/Cargo.toml
git -c commit.gpgsign=false commit \
  --author="Claude Opus 4.7 (1M context) <noreply@anthropic.com>" \
  -m "$(cat <<'EOF'
chore: add serde_yml dep for wiki frontmatter parsing

Preparing wiki.rs (Decision parser). serde_yml is the maintained
successor to the deprecated serde_yaml.

Co-Authored-By: Andrew Waterman <andrew.waterman@gmail.com>
EOF
)"
```

---

### Task 1.2: `wiki::Decision` types + `parse_decision_file`

**Files:**
- Create: `crates/phronesis-mcp/src/wiki.rs`
- Modify: `crates/phronesis-mcp/src/lib.rs`

- [ ] **Step 1: Wire the module into lib.rs (so tests compile)**

In `crates/phronesis-mcp/src/lib.rs`, add (alphabetically after `syntax`):

```rust
pub mod wiki;
```

- [ ] **Step 2: Write the failing tests in a new wiki.rs**

Create `crates/phronesis-mcp/src/wiki.rs` with the file header + types + tests but NO impl of `parse_decision_file` yet (use a `todo!()` stub so tests compile but fail):

```rust
//! Wiki page primitives — ADR-style decision pages under
//! `.phronesis/wiki/decisions/`. Shared by `wiki_drift` and by any
//! future wiki-consuming module.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

/// Structured YAML frontmatter at the top of every decision page.
/// See SPEC-wiki-drift.md §"Decision page schema".
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionFrontmatter {
    pub id: String,
    pub date: String,
    pub status: String,
    #[serde(default)]
    pub enforces: Vec<String>,
    #[serde(default)]
    pub superseded_by: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// One decision page on disk: structured frontmatter + free-form body.
#[derive(Debug, Clone)]
pub struct Decision {
    pub frontmatter: DecisionFrontmatter,
    pub body: String,
    pub path: PathBuf,
}

#[derive(Debug, Error)]
pub enum WikiError {
    #[error("wiki directory not found at {0}")]
    DirMissing(String),
    #[error("failed to read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: missing or malformed frontmatter ({message})")]
    Frontmatter { path: String, message: String },
}

/// Default per-project wiki directory: `<project_root>/.phronesis/wiki/`.
pub fn default_wiki_dir(project_root: &Path) -> PathBuf {
    project_root.join(".phronesis").join("wiki")
}

/// Parse a single decision page. Expects:
///
/// ```text
/// ---
/// id: ...
/// date: ...
/// ---
///
/// body text
/// ```
pub fn parse_decision_file(path: &Path) -> Result<Decision, WikiError> {
    todo!("Task 1.2 step 4")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(dir: &TempDir, name: &str, content: &str) -> PathBuf {
        let p = dir.path().join(name);
        fs::write(&p, content).expect("write fixture");
        p
    }

    #[test]
    fn parse_minimal_decision() {
        let dir = TempDir::new().unwrap();
        let p = write(
            &dir,
            "2026-05-29-card-game-vocab.md",
            "---\n\
             id: card-game-vocab\n\
             date: 2026-05-29\n\
             status: accepted\n\
             ---\n\
             \n\
             ## Decision\n\
             Use card-game vocabulary.\n",
        );
        let d = parse_decision_file(&p).expect("parse");
        assert_eq!(d.frontmatter.id, "card-game-vocab");
        assert_eq!(d.frontmatter.date, "2026-05-29");
        assert_eq!(d.frontmatter.status, "accepted");
        assert!(d.frontmatter.enforces.is_empty());
        assert!(d.frontmatter.superseded_by.is_none());
        assert!(d.body.contains("## Decision"));
        assert!(d.body.contains("Use card-game vocabulary"));
        assert_eq!(d.path, p);
    }

    #[test]
    fn parse_decision_with_enforces_list_and_tags() {
        let dir = TempDir::new().unwrap();
        let p = write(
            &dir,
            "x.md",
            "---\n\
             id: workspace-cargo\n\
             date: 2026-05-29\n\
             status: accepted\n\
             enforces:\n  - warn-cargo-build-without-workspace\n  - some-other-rule\n\
             tags: [build, hygiene]\n\
             ---\n\
             body\n",
        );
        let d = parse_decision_file(&p).expect("parse");
        assert_eq!(
            d.frontmatter.enforces,
            vec!["warn-cargo-build-without-workspace".to_string(), "some-other-rule".to_string()]
        );
        assert_eq!(d.frontmatter.tags, vec!["build".to_string(), "hygiene".to_string()]);
    }

    #[test]
    fn parse_decision_with_superseded_by() {
        let dir = TempDir::new().unwrap();
        let p = write(
            &dir,
            "x.md",
            "---\n\
             id: old\n\
             date: 2026-01-01\n\
             status: superseded\n\
             superseded_by: new-decision\n\
             ---\n\
             ",
        );
        let d = parse_decision_file(&p).expect("parse");
        assert_eq!(d.frontmatter.superseded_by, Some("new-decision".to_string()));
    }

    #[test]
    fn parse_decision_missing_frontmatter_errors() {
        let dir = TempDir::new().unwrap();
        let p = write(&dir, "x.md", "no frontmatter here\njust prose\n");
        match parse_decision_file(&p) {
            Err(WikiError::Frontmatter { .. }) => {}
            other => panic!("expected Frontmatter error, got {:?}", other),
        }
    }

    #[test]
    fn parse_decision_missing_required_field_errors() {
        let dir = TempDir::new().unwrap();
        let p = write(
            &dir,
            "x.md",
            "---\nid: x\ndate: 2026-01-01\n---\nbody\n", // missing `status`
        );
        match parse_decision_file(&p) {
            Err(WikiError::Frontmatter { message, .. }) => assert!(message.contains("status")),
            other => panic!("expected Frontmatter error, got {:?}", other),
        }
    }

    #[test]
    fn parse_decision_unknown_field_errors() {
        // deny_unknown_fields catches typos that would otherwise silently drop.
        let dir = TempDir::new().unwrap();
        let p = write(
            &dir,
            "x.md",
            "---\nid: x\ndate: 2026-01-01\nstatus: accepted\nstauts: typo\n---\n",
        );
        assert!(matches!(parse_decision_file(&p), Err(WikiError::Frontmatter { .. })));
    }
}
```

- [ ] **Step 3: Run tests to verify they fail with `todo!()` (not compile errors)**

Run: `cargo test -p phronesis-mcp --lib wiki:: 2>&1 | tail -15`
Expected: 6 tests panic at `todo!("Task 1.2 step 4")`.

- [ ] **Step 4: Implement `parse_decision_file`**

Replace the `todo!()` stub with:

```rust
pub fn parse_decision_file(path: &Path) -> Result<Decision, WikiError> {
    let content = std::fs::read_to_string(path).map_err(|source| WikiError::Io {
        path: path.display().to_string(),
        source,
    })?;

    // Frontmatter is bracketed by `---` on its own line at the top of the file.
    // We accept an optional leading whitespace before the opening fence.
    let trimmed = content.trim_start_matches('\u{FEFF}'); // strip UTF-8 BOM if present
    let rest = trimmed.strip_prefix("---\n").or_else(|| trimmed.strip_prefix("---\r\n"))
        .ok_or_else(|| WikiError::Frontmatter {
            path: path.display().to_string(),
            message: "expected file to start with `---`".to_string(),
        })?;
    let close_idx = rest.find("\n---").ok_or_else(|| WikiError::Frontmatter {
        path: path.display().to_string(),
        message: "missing closing `---` fence".to_string(),
    })?;
    let yaml = &rest[..close_idx];
    // Body starts after the closing fence and the newline that follows it.
    let after_fence = &rest[close_idx + 4..]; // skip "\n---"
    let body = after_fence
        .strip_prefix('\n')
        .or_else(|| after_fence.strip_prefix("\r\n"))
        .unwrap_or(after_fence)
        .to_string();

    let frontmatter: DecisionFrontmatter =
        serde_yml::from_str(yaml).map_err(|e| WikiError::Frontmatter {
            path: path.display().to_string(),
            message: e.to_string(),
        })?;

    Ok(Decision {
        frontmatter,
        body,
        path: path.to_path_buf(),
    })
}
```

- [ ] **Step 5: Run tests to verify pass**

Run: `cargo test -p phronesis-mcp --lib wiki:: 2>&1 | tail -12`
Expected: `test result: ok. 6 passed; 0 failed`.

- [ ] **Step 6: Format + sanity build**

Run: `cargo fmt -p phronesis-mcp && cargo build -p phronesis-mcp 2>&1 | tail -3`
Expected: Finished, no new warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/phronesis-mcp/src/wiki.rs crates/phronesis-mcp/src/lib.rs
git -c commit.gpgsign=false commit \
  --author="Claude Opus 4.7 (1M context) <noreply@anthropic.com>" \
  -m "$(cat <<'EOF'
feat(wiki): Decision parser with YAML frontmatter

Introduces crates/phronesis-mcp/src/wiki.rs with Decision /
DecisionFrontmatter / WikiError + parse_decision_file. Frontmatter
uses serde + serde_yml with #[serde(deny_unknown_fields)] so typos
in field names surface loudly instead of silently dropping.

Six unit tests cover the minimal happy path, lists (enforces/tags),
superseded_by, missing-frontmatter, missing-required-field, and
unknown-field errors.

Co-Authored-By: Andrew Waterman <andrew.waterman@gmail.com>
EOF
)"
```

---

### Task 1.3: `wiki::walk_decisions` (directory iterator)

**Files:**
- Modify: `crates/phronesis-mcp/src/wiki.rs`

- [ ] **Step 1: Write the failing tests**

Append to `wiki.rs`'s `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn walk_decisions_returns_empty_for_empty_dir() {
        let dir = TempDir::new().unwrap();
        let wiki = dir.path().join("decisions");
        fs::create_dir(&wiki).unwrap();
        let decisions = walk_decisions(&wiki).unwrap();
        assert!(decisions.is_empty());
    }

    #[test]
    fn walk_decisions_missing_dir_errors() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("nope");
        match walk_decisions(&missing) {
            Err(WikiError::DirMissing(_)) => {}
            other => panic!("expected DirMissing, got {:?}", other),
        }
    }

    #[test]
    fn walk_decisions_parses_md_files_and_skips_readme() {
        let dir = TempDir::new().unwrap();
        let wiki = dir.path().join("decisions");
        fs::create_dir(&wiki).unwrap();
        fs::write(wiki.join("README.md"), "# not a decision\n").unwrap();
        fs::write(
            wiki.join("2026-05-29-a.md"),
            "---\nid: a\ndate: 2026-05-29\nstatus: accepted\n---\nbody\n",
        )
        .unwrap();
        fs::write(
            wiki.join("2026-05-28-b.md"),
            "---\nid: b\ndate: 2026-05-28\nstatus: accepted\n---\nbody\n",
        )
        .unwrap();
        fs::write(wiki.join("notes.txt"), "non-md, should be skipped").unwrap();

        let decisions = walk_decisions(&wiki).unwrap();
        // README.md and notes.txt skipped; both .md decisions parsed.
        let ids: Vec<&str> = decisions.iter().map(|d| d.frontmatter.id.as_str()).collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"a"));
        assert!(ids.contains(&"b"));
    }

    #[test]
    fn walk_decisions_sorted_by_date_desc() {
        let dir = TempDir::new().unwrap();
        let wiki = dir.path().join("decisions");
        fs::create_dir(&wiki).unwrap();
        fs::write(
            wiki.join("c.md"),
            "---\nid: c\ndate: 2025-01-01\nstatus: accepted\n---\n",
        )
        .unwrap();
        fs::write(
            wiki.join("a.md"),
            "---\nid: a\ndate: 2026-06-01\nstatus: accepted\n---\n",
        )
        .unwrap();
        fs::write(
            wiki.join("b.md"),
            "---\nid: b\ndate: 2026-03-15\nstatus: accepted\n---\n",
        )
        .unwrap();
        let decisions = walk_decisions(&wiki).unwrap();
        let dates: Vec<&str> = decisions.iter().map(|d| d.frontmatter.date.as_str()).collect();
        // Newest first.
        assert_eq!(dates, vec!["2026-06-01", "2026-03-15", "2025-01-01"]);
    }
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test -p phronesis-mcp --lib wiki::tests::walk 2>&1 | head -15`
Expected: compile error — `walk_decisions` undefined.

- [ ] **Step 3: Implement `walk_decisions`**

Add to `wiki.rs` (after `parse_decision_file`):

```rust
/// Walk a wiki decisions directory and parse every `*.md` file (except
/// `README.md`). Returns decisions sorted by `date` field, newest first.
///
/// Subdirectories are ignored — the convention is flat
/// `<date>-<slug>.md` files.
pub fn walk_decisions(wiki_dir: &Path) -> Result<Vec<Decision>, WikiError> {
    if !wiki_dir.exists() {
        return Err(WikiError::DirMissing(wiki_dir.display().to_string()));
    }
    let entries = std::fs::read_dir(wiki_dir).map_err(|source| WikiError::Io {
        path: wiki_dir.display().to_string(),
        source,
    })?;

    let mut decisions = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| WikiError::Io {
            path: wiki_dir.display().to_string(),
            source,
        })?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        if path.file_name().and_then(|n| n.to_str()) == Some("README.md") {
            continue;
        }
        decisions.push(parse_decision_file(&path)?);
    }

    // Newest first. String comparison works because dates are ISO YYYY-MM-DD.
    decisions.sort_by(|a, b| b.frontmatter.date.cmp(&a.frontmatter.date));
    Ok(decisions)
}
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p phronesis-mcp --lib wiki:: 2>&1 | tail -10`
Expected: `test result: ok. 10 passed` (6 from Task 1.2 + 4 new).

- [ ] **Step 5: Format + commit**

```bash
cargo fmt -p phronesis-mcp
git add crates/phronesis-mcp/src/wiki.rs
git -c commit.gpgsign=false commit \
  --author="Claude Opus 4.7 (1M context) <noreply@anthropic.com>" \
  -m "$(cat <<'EOF'
feat(wiki): walk_decisions directory iterator

Iterates `<wiki>/decisions/*.md`, skipping README.md and non-md
files. Returns Decisions sorted by date (newest first; ISO date
string-compare is correct for YYYY-MM-DD). Missing directory is a
typed WikiError::DirMissing rather than silent empty.

Four unit tests: empty dir → empty list, missing dir → error,
README/non-md skipped, sort by date desc.

Co-Authored-By: Andrew Waterman <andrew.waterman@gmail.com>
EOF
)"
```

---

## COMMIT 2 — wiki_drift extractor

### Task 2.1: `wiki_drift::run` with scoring (enforces shortcut + Jaccard fallback)

**Files:**
- Create: `crates/phronesis-mcp/src/wiki_drift.rs`
- Modify: `crates/phronesis-mcp/src/lib.rs`

- [ ] **Step 1: Wire the module**

In `crates/phronesis-mcp/src/lib.rs`, add (alphabetical, after `wiki`):

```rust
pub mod wiki_drift;
```

- [ ] **Step 2: Write the failing tests in a new wiki_drift.rs**

Create `crates/phronesis-mcp/src/wiki_drift.rs` with types + tests but `todo!()` stub for `run_with_dir`:

```rust
//! Detect drift between ADR-style decision documents in
//! `.phronesis/wiki/decisions/` and the current rule pack. Heuristic
//! by design (no LLM call); output is a triage list.

use std::path::{Path, PathBuf};

use phr::RuleId;
use thiserror::Error;

use crate::rules_file::{self, DiskRule};
use crate::wiki::{self, Decision};

const COVERAGE_THRESHOLD: f32 = 0.15;

const STOPWORDS: &[&str] = &[
    "the", "a", "an", "of", "to", "for", "in", "on", "at", "by", "is", "are", "be", "and", "or",
    "but", "with", "as", "it", "its", "this", "that", "you", "your", "we", "our", "i", "me",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bucket {
    Covered,
    LikelyCovered,
    Uncovered,
    Superseded,
}

#[derive(Debug, Clone)]
pub struct MatchedRule {
    pub rule_id: RuleId,
    pub shared_terms: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DriftItem {
    pub decision: Decision,
    pub bucket: Bucket,
    pub best_match: Option<MatchedRule>,
    pub similarity: f32,
}

#[derive(Debug, Clone)]
pub struct DriftReport {
    pub wiki_dir: String,
    pub rules_path: String,
    pub items: Vec<DriftItem>,
    pub coverage_threshold: f32,
}

#[derive(Debug, Error)]
pub enum DriftError {
    #[error(transparent)]
    Wiki(#[from] wiki::WikiError),
    #[error("failed to read rules file: {0}")]
    RulesIo(String),
}

/// Default entry: walks `<project_root>/.phronesis/wiki/decisions/`.
pub fn run(project_root: &Path) -> Result<DriftReport, DriftError> {
    let dir = wiki::default_wiki_dir(project_root).join("decisions");
    run_with_dir(project_root, &dir)
}

/// Override entry: scan an arbitrary decisions directory. Useful for tests
/// and for callers who put the wiki elsewhere.
pub fn run_with_dir(
    project_root: &Path,
    decisions_dir: &Path,
) -> Result<DriftReport, DriftError> {
    todo!("Task 2.1 step 4")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Build a project root with: a .phronesis/ dir containing a v2-shaped
    /// rules.json with the given rule ids, and a decisions directory
    /// (caller writes decision files into it).
    fn fixture_project(rule_ids: &[&str]) -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let phr = dir.path().join(".phronesis");
        fs::create_dir_all(&phr).unwrap();
        let rules_json = if rule_ids.is_empty() {
            r#"{"rules":[]}"#.to_string()
        } else {
            let rules: Vec<String> = rule_ids
                .iter()
                .map(|id| {
                    format!(
                        r#"{{"id":"{}","phase":"pre","priority":1,"when":[{{"new_content_contains":"x"}}],"then":{{"warn":"m"}}}}"#,
                        id
                    )
                })
                .collect();
            format!(r#"{{"rules":[{}]}}"#, rules.join(","))
        };
        fs::write(phr.join("rules.json"), rules_json).unwrap();
        let dec_dir = phr.join("wiki").join("decisions");
        fs::create_dir_all(&dec_dir).unwrap();
        let project_root = dir.path().to_path_buf();
        (dir, dec_dir)
    }

    fn write_decision(dir: &Path, name: &str, content: &str) {
        fs::write(dir.join(name), content).unwrap();
    }

    #[test]
    fn enforces_listing_an_existing_rule_buckets_as_covered() {
        let (tmp, dec) = fixture_project(&["warn-cargo-no-workspace"]);
        write_decision(
            &dec,
            "2026-05-29-x.md",
            "---\n\
             id: x\n\
             date: 2026-05-29\n\
             status: accepted\n\
             enforces:\n  - warn-cargo-no-workspace\n\
             ---\n\
             body about something completely unrelated to cargo\n",
        );
        let report = run_with_dir(tmp.path(), &dec).unwrap();
        assert_eq!(report.items.len(), 1);
        assert_eq!(report.items[0].bucket, Bucket::Covered);
        let m = report.items[0].best_match.as_ref().unwrap();
        assert_eq!(m.rule_id.as_str(), "warn-cargo-no-workspace");
        // Covered is deterministic — shared_terms is empty.
        assert!(m.shared_terms.is_empty());
        // Similarity reported as 1.0 to convey deterministic match.
        assert!((report.items[0].similarity - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn enforces_listing_nonexistent_rule_falls_through_to_fuzzy() {
        let (tmp, dec) = fixture_project(&["other-rule"]);
        write_decision(
            &dec,
            "x.md",
            "---\n\
             id: x\n\
             date: 2026-05-29\n\
             status: accepted\n\
             enforces:\n  - not-in-the-pack\n\
             ---\n\
             completely unrelated content\n",
        );
        let report = run_with_dir(tmp.path(), &dec).unwrap();
        // Bucket falls back to Uncovered since fuzzy doesn't match either.
        assert_eq!(report.items[0].bucket, Bucket::Uncovered);
    }

    #[test]
    fn no_enforces_with_strong_fuzzy_match_is_likely_covered() {
        let (tmp, dec) = fixture_project(&["enforce-no-unwrap-in-src"]);
        write_decision(
            &dec,
            "x.md",
            "---\n\
             id: x\n\
             date: 2026-05-29\n\
             status: accepted\n\
             ---\n\
             ## Decision\n\
             We enforce no unwrap in src code paths.\n",
        );
        let report = run_with_dir(tmp.path(), &dec).unwrap();
        assert_eq!(report.items[0].bucket, Bucket::LikelyCovered);
        assert!(report.items[0].best_match.is_some());
        assert!(report.items[0].similarity >= COVERAGE_THRESHOLD);
    }

    #[test]
    fn no_match_anywhere_is_uncovered() {
        let (tmp, dec) = fixture_project(&["unrelated-rule"]);
        write_decision(
            &dec,
            "x.md",
            "---\n\
             id: x\n\
             date: 2026-05-29\n\
             status: accepted\n\
             ---\n\
             completely orthogonal subject matter zzz qqq\n",
        );
        let report = run_with_dir(tmp.path(), &dec).unwrap();
        assert_eq!(report.items[0].bucket, Bucket::Uncovered);
        assert!(report.items[0].best_match.is_none());
    }

    #[test]
    fn superseded_decision_is_classified_as_superseded() {
        let (tmp, dec) = fixture_project(&[]);
        write_decision(
            &dec,
            "old.md",
            "---\n\
             id: old\n\
             date: 2025-01-01\n\
             status: superseded\n\
             superseded_by: new-thing\n\
             ---\n\
             ",
        );
        let report = run_with_dir(tmp.path(), &dec).unwrap();
        assert_eq!(report.items[0].bucket, Bucket::Superseded);
        assert!(report.items[0].best_match.is_none());
    }

    #[test]
    fn report_carries_threshold_and_paths() {
        let (tmp, dec) = fixture_project(&[]);
        let report = run_with_dir(tmp.path(), &dec).unwrap();
        assert!((report.coverage_threshold - COVERAGE_THRESHOLD).abs() < f32::EPSILON);
        assert!(report.wiki_dir.contains("decisions"));
        assert!(report.rules_path.contains("rules.json"));
    }
}
```

- [ ] **Step 3: Run, verify the tests fail with `todo!()`**

Run: `cargo test -p phronesis-mcp --lib wiki_drift:: 2>&1 | tail -10`
Expected: 6 tests panic at the `todo!()`.

- [ ] **Step 4: Implement `run_with_dir`**

Replace the `todo!()` with:

```rust
pub fn run_with_dir(
    project_root: &Path,
    decisions_dir: &Path,
) -> Result<DriftReport, DriftError> {
    let decisions = wiki::walk_decisions(decisions_dir)?;
    let rules_path = rules_file::default_path(project_root);
    let rules = rules_file::read(&rules_path).map_err(|e| DriftError::RulesIo(e.to_string()))?;
    let rule_id_set: std::collections::HashSet<&str> =
        rules.rules.iter().map(|r| r.id.as_str()).collect();

    let items = decisions
        .into_iter()
        .map(|d| score_decision(d, &rules.rules, &rule_id_set))
        .collect();

    Ok(DriftReport {
        wiki_dir: decisions_dir.display().to_string(),
        rules_path: rules_path.display().to_string(),
        items,
        coverage_threshold: COVERAGE_THRESHOLD,
    })
}

fn score_decision(
    decision: Decision,
    rules: &[DiskRule],
    rule_id_set: &std::collections::HashSet<&str>,
) -> DriftItem {
    // Superseded decisions are history; exclude from active drift scoring.
    if decision.frontmatter.status == "superseded" {
        return DriftItem {
            decision,
            bucket: Bucket::Superseded,
            best_match: None,
            similarity: 0.0,
        };
    }

    // 1. Explicit `enforces:` shortcut. If any listed rule id exists in the
    //    pack, the decision is deterministically Covered.
    for rid in &decision.frontmatter.enforces {
        if rule_id_set.contains(rid.as_str()) {
            return DriftItem {
                decision,
                bucket: Bucket::Covered,
                best_match: Some(MatchedRule {
                    rule_id: rid.clone().into(),
                    shared_terms: Vec::new(),
                }),
                similarity: 1.0,
            };
        }
    }

    // 2. Fuzzy fallback. Jaccard token overlap between the decision body
    //    and each rule's textual blob (id + condition args + action params).
    let decision_tokens = meaningful_tokens(&decision.body);
    if decision_tokens.is_empty() {
        return DriftItem {
            decision,
            bucket: Bucket::Uncovered,
            best_match: None,
            similarity: 0.0,
        };
    }

    let mut best: Option<(f32, RuleId, Vec<String>)> = None;
    for rule in rules {
        let rule_tokens = meaningful_tokens(&rule_blob(rule));
        if rule_tokens.is_empty() {
            continue;
        }
        let shared: Vec<String> = decision_tokens
            .iter()
            .filter(|t| rule_tokens.contains(*t))
            .cloned()
            .collect();
        if shared.is_empty() {
            continue;
        }
        let union: std::collections::HashSet<&String> =
            decision_tokens.iter().chain(rule_tokens.iter()).collect();
        let jaccard = shared.len() as f32 / union.len() as f32;
        match &best {
            None => best = Some((jaccard, rule.id.clone().into(), shared)),
            Some((cur, _, _)) if jaccard > *cur => {
                best = Some((jaccard, rule.id.clone().into(), shared))
            }
            _ => {}
        }
    }

    match best {
        Some((similarity, rule_id, shared_terms)) if similarity >= COVERAGE_THRESHOLD => {
            DriftItem {
                decision,
                bucket: Bucket::LikelyCovered,
                best_match: Some(MatchedRule { rule_id, shared_terms }),
                similarity,
            }
        }
        _ => DriftItem {
            decision,
            bucket: Bucket::Uncovered,
            best_match: None,
            similarity: 0.0,
        },
    }
}

fn rule_blob(rule: &DiskRule) -> String {
    let mut parts: Vec<String> = vec![rule.id.clone()];
    for c in &rule.conditions {
        for a in &c.args {
            parts.push(a.clone());
        }
    }
    for a in &rule.actions {
        for p in &a.params {
            parts.push(p.clone());
        }
    }
    parts.join(" ")
}

fn meaningful_tokens(s: &str) -> std::collections::HashSet<String> {
    let stops: std::collections::HashSet<&str> = STOPWORDS.iter().copied().collect();
    s.to_ascii_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 1 && !stops.contains(t))
        .map(String::from)
        .collect()
}
```

- [ ] **Step 5: Run, verify all 6 tests pass**

Run: `cargo test -p phronesis-mcp --lib wiki_drift:: 2>&1 | tail -12`
Expected: `test result: ok. 6 passed; 0 failed`.

- [ ] **Step 6: Format + commit**

```bash
cargo fmt -p phronesis-mcp
git add crates/phronesis-mcp/src/wiki_drift.rs crates/phronesis-mcp/src/lib.rs
git -c commit.gpgsign=false commit \
  --author="Claude Opus 4.7 (1M context) <noreply@anthropic.com>" \
  -m "$(cat <<'EOF'
feat(wiki-drift): extractor with enforces shortcut + Jaccard fallback

Adds crates/phronesis-mcp/src/wiki_drift.rs. Two-tier scoring:

  1. `enforces:` frontmatter shortcut — when a decision explicitly
     lists a rule id and that rule exists in the pack, classify as
     Covered (deterministic, similarity 1.0, empty shared_terms).
  2. Jaccard token overlap — fallback for decisions without explicit
     enforces. Above COVERAGE_THRESHOLD (0.15, same as claude-md-drift)
     → LikelyCovered; below → Uncovered.

Superseded decisions are excluded from active drift, classified as
Superseded so they still appear in the report (as history) without
inflating the uncovered count.

Six unit tests cover all four buckets plus the "enforces lists a
nonexistent rule" fallthrough case.

Co-Authored-By: Andrew Waterman <andrew.waterman@gmail.com>
EOF
)"
```

---

### Task 2.2: `render_table`, `render_json`, `suggest_rule`

**Files:**
- Modify: `crates/phronesis-mcp/src/wiki_drift.rs`

- [ ] **Step 1: Write the failing tests**

Append to `wiki_drift.rs`'s `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn render_table_includes_all_buckets() {
        let (tmp, dec) = fixture_project(&["existing-rule"]);
        write_decision(
            &dec,
            "covered.md",
            "---\nid: a\ndate: 2026-05-01\nstatus: accepted\nenforces:\n  - existing-rule\n---\n",
        );
        write_decision(
            &dec,
            "uncovered.md",
            "---\nid: b\ndate: 2026-04-01\nstatus: accepted\n---\northogonal zzz\n",
        );
        write_decision(
            &dec,
            "old.md",
            "---\nid: c\ndate: 2025-01-01\nstatus: superseded\n---\n",
        );
        let report = run_with_dir(tmp.path(), &dec).unwrap();
        let table = render_table(&report);
        assert!(table.contains("covered"));
        assert!(table.contains("uncovered"));
        assert!(table.contains("superseded"));
        // Decision ids appear in the table.
        assert!(table.contains("a"));
        assert!(table.contains("b"));
        assert!(table.contains("c"));
    }

    #[test]
    fn render_json_is_valid_and_has_expected_shape() {
        let (tmp, dec) = fixture_project(&["existing-rule"]);
        write_decision(
            &dec,
            "x.md",
            "---\nid: a\ndate: 2026-05-01\nstatus: accepted\nenforces:\n  - existing-rule\n---\nbody\n",
        );
        let report = run_with_dir(tmp.path(), &dec).unwrap();
        let json = render_json(&report);
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert!(v["items"].is_array());
        assert_eq!(v["items"][0]["id"], "a");
        assert_eq!(v["items"][0]["bucket"], "covered");
        assert_eq!(v["items"][0]["best_match"]["rule_id"], "existing-rule");
        assert_eq!(v["coverage_threshold"].as_f64().unwrap(), 0.15);
    }

    #[test]
    fn suggest_rule_emits_template_for_uncovered_only() {
        let (tmp, dec) = fixture_project(&["unrelated-rule"]);
        write_decision(
            &dec,
            "x.md",
            "---\nid: my-decision\ndate: 2026-05-29\nstatus: accepted\n---\nuncovered content qqq\n",
        );
        let report = run_with_dir(tmp.path(), &dec).unwrap();
        let item = &report.items[0];
        assert_eq!(item.bucket, Bucket::Uncovered);
        let suggestion = suggest_rule(item).expect("uncovered → suggestion");
        let v: serde_json::Value = serde_json::from_str(&suggestion).unwrap();
        assert_eq!(v["id"], "decision-my-decision");
        assert_eq!(v["phase"], "pre");
        // Has a TODO placeholder so the operator picks the predicate.
        let cond_arg = v["when"][0].as_object().unwrap().values().next().unwrap();
        assert!(cond_arg.as_str().unwrap().contains("TODO"));
    }

    #[test]
    fn suggest_rule_returns_none_for_covered_and_superseded() {
        // Covered case.
        let (tmp, dec) = fixture_project(&["r"]);
        write_decision(&dec, "x.md",
            "---\nid: a\ndate: 2026-01-01\nstatus: accepted\nenforces:\n  - r\n---\n");
        let report = run_with_dir(tmp.path(), &dec).unwrap();
        assert_eq!(report.items[0].bucket, Bucket::Covered);
        assert!(suggest_rule(&report.items[0]).is_none());

        // Superseded case.
        let (tmp2, dec2) = fixture_project(&[]);
        write_decision(&dec2, "old.md",
            "---\nid: old\ndate: 2025-01-01\nstatus: superseded\n---\n");
        let report2 = run_with_dir(tmp2.path(), &dec2).unwrap();
        assert_eq!(report2.items[0].bucket, Bucket::Superseded);
        assert!(suggest_rule(&report2.items[0]).is_none());
    }
```

- [ ] **Step 2: Run, verify the 4 tests fail to compile**

Run: `cargo test -p phronesis-mcp --lib wiki_drift::tests::render 2>&1 | head -15`
Expected: compile errors — `render_table`, `render_json`, `suggest_rule` undefined.

- [ ] **Step 3: Implement the three functions**

Append to `wiki_drift.rs` (alongside `score_decision`):

```rust
/// Truncate a string for terminal display, appending `…` when truncated.
fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let head: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{}…", head)
    }
}

fn bucket_label(b: Bucket) -> &'static str {
    match b {
        Bucket::Covered => "covered",
        Bucket::LikelyCovered => "likely-covered",
        Bucket::Uncovered => "uncovered",
        Bucket::Superseded => "superseded",
    }
}

pub fn render_table(report: &DriftReport) -> String {
    let mut out = String::new();
    out.push_str(&format!("Wiki:  {}\n", report.wiki_dir));
    out.push_str(&format!("Rules: {}\n\n", report.rules_path));

    if report.items.is_empty() {
        out.push_str("No decisions found.\n");
        return out;
    }

    out.push_str(&format!(
        "{:<28}  {:<14}  {:<10}  Match\n",
        "Decision", "Bucket", "Similarity"
    ));
    out.push_str(&format!(
        "{:-<28}  {:-<14}  {:-<10}  {:-<40}\n",
        "", "", "", ""
    ));

    for item in &report.items {
        let id = truncate(&item.decision.frontmatter.id, 28);
        let bucket = bucket_label(item.bucket);
        let match_desc = match &item.best_match {
            Some(m) => format!("→ rule {}", m.rule_id.as_str()),
            None => "(no match)".to_string(),
        };
        out.push_str(&format!(
            "{:<28}  {:<14}  {:<10.2}  {}\n",
            id, bucket, item.similarity, match_desc,
        ));
    }
    out
}

pub fn render_json(report: &DriftReport) -> String {
    let items: Vec<serde_json::Value> = report
        .items
        .iter()
        .map(|item| {
            let best_match = item.best_match.as_ref().map(|m| {
                serde_json::json!({
                    "rule_id": m.rule_id.as_str(),
                    "shared_terms": m.shared_terms,
                })
            });
            serde_json::json!({
                "id": item.decision.frontmatter.id,
                "date": item.decision.frontmatter.date,
                "status": item.decision.frontmatter.status,
                "bucket": bucket_label(item.bucket),
                "similarity": item.similarity,
                "best_match": best_match,
                "file": item.decision.path.display().to_string(),
            })
        })
        .collect();

    serde_json::to_string_pretty(&serde_json::json!({
        "wiki_dir": report.wiki_dir,
        "rules_path": report.rules_path,
        "coverage_threshold": report.coverage_threshold,
        "items": items,
    }))
    .unwrap_or_else(|_| String::from("{}"))
}

/// Emit a draft v2 rule JSON for an Uncovered decision. The condition
/// carries a TODO placeholder — the operator picks the actual predicate
/// and substring at review time. Returns None for Covered, LikelyCovered,
/// and Superseded — only uncovered items get suggestions.
pub fn suggest_rule(item: &DriftItem) -> Option<String> {
    if item.bucket != Bucket::Uncovered {
        return None;
    }
    let rule_id = format!("decision-{}", item.decision.frontmatter.id);
    // Use the first imperative-looking sentence from the body as the message
    // if we can find one, else fall back to a generic line. Cheap heuristic:
    // first non-blank line of the body.
    let message = item
        .decision
        .body
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .unwrap_or("(decision body was empty — fill in a rule message)")
        .to_string();

    let suggestion = serde_json::json!({
        "id": rule_id,
        "phase": "pre",
        "priority": 5,
        "when": [
            { "new_content_contains": "// TODO: pick a substring or command to match" }
        ],
        "then": { "warn": message },
        "_source": {
            "decision_id": item.decision.frontmatter.id,
            "decision_file": item.decision.path.display().to_string(),
        }
    });
    Some(serde_json::to_string_pretty(&suggestion).unwrap_or_default())
}
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p phronesis-mcp --lib wiki_drift:: 2>&1 | tail -10`
Expected: `test result: ok. 10 passed; 0 failed` (6 from Task 2.1 + 4 new).

- [ ] **Step 5: Format + commit**

```bash
cargo fmt -p phronesis-mcp
git add crates/phronesis-mcp/src/wiki_drift.rs
git -c commit.gpgsign=false commit \
  --author="Claude Opus 4.7 (1M context) <noreply@anthropic.com>" \
  -m "$(cat <<'EOF'
feat(wiki-drift): render_table, render_json, suggest_rule

Rendering surfaces:
- render_table: human-scannable table grouping all four buckets
  (covered / likely-covered / uncovered / superseded).
- render_json: stable JSON for programmatic callers; round-trips
  the bucket label, similarity, best_match (rule id + shared
  terms), and source file path.
- suggest_rule: draft v2 rule JSON for Uncovered decisions only.
  Carries a `// TODO: pick a substring` placeholder so the
  operator picks the predicate after review. Returns None for
  Covered / LikelyCovered / Superseded.

Four new tests cover the table shape, JSON validity + key shape,
suggest_rule emit-for-uncovered, suggest_rule None-for-others.

Co-Authored-By: Andrew Waterman <andrew.waterman@gmail.com>
EOF
)"
```

---

## COMMIT 3 — CLI + MCP tool

### Task 3.1: `phr-mcp wiki-drift` CLI subcommand

**Files:**
- Modify: `crates/phronesis-mcp/src/main.rs`
- Create: `crates/phronesis-mcp/tests/wiki_drift_integration.rs`

- [ ] **Step 1: Write the failing integration tests**

Create `crates/phronesis-mcp/tests/wiki_drift_integration.rs`:

```rust
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn run_wiki_drift(project_root: &Path, args: &[&str]) -> Output {
    let bin = env!("CARGO_BIN_EXE_phr-mcp");
    Command::new(bin)
        .arg("wiki-drift")
        .args(args)
        .current_dir(project_root)
        .output()
        .expect("spawn phr-mcp wiki-drift")
}

fn fixture(rules_json: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let phr = dir.path().join(".phronesis");
    let dec = phr.join("wiki").join("decisions");
    fs::create_dir_all(&dec).unwrap();
    fs::write(phr.join("rules.json"), rules_json).unwrap();
    dir
}

#[test]
fn wiki_drift_table_lists_decision_buckets() {
    let dir = fixture(r#"{"rules":[{"id":"r","phase":"pre","priority":1,"when":[{"new_content_contains":"x"}],"then":{"warn":"m"}}]}"#);
    let dec = dir.path().join(".phronesis/wiki/decisions");
    fs::write(
        dec.join("a.md"),
        "---\nid: a\ndate: 2026-05-29\nstatus: accepted\nenforces:\n  - r\n---\n",
    ).unwrap();
    fs::write(
        dec.join("b.md"),
        "---\nid: b\ndate: 2026-05-29\nstatus: accepted\n---\northogonal zzz\n",
    ).unwrap();

    let out = run_wiki_drift(dir.path(), &[]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("covered"));
    assert!(stdout.contains("uncovered"));
}

#[test]
fn wiki_drift_json_is_machine_readable() {
    let dir = fixture(r#"{"rules":[]}"#);
    fs::write(
        dir.path().join(".phronesis/wiki/decisions/x.md"),
        "---\nid: x\ndate: 2026-05-29\nstatus: accepted\n---\nbody\n",
    ).unwrap();
    let out = run_wiki_drift(dir.path(), &["--json"]);
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert!(v["items"].is_array());
    assert_eq!(v["items"][0]["id"], "x");
}

#[test]
fn wiki_drift_suggest_emits_draft_for_uncovered() {
    let dir = fixture(r#"{"rules":[]}"#);
    fs::write(
        dir.path().join(".phronesis/wiki/decisions/uncov.md"),
        "---\nid: my-decision\ndate: 2026-05-29\nstatus: accepted\n---\nimperative one-liner\n",
    ).unwrap();
    let out = run_wiki_drift(dir.path(), &["--suggest"]);
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("decision-my-decision"));
    assert!(stderr.contains("TODO"));
}

#[test]
fn wiki_drift_missing_dir_errors_clearly() {
    let dir = tempfile::tempdir().unwrap();
    // No .phronesis/wiki/decisions/ at all.
    let out = run_wiki_drift(dir.path(), &[]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.to_lowercase().contains("not found") || stderr.to_lowercase().contains("missing"));
}

#[test]
fn wiki_drift_override_wiki_dir_arg() {
    // Decisions live in a non-default location.
    let dir = fixture(r#"{"rules":[]}"#);
    let custom = dir.path().join("custom").join("decisions");
    fs::create_dir_all(&custom).unwrap();
    fs::write(
        custom.join("x.md"),
        "---\nid: x\ndate: 2026-05-29\nstatus: accepted\n---\n",
    ).unwrap();
    let out = run_wiki_drift(dir.path(), &["--wiki-dir", custom.to_str().unwrap()]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("x"));
}
```

- [ ] **Step 2: Run, verify failure (subcommand missing)**

Run: `cargo test -p phronesis-mcp --test wiki_drift_integration 2>&1 | tail -15`
Expected: failures — `wiki-drift` subcommand doesn't exist (clap rejects, non-zero exits, no expected stdout).

- [ ] **Step 3: Add the `WikiDrift` subcommand to `main.rs`**

In the `Command` enum in `crates/phronesis-mcp/src/main.rs`, insert this variant just below `MemoryDrift`:

```rust
    /// Detect drift between ADR-style decision documents in
    /// `.phronesis/wiki/decisions/` and the current rule pack.
    /// Heuristic — explicit `enforces:` frontmatter lookups beat
    /// Jaccard fallback. Read-only; always exits 0 on success.
    #[command(name = "wiki-drift")]
    WikiDrift {
        /// Project root (defaults to current directory).
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Override the decisions directory. Defaults to
        /// `<project_root>/.phronesis/wiki/decisions/`.
        #[arg(long)]
        wiki_dir: Option<PathBuf>,
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
        /// Emit draft v2 rule JSON for each uncovered decision, on stderr.
        #[arg(long)]
        suggest: bool,
    },
```

Then in the `main()` match, add (modeled on the existing `MemoryDrift` arm):

```rust
        Command::WikiDrift { path, wiki_dir, json, suggest } => {
            use phronesis_mcp::wiki;
            use phronesis_mcp::wiki_drift::{
                DriftError, render_json, render_table, run_with_dir, suggest_rule,
            };
            let root = if path.is_absolute() {
                path
            } else {
                std::env::current_dir()
                    .map(|p| p.join(&path))
                    .unwrap_or(path)
            };
            let dir = wiki_dir.unwrap_or_else(|| wiki::default_wiki_dir(&root).join("decisions"));
            match run_with_dir(&root, &dir) {
                Ok(report) => {
                    if json {
                        println!("{}", render_json(&report));
                    } else {
                        print!("{}", render_table(&report));
                    }
                    if suggest {
                        let drafts: Vec<String> =
                            report.items.iter().filter_map(suggest_rule).collect();
                        if !drafts.is_empty() {
                            eprintln!("\n--- draft rules for uncovered decisions ---\n");
                            for draft in drafts {
                                eprintln!("{}\n", draft);
                            }
                        }
                    }
                    Ok(())
                }
                Err(DriftError::Wiki(phronesis_mcp::wiki::WikiError::DirMissing(p))) => {
                    eprintln!("error: wiki decisions directory not found at {}", p);
                    eprintln!(
                        "hint: run `phr-mcp init` to create it, or pass `--wiki-dir <path>`."
                    );
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("error: {}", e);
                    std::process::exit(1);
                }
            }
        }
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p phronesis-mcp --test wiki_drift_integration 2>&1 | tail -12`
Expected: `test result: ok. 5 passed; 0 failed`.

- [ ] **Step 5: Format + commit**

```bash
cargo fmt -p phronesis-mcp
git add crates/phronesis-mcp/src/main.rs crates/phronesis-mcp/tests/wiki_drift_integration.rs
git -c commit.gpgsign=false commit \
  --author="Claude Opus 4.7 (1M context) <noreply@anthropic.com>" \
  -m "$(cat <<'EOF'
feat: phr-mcp wiki-drift CLI subcommand

Reads .phronesis/wiki/decisions/, classifies each decision into
covered / likely-covered / uncovered / superseded against the
current rule pack, and renders a table or JSON. Optional --suggest
mode emits draft v2 rule JSON on stderr for each uncovered decision.

The handler accepts --wiki-dir for testing and non-default layouts;
defaults to <project_root>/.phronesis/wiki/decisions/. Five
integration tests cover the table/JSON/suggest/missing-dir/override
paths end-to-end through the real binary.

Co-Authored-By: Andrew Waterman <andrew.waterman@gmail.com>
EOF
)"
```

---

### Task 3.2: `phr-mcp decision new <slug>` helper

**Files:**
- Modify: `crates/phronesis-mcp/src/main.rs`
- Create: `crates/phronesis-mcp/tests/decision_new_integration.rs`

- [ ] **Step 1: Write the failing integration tests**

Create `crates/phronesis-mcp/tests/decision_new_integration.rs`:

```rust
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn run_decision_new(project_root: &Path, slug: &str) -> Output {
    let bin = env!("CARGO_BIN_EXE_phr-mcp");
    Command::new(bin)
        .args(["decision", "new", slug])
        .current_dir(project_root)
        .output()
        .expect("spawn phr-mcp decision new")
}

#[test]
fn decision_new_creates_file_with_template() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join(".phronesis/wiki/decisions")).unwrap();

    let out = run_decision_new(dir.path(), "my-first-decision");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

    // Find the created file (matches today's date + slug).
    let dec_dir = dir.path().join(".phronesis/wiki/decisions");
    let files: Vec<_> = fs::read_dir(&dec_dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("md"))
        .collect();
    assert_eq!(files.len(), 1);
    let name = files[0].file_name().into_string().unwrap();
    assert!(name.ends_with("-my-first-decision.md"));
    // Filename starts with an ISO date.
    assert!(name.chars().take(10).all(|c| c.is_ascii_digit() || c == '-'));

    let content = fs::read_to_string(files[0].path()).unwrap();
    assert!(content.starts_with("---\n"));
    assert!(content.contains("id: my-first-decision"));
    assert!(content.contains("status: proposed"));
    assert!(content.contains("## Context"));
    assert!(content.contains("## Decision"));
    assert!(content.contains("## Enforcement"));
}

#[test]
fn decision_new_refuses_to_overwrite_existing() {
    let dir = tempfile::tempdir().unwrap();
    let dec_dir = dir.path().join(".phronesis/wiki/decisions");
    fs::create_dir_all(&dec_dir).unwrap();

    // First run succeeds.
    let out1 = run_decision_new(dir.path(), "same-slug");
    assert!(out1.status.success());

    // Second run with same slug on the same day must refuse.
    let out2 = run_decision_new(dir.path(), "same-slug");
    assert!(!out2.status.success());
    let stderr = String::from_utf8_lossy(&out2.stderr);
    assert!(stderr.to_lowercase().contains("exists") || stderr.to_lowercase().contains("refuse"));
}

#[test]
fn decision_new_rejects_invalid_slug() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join(".phronesis/wiki/decisions")).unwrap();

    // Spaces are invalid (slugs are kebab-case).
    let out = run_decision_new(dir.path(), "has spaces");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.to_lowercase().contains("slug"));
}
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test -p phronesis-mcp --test decision_new_integration 2>&1 | tail -15`
Expected: failures — `decision new` subcommand missing.

- [ ] **Step 3: Add the `Decision` subcommand to `main.rs`**

In `Command` enum, insert this variant just below `WikiDrift`:

```rust
    /// Wiki-related helpers (scaffold a new ADR-style decision page).
    Decision {
        #[command(subcommand)]
        cmd: DecisionCmd,
    },
```

And add the inner subcommand enum near the other `Command` definitions:

```rust
#[derive(clap::Subcommand, Debug)]
enum DecisionCmd {
    /// Scaffold a new decision page at
    /// `.phronesis/wiki/decisions/<today>-<slug>.md`.
    New {
        /// Kebab-case slug for the decision. Must match `[a-z0-9-]+`.
        slug: String,
        /// Project root (defaults to current directory).
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
}
```

Then in `main()`'s match, add the dispatch arm:

```rust
        Command::Decision { cmd } => match cmd {
            DecisionCmd::New { slug, path } => {
                use phronesis_mcp::wiki;
                let root = if path.is_absolute() {
                    path
                } else {
                    std::env::current_dir()
                        .map(|p| p.join(&path))
                        .unwrap_or(path)
                };
                // Validate slug: kebab-case, alphanumeric + hyphen, non-empty.
                let valid = !slug.is_empty()
                    && slug.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
                if !valid {
                    eprintln!(
                        "error: invalid slug `{}`. Slugs must match `[a-z0-9-]+` (kebab-case).",
                        slug
                    );
                    std::process::exit(1);
                }

                let date = today_iso();
                let dir = wiki::default_wiki_dir(&root).join("decisions");
                let filename = format!("{}-{}.md", date, slug);
                let path = dir.join(&filename);
                if path.exists() {
                    eprintln!("error: {} already exists; refusing to overwrite.", path.display());
                    std::process::exit(1);
                }
                std::fs::create_dir_all(&dir).map_err(|e| {
                    anyhow::anyhow!("create {}: {}", dir.display(), e)
                })?;

                let template = format!(
                    "---\n\
                     id: {slug}\n\
                     date: {date}\n\
                     status: proposed\n\
                     enforces: []\n\
                     superseded_by: null\n\
                     tags: []\n\
                     ---\n\
                     \n\
                     # {slug}\n\
                     \n\
                     ## Context\n\
                     \n\
                     What problem are we solving / what observations led here?\n\
                     \n\
                     ## Decision\n\
                     \n\
                     What we decided.\n\
                     \n\
                     ## Enforcement\n\
                     \n\
                     - (none yet — add `enforces:` rule ids in frontmatter when a rule lands)\n\
                     \n\
                     ## Consequences\n\
                     \n\
                     What follows from this.\n",
                    slug = slug,
                    date = date,
                );
                std::fs::write(&path, template).map_err(|e| {
                    anyhow::anyhow!("write {}: {}", path.display(), e)
                })?;
                println!("created {}", path.display());
                Ok(())
            }
        },
```

And add the `today_iso` helper near the top of the file (or alongside other small helpers):

```rust
/// ISO-8601 date string for the local clock (YYYY-MM-DD). Uses chrono,
/// which is already a phronesis-mcp dep (clock_facts).
fn today_iso() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p phronesis-mcp --test decision_new_integration 2>&1 | tail -12`
Expected: `test result: ok. 3 passed; 0 failed`.

- [ ] **Step 5: Format + commit**

```bash
cargo fmt -p phronesis-mcp
git add crates/phronesis-mcp/src/main.rs crates/phronesis-mcp/tests/decision_new_integration.rs
git -c commit.gpgsign=false commit \
  --author="Claude Opus 4.7 (1M context) <noreply@anthropic.com>" \
  -m "$(cat <<'EOF'
feat: phr-mcp decision new <slug> helper

Scaffolds .phronesis/wiki/decisions/<today>-<slug>.md from a
template (frontmatter + Context/Decision/Enforcement/Consequences
sections). Slug validation: kebab-case `[a-z0-9-]+`. Refuses to
overwrite an existing same-day file. ISO date via chrono::Local
(already a dep from clock_facts).

Three integration tests: happy path (file shape + filename
convention), refuse-overwrite, reject-invalid-slug.

Co-Authored-By: Andrew Waterman <andrew.waterman@gmail.com>
EOF
)"
```

---

### Task 3.3: `get_wiki_drift` MCP tool

**Files:**
- Modify: `crates/phronesis-mcp/src/server_params.rs`
- Modify: `crates/phronesis-mcp/src/server.rs`

- [ ] **Step 1: Write the failing tests**

In `crates/phronesis-mcp/src/server.rs`, find the `#[cfg(test)] mod tool_registration_tests` block (added in Task 1.1 of v0.8.0) and append a test:

```rust
    /// Regression: `get_wiki_drift` (0.9.0) is registered.
    #[test]
    fn wiki_drift_tool_is_registered() {
        let mcp = EpistemeMcp::new();
        assert!(
            mcp.tool_router.has_route("get_wiki_drift"),
            "get_wiki_drift tool must be registered (matches `phr-mcp wiki-drift` CLI)"
        );
    }
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test -p phronesis-mcp --lib tool_registration_tests::wiki_drift 2>&1 | tail -8`
Expected: assertion fails — `get_wiki_drift` not in router.

- [ ] **Step 3: Add the params struct**

In `crates/phronesis-mcp/src/server_params.rs`, append (matching the shape of `GetMemoryDriftParams`):

```rust
#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct GetWikiDriftParams {
    /// Override the decisions directory. Defaults to
    /// `<project_root>/.phronesis/wiki/decisions/`.
    #[serde(default)]
    pub wiki_dir: Option<String>,
    /// `"json"` (default) or `"table"`.
    #[serde(default)]
    pub format: Option<String>,
}
```

- [ ] **Step 4: Add the tool method to `server.rs`**

In `crates/phronesis-mcp/src/server.rs`, in the `#[tool_router] impl EpistemeMcp` block, add a new tool method alongside `get_memory_drift` (mirror its shape exactly — same error-handling pattern with `.map_err(|e| match e {...})?`):

```rust
    #[tool(
        description = "Detect drift between ADR-style decision documents in `.phronesis/wiki/decisions/` and the current rule pack. Each decision is classified as `covered` (an explicit `enforces:` frontmatter entry matches an existing rule), `likely-covered` (Jaccard fuzzy match above 0.15), `uncovered` (drift candidate — should become a rule or be marked superseded), or `superseded` (history, excluded from active drift). Heuristic by design, no LLM call. Use when the user mentions decisions, ADRs, or asks whether a recorded choice is being enforced. Optional `wiki_dir` overrides the default. Optional `format`: \"json\" (default) or \"table\"."
    )]
    async fn get_wiki_drift(
        &self,
        Parameters(params): Parameters<GetWikiDriftParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::wiki;
        use crate::wiki_drift::{DriftError, render_json, render_table, run_with_dir};

        let root = security::project_root();
        let dir = match params.wiki_dir.as_deref() {
            Some(p) => std::path::PathBuf::from(p),
            None => wiki::default_wiki_dir(&root).join("decisions"),
        };
        let report = run_with_dir(&root, &dir).map_err(|e| match e {
            DriftError::Wiki(wiki::WikiError::DirMissing(p)) => Self::err(format!(
                "wiki decisions directory not found at {} — run `phr-mcp init` to create it, or pass `wiki_dir` to point elsewhere",
                p
            )),
            other => Self::err(other.to_string()),
        })?;

        let uncovered = report
            .items
            .iter()
            .filter(|i| matches!(i.bucket, crate::wiki_drift::Bucket::Uncovered))
            .count();
        Self::log_event("get_wiki_drift", |e| {
            e.with("items_total", report.items.len() as u64)
                .with("items_uncovered", uncovered as u64)
        });

        match params.format.as_deref() {
            Some("table") => Self::ok_text(render_table(&report)),
            _ => Self::ok_text(render_json(&report)),
        }
    }
```

- [ ] **Step 5: Run, verify pass**

Run: `cargo test -p phronesis-mcp --lib tool_registration_tests 2>&1 | tail -10`
Expected: all 4 tool-registration tests pass (the 3 existing + the new wiki one).

- [ ] **Step 6: Format + commit**

```bash
cargo fmt -p phronesis-mcp
git add crates/phronesis-mcp/src/server.rs crates/phronesis-mcp/src/server_params.rs
git -c commit.gpgsign=false commit \
  --author="Claude Opus 4.7 (1M context) <noreply@anthropic.com>" \
  -m "$(cat <<'EOF'
feat: get_wiki_drift MCP tool

Exposes the wiki-drift extractor on the MCP surface so models can
invoke it during conversation (e.g. when the user mentions a recent
decision or asks whether a recorded choice is being enforced).
Mirrors get_memory_drift / get_claude_md_drift shape: optional
wiki_dir override, optional format, error-handling via map_err with
a hinted DirMissing message.

A new tool_registration test guards against the tool name slipping
out of sync with the CLI.

Co-Authored-By: Andrew Waterman <andrew.waterman@gmail.com>
EOF
)"
```

---

## COMMIT 4 — init scaffold + release

### Task 4.1: `init` scaffolds `.phronesis/wiki/decisions/` + gitignore exception

**Files:**
- Modify: `crates/phronesis-mcp/src/init.rs`
- Modify: `crates/phronesis-mcp/tests/init_integration.rs`

- [ ] **Step 1: Write the failing tests**

Append to `crates/phronesis-mcp/tests/init_integration.rs`:

```rust
#[test]
fn init_creates_wiki_decisions_directory_and_readme() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_init(&[], dir.path());
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

    let dec = dir.path().join(".phronesis/wiki/decisions");
    assert!(dec.is_dir(), "decisions dir should be created");
    let readme = dec.join("README.md");
    assert!(readme.is_file(), "README.md should be created");
    let body = std::fs::read_to_string(&readme).unwrap();
    assert!(body.to_lowercase().contains("decision"));
    assert!(body.contains("frontmatter") || body.contains("frontmatter"));
}

#[test]
fn init_preserves_existing_wiki_readme() {
    let dir = tempfile::tempdir().unwrap();
    let dec = dir.path().join(".phronesis/wiki/decisions");
    std::fs::create_dir_all(&dec).unwrap();
    let original = "# my custom README\n\nproject-specific notes\n";
    std::fs::write(dec.join("README.md"), original).unwrap();

    let out = run_init(&[], dir.path());
    assert!(out.status.success());

    let after = std::fs::read_to_string(dec.join("README.md")).unwrap();
    assert_eq!(after, original, "init must not overwrite an existing README");
}

#[test]
fn init_gitignore_carves_out_wiki_exception() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_init(&[], dir.path());
    assert!(out.status.success());

    let gi = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    // Broad ignore still present.
    assert!(gi.contains(".phronesis/"));
    // Un-ignore lines for the wiki tree.
    assert!(gi.contains("!.phronesis/wiki/"));
    assert!(gi.contains("!.phronesis/wiki/**"));
    // The un-ignore appears AFTER the broad ignore (gitignore rule order matters).
    let broad = gi.find(".phronesis/").unwrap();
    let unignore = gi.find("!.phronesis/wiki/").unwrap();
    assert!(broad < unignore, "un-ignore must come after the broad ignore");
}

#[test]
fn init_gitignore_idempotent_on_second_run() {
    let dir = tempfile::tempdir().unwrap();
    run_init(&[], dir.path());
    let first = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    run_init(&[], dir.path());
    let second = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert_eq!(first, second, "gitignore must not duplicate entries on re-run");
}
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test -p phronesis-mcp --test init_integration init_ 2>&1 | grep -E '(FAILED|wiki|gitignore)' | head -10`
Expected: failures — `decisions` dir not created, gitignore lacks the exception, etc.

- [ ] **Step 3: Add the scaffold logic to `init.rs`**

In `crates/phronesis-mcp/src/init.rs`'s `pub fn run`, find the section that calls `update_gitignore` and the surrounding writers, and add a call to a new `write_wiki_scaffold` writer alongside the other `write_*` calls (after `write_rules_file` so the .phronesis/ dir exists). Around line 380–402 (the body of `run`):

```rust
    if !opts.hooks_only {
        write_rules_file(&root, &opts, &mut report)?;
        write_durable_md(&root, &opts, &mut report)?;
        write_wiki_scaffold(&root, &opts, &mut report)?;     // <-- ADD
    }
    if !opts.rules_only && !opts.hooks_only {
        update_gitignore(&root, &opts, &mut report)?;
    }
```

Then add the `write_wiki_scaffold` function next to the other file writers (after `write_durable_md`):

```rust
const WIKI_DECISIONS_README: &str = "\
# `.phronesis/wiki/decisions/`

ADR-style decision pages. Each file is one decision (e.g. \
`2026-05-29-card-game-terminology.md`). The first block is YAML \
frontmatter (`id`, `date`, `status`, optional `enforces`, \
`superseded_by`, `tags`). The body uses Context / Decision / \
Enforcement / Consequences sections.

Run `phr-mcp wiki-drift` to see which decisions lack rule coverage.
Create new pages with `phr-mcp decision new <slug>`.

This directory is tracked in git (un-ignored from the broader \
`.phronesis/` ignore) because decisions are project knowledge. \
The rest of `.phronesis/` (rules.json, log.jsonl, etc.) stays \
gitignored.
";

fn write_wiki_scaffold(
    root: &Path,
    opts: &InitOpts,
    report: &mut InitReport,
) -> Result<(), InitError> {
    let dir = root.join(".phronesis").join("wiki").join("decisions");
    let readme = dir.join("README.md");

    if readme.exists() {
        report.steps.push(
            "= .phronesis/wiki/decisions/README.md already exists — leaving unchanged".to_string(),
        );
        return Ok(());
    }

    if opts.dry_run {
        report.steps.push(
            "+ would create .phronesis/wiki/decisions/ + README.md".to_string(),
        );
        return Ok(());
    }

    std::fs::create_dir_all(&dir).map_err(|e| InitError::Io {
        path: dir.display().to_string(),
        source: e,
    })?;
    std::fs::write(&readme, WIKI_DECISIONS_README).map_err(|e| InitError::Io {
        path: readme.display().to_string(),
        source: e,
    })?;
    report
        .steps
        .push("+ created .phronesis/wiki/decisions/ + README.md".to_string());
    Ok(())
}
```

- [ ] **Step 4: Extend `update_gitignore` to add the wiki exception**

Find `update_gitignore` (after `write_wiki_scaffold`) and locate where it computes the `needed` list of lines. Add the un-ignore entries to that list AFTER the existing entries so they land below the broad `.phronesis/` ignore (gitignore reads top-down; un-ignores must come after the ignore they exempt).

Replace the array literal `&[".phronesis/log.jsonl", ".phronesis/log.jsonl.1", ".phronesis/rules.json.bak"]` with:

```rust
    let needed: &[&str] = &[
        ".phronesis/log.jsonl",
        ".phronesis/log.jsonl.1",
        ".phronesis/rules.json.bak",
        "!.phronesis/wiki/",
        "!.phronesis/wiki/**",
    ];
```

The existing append logic already handles "skip lines that are already present" — that's the idempotence guarantee the test asserts.

- [ ] **Step 5: Run, verify pass**

Run: `cargo test -p phronesis-mcp --test init_integration 2>&1 | grep -E 'test result|FAILED' | head -5`
Expected: all init_integration tests pass (the 4 new ones plus the existing ones).

- [ ] **Step 6: Format + commit**

```bash
cargo fmt -p phronesis-mcp
git add crates/phronesis-mcp/src/init.rs crates/phronesis-mcp/tests/init_integration.rs
git -c commit.gpgsign=false commit \
  --author="Claude Opus 4.7 (1M context) <noreply@anthropic.com>" \
  -m "$(cat <<'EOF'
feat(init): scaffold .phronesis/wiki/decisions/ + gitignore exception

phr-mcp init now creates .phronesis/wiki/decisions/ with a README
that explains the page schema and points at wiki-drift + decision new.
The directory is left alone on re-run if README.md already exists.

Idempotent gitignore extension: !.phronesis/wiki/ and
!.phronesis/wiki/** are appended AFTER the existing
.phronesis/ entries so the un-ignore takes effect (gitignore
reads top-down). The decisions tree becomes versioned project
knowledge while rules.json / log.jsonl stay ignored.

Four new integration tests: scaffold creation, existing-README
preservation, gitignore exception present + ordered, idempotency
on re-run.

Co-Authored-By: Andrew Waterman <andrew.waterman@gmail.com>
EOF
)"
```

---

### Task 4.2: Version bump to 0.9.0 + CLAUDE.md docs

**Files:**
- Modify: `crates/phronesis-mcp/Cargo.toml`
- Modify: `crates/phronesis-mcp/CLAUDE.md`

- [ ] **Step 1: Bump version**

In `crates/phronesis-mcp/Cargo.toml`, change `version = "0.8.1"` to `version = "0.9.0"`.

- [ ] **Step 2: Document `wiki-drift` and `decision new` in CLAUDE.md**

In `crates/phronesis-mcp/CLAUDE.md`, locate the "Build & Run" command list (search for `cargo run -- memory-drift`) and add immediately below it:

```
cargo run -- wiki-drift      # Heuristic: which .phronesis/wiki/decisions/ ADRs lack rule coverage?
cargo run -- decision new <slug>  # Scaffold a new ADR page at .phronesis/wiki/decisions/<today>-<slug>.md
```

Then locate the "Drift detection — CLAUDE.md and auto-memory ↔ rules" subsection and append a third paragraph for wiki-drift:

```markdown
`phr-mcp wiki-drift` (MCP: `get_wiki_drift`) walks
`.phronesis/wiki/decisions/`, parses ADR-style frontmatter on each
page, and classifies decisions into `covered` / `likely-covered` /
`uncovered` / `superseded` against the current rule pack. Explicit
`enforces: [rule-id]` frontmatter beats the Jaccard fallback —
authors who list which rules enforce a decision get a deterministic
match. `--suggest` emits draft v2 rule JSON on stderr for uncovered
decisions. Pair with `phr-mcp decision new <slug>` to scaffold new
ADR pages from a template.
```

In the "init writes/merges N files" list, increment the count and add the wiki entry:

```markdown
`init` writes/merges seven files:
- `.claude/settings.local.json` — hook config (preserves existing permissions/hooks)
- `.mcp.json` — MCP server registration
- `.phronesis/rules.json` — starter rule pack (left alone on re-run unless --force)
- `.phronesis/durable.md` — default re-injected directives, including drift-discipline nudges that point the model at `get_claude_md_drift` / `get_memory_drift` / `get_wiki_drift`. Left alone on re-run; edit in place to customize.
- `.phronesis/wiki/decisions/README.md` — wiki scaffold; the directory is un-ignored from the broad `.phronesis/` gitignore. Left alone on re-run.
- `.gemini/settings.json` — MCP server registration + BeforeTool/AfterTool hooks for Gemini CLI
- `.gitignore` — log/backup paths + `!.phronesis/wiki/**` exception so the decisions tree is versioned
```

In the Architecture section, add the new modules to the file map:

```markdown
- `src/wiki.rs` — Page primitives: Decision struct, YAML-frontmatter parser, `walk_decisions` iterator. Shared by wiki_drift and future wiki-consuming modules.
- `src/wiki_drift.rs` — Drift extractor: scores decisions vs rules.json, surfaces `Uncovered` ones; `enforces:` frontmatter shortcut beats Jaccard.
```

And update the server.rs MCP-tools list to include the new tool:

```markdown
- `src/server.rs` — `EpistemeMcp` with MCP tools via rmcp macros (rules, facts, fire/agenda, get_stats, audit_codebase, get_debt_trend, get_claude_md_drift, get_memory_drift, get_wiki_drift)
```

- [ ] **Step 3: Build + full test run**

Run: `cargo build -p phronesis-mcp 2>&1 | tail -3`
Expected: Finished, no new warnings.

Run: `cargo test --workspace --tests 2>&1 | grep -E '^test result: ok' | awk '{p+=$4;f+=$6} END{print "passed:",p,"failed:",f}'`
Expected: failed: 0.

- [ ] **Step 4: Format + commit**

```bash
cargo fmt -p phronesis-mcp
git add crates/phronesis-mcp/Cargo.toml crates/phronesis-mcp/CLAUDE.md
git -c commit.gpgsign=false commit \
  --author="Claude Opus 4.7 (1M context) <noreply@anthropic.com>" \
  -m "$(cat <<'EOF'
chore: bump phronesis-mcp 0.9.0 + document wiki-drift

MINOR bump per CLAUDE.md semver: wiki-drift is a new user-visible
surface (CLI subcommand + MCP tool), decision new is a new helper,
init now scaffolds a new file (.phronesis/wiki/decisions/README.md)
and extends .gitignore with a versioning exception.

CLAUDE.md updated: Build & Run list, Drift detection subsection,
init's writes/merges file list (six → seven), architecture file
map (+wiki.rs +wiki_drift.rs), server.rs MCP-tools enumeration.

Co-Authored-By: Andrew Waterman <andrew.waterman@gmail.com>
EOF
)"
```

---

## Rollout (after all commits land — operational, run by the user)

These create no commits (init writes are local; reinstall is a build artifact). Run any time.

- [ ] Reinstall the binary: `cargo install --path crates/phronesis-mcp` → `phr-mcp --version` shows 0.9.0.
- [ ] Run `phr-mcp init --hooks-only` (or full `phr-mcp init`) on this project to create `.phronesis/wiki/decisions/` + README + the `.gitignore` exception. Existing rules / hooks / durable.md are left alone.
- [ ] Seed the corpus: run `phr-mcp decision new card-game-terminology` (or any meaningful slug), open the file, fill in Context / Decision / Enforcement / Consequences. Optionally add `enforces: [block-commit-during-business-hours, block-push-during-business-hours]` to the commit-timing decision.
- [ ] Run `phr-mcp wiki-drift` to see initial coverage status.
- [ ] (Optional) Same for `~/Git/rulgamr` — `phr-mcp init --hooks-only` to scaffold there too.

---

## Self-Review

- **Spec coverage:**
  - §"The wiki scaffold" (Layout, gitignore, decision page schema) → Tasks 4.1 + 1.2 (DecisionFrontmatter shape)
  - §"The extractor algorithm" → Tasks 2.1 (scoring) + 2.2 (rendering + suggest)
  - §"CLI surface" → Tasks 3.1 (wiki-drift) + 3.2 (decision new)
  - §"MCP tool surface" → Task 3.3
  - §"Module layout" → file structure table at top
  - §"Testing strategy" → unit tests live in each module's task; integration tests in Task 3.1 / 3.2 / 4.1
  - §"Commit plan (4 commits)" → expanded to 10 commits in this plan, each task = one commit. The SPEC's 4 conceptual commits map to: Tasks 1.1+1.2+1.3 (Commit 1 group), Tasks 2.1+2.2 (Commit 2 group), Tasks 3.1+3.2+3.3 (Commit 3 group), Tasks 4.1+4.2 (Commit 4 group). Splitting at task boundaries makes subagent execution + per-task review cleanner; the SPEC's grouping is preserved in section headers.
  - §"Rollout" → Rollout section, mirrors SPEC verbatim
  - §"Open questions" → preserved in SPEC as-is; nothing in the plan needs to resolve them
- **Placeholder scan:** every step has runnable code or exact text edits. `// TODO: pick a substring or command to match` is intentional (the operator fills it in during review), not a plan-level placeholder.
- **Type consistency:** `Decision`, `DecisionFrontmatter`, `WikiError`, `DriftReport`, `DriftItem`, `Bucket`, `MatchedRule`, `DriftError`, `GetWikiDriftParams` are defined once and reused consistently. `Bucket` variants (`Covered` / `LikelyCovered` / `Uncovered` / `Superseded`) appear in scoring (2.1), rendering (2.2), suggest_rule (2.2), and the MCP tool's uncovered count (3.3).
- **Bonus fix opportunity (not folded in):** the audit-only `audit-newtype-id-string` rule would flag `MatchedRule.rule_id: phr::RuleId` as ALREADY a newtype (good), but `DecisionFrontmatter.id: String` as a `*_id` String. Not flagging it because (a) it's a parsed-from-disk YAML scalar where String is the natural type, (b) the rule is audit-only and advisory. If the post-release audit surfaces it, address there.

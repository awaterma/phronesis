# Journey Facts Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement SPEC-journey-facts v1 — durable per-call journal, project-defined taggers, rule-driven derivation of `journey_*` facts — and fold the existing per-subject outcomes ledger into the same storage layer.

**Architecture:** Stateless per-invocation hook continues; journey facts are recomputed every call from a bounded suffix of `.phronesis/journey/events.jsonl`. Taggers reuse the existing predicate engine via a throwaway ReteNetwork. Derive scans loaded rules, computes only referenced aggregates, asserts ordinary `Fact`s into the live network. Outcomes adapter stamps `subject` on the journal record and emits `outcome:*` tags; `outcomes/ledger.rs` deletes. No changes to the `phr` library crate.

**Tech Stack:** Rust 2024 edition, `phr` crate (existing ReteNetwork / Fact / Rule / Predicate), `fs2` for advisory file locks, `serde` + `serde_json`, `tempfile` + `tokio` + `tracing` in tests, MSRV 1.90.

**Spec:** `docs/specs/SPEC-journey-facts.md`

---

## File structure

**Created**
- `crates/phronesis-mcp/src/journey/mod.rs` — Public surface (`Config`, errors, re-exports)
- `crates/phronesis-mcp/src/journey/journal.rs` — Record schema + append (flock) + tail-read + per-subject read
- `crates/phronesis-mcp/src/journey/tagger.rs` — Tagger config + throwaway-network firing + module resolution
- `crates/phronesis-mcp/src/journey/derive.rs` — Rule scan + selector validation + window parse + aggregator emission

**Modified**
- `crates/phronesis-mcp/src/lib.rs` — Add `pub mod journey;`
- `crates/phronesis-mcp/src/hook.rs` — Wire `journey::derive::assert_facts` (pre + post) and `journey::journal::record` (post tail)
- `crates/phronesis-mcp/src/hook_facts.rs` — Expose the common facts the tagger consumes
- `crates/phronesis-mcp/src/context.rs` — Stamp `.phronesis/journey/session` at SessionStart
- `crates/phronesis-mcp/src/init.rs` — `--packs journey` writes starter `journey.json` + gitignore entry
- `crates/phronesis-mcp/src/main.rs` — `journey` CLI subcommand
- `crates/phronesis-mcp/src/server.rs` — `get_journey` MCP tool
- `crates/phronesis-mcp/src/server_params.rs` — `GetJourneyParams`
- `crates/phronesis-mcp/src/outcomes/mod.rs` — Drop `pub mod ledger;`
- `crates/phronesis-mcp/src/outcomes/derive.rs` — Read via `journey::journal::read_recent_subject`
- `crates/phronesis-mcp/src/outcomes/cargo.rs` — Emit `outcome:*` tags + return `subject` so the hook can stamp the record
- `crates/phronesis-mcp/Cargo.toml` — Bump to `0.13.0`; bump `phr` workspace dep accordingly
- `crates/phronesis-mcp/CLAUDE.md` — Document `journey` subcommand, `get_journey` MCP tool, `--packs journey`

**Deleted**
- `crates/phronesis-mcp/src/outcomes/ledger.rs` — Storage folds into the journey journal (commit 4)

**New test files**
- `crates/phronesis-mcp/tests/journey_journal.rs` — Append/tail-read + flock concurrency
- `crates/phronesis-mcp/tests/journey_tagger.rs` — Tag emission + module resolution + `perf_smoke`
- `crates/phronesis-mcp/tests/journey_derive.rs` — Aggregator emission + window semantics + determinism contract + selector validation
- `crates/phronesis-mcp/tests/journey_hook_integration.rs` — Hook wiring + fail-open + `PHRONESIS_NO_JOURNEY` + outcomes fold-in end-to-end
- `crates/phronesis-mcp/tests/journey_cli_integration.rs` — `phr-mcp journey` CLI

---

## Task 1: Journal — append, tail-read, per-subject read

**Files:**
- Create: `crates/phronesis-mcp/src/journey/mod.rs`
- Create: `crates/phronesis-mcp/src/journey/journal.rs`
- Modify: `crates/phronesis-mcp/src/lib.rs`
- Create: `crates/phronesis-mcp/tests/journey_journal.rs`

- [ ] **Step 1.1: Add the module hook**

Modify `crates/phronesis-mcp/src/lib.rs` to add `pub mod journey;` in alphabetical order alongside `pub mod outcomes;`.

```rust
pub mod journey;
```

- [ ] **Step 1.2: Create the journey module skeleton**

Create `crates/phronesis-mcp/src/journey/mod.rs`:

```rust
//! Journey facts — durable, recomputed-per-call temporal predicates.
//!
//! See `docs/specs/SPEC-journey-facts.md`. The stateless hook stays stateless:
//! every invocation rebuilds the network and re-derives `journey_*` facts from
//! a bounded suffix of `.phronesis/journey/events.jsonl`. State lives on disk;
//! decay is the sliding window; determinism is a pure function of
//! (journal bytes, ts, sid).

pub mod journal;
```

- [ ] **Step 1.3: Write the failing journal append/read round-trip test**

Create `crates/phronesis-mcp/tests/journey_journal.rs`:

```rust
use phronesis_mcp::journey::journal::{self, JournalRecord};

fn rec(seq: u64, ts: u64, tool: &str, path: &str, tags: &[&str], subject: Option<&str>) -> JournalRecord {
    JournalRecord {
        v: 1,
        ts,
        sid: "s-test".to_string(),
        seq,
        tool: tool.to_string(),
        path: path.to_string(),
        ext: path.rsplit('.').next().map(|s| s.to_string()),
        module: None,
        tags: tags.iter().map(|s| s.to_string()).collect(),
        subject: subject.map(|s| s.to_string()),
    }
}

#[test]
fn append_and_read_recent_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    journal::append(dir.path(), &rec(1, 1000, "Edit", "src/auth/a.rs", &["auth"], None)).unwrap();
    journal::append(dir.path(), &rec(2, 1010, "Edit", "tests/a.rs", &["tests"], None)).unwrap();
    journal::append(dir.path(), &rec(3, 1020, "Bash", "<cmd>", &["build"], Some("u1"))).unwrap();

    let recs = journal::read_recent(dir.path(), 10).unwrap();
    assert_eq!(recs.len(), 3);
    assert_eq!(recs[0].seq, 1);
    assert_eq!(recs[2].subject.as_deref(), Some("u1"));
    assert_eq!(recs[2].tags, vec!["build".to_string()]);
}

#[test]
fn read_recent_bounded_returns_tail() {
    let dir = tempfile::tempdir().unwrap();
    for seq in 1..=10 {
        journal::append(
            dir.path(),
            &rec(seq, 1000 + seq, "Edit", "src/a.rs", &["auth"], None),
        )
        .unwrap();
    }
    let recs = journal::read_recent(dir.path(), 3).unwrap();
    assert_eq!(recs.len(), 3);
    assert_eq!(recs.iter().map(|r| r.seq).collect::<Vec<_>>(), vec![8, 9, 10]);
}

#[test]
fn read_recent_subject_filters() {
    let dir = tempfile::tempdir().unwrap();
    journal::append(dir.path(), &rec(1, 1000, "Edit", "src/a.rs", &["auth"], None)).unwrap();
    journal::append(dir.path(), &rec(2, 1010, "Bash", "<cmd>", &["build"], Some("u1"))).unwrap();
    journal::append(dir.path(), &rec(3, 1020, "Bash", "<cmd>", &["build"], Some("u2"))).unwrap();
    journal::append(dir.path(), &rec(4, 1030, "Bash", "<cmd>", &["test"], Some("u1"))).unwrap();

    let recs = journal::read_recent_subject(dir.path(), "u1", 10).unwrap();
    assert_eq!(recs.len(), 2);
    assert_eq!(recs[0].seq, 2);
    assert_eq!(recs[1].seq, 4);
}

#[test]
fn missing_file_reads_empty() {
    let dir = tempfile::tempdir().unwrap();
    assert!(journal::read_recent(dir.path(), 10).unwrap().is_empty());
    assert!(
        journal::read_recent_subject(dir.path(), "u1", 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn malformed_lines_are_skipped() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let journey_dir = dir.path().join(".phronesis").join("journey");
    std::fs::create_dir_all(&journey_dir).unwrap();
    let path = journey_dir.join("events.jsonl");
    let good = serde_json::to_string(&rec(1, 1000, "Edit", "src/a.rs", &["auth"], None)).unwrap();
    let mut f = std::fs::File::create(&path).unwrap();
    writeln!(f, "{}", good).unwrap();
    writeln!(f, "{{not json").unwrap();
    writeln!(f, "{}", good).unwrap();
    drop(f);
    let recs = journal::read_recent(dir.path(), 10).unwrap();
    assert_eq!(recs.len(), 2);
}
```

- [ ] **Step 1.4: Run the test to verify it fails**

Run: `cargo test --package phronesis-mcp --test journey_journal`
Expected: FAIL — module `journey::journal` does not exist.

- [ ] **Step 1.5: Implement the journal**

Create `crates/phronesis-mcp/src/journey/journal.rs`:

```rust
//! Append-only per-call journal at `.phronesis/journey/events.jsonl`.
//!
//! Same flock discipline as `action_log` / `outcomes::ledger`: exclusive
//! advisory lock around each write, auto-released on fd close. One JSON Lines
//! record per *executed* tool call (post-check only — blocked calls never
//! reach here, so the journal reflects what the agent actually did).
//!
//! Reads tail-bias: `read_recent(n)` returns the last `n` records in append
//! order; `read_recent_subject(s, n)` filters those by subject. v1 reads the
//! whole file with a hard line cap; reverse-read is a future optimization.

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JournalRecord {
    pub v: u32,
    pub ts: u64,
    pub sid: String,
    pub seq: u64,
    pub tool: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ext: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
}

#[derive(Debug, Error)]
pub enum JournalError {
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Hard cap on lines read by `read_recent*` regardless of caller-requested `n`.
/// Bounds the pathological case where retention misbehaves. See SPEC §"Cost".
pub const SUFFIX_HARD_CAP: usize = 10_000;

fn dir(root: &Path) -> PathBuf {
    root.join(".phronesis").join("journey")
}

fn events_path(root: &Path) -> PathBuf {
    dir(root).join("events.jsonl")
}

pub fn append(root: &Path, record: &JournalRecord) -> Result<(), JournalError> {
    let d = dir(root);
    std::fs::create_dir_all(&d).map_err(|e| JournalError::Io {
        path: d.display().to_string(),
        source: e,
    })?;
    let path = events_path(root);
    let line = format!("{}\n", serde_json::to_string(record)?);

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| JournalError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
    file.lock_exclusive().map_err(|e| JournalError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    let result = (&file)
        .write_all(line.as_bytes())
        .map_err(|e| JournalError::Io {
            path: path.display().to_string(),
            source: e,
        });
    let _ = FileExt::unlock(&file);
    result
}

pub fn read_recent(root: &Path, n: usize) -> Result<Vec<JournalRecord>, JournalError> {
    let limit = n.min(SUFFIX_HARD_CAP);
    let lines = match std::fs::read_to_string(events_path(root)) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(JournalError::Io {
                path: events_path(root).display().to_string(),
                source: e,
            });
        }
    };
    let all: Vec<JournalRecord> = lines
        .lines()
        .filter_map(|l| serde_json::from_str::<JournalRecord>(l).ok())
        .collect();
    let start = all.len().saturating_sub(limit);
    Ok(all[start..].to_vec())
}

pub fn read_recent_subject(
    root: &Path,
    subject: &str,
    n: usize,
) -> Result<Vec<JournalRecord>, JournalError> {
    let all = read_recent(root, SUFFIX_HARD_CAP)?;
    let filtered: Vec<JournalRecord> = all
        .into_iter()
        .filter(|r| r.subject.as_deref() == Some(subject))
        .collect();
    let limit = n.min(filtered.len());
    let start = filtered.len() - limit;
    Ok(filtered[start..].to_vec())
}
```

- [ ] **Step 1.6: Run tests to verify they pass**

Run: `cargo test --package phronesis-mcp --test journey_journal`
Expected: PASS — 5 tests.

- [ ] **Step 1.7: Add flock concurrency test**

Append to `crates/phronesis-mcp/tests/journey_journal.rs`:

```rust
#[test]
fn concurrent_appends_serialize() {
    use std::sync::Arc;
    use std::thread;

    let dir = Arc::new(tempfile::tempdir().unwrap());
    let mut handles = Vec::new();
    for t in 0..8u64 {
        let dir = Arc::clone(&dir);
        handles.push(thread::spawn(move || {
            for i in 0..50u64 {
                let seq = t * 100 + i;
                journal::append(
                    dir.path(),
                    &rec(seq, 1000 + seq, "Edit", "src/a.rs", &["auth"], None),
                )
                .unwrap();
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    let recs = journal::read_recent(dir.path(), 10_000).unwrap();
    assert_eq!(recs.len(), 400, "all appends preserved");
    // Each line parsed as a record — no interleaved partials.
}
```

- [ ] **Step 1.8: Run the concurrency test**

Run: `cargo test --package phronesis-mcp --test journey_journal -- --test-threads=1 concurrent`
Expected: PASS.

- [ ] **Step 1.9: Commit**

```bash
git add crates/phronesis-mcp/src/journey/mod.rs \
        crates/phronesis-mcp/src/journey/journal.rs \
        crates/phronesis-mcp/src/lib.rs \
        crates/phronesis-mcp/tests/journey_journal.rs
git commit -m "$(cat <<'EOF'
feat(journey): journal with subject — append/tail-read/per-subject read

Append-only per-call journal at .phronesis/journey/events.jsonl. Same flock
discipline as action_log/outcomes/ledger. Record schema carries optional
subject (the outcomes fold-in seam, lands in commit 4) and a tags vec. Reads:
read_recent(n) for the call-window family, read_recent_subject(s, n) for the
per-work-unit reads outcomes::derive will use. Hard suffix cap (10k) bounds
pathological retention. No hook wiring yet — driven by unit tests.

Refs: docs/specs/SPEC-journey-facts.md
EOF
)"
```

---

## Task 2: Tagger — config, throwaway-network firing, module resolution, perf budget

**Files:**
- Create: `crates/phronesis-mcp/src/journey/tagger.rs`
- Modify: `crates/phronesis-mcp/src/journey/mod.rs`
- Modify: `crates/phronesis-mcp/src/hook_facts.rs`
- Create: `crates/phronesis-mcp/tests/journey_tagger.rs`

- [ ] **Step 2.1: Expose hook_facts builder for reuse**

Read `crates/phronesis-mcp/src/hook_facts.rs` and confirm there is a shared "build common facts for a tool call" path. If the existing entry point is private, add a `pub fn build_common_facts(...) -> Vec<Fact>` that returns the same fact vec the hook already asserts (tool, file path, ext, new/old content, diff facts, etc.). This is the surface the tagger reuses — no new fact extraction code.

If `build_common_facts` already exists, no change. Verify with:

Run: `grep -n "pub fn build_common_facts\|pub fn common_facts\|pub fn collect_facts" crates/phronesis-mcp/src/hook_facts.rs`

If absent, refactor: extract the post-check fact-building block from `hook.rs::run_post_check` (and `run_pre_check`) into `hook_facts::build_common_facts(tool_name, file_path, old, new) -> Vec<Fact>` and have both phases call it. Tests in `crates/phronesis-mcp/tests/hook_integration.rs` continue to pass.

- [ ] **Step 2.2: Write the failing tagger test**

Create `crates/phronesis-mcp/tests/journey_tagger.rs`:

```rust
use phr::Fact;
use phronesis_mcp::journey::tagger::{self, TaggerConfig};

fn cfg(json: &str) -> TaggerConfig {
    serde_json::from_str(json).expect("valid tagger config")
}

fn fact(pred: &str, args: &[&str]) -> Fact {
    Fact::new(
        pred.to_string(),
        args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
    )
}

#[test]
fn tagger_attaches_tag_on_path_match() {
    let c = cfg(
        r#"{
        "version": 1,
        "taggers": [
            { "tag": "auth", "when": [ { "file_path_matches": "src/auth/" } ] }
        ],
        "modules": []
    }"#,
    );
    let facts = vec![fact("file_path", &["src/auth/login.rs"])];
    let result = tagger::fire(&c, &facts).unwrap();
    assert_eq!(result.tags, vec!["auth".to_string()]);
    assert_eq!(result.module, None);
}

#[test]
fn tagger_attaches_multiple_tags() {
    let c = cfg(
        r#"{
        "version": 1,
        "taggers": [
            { "tag": "auth",  "when": [ { "file_path_matches": "src/auth/" } ] },
            { "tag": "rust",  "when": [ { "file_extension_is": "rs" } ] }
        ],
        "modules": []
    }"#,
    );
    let facts = vec![
        fact("file_path", &["src/auth/login.rs"]),
        fact("file_extension", &["rs"]),
    ];
    let result = tagger::fire(&c, &facts).unwrap();
    let mut got = result.tags.clone();
    got.sort();
    assert_eq!(got, vec!["auth".to_string(), "rust".to_string()]);
}

#[test]
fn tagger_no_match_no_tag() {
    let c = cfg(
        r#"{
        "version": 1,
        "taggers": [
            { "tag": "auth", "when": [ { "file_path_matches": "src/auth/" } ] }
        ],
        "modules": []
    }"#,
    );
    let facts = vec![fact("file_path", &["src/payments/charge.rs"])];
    let result = tagger::fire(&c, &facts).unwrap();
    assert!(result.tags.is_empty());
}

#[test]
fn module_resolves_from_globs() {
    let c = cfg(
        r#"{
        "version": 1,
        "taggers": [],
        "modules": [
            { "name": "auth", "paths": ["src/auth/**"] },
            { "name": "payments", "paths": ["src/payments/**", "crates/pay/**"] }
        ]
    }"#,
    );
    assert_eq!(tagger::resolve_module(&c, "src/auth/login.rs"), Some("auth".to_string()));
    assert_eq!(tagger::resolve_module(&c, "crates/pay/lib.rs"), Some("payments".to_string()));
    assert_eq!(tagger::resolve_module(&c, "src/util/x.rs"), None);
}

#[test]
fn or_dnf_expansion_fires_for_either_branch() {
    let c = cfg(
        r#"{
        "version": 1,
        "taggers": [
            { "tag": "sql", "when": [
                { "or": [
                    { "new_content_contains": "INSERT INTO" },
                    { "new_content_contains": "DELETE FROM" }
                ] }
            ] }
        ],
        "modules": []
    }"#,
    );
    let facts_a = vec![fact("new_content_contains", &["INSERT INTO"])];
    let facts_b = vec![fact("new_content_contains", &["DELETE FROM"])];
    let facts_c = vec![fact("new_content_contains", &["SELECT *"])];
    assert_eq!(tagger::fire(&c, &facts_a).unwrap().tags, vec!["sql".to_string()]);
    assert_eq!(tagger::fire(&c, &facts_b).unwrap().tags, vec!["sql".to_string()]);
    assert!(tagger::fire(&c, &facts_c).unwrap().tags.is_empty());
}
```

- [ ] **Step 2.3: Run to verify it fails**

Run: `cargo test --package phronesis-mcp --test journey_tagger`
Expected: FAIL — module `journey::tagger` does not exist.

- [ ] **Step 2.4: Implement the tagger**

Append to `crates/phronesis-mcp/src/journey/mod.rs`:

```rust
pub mod tagger;
```

Create `crates/phronesis-mcp/src/journey/tagger.rs`. The implementation reuses the rules-file `SourceRule`/`WhenClause` path: load taggers as a `Vec<SourceRule>` whose `then` is a sentinel `tag` action and whose `id` is the tag name, expand `or` via `unfold_or`, build a throwaway `ReteNetwork`, assert the supplied facts, fire, collect the tags that fired.

```rust
//! Taggers — project-defined rules that attach domain tags to journal records.
//!
//! See SPEC §"The project-defined seam". A tagger is structurally a rule
//! whose `when` is evaluated against the same point-in-time facts a normal
//! hook rule sees, and whose effect is "attach tag T" instead of "block/warn."
//! Zero new matching code: build a throwaway ReteNetwork, load the taggers as
//! rules whose action is a sentinel string the caller filters for, fire,
//! collect fired tags.

use std::collections::HashSet;

use phr::{Fact, ReteNetwork, Rule};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::rules_file::{self, SourceRule};

const TAG_ACTION: &str = "tag";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaggerConfig {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub taggers: Vec<TaggerEntry>,
    #[serde(default)]
    pub modules: Vec<ModuleEntry>,
}

fn default_version() -> u32 { 1 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaggerEntry {
    pub tag: String,
    pub when: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleEntry {
    pub name: String,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct TagResult {
    pub tags: Vec<String>,
    pub module: Option<String>,
}

#[derive(Debug, Error)]
pub enum TaggerError {
    #[error("malformed tagger config: {0}")]
    Config(String),
    #[error("engine error: {0}")]
    Engine(String),
}

/// Fire every tagger against `facts`, return the set of tags whose `when` matched.
pub fn fire(cfg: &TaggerConfig, facts: &[Fact]) -> Result<TagResult, TaggerError> {
    // Build a synthetic SourceRule per tagger, expand `or`, materialize as
    // engine Rules with a sentinel `tag` action whose message is the tag name.
    let mut rules: Vec<Rule> = Vec::new();
    for entry in &cfg.taggers {
        let src = SourceRule::synthetic_tagger(&entry.tag, &entry.when)
            .map_err(|e| TaggerError::Config(e.to_string()))?;
        let disk_rules = rules_file::unfold_or(&src)
            .map_err(|e| TaggerError::Config(e.to_string()))?;
        for dr in disk_rules {
            let (rule, _phase) = rules_file::rule_from_disk(&dr);
            rules.push(rule);
        }
    }

    let mut network = ReteNetwork::new();
    for r in rules {
        network.add_rule(r);
    }
    for f in facts {
        network.assert_fact(f.clone());
    }
    network.update_agenda();
    let consequences = network.fire_all_consequences();

    let mut fired: HashSet<String> = HashSet::new();
    for c in consequences {
        if c.action_type() == TAG_ACTION {
            fired.insert(c.message().to_string());
        }
    }
    let mut tags: Vec<String> = fired.into_iter().collect();
    tags.sort();
    Ok(TagResult { tags, module: None })
}

/// Resolve a file path to a configured module name. First-match-wins by glob.
pub fn resolve_module(cfg: &TaggerConfig, path: &str) -> Option<String> {
    for m in &cfg.modules {
        for g in &m.paths {
            if glob_match(g, path) {
                return Some(m.name.clone());
            }
        }
    }
    None
}

/// Minimal glob: `**` matches any chars including `/`; `*` matches non-`/`;
/// everything else literal. Sufficient for the `src/auth/**` / `crates/pay/**`
/// shapes the spec calls out; anything richer is a future predicate.
fn glob_match(pattern: &str, path: &str) -> bool {
    let mut p = 0usize;
    let mut s = 0usize;
    let pb = pattern.as_bytes();
    let sb = path.as_bytes();
    while p < pb.len() && s < sb.len() {
        if pb[p] == b'*' {
            if p + 1 < pb.len() && pb[p + 1] == b'*' {
                // `**` — match the rest of the path
                if p + 2 >= pb.len() {
                    return true;
                }
                let rest = &pb[p + 2..];
                for i in s..=sb.len() {
                    if glob_match(std::str::from_utf8(rest).unwrap(), &path[i..]) {
                        return true;
                    }
                }
                return false;
            }
            // `*` — match any non-`/` run
            let rest = &pb[p + 1..];
            for i in s..=sb.len() {
                if i > s && sb[i - 1] == b'/' {
                    break;
                }
                if glob_match(std::str::from_utf8(rest).unwrap(), &path[i..]) {
                    return true;
                }
            }
            return false;
        }
        if pb[p] != sb[s] {
            return false;
        }
        p += 1;
        s += 1;
    }
    p == pb.len() && s == sb.len()
}
```

- [ ] **Step 2.5: Add the `SourceRule::synthetic_tagger` helper**

Modify `crates/phronesis-mcp/src/rules_file.rs` to add a constructor that builds a `SourceRule` from a tag name + raw `when` JSON. Place it as an `impl SourceRule` method:

```rust
impl SourceRule {
    /// Build a SourceRule whose `then` action is the sentinel `tag` verb and
    /// whose message is the tag itself. Used by the journey tagger to ride the
    /// existing rule-firing path without new matching code.
    pub fn synthetic_tagger(
        tag: &str,
        when: &[serde_json::Value],
    ) -> Result<Self, RulesFileError> {
        let id = format!("tagger:{}", tag);
        let when_clauses: Vec<WhenClause> = when
            .iter()
            .map(|v| serde_json::from_value::<WhenClause>(v.clone()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| RulesFileError::Parse(e.to_string()))?;
        Ok(SourceRule {
            id,
            phase: "tag".to_string(),
            priority: 0,
            audit: false,
            when: when_clauses,
            then: DiskAction {
                action_type: "tag".to_string(),
                message: tag.to_string(),
            },
        })
    }
}
```

(Adjust field names to match the live `SourceRule` shape if different — read the file first and conform.)

- [ ] **Step 2.6: Run tagger tests**

Run: `cargo test --package phronesis-mcp --test journey_tagger`
Expected: PASS — 5 tests.

- [ ] **Step 2.7: Write the perf_smoke test**

Append to `crates/phronesis-mcp/tests/journey_tagger.rs`:

```rust
#[test]
fn perf_smoke_20_taggers_100_facts() {
    let mut json = String::from(r#"{"version":1,"taggers":["#);
    for i in 0..20 {
        if i > 0 { json.push(','); }
        json.push_str(&format!(
            r#"{{"tag":"t{}","when":[{{"file_path_matches":"src/m{}/"}}]}}"#,
            i, i
        ));
    }
    json.push_str(r#"],"modules":[]}"#);
    let c = cfg(&json);

    let mut facts = Vec::new();
    for i in 0..100 {
        facts.push(fact("file_path", &[&format!("src/m{}/file.rs", i % 20)]));
    }

    // Warm
    let _ = tagger::fire(&c, &facts).unwrap();

    let mut samples = Vec::new();
    for _ in 0..50 {
        let t = std::time::Instant::now();
        let _ = tagger::fire(&c, &facts).unwrap();
        samples.push(t.elapsed());
    }
    samples.sort();
    let p95 = samples[(samples.len() * 95) / 100];
    assert!(
        p95 <= std::time::Duration::from_millis(2),
        "tagger p95 {:?} exceeds 2ms budget (samples sorted: first {:?}, last {:?})",
        p95, samples.first(), samples.last()
    );
}
```

- [ ] **Step 2.8: Run the perf test**

Run: `cargo test --release --package phronesis-mcp --test journey_tagger perf_smoke`
Expected: PASS.

If the test fails, do **not** widen the budget. The budget is the spec's contract. Investigate: throwaway-network construction is probably the dominant cost. The acceptable fixes are (a) reuse a network across taggers in a single call, (b) cache compiled rules per `journey.json` mtime, (c) special-case the predicate set to skip RETE for trivial path matches. Pick the smallest fix that passes; the goal is the budget, not the architecture.

- [ ] **Step 2.9: Commit**

```bash
git add crates/phronesis-mcp/src/journey/mod.rs \
        crates/phronesis-mcp/src/journey/tagger.rs \
        crates/phronesis-mcp/src/rules_file.rs \
        crates/phronesis-mcp/src/hook_facts.rs \
        crates/phronesis-mcp/tests/journey_tagger.rs
git commit -m "$(cat <<'EOF'
feat(journey): taggers reuse the predicate engine

Project-defined taggers in journey.json compile into synthetic SourceRules
whose then-action is the sentinel `tag` verb. Firing reuses the existing
ReteNetwork — DNF or-expansion, regex/path/bash predicates, no new matching
code. resolve_module() does first-match-wins glob lookup for the `module`
field on records.

perf_smoke gate: ≤2ms p95 for 20 taggers × 100 facts on a 2024-class laptop
(SPEC contract). Fail CI on regression; do not relax the budget — fix the
implementation.

Refs: docs/specs/SPEC-journey-facts.md §Tagger
EOF
)"
```

---

## Task 3: Derive — rule scan, selector validation, window parse, aggregator emission

**Files:**
- Create: `crates/phronesis-mcp/src/journey/derive.rs`
- Modify: `crates/phronesis-mcp/src/journey/mod.rs`
- Create: `crates/phronesis-mcp/tests/journey_derive.rs`

- [ ] **Step 3.1: Wire the module**

Append to `crates/phronesis-mcp/src/journey/mod.rs`:

```rust
pub mod derive;
```

- [ ] **Step 3.2: Write failing window-parse tests**

Create `crates/phronesis-mcp/tests/journey_derive.rs`:

```rust
use phronesis_mcp::journey::derive::{self, Window};

#[test]
fn window_parses_calls() {
    assert_eq!(Window::parse("5c").unwrap(), Window::Calls(5));
    assert_eq!(Window::parse("100c").unwrap(), Window::Calls(100));
}

#[test]
fn window_parses_time() {
    assert_eq!(Window::parse("30m").unwrap(), Window::Seconds(30 * 60));
    assert_eq!(Window::parse("2h").unwrap(), Window::Seconds(2 * 3600));
    assert_eq!(Window::parse("7d").unwrap(), Window::Seconds(7 * 86_400));
}

#[test]
fn window_parses_session() {
    assert_eq!(Window::parse("s").unwrap(), Window::Session);
}

#[test]
fn window_repo_is_phase_2() {
    assert!(Window::parse("r").is_err());
}

#[test]
fn window_rejects_malformed() {
    assert!(Window::parse("").is_err());
    assert!(Window::parse("5").is_err());
    assert!(Window::parse("5C").is_err());
    assert!(Window::parse("abc").is_err());
}
```

- [ ] **Step 3.3: Run to verify fail**

Run: `cargo test --package phronesis-mcp --test journey_derive`
Expected: FAIL — `journey::derive` does not exist.

- [ ] **Step 3.4: Implement window parsing**

Create `crates/phronesis-mcp/src/journey/derive.rs`:

```rust
//! Derivation of `journey_*` facts from the journal.
//!
//! Per-invocation pass: scan loaded rules for `journey_*` conditions, validate
//! every referenced tag/module selector against the loaded TaggerConfig
//! (silent-typo guard), read a bounded suffix of the journal sized by the
//! widest window any rule references, bucket and emit aggregator facts.
//! No state survives the call.

use std::collections::{BTreeSet, HashSet};

use phr::{Fact, ReteNetwork, Rule};
use thiserror::Error;

use crate::journey::journal::{self, JournalRecord};
use crate::journey::tagger::TaggerConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    Calls(u32),
    Seconds(u64),
    Session,
}

#[derive(Debug, Error)]
pub enum DeriveError {
    #[error("malformed window token `{0}`")]
    BadWindow(String),
    #[error("rule `{rule}` references undefined selector `{selector}` — not in journey.json taggers or modules")]
    UndefinedSelector { rule: String, selector: String },
    #[error("journal read failed: {0}")]
    Journal(#[from] journal::JournalError),
}

impl Window {
    pub fn parse(token: &str) -> Result<Self, DeriveError> {
        if token == "s" {
            return Ok(Window::Session);
        }
        if token == "r" {
            return Err(DeriveError::BadWindow(format!(
                "{} (r is phase 2 — not in v1)", token
            )));
        }
        if token.is_empty() {
            return Err(DeriveError::BadWindow(token.to_string()));
        }
        let (num, unit) = token.split_at(token.len() - 1);
        let n: u64 = num.parse().map_err(|_| DeriveError::BadWindow(token.to_string()))?;
        match unit {
            "c" => Ok(Window::Calls(n as u32)),
            "m" => Ok(Window::Seconds(n * 60)),
            "h" => Ok(Window::Seconds(n * 3600)),
            "d" => Ok(Window::Seconds(n * 86_400)),
            _ => Err(DeriveError::BadWindow(token.to_string())),
        }
    }
}
```

- [ ] **Step 3.5: Run window tests**

Run: `cargo test --package phronesis-mcp --test journey_derive`
Expected: PASS — 5 tests.

- [ ] **Step 3.6: Write failing rule-scan + aggregator tests**

Append to `crates/phronesis-mcp/tests/journey_derive.rs`:

```rust
use phr::{Condition, Fact, ReteNetwork, Rule};
use phronesis_mcp::journey::derive::{assert_facts, RuleScan};
use phronesis_mcp::journey::journal::{self, JournalRecord};
use phronesis_mcp::journey::tagger::TaggerConfig;

fn rec(seq: u64, ts: u64, sid: &str, tags: &[&str], subject: Option<&str>) -> JournalRecord {
    JournalRecord {
        v: 1, ts, sid: sid.to_string(), seq,
        tool: "Edit".to_string(),
        path: "src/a.rs".to_string(),
        ext: Some("rs".to_string()),
        module: None,
        tags: tags.iter().map(|s| s.to_string()).collect(),
        subject: subject.map(|s| s.to_string()),
    }
}

fn cfg(json: &str) -> TaggerConfig {
    serde_json::from_str(json).unwrap()
}

fn script(s: &str) -> Condition {
    // Whatever your project uses for `__script__` Condition construction.
    Condition::script(s.to_string())
}

fn rule_with_script(id: &str, conds: Vec<&str>) -> Rule {
    Rule::new(
        id.to_string(),
        conds.into_iter().map(|c| script(c)).collect(),
        // action irrelevant for derivation tests — anything compiles
        Default::default(),
    )
}

#[test]
fn journey_occurrence_count_in_session() {
    let dir = tempfile::tempdir().unwrap();
    for s in 1..=4u64 {
        journal::append(dir.path(), &rec(s, 1000 + s, "s-now", &["auth"], None)).unwrap();
    }
    journal::append(dir.path(), &rec(5, 1100, "s-OLD", &["auth"], None)).unwrap();

    let c = cfg(r#"{
        "version":1,
        "taggers":[{"tag":"auth","when":[{"file_path_matches":"src/auth/"}]}],
        "modules":[]
    }"#);
    let rules = vec![rule_with_script(
        "auth-churn",
        vec!["facts_count('journey_occurrence', ['auth','s']) >= 3"],
    )];

    let mut net = ReteNetwork::new();
    assert_facts(&mut net, dir.path(), &rules, &c, "s-now", 2_000).unwrap();
    let facts: Vec<Fact> = net.facts().to_vec();
    let occurrences: Vec<&Fact> = facts.iter()
        .filter(|f| f.predicate == "journey_occurrence"
            && f.args.get(0).map(|s| s.as_str()) == Some("auth")
            && f.args.get(1).map(|s| s.as_str()) == Some("s"))
        .collect();
    assert_eq!(occurrences.len(), 4, "one journey_occurrence per matching record in session");
}

#[test]
fn journey_count_emits_single_bindable() {
    let dir = tempfile::tempdir().unwrap();
    for s in 1..=4u64 {
        journal::append(dir.path(), &rec(s, 1000 + s, "s-now", &["auth"], None)).unwrap();
    }
    let c = cfg(r#"{
        "version":1,
        "taggers":[{"tag":"auth","when":[{"file_path_matches":"src/auth/"}]}],
        "modules":[]
    }"#);
    let rules = vec![rule_with_script(
        "report-count",
        vec!["facts_contain('journey_count', ['auth','s','?n'])"],
    )];
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, dir.path(), &rules, &c, "s-now", 2_000).unwrap();
    let counts: Vec<&Fact> = net.facts().iter()
        .filter(|f| f.predicate == "journey_count").collect();
    assert_eq!(counts.len(), 1);
    assert_eq!(counts[0].args, vec!["auth".to_string(), "s".to_string(), "4".to_string()]);
}

#[test]
fn journey_seen_emits_boolean_on_presence() {
    let dir = tempfile::tempdir().unwrap();
    journal::append(dir.path(), &rec(1, 1000, "s-now", &["sql"], None)).unwrap();
    let c = cfg(r#"{
        "version":1,
        "taggers":[{"tag":"sql","when":[{"new_content_contains":"INSERT INTO"}]}],
        "modules":[]
    }"#);
    let rules = vec![rule_with_script(
        "sql-recent",
        vec!["facts_contain('journey_seen', ['sql','5c'])"],
    )];
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, dir.path(), &rules, &c, "s-now", 2_000).unwrap();
    let seen: Vec<&Fact> = net.facts().iter()
        .filter(|f| f.predicate == "journey_seen").collect();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].args, vec!["sql".to_string(), "5c".to_string()]);
}

#[test]
fn journey_since_ge_ladders_to_max_k() {
    let dir = tempfile::tempdir().unwrap();
    // last build was 8 calls ago, then 8 non-build edits
    journal::append(dir.path(), &rec(1, 1000, "s-now", &["build"], None)).unwrap();
    for s in 2..=9u64 {
        journal::append(dir.path(), &rec(s, 1000 + s, "s-now", &["auth"], None)).unwrap();
    }
    let c = cfg(r#"{
        "version":1,
        "taggers":[
            {"tag":"build","when":[{"bash_command_matches":"cargo (build|test)"}]},
            {"tag":"auth","when":[{"file_path_matches":"src/auth/"}]}
        ],
        "modules":[]
    }"#);
    let rules = vec![rule_with_script(
        "build-stale",
        vec!["facts_count('journey_since_ge', ['build','8']) >= 1"],
    )];
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, dir.path(), &rules, &c, "s-now", 2_000).unwrap();
    let since: Vec<&Fact> = net.facts().iter()
        .filter(|f| f.predicate == "journey_since_ge" && f.args.get(0) == Some(&"build".to_string()))
        .collect();
    // distance-since-last-build = 8; should ladder k=1..8
    assert_eq!(since.len(), 8);
    let mut ks: Vec<&str> = since.iter().map(|f| f.args[1].as_str()).collect();
    ks.sort();
    assert_eq!(ks, vec!["1","2","3","4","5","6","7","8"]);
}

#[test]
fn absence_via_zero_count_fires() {
    let dir = tempfile::tempdir().unwrap();
    journal::append(dir.path(), &rec(1, 1000, "s-now", &["auth"], None)).unwrap();
    journal::append(dir.path(), &rec(2, 1010, "s-now", &["auth"], None)).unwrap();
    journal::append(dir.path(), &rec(3, 1020, "s-now", &["auth"], None)).unwrap();
    // no "tests" tag anywhere

    let c = cfg(r#"{
        "version":1,
        "taggers":[
            {"tag":"auth","when":[{"file_path_matches":"src/auth/"}]},
            {"tag":"tests","when":[{"file_path_matches":"tests/"}]}
        ],
        "modules":[]
    }"#);
    let rules = vec![rule_with_script(
        "auth-without-tests",
        vec![
            "facts_count('journey_occurrence', ['auth','s']) >= 3",
            "facts_count('journey_occurrence', ['tests','s']) == 0",
        ],
    )];
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, dir.path(), &rules, &c, "s-now", 2_000).unwrap();

    net.update_agenda();
    let consequences = net.fire_all_consequences();
    assert_eq!(consequences.len(), 1, "auth-without-tests fires on absence");
}

#[test]
fn undefined_selector_rejected_at_load() {
    let c = cfg(r#"{"version":1,"taggers":[{"tag":"auth","when":[]}],"modules":[]}"#);
    let rules = vec![rule_with_script(
        "typo",
        vec!["facts_count('journey_occurrence', ['testz','s']) == 0"],
    )];

    let dir = tempfile::tempdir().unwrap();
    let mut net = ReteNetwork::new();
    let err = assert_facts(&mut net, dir.path(), &rules, &c, "s-now", 2_000)
        .unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("typo") && msg.contains("testz"), "{}", msg);
}

#[test]
fn determinism_contract() {
    let dir = tempfile::tempdir().unwrap();
    for s in 1..=5u64 {
        journal::append(dir.path(), &rec(s, 1000 + s, "s-now", &["auth"], None)).unwrap();
    }
    let c = cfg(r#"{
        "version":1,
        "taggers":[{"tag":"auth","when":[]}],
        "modules":[]
    }"#);
    let rules = vec![rule_with_script(
        "auth-churn",
        vec!["facts_count('journey_occurrence', ['auth','s']) >= 3"],
    )];
    let mut a = ReteNetwork::new();
    let mut b = ReteNetwork::new();
    assert_facts(&mut a, dir.path(), &rules, &c, "s-now", 2_000).unwrap();
    assert_facts(&mut b, dir.path(), &rules, &c, "s-now", 2_000).unwrap();

    let serialize = |n: &ReteNetwork| -> String {
        let mut facts: Vec<String> = n.facts().iter()
            .filter(|f| f.predicate.starts_with("journey_"))
            .map(|f| format!("{}({})", f.predicate, f.args.join(",")))
            .collect();
        facts.sort();
        facts.join("\n")
    };
    assert_eq!(serialize(&a), serialize(&b));
}
```

- [ ] **Step 3.7: Implement derive**

Continue `crates/phronesis-mcp/src/journey/derive.rs` with the rule scan, selector validation, suffix read, and aggregator emission. The exact `Fact`/`Rule`/`Condition` constructor surface here is the one your `phr` crate exposes — match what the existing hook does in `hook.rs::run_pre_check`. Match `__script__` strings by inspecting the rule's conditions for the supported DSL forms.

Key structure (sketch — fill in `phr` API specifics from `hook.rs`):

```rust
#[derive(Debug, Default)]
pub struct RuleScan {
    /// Selector ↔ window pairs the rules reference. Each pair drives one
    /// aggregator emission. Selectors are tag names or `module:<name>`.
    pub occurrence_pairs: BTreeSet<(String, String)>,  // (selector, window_token)
    pub count_pairs: BTreeSet<(String, String)>,
    pub seen_pairs: BTreeSet<(String, String)>,
    pub since_max_k: std::collections::BTreeMap<String, u32>,
    pub distinct_pairs: BTreeSet<(String, String, u32)>,
}

pub fn scan_rules(rules: &[Rule]) -> Result<RuleScan, DeriveError> {
    // Walk every condition; pick out `__script__` strings matching
    //   facts_count('journey_<kind>', ['<selector>','<window>'(,'<k>')]) <op> N
    //   facts_contain('journey_<kind>', ['<selector>','<window>'(,'?n')])
    // and the bare equality forms { "journey_seen": ["sql","5c"] }.
    // Collect into the RuleScan buckets. Return DeriveError::BadWindow on
    // malformed windows immediately.
    todo!()
}

pub fn validate_selectors(
    rules: &[Rule],
    scan: &RuleScan,
    cfg: &TaggerConfig,
) -> Result<(), DeriveError> {
    let defined_tags: HashSet<&str> = cfg.taggers.iter().map(|t| t.tag.as_str()).collect();
    let defined_modules: HashSet<String> = cfg.modules.iter().map(|m| format!("module:{}", m.name)).collect();

    let referenced: BTreeSet<&str> = scan.occurrence_pairs.iter()
        .chain(scan.count_pairs.iter())
        .chain(scan.seen_pairs.iter())
        .map(|(s, _)| s.as_str())
        .chain(scan.since_max_k.keys().map(|s| s.as_str()))
        .collect();

    for selector in referenced {
        let ok = defined_tags.contains(selector)
              || defined_modules.contains(selector)
              // `module:<name>` selectors check the prefixed form
              || (selector.starts_with("module:") && defined_modules.contains(selector));
        if !ok {
            // find the offending rule for the error message
            let rule_id = rules.iter()
                .find(|r| condition_refs(r, selector))
                .map(|r| r.id().to_string())
                .unwrap_or_else(|| "<unknown>".to_string());
            return Err(DeriveError::UndefinedSelector {
                rule: rule_id,
                selector: selector.to_string(),
            });
        }
    }
    Ok(())
}

fn condition_refs(rule: &Rule, selector: &str) -> bool {
    // textually scan __script__ / Condition args for the selector string
    todo!()
}

/// The entry point both pre- and post-check call.
pub fn assert_facts(
    network: &mut ReteNetwork,
    project_root: &std::path::Path,
    rules: &[Rule],
    cfg: &TaggerConfig,
    current_sid: &str,
    now_ts: u64,
) -> Result<(), DeriveError> {
    let scan = scan_rules(rules)?;
    validate_selectors(rules, &scan, cfg)?;

    // Compute read bound: largest of max_call_window, max_time_window
    // converted to records, plus session floor if any rule references `s`.
    let max_calls = scan.max_call_window();
    let max_seconds = scan.max_time_window_seconds();
    let needs_session = scan.references_session();

    let mut records = journal::read_recent(project_root, suffix_bound(max_calls, max_seconds))?;
    // session-floor scan: keep records with sid == current_sid, plus enough
    // older to satisfy max_calls/max_seconds.
    let records = filter_to_windows(records, current_sid, now_ts, max_seconds, needs_session);

    // Emit aggregators
    emit_occurrence(network, &records, &scan, current_sid, now_ts);
    emit_count(network, &records, &scan, current_sid, now_ts);
    emit_seen(network, &records, &scan, current_sid, now_ts);
    emit_since_ge(network, &records, &scan);
    emit_distinct(network, &records, &scan, current_sid, now_ts);
    Ok(())
}

// Each emit_* iterates the scan buckets, filters `records` to the
// selector+window slice, asserts the relevant Fact form. See spec §
// "The fixed v1 aggregator family" for exact shapes.
```

The TODO methods (`scan_rules`, `condition_refs`, `filter_to_windows`, `emit_*`) are the bulk of this commit. They each have natural unit boundaries — extract them into private helpers and unit-test individually if it helps. The integration tests in step 3.6 are the contract that matters; the helper unit tests are TDD scaffolding you can keep or drop.

- [ ] **Step 3.8: Run derive tests**

Run: `cargo test --package phronesis-mcp --test journey_derive`
Expected: PASS — all tests including the absence rule, selector validation, and determinism contract.

If a test fails, the failure message should point at one aggregator. Fix that aggregator, re-run. Do not silence tests.

- [ ] **Step 3.9: Commit**

```bash
git add crates/phronesis-mcp/src/journey/mod.rs \
        crates/phronesis-mcp/src/journey/derive.rs \
        crates/phronesis-mcp/tests/journey_derive.rs
git commit -m "$(cat <<'EOF'
feat(journey): rule-driven derivation + selector validation

derive::assert_facts() scans loaded rules for journey_* conditions, validates
every referenced tag/module against journey.json (silent-typo guard), reads a
bounded journal suffix sized by the widest window any rule references, and
emits the five v1 aggregators (occurrence, count, seen, since_ge, distinct).

Selector validation makes the == 0 absence form safe: a rule referencing
['testz','s'] when the project defines `tests` is rejected at load time, not
silently always-true.

Determinism contract test: fixed journal + fixed ts/sid ⇒ byte-identical
journey_* fact sets across two runs.

Window encoding: c/m/h/d/s in v1; r is phase 2 and rejected with a hint.

Refs: docs/specs/SPEC-journey-facts.md §Derivation
EOF
)"
```

---

## Task 4: Hook wiring + outcomes ledger fold-in

**Files:**
- Modify: `crates/phronesis-mcp/src/hook.rs`
- Modify: `crates/phronesis-mcp/src/context.rs`
- Modify: `crates/phronesis-mcp/src/init.rs`
- Modify: `crates/phronesis-mcp/src/outcomes/mod.rs`
- Modify: `crates/phronesis-mcp/src/outcomes/derive.rs`
- Modify: `crates/phronesis-mcp/src/outcomes/cargo.rs`
- Delete: `crates/phronesis-mcp/src/outcomes/ledger.rs`
- Create: `crates/phronesis-mcp/tests/journey_hook_integration.rs`

- [ ] **Step 4.1: Add the session-stamp at SessionStart**

Modify `crates/phronesis-mcp/src/context.rs::run_session_context` to write a fresh session id to `.phronesis/journey/session` if not already present. The session id format matches the spec: `s-YYYY-MM-DD-<6 hex>`.

```rust
fn ensure_session_id(project_root: &Path) -> Result<String, std::io::Error> {
    let path = project_root.join(".phronesis").join("journey").join("session");
    if let Ok(sid) = std::fs::read_to_string(&path) {
        let sid = sid.trim();
        if !sid.is_empty() {
            return Ok(sid.to_string());
        }
    }
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let hex: u32 = (ts as u32) ^ std::process::id();
    let date = chrono_date_for_ts(ts);  // YYYY-MM-DD via existing date helper
    let sid = format!("s-{}-{:06x}", date, hex & 0xFFFFFF);
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, &sid)?;
    Ok(sid)
}
```

If the project already has a date-helper, reuse it. Otherwise add a thin one in this file. (If `chrono` is not a dep, format the date by hand from epoch seconds — no need to add a new crate.)

Wire `ensure_session_id` into the existing `run_session_context` so SessionStart hooks stamp the file even when the file doesn't exist.

- [ ] **Step 4.2: Read session id in the hook**

Add a helper to `hook.rs` that returns the active session id by reading `.phronesis/journey/session`, falling back to a date-bucket if unreadable (per SPEC).

```rust
fn current_sid(project_root: &Path) -> String {
    if let Ok(s) = std::fs::read_to_string(project_root.join(".phronesis").join("journey").join("session")) {
        let s = s.trim();
        if !s.is_empty() { return s.to_string(); }
    }
    // date-bucket fallback
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    format!("s-{}-fallback", chrono_date_for_ts(ts))
}
```

- [ ] **Step 4.3: Wire derive into pre-check and post-check**

In `crates/phronesis-mcp/src/hook.rs::run_pre_check` and `run_post_check`, after the existing `assert_*_facts` block and **before** `network.update_agenda()`:

```rust
// Journey facts: recomputed from the durable journal, bounded by what the
// loaded rules actually reference. Fail-open — never block on a missing or
// corrupt journal.
if std::env::var("PHRONESIS_NO_JOURNEY").is_err() {
    let cfg = journey::load_config(&project_root).unwrap_or_default();
    let sid = current_sid(&project_root);
    let now = unix_secs_now();
    if let Err(e) = journey::derive::assert_facts(
        &mut network, &project_root, &rules, &cfg, &sid, now,
    ) {
        eprintln!("phronesis: journey derivation skipped: {}", e);
    }
}
```

Add `journey::load_config` in `journey/mod.rs`:

```rust
pub fn load_config(project_root: &std::path::Path) -> Result<tagger::TaggerConfig, std::io::Error> {
    let path = project_root.join(".phronesis").join("journey.json");
    let s = std::fs::read_to_string(&path)?;
    serde_json::from_str(&s).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

impl Default for tagger::TaggerConfig {
    fn default() -> Self {
        Self { version: 1, taggers: vec![], modules: vec![] }
    }
}
```

- [ ] **Step 4.4: Wire journaling at post-check tail**

At the tail of `run_post_check`, *after* the existing decision logging and *before* the exit:

```rust
if std::env::var("PHRONESIS_NO_JOURNEY").is_err() {
    let cfg = journey::load_config(&project_root).unwrap_or_default();
    let facts = hook_facts::build_common_facts(&tool_name, &file_path, old_content.as_deref(), new_content.as_deref());
    let tag_result = journey::tagger::fire(&cfg, &facts).unwrap_or_default();
    let module = journey::tagger::resolve_module(&cfg, &file_path);

    // The outcomes adapter fires first and emits outcome tags + a subject.
    let (outcome_tags, subject) = outcomes::cargo::extract_from(&payload, &tool_name);
    let mut all_tags = tag_result.tags;
    all_tags.extend(outcome_tags);

    let record = journey::journal::JournalRecord {
        v: 1,
        ts: unix_secs_now(),
        sid: current_sid(&project_root),
        seq: next_seq(&project_root),  // reads/writes .phronesis/journey/seq, flocked
        tool: tool_name.clone(),
        path: file_path.clone(),
        ext: std::path::Path::new(&file_path)
            .extension().and_then(|s| s.to_str()).map(|s| s.to_string()),
        module,
        tags: all_tags,
        subject,
    };
    let _ = journey::journal::append(&project_root, &record);
}
```

Implement `next_seq` as a flocked read-increment-write of `.phronesis/journey/seq` (a tiny file containing a u64 decimal). On error, return 0 — the seq is for ordering, not correctness.

- [ ] **Step 4.5: Refactor `outcomes::cargo` to return `(tags, subject)` instead of writing to ledger**

`outcomes/cargo.rs::extract_from(payload, tool_name)` becomes the pure "parse cargo output, return outcome tags + subject" function. The old "append to ledger" call sites are removed; the journal takes over.

Names of tags (per SPEC §"Subject and the outcomes fold-in"):
- `outcome:compile_ok` / `outcome:compile_error`
- `outcome:test_pass` / `outcome:test_fail`
- `outcome:bug_caught` (when a known-bug test went from red to green)

- [ ] **Step 4.6: Rewire `outcomes::derive::signals` to read from the journal**

`outcomes/derive.rs::signals(root, subject)` now calls `journey::journal::read_recent_subject(root, subject, n)` and synthesizes the existing `OutcomeFact` shapes from the journal record tags. The public signature stays the same so confidence-scoring callers (hook gate + CLI report + MCP tool) don't change. The internal `LedgerEntry` type is no longer referenced; replace its uses.

- [ ] **Step 4.7: Delete `outcomes/ledger.rs`**

Remove the file. Remove `pub mod ledger;` from `outcomes/mod.rs`. Resolve any compile errors by removing dead imports.

```bash
git rm crates/phronesis-mcp/src/outcomes/ledger.rs
```

- [ ] **Step 4.8: `--packs journey` in init**

Modify `crates/phronesis-mcp/src/init.rs` to recognize a new `journey` pack. When selected:
- Write a starter `.phronesis/journey.json` if it doesn't exist:

```json
{
  "version": 1,
  "taggers": [
    { "tag": "build", "when": [ { "bash_command_matches": "cargo (build|check|test)" } ] }
  ],
  "modules": []
}
```

- Add `.phronesis/journey/` to the project gitignore set (it already gets `.phronesis/*` ignored — verify and add a `!.phronesis/journey.json` un-ignore if needed so the config is tracked).

- [ ] **Step 4.9: Write hook integration tests**

Create `crates/phronesis-mcp/tests/journey_hook_integration.rs`:

```rust
// These tests drive the post-check hook binary against a tempdir project and
// verify the journal grows, then drive pre-check and verify a journey rule
// blocks. Mirror the structure of crates/phronesis-mcp/tests/hook_integration.rs.

use std::process::Command;

fn binary() -> std::path::PathBuf {
    let mut p = std::env::current_exe().unwrap();
    p.pop(); p.pop();
    p.join("phr-mcp")
}

fn setup_project(rules_json: &str, journey_json: Option<&str>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let phr = dir.path().join(".phronesis");
    std::fs::create_dir_all(&phr).unwrap();
    std::fs::write(phr.join("rules.json"), rules_json).unwrap();
    if let Some(j) = journey_json {
        std::fs::write(phr.join("journey.json"), j).unwrap();
    }
    dir
}

#[test]
fn post_check_journals_executed_call() {
    let rules = r#"{"version":2,"rules":[]}"#;
    let journey = r#"{"version":1,"taggers":[{"tag":"auth","when":[{"file_path_matches":"src/auth/"}]}],"modules":[]}"#;
    let dir = setup_project(rules, Some(journey));

    let payload = serde_json::json!({
        "tool_name": "Edit",
        "tool_input": {
            "file_path": format!("{}/src/auth/login.rs", dir.path().display()),
            "old_string": "",
            "new_string": "pub fn login() {}"
        }
    });
    let out = Command::new(binary())
        .arg("post-check")
        .current_dir(dir.path())
        .env("PHRONESIS_NO_ACTION_LOG", "1") // optional — keep test output clean
        .stdin(std::process::Stdio::piped())
        .spawn().unwrap();
    use std::io::Write;
    out.stdin.as_ref().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
    out.wait_with_output().unwrap();

    let journal = std::fs::read_to_string(
        dir.path().join(".phronesis").join("journey").join("events.jsonl"),
    ).unwrap();
    assert!(journal.contains("\"tags\":[\"auth\"]"), "auth tag should be in record: {}", journal);
}

#[test]
fn pre_check_blocks_on_journey_rule() {
    let rules = r#"{"version":2,"rules":[
        {"id":"sql-recent","phase":"pre","priority":10,
         "when":[{"journey_seen":["sql","5c"]}],
         "then":{"block":"Recent SQL — verify the target."}}
    ]}"#;
    let journey = r#"{"version":1,"taggers":[{"tag":"sql","when":[{"new_content_contains":"INSERT INTO"}]}],"modules":[]}"#;
    let dir = setup_project(rules, Some(journey));

    // seed: one prior call that produced a `sql` tag in the current session.
    // simplest: write the events.jsonl directly with a matching record.
    let rec = serde_json::json!({
        "v":1,"ts":1718700000,"sid":"s-test","seq":1,
        "tool":"Edit","path":"src/db.rs","ext":"rs",
        "tags":["sql"]
    });
    let journey_dir = dir.path().join(".phronesis").join("journey");
    std::fs::create_dir_all(&journey_dir).unwrap();
    std::fs::write(journey_dir.join("events.jsonl"), format!("{}\n", rec)).unwrap();
    std::fs::write(journey_dir.join("session"), "s-test").unwrap();

    let payload = serde_json::json!({
        "tool_name": "Edit",
        "tool_input": {
            "file_path": format!("{}/src/unrelated.rs", dir.path().display()),
            "old_string": "",
            "new_string": "pub fn foo() {}"
        }
    });
    let mut child = Command::new(binary())
        .arg("pre-check")
        .current_dir(dir.path())
        .stdin(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn().unwrap();
    use std::io::Write;
    child.stdin.as_mut().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(2), "exit 2 = blocked");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Recent SQL"), "stderr: {}", stderr);
}

#[test]
fn no_journey_env_var_disables_both_paths() {
    let rules = r#"{"version":2,"rules":[]}"#;
    let dir = setup_project(rules, None);
    let payload = serde_json::json!({
        "tool_name": "Edit",
        "tool_input": {
            "file_path": format!("{}/src/a.rs", dir.path().display()),
            "old_string": "",
            "new_string": "pub fn a() {}"
        }
    });
    let mut child = Command::new(binary())
        .arg("post-check")
        .current_dir(dir.path())
        .env("PHRONESIS_NO_JOURNEY", "1")
        .stdin(std::process::Stdio::piped())
        .spawn().unwrap();
    use std::io::Write;
    child.stdin.as_mut().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
    child.wait().unwrap();

    let events = dir.path().join(".phronesis").join("journey").join("events.jsonl");
    assert!(!events.exists(), "journal not written when PHRONESIS_NO_JOURNEY=1");
}

#[test]
fn corrupt_journey_json_is_fail_open() {
    let rules = r#"{"version":2,"rules":[]}"#;
    let dir = setup_project(rules, Some("{not json"));
    let payload = serde_json::json!({
        "tool_name": "Edit",
        "tool_input": {
            "file_path": format!("{}/src/a.rs", dir.path().display()),
            "old_string": "",
            "new_string": "pub fn a() {}"
        }
    });
    let mut child = Command::new(binary())
        .arg("pre-check")
        .current_dir(dir.path())
        .stdin(std::process::Stdio::piped())
        .spawn().unwrap();
    use std::io::Write;
    child.stdin.as_mut().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(0), "fail-open: corrupt journey.json must not block");
}
```

- [ ] **Step 4.10: Run hook integration tests**

Run: `cargo build --package phronesis-mcp && cargo test --package phronesis-mcp --test journey_hook_integration -- --test-threads=1`
Expected: PASS — 4 tests.

- [ ] **Step 4.11: Update confidence-scoring tests for the fold-in**

The existing tests in `crates/phronesis-mcp/tests/confidence_cli_integration.rs` and `confidence_gate_integration.rs` previously seeded `.phronesis/outcomes/<subject>.jsonl`. Now they should seed `.phronesis/journey/events.jsonl` with records carrying `subject` + outcome tags. Update the seeding helpers; the assertion targets (confidence band, gate behavior) should still pass byte-for-byte.

Run: `cargo test --package phronesis-mcp --test confidence_cli_integration --test confidence_gate_integration`
Expected: PASS.

- [ ] **Step 4.12: Commit**

```bash
git add -A
git rm crates/phronesis-mcp/src/outcomes/ledger.rs
git commit -m "$(cat <<'EOF'
feat(journey): wire hook + fold outcomes ledger into the journey journal

Both run_pre_check and run_post_check now call journey::derive::assert_facts
before update_agenda(). run_post_check appends a journal record at the tail
with subject + tags. SessionStart stamps .phronesis/journey/session;
pre/post-check read it. Fail-open everywhere — corrupt journey.json or
missing journal degrades to "no journey facts," never exit 2.
PHRONESIS_NO_JOURNEY=1 disables both paths.

Outcomes ledger fold-in: outcomes/ledger.rs deleted. outcomes/cargo.rs now
returns (tags, subject); the hook stamps them on the journal record.
outcomes/derive::signals reads via journey::journal::read_recent_subject.
Confidence-scoring behavior is byte-identical; the storage is unified.

init --packs journey writes a starter journey.json and ensures
.phronesis/journey.json is tracked.

Refs: docs/specs/SPEC-journey-facts.md §"Where it plugs into the hook",
      §"Subject and the outcomes fold-in"
EOF
)"
```

---

## Task 5: CLI + MCP surface, 0.13.0 bump

**Files:**
- Modify: `crates/phronesis-mcp/src/main.rs`
- Modify: `crates/phronesis-mcp/src/server.rs`
- Modify: `crates/phronesis-mcp/src/server_params.rs`
- Modify: `crates/phronesis-mcp/Cargo.toml`
- Modify: `crates/phronesis-mcp/CLAUDE.md`
- Create: `crates/phronesis-mcp/tests/journey_cli_integration.rs`

- [ ] **Step 5.1: Write failing CLI test**

Create `crates/phronesis-mcp/tests/journey_cli_integration.rs`:

```rust
use std::process::Command;

fn binary() -> std::path::PathBuf {
    let mut p = std::env::current_exe().unwrap();
    p.pop(); p.pop();
    p.join("phr-mcp")
}

#[test]
fn journey_command_renders_current_facts() {
    let dir = tempfile::tempdir().unwrap();
    let phr = dir.path().join(".phronesis");
    std::fs::create_dir_all(phr.join("journey")).unwrap();
    std::fs::write(phr.join("rules.json"), r#"{"version":2,"rules":[
        {"id":"auth-churn","phase":"pre","priority":10,
         "when":[{"__script__":"facts_count('journey_occurrence', ['auth','s']) >= 3"}],
         "then":{"warn":"churn"}}
    ]}"#).unwrap();
    std::fs::write(phr.join("journey.json"), r#"{
        "version":1,
        "taggers":[{"tag":"auth","when":[{"file_path_matches":"src/auth/"}]}],
        "modules":[]
    }"#).unwrap();
    std::fs::write(phr.join("journey").join("session"), "s-test").unwrap();
    let recs = (1..=3u64).map(|s| {
        serde_json::json!({
            "v":1, "ts":1000+s, "sid":"s-test", "seq":s,
            "tool":"Edit", "path":"src/auth/x.rs", "ext":"rs",
            "tags":["auth"]
        }).to_string()
    }).collect::<Vec<_>>().join("\n");
    std::fs::write(phr.join("journey").join("events.jsonl"), recs + "\n").unwrap();

    let out = Command::new(binary())
        .arg("journey")
        .arg("--json")
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("journey_occurrence"), "{}", stdout);
    assert!(stdout.contains("\"selector\":\"auth\""), "{}", stdout);
}
```

- [ ] **Step 5.2: Run to verify fail**

Run: `cargo build && cargo test --package phronesis-mcp --test journey_cli_integration`
Expected: FAIL — `journey` is not a known subcommand.

- [ ] **Step 5.3: Add the `journey` CLI subcommand**

In `crates/phronesis-mcp/src/main.rs`, add a `Journey` variant to the `Command` enum and a handler. The handler:

1. Reads rules (`rules_file::read(...)`) and journey config (`journey::load_config`).
2. Calls `journey::derive::assert_facts` against a fresh `ReteNetwork` with `now = unix_secs_now()` and the current sid.
3. Filters `network.facts()` to those whose predicate starts with `journey_`.
4. Renders as a table by default; `--json` emits the JSON form.
5. `--explain <rule-id>` filters to facts referenced by that rule (use the same scan path as derivation).

Place the implementation in a new file `crates/phronesis-mcp/src/journey_cli.rs` if it grows past ~50 lines.

- [ ] **Step 5.4: Run the CLI test**

Run: `cargo build && cargo test --package phronesis-mcp --test journey_cli_integration`
Expected: PASS.

- [ ] **Step 5.5: Add the `get_journey` MCP tool**

In `crates/phronesis-mcp/src/server.rs`, add a tool method on `EpistemeMcp` (or whatever the server struct is named):

```rust
#[tool(description = "Return the journey_* facts that would assert right now — call to see your trajectory.")]
pub async fn get_journey(&self, Parameters(params): Parameters<GetJourneyParams>) -> Result<CallToolResult, ErrorData> {
    // Same shape as the CLI handler; format as JSON content.
    todo!()
}
```

In `crates/phronesis-mcp/src/server_params.rs`:

```rust
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GetJourneyParams {
    /// Optional rule id; if set, only return facts that rule references.
    #[serde(default)]
    pub explain_rule: Option<String>,
}
```

- [ ] **Step 5.6: Bump the version**

Modify `crates/phronesis-mcp/Cargo.toml`: `version = "0.13.0"`.

Run: `cargo build && cargo test --workspace`
Expected: workspace builds clean; all tests pass.

- [ ] **Step 5.7: Update CLAUDE.md**

In `crates/phronesis-mcp/CLAUDE.md`:
- Add `cargo run -- journey   # what journey_* facts assert right now` to the Build & Run section.
- Add a short "Journey facts" section under Architecture pointing at `src/journey/` and SPEC-journey-facts.md, mirroring the existing `src/outcomes/` paragraph.
- Add `--packs journey` to the list of init packs.
- Note the `get_journey` MCP tool in the server section.

- [ ] **Step 5.8: Run the whole suite**

Run: `cargo fmt --all && cargo clippy --workspace -- -D warnings && cargo test --workspace`
Expected: PASS.

- [ ] **Step 5.9: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat: phr-mcp journey command + get_journey MCP tool; bump 0.13.0

CLI: `phr-mcp journey [--json] [--explain <rule-id>]` renders the journey_*
facts a derivation pass would assert against the current journal, with an
explain mode that filters to a single rule's dependencies.

MCP: get_journey mirrors the table/json view so the agent can ask "what does
my trajectory look like" mid-conversation.

CLAUDE.md updated. Workspace bumps to 0.13.0.

phr-mcp install on top of this build refreshes the user-level binary so hook
invocations pick up the journey path.

Refs: docs/specs/SPEC-journey-facts.md §"CLI & MCP surface"
EOF
)"
```

---

## Self-review notes

**Spec coverage check:**
- §Premise → Tasks 3, 4 (rules see prior trajectory)
- §The one hard constraint → Task 3 (derive recomputes; no in-network accumulation)
- §Architecture pieces 1–3 → Tasks 1, 2, 3
- §Phase 2 (deferred) → noted in commits; out of scope
- §Project-defined seam → Task 2
- §Journal record schema (incl. subject, no atoms) → Task 1
- §Outcomes fold-in → Task 4
- §Headline v1 rules (4) → exercised across Tasks 3, 4 tests
- §Cost / session floor → Task 3 (`filter_to_windows`, `suffix_bound`)
- §Where it plugs into hook → Task 4
- §Determinism contract → Task 3 step 3.6 test
- §Tagger performance budget → Task 2 step 2.7 test
- §CLI & MCP surface → Task 5
- §init `--packs journey` → Task 4 step 4.8
- §Non-goals (rename tracking, first-class `not`, `journey_sequence`, atoms) → no tasks, intentional
- §Open questions (negation ergonomics, window encoding, retention) → no tasks, future work

**Known sketch areas (for the executing agent):**
- `phr::Condition::script` constructor in Task 3.6 — match the actual constructor your `phr` crate exposes.
- Several `todo!()` placeholders in Task 3.7 are explicit invitations to fill in — they are the bulk of the derivation logic. The test set in 3.6 is the contract.
- `outcomes::cargo::extract_from` in Task 4 — the existing signature/contents drive the refactor; replace ledger writes with tag returns.

**Out of scope (phase 2, separate plan):**
- `r` window selector
- `journey/checkpoint.rs` + `journey-compact` command
- atom-keyed aggregator + atoms field restoration
- `journey_sequence` aggregator
- first-class `not` / `not_seen` sugar

---

## Execution handoff

Plan complete and saved to `docs/specs/PLAN-journey-facts.md`. Two execution options:

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration. Best for the four-hour-plus runs where each task is large.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints. Better for a tight evening focused on one or two tasks.

Which approach?

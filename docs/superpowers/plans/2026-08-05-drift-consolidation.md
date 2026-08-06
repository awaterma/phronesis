# Drift Consolidation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the three MCP tools `get_claude_md_drift`, `get_memory_drift`, and `get_wiki_drift` with one `get_drift(source)` tool backed by a source registry, without changing any scoring logic.

**Architecture:** A new `src/drift/` module holds a common envelope (`types.rs`), enum-dispatch registry (`registry.rs`), and unified renderers (`render.rs`). The three existing modules keep their extraction and scoring verbatim and gain a small `into_items()` adapter that maps their native types into the envelope. The MCP layer registers one tool; the CLI keeps three aliases forwarding to `drift --source X`.

**Tech Stack:** Rust 2024 edition, serde, thiserror, rmcp macros, clap.

## Global Constraints

- Spec: `docs/specs/SPEC-drift-consolidation.md`. Read it before Task 1.
- **Do not modify scoring logic** in `claude_md_drift.rs`, `memory_drift.rs`, or `wiki_drift.rs`. Their existing tests passing unchanged is the regression signal.
- Similarity values are `f32` throughout (matching the existing sources). Do not widen to `f64`.
- Rule ids are `phr::RuleId`, not `String`. Convert with `.to_string()` only at the serde boundary.
- Every commit must pass `cargo build --workspace` and `cargo test --workspace`.
- Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` before each commit.
- No `.unwrap()` in `src/` (enforced by the `rust` pack). Use `?` or `expect` with a message in tests only.
- Do not `git push`.

## Spec correction adopted by this plan

The spec's `Verdict` enum fused two independent axes. `memory_drift::Bucket`
(`Actionable`/`Ambient`/`Personal`) classifies *what kind of guidance* an
entry is — it is orthogonal to whether a rule covers it, which is
`similarity` vs `coverage_threshold`. An `Actionable` entry can be covered.

This plan therefore splits them:

- `verdict: Verdict` — the coverage axis
- `category: Option<Category>` — memory's guidance-kind axis, `None` for
  sources that do not classify

Task 8 updates the spec to match.

## File Structure

| File | Responsibility |
|------|----------------|
| `src/drift/mod.rs` | module wiring, re-exports |
| `src/drift/types.rs` | envelope: `Source`, `Availability`, `Verdict`, `Category`, `Evidence`, `DriftItem`, `DriftReport`, `AggregateReport`, `Totals`. Pure data. |
| `src/drift/registry.rs` | `SourceInputs`, `run_source`, `run_all`, availability resolution |
| `src/drift/render.rs` | `render_table`, `render_json` over `&[DriftReport]` |
| `src/claude_md_drift.rs` | + `into_items()` adapter |
| `src/memory_drift.rs` | + `into_items()` adapter |
| `src/wiki_drift.rs` | + `into_items()` adapter |
| `src/server.rs` | remove 3 tools, add `get_drift` |
| `src/server_params.rs` | remove 3 param structs, add `GetDriftParams` |
| `src/main.rs` | add `drift --source`, keep 3 aliases, drop the `drift` alias on `claude-md-drift` |
| `src/init.rs` | durable template drift section |

---

### Task 1: Envelope types

**Files:**
- Create: `crates/phronesis-mcp/src/drift/types.rs`
- Create: `crates/phronesis-mcp/src/drift/mod.rs`
- Modify: `crates/phronesis-mcp/src/lib.rs` (add `pub mod drift;`)

**Interfaces:**
- Consumes: nothing
- Produces: `Source`, `Availability`, `MissingReason`, `Verdict`, `Family`, `Category`, `Evidence`, `DriftItem`, `DriftReport`, `AggregateReport`, `Totals`

- [ ] **Step 1: Write the failing test**

Create `crates/phronesis-mcp/src/drift/types.rs` with only this test module at the bottom (types come in Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structural_evidence_has_no_score_field() {
        let e = Evidence::Structural {
            symbol: "execute_all_agenda_items".to_string(),
            bound_to: vec!["crate::engine::Agenda::execute_all_agenda_items".to_string()],
            resolves: false,
        };
        let json = serde_json::to_string(&e).expect("serialize");
        assert!(json.contains("\"kind\":\"structural\""), "got {json}");
        assert!(!json.contains("score"), "structural must not carry a score: {json}");
        assert!(!json.contains("threshold"), "structural must not carry a threshold: {json}");
    }

    #[test]
    fn declared_evidence_has_no_score_field() {
        let e = Evidence::Declared {
            rules: vec!["rule-a".to_string()],
            superseded_by: None,
        };
        let json = serde_json::to_string(&e).expect("serialize");
        assert!(json.contains("\"kind\":\"declared\""), "got {json}");
        assert!(!json.contains("score"), "declared must not carry a score: {json}");
    }

    #[test]
    fn heuristic_evidence_has_no_resolves_field() {
        let e = Evidence::Heuristic {
            score: 0.42,
            threshold: 0.15,
            matched_rules: vec!["rule-a".to_string()],
        };
        let json = serde_json::to_string(&e).expect("serialize");
        assert!(json.contains("\"kind\":\"heuristic\""), "got {json}");
        assert!(!json.contains("resolves"), "heuristic must not carry resolves: {json}");
    }

    #[test]
    fn family_orders_by_triage_urgency() {
        assert!(Family::Broken > Family::Uncovered);
        assert!(Family::Uncovered > Family::Superseded);
        assert!(Family::Superseded > Family::Covered);
    }

    #[test]
    fn verdict_families_are_assigned() {
        assert_eq!(Verdict::Covered.family(), Family::Covered);
        assert_eq!(Verdict::LikelyCovered.family(), Family::Covered);
        assert_eq!(Verdict::Uncovered.family(), Family::Uncovered);
        assert_eq!(Verdict::Superseded.family(), Family::Superseded);
        assert_eq!(Verdict::Moved.family(), Family::Broken);
        assert_eq!(Verdict::Stale.family(), Family::Broken);
    }

    #[test]
    fn category_is_orthogonal_to_verdict() {
        // A memory entry can be Actionable AND covered — the two axes are
        // independent. This is the spec correction in this plan's header.
        let item = DriftItem {
            subject: "always log via tracing".to_string(),
            verdict: Verdict::Covered,
            category: Some(Category::Actionable),
            suggestion: None,
            evidence: Evidence::Heuristic {
                score: 0.8,
                threshold: 0.15,
                matched_rules: vec!["rule-a".to_string()],
            },
        };
        assert_eq!(item.verdict, Verdict::Covered);
        assert_eq!(item.category, Some(Category::Actionable));
    }

    #[test]
    fn source_all_lists_every_variant() {
        assert_eq!(Source::ALL.len(), 4);
        assert!(Source::ALL.contains(&Source::ClaudeMd));
        assert!(Source::ALL.contains(&Source::Code));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p phronesis-mcp drift::types 2>&1 | tail -20`
Expected: FAIL — compile errors, `Evidence` not found.

- [ ] **Step 3: Write minimal implementation**

Prepend to `crates/phronesis-mcp/src/drift/types.rs` (above the test module):

```rust
//! The common drift envelope. Pure data: serde only, no I/O, no formatting.
//!
//! See `docs/specs/SPEC-drift-consolidation.md` §2-§3.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// One drift corpus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    ClaudeMd,
    Memory,
    Wiki,
    /// Registered by SPEC-rule-staleness. Always reports `Missing` until then.
    Code,
}

impl Source {
    pub const ALL: &'static [Source] =
        &[Source::ClaudeMd, Source::Memory, Source::Wiki, Source::Code];

    pub fn as_str(self) -> &'static str {
        match self {
            Source::ClaudeMd => "claude_md",
            Source::Memory => "memory",
            Source::Wiki => "wiki",
            Source::Code => "code",
        }
    }
}

/// Whether a corpus could be read at all. An absent corpus is data, not a
/// fault: on a fresh project the wiki and memory directories legitimately
/// do not exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Availability {
    Present { scanned: usize },
    Missing { reason: MissingReason },
    Errored { detail: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingReason {
    NoFile,
    NoDir,
    NoGraph,
}

/// The coverage axis: does a rule already enforce this?
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Covered,
    LikelyCovered,
    Uncovered,
    Superseded,
    /// SPEC-rule-staleness §3.2.
    Moved,
    /// SPEC-rule-staleness §3.2.
    Stale,
}

/// Triage urgency. `Ord` is derived, so declaration order is the ordering:
/// least urgent first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    Covered,
    Superseded,
    Uncovered,
    Broken,
}

impl Verdict {
    pub fn family(self) -> Family {
        match self {
            Verdict::Covered | Verdict::LikelyCovered => Family::Covered,
            Verdict::Superseded => Family::Superseded,
            Verdict::Uncovered => Family::Uncovered,
            Verdict::Moved | Verdict::Stale => Family::Broken,
        }
    }
}

/// What kind of guidance this is. Orthogonal to [`Verdict`]: an
/// `Actionable` memory entry may be covered or uncovered. Only the memory
/// source classifies; others emit `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// Names a tool / command / code shape. Should become a rule.
    Actionable,
    /// Project-shareable ambient guidance. Belongs in durable.md.
    Ambient,
    /// Personal preference. Stays in MEMORY.md.
    Personal,
}

/// Why we believe what we believe. The three variants deliberately share no
/// field, so a consumer can tell them apart from the `kind` tag alone.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Evidence {
    /// An author wrote down which rules enforce this. Not inferred, and so
    /// carries no score.
    Declared {
        rules: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        superseded_by: Option<String>,
    },
    /// Token-overlap heuristic. A triage hint, not ground truth.
    Heuristic {
        score: f32,
        threshold: f32,
        matched_rules: Vec<String>,
    },
    /// Resolved against the code graph. Boolean, not a confidence.
    Structural {
        symbol: String,
        bound_to: Vec<String>,
        resolves: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DriftItem {
    pub subject: String,
    pub verdict: Verdict,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<Category>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
    pub evidence: Evidence,
}

#[derive(Debug, Clone, Serialize)]
pub struct DriftReport {
    pub source: Source,
    pub availability: Availability,
    pub uncovered_count: usize,
    pub items: Vec<DriftItem>,
}

impl DriftReport {
    /// A report for a corpus that is not present.
    pub fn missing(source: Source, reason: MissingReason) -> Self {
        DriftReport {
            source,
            availability: Availability::Missing { reason },
            uncovered_count: 0,
            items: Vec::new(),
        }
    }

    /// A report for a corpus that is present but could not be read.
    pub fn errored(source: Source, detail: String) -> Self {
        DriftReport {
            source,
            availability: Availability::Errored { detail },
            uncovered_count: 0,
            items: Vec::new(),
        }
    }
}

/// Counts `Uncovered` and `Broken` only. `Personal` entries and
/// `Superseded` decisions are not drift — nothing is missing — and
/// counting them would inflate the one number that drives a decision.
pub fn uncovered_count(items: &[DriftItem]) -> usize {
    items
        .iter()
        .filter(|i| matches!(i.verdict.family(), Family::Uncovered | Family::Broken))
        .count()
}

#[derive(Debug, Clone, Serialize)]
pub struct AggregateReport {
    pub sources: Vec<DriftReport>,
    pub totals: Totals,
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Totals {
    pub sources_present: usize,
    pub sources_missing: usize,
    pub sources_errored: usize,
    pub uncovered_total: usize,
    pub by_family: BTreeMap<Family, usize>,
}
```

Create `crates/phronesis-mcp/src/drift/mod.rs`:

```rust
//! Unified drift detection across corpora. See
//! `docs/specs/SPEC-drift-consolidation.md`.

pub mod types;

pub use types::{
    AggregateReport, Availability, Category, DriftItem, DriftReport, Evidence, Family,
    MissingReason, Source, Totals, Verdict, uncovered_count,
};
```

Add to `crates/phronesis-mcp/src/lib.rs`, in alphabetical position among the existing `pub mod` lines:

```rust
pub mod drift;
```

`Family` is used as a `BTreeMap` key, which is why it derives `Ord`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p phronesis-mcp drift::types 2>&1 | tail -20`
Expected: PASS, 7 tests.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --workspace -- -D warnings && cargo test --workspace 2>&1 | tail -5
git add crates/phronesis-mcp/src/drift crates/phronesis-mcp/src/lib.rs
git commit -m "feat(drift): add the common drift envelope types"
```

---

### Task 2: Source adapters

**Files:**
- Modify: `crates/phronesis-mcp/src/claude_md_drift.rs` (append adapter + tests)
- Modify: `crates/phronesis-mcp/src/wiki_drift.rs` (append adapter + tests)
- Modify: `crates/phronesis-mcp/src/memory_drift.rs` (append adapter + tests)

**Interfaces:**
- Consumes: `drift::types::{DriftItem, Evidence, Verdict, Category, uncovered_count}` from Task 1
- Produces: `claude_md_drift::into_items(&DriftReport) -> Vec<drift::DriftItem>`, `wiki_drift::into_items(&DriftReport) -> Vec<drift::DriftItem>`, `memory_drift::into_items(&DriftReport) -> Vec<drift::DriftItem>`

Note the name collision: each module already has its own `DriftItem` and
`DriftReport`. Inside these modules, refer to the envelope types with an
explicit `crate::drift::` prefix. Do not `use crate::drift::DriftItem`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/phronesis-mcp/src/claude_md_drift.rs` inside its existing `mod tests`:

```rust
    #[test]
    fn adapter_maps_below_threshold_to_uncovered() {
        let report = DriftReport {
            claude_md_path: "CLAUDE.md".to_string(),
            rules_path: ".phronesis/rules.json".to_string(),
            coverage_threshold: 0.15,
            items: vec![DriftItem {
                imperative: "Prefer ? over manual match".to_string(),
                best_match: None,
                similarity: 0.0,
            }],
        };
        let mapped = into_items(&report);
        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].verdict, crate::drift::Verdict::Uncovered);
        assert_eq!(mapped[0].category, None, "claude_md does not classify");
        assert!(matches!(
            mapped[0].evidence,
            crate::drift::Evidence::Heuristic { .. }
        ));
    }

    #[test]
    fn adapter_maps_at_or_above_threshold_to_covered() {
        let report = DriftReport {
            claude_md_path: "CLAUDE.md".to_string(),
            rules_path: ".phronesis/rules.json".to_string(),
            coverage_threshold: 0.15,
            items: vec![DriftItem {
                imperative: "Avoid unwrap".to_string(),
                best_match: Some(MatchedRule {
                    rule_id: "enforce-no-unwrap-in-src".into(),
                    shared_terms: vec!["unwrap".to_string()],
                }),
                similarity: 0.9,
            }],
        };
        let mapped = into_items(&report);
        assert_eq!(mapped[0].verdict, crate::drift::Verdict::Covered);
    }
```

Append to `crates/phronesis-mcp/src/wiki_drift.rs` inside its existing `mod tests`:

```rust
    #[test]
    fn adapter_maps_declared_frontmatter_to_declared_evidence() {
        // A decision with `enforces:` frontmatter is a declaration, not a
        // measurement — it must not become a 1.0 Heuristic score.
        let report = DriftReport {
            wiki_dir: ".phronesis/wiki/decisions".to_string(),
            rules_path: ".phronesis/rules.json".to_string(),
            coverage_threshold: 0.15,
            items: vec![DriftItem {
                decision: sample_decision_with_enforces(),
                bucket: Bucket::Covered,
                best_match: Some(MatchedRule {
                    rule_id: "rule-a".into(),
                    shared_terms: vec![],
                }),
                similarity: 1.0,
            }],
        };
        let mapped = into_items(&report);
        assert!(
            matches!(mapped[0].evidence, crate::drift::Evidence::Declared { .. }),
            "expected Declared, got {:?}",
            mapped[0].evidence
        );
    }

    #[test]
    fn adapter_maps_jaccard_fallback_to_heuristic_evidence() {
        let report = DriftReport {
            wiki_dir: ".phronesis/wiki/decisions".to_string(),
            rules_path: ".phronesis/rules.json".to_string(),
            coverage_threshold: 0.15,
            items: vec![DriftItem {
                decision: sample_decision_without_enforces(),
                bucket: Bucket::Uncovered,
                best_match: None,
                similarity: 0.0,
            }],
        };
        let mapped = into_items(&report);
        assert!(matches!(
            mapped[0].evidence,
            crate::drift::Evidence::Heuristic { .. }
        ));
        assert_eq!(mapped[0].verdict, crate::drift::Verdict::Uncovered);
    }
```

If helpers `sample_decision_with_enforces()` / `sample_decision_without_enforces()`
do not already exist in that test module, add them. Build a `wiki::Decision`
with the same field values the existing tests in this file use, setting
`enforces` to `vec!["rule-a".to_string()]` and `vec![]` respectively. Read the
existing tests first and copy their construction pattern exactly.

Append to `crates/phronesis-mcp/src/memory_drift.rs` inside its existing `mod tests`:

```rust
    #[test]
    fn adapter_keeps_category_orthogonal_to_verdict() {
        // Actionable + covered is a real, representable state.
        let report = DriftReport {
            memory_dir: "/tmp/memory".to_string(),
            rules_path: ".phronesis/rules.json".to_string(),
            durable_md_path: ".phronesis/durable.md".to_string(),
            coverage_threshold: 0.15,
            items: vec![DriftItem {
                entry: sample_entry("always-use-tracing"),
                bucket: Bucket::Actionable,
                best_match: Some(MatchedTarget::Rule {
                    rule_id: "rule-a".into(),
                    shared_terms: vec!["tracing".to_string()],
                }),
                similarity: 0.9,
            }],
        };
        let mapped = into_items(&report);
        assert_eq!(mapped[0].verdict, crate::drift::Verdict::Covered);
        assert_eq!(mapped[0].category, Some(crate::drift::Category::Actionable));
    }

    #[test]
    fn adapter_maps_durable_paragraph_match_into_matched_rules() {
        let report = DriftReport {
            memory_dir: "/tmp/memory".to_string(),
            rules_path: ".phronesis/rules.json".to_string(),
            durable_md_path: ".phronesis/durable.md".to_string(),
            coverage_threshold: 0.15,
            items: vec![DriftItem {
                entry: sample_entry("ambient-thing"),
                bucket: Bucket::Ambient,
                best_match: Some(MatchedTarget::DurableParagraph {
                    excerpt: "we prefer X".to_string(),
                    shared_terms: vec!["prefer".to_string()],
                }),
                similarity: 0.5,
            }],
        };
        let mapped = into_items(&report);
        match &mapped[0].evidence {
            crate::drift::Evidence::Heuristic { matched_rules, .. } => {
                assert_eq!(matched_rules, &vec!["durable.md: we prefer X".to_string()]);
            }
            other => panic!("expected Heuristic, got {other:?}"),
        }
    }
```

If a `sample_entry(name: &str) -> MemoryEntry` helper does not already exist
in that test module, add it, constructing a `MemoryEntry` with
`file_path: PathBuf::from(format!("/tmp/memory/{name}.md"))`, `name` from the
argument, and empty strings for `description`, `memory_type`, and `body`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p phronesis-mcp adapter_ 2>&1 | tail -20`
Expected: FAIL — `into_items` not found in all three modules.

- [ ] **Step 3: Write the adapters**

Append to `crates/phronesis-mcp/src/claude_md_drift.rs` (outside `mod tests`):

```rust
/// Map this source's native report into the common drift envelope.
/// Scoring is not re-run; this is a pure projection.
pub fn into_items(report: &DriftReport) -> Vec<crate::drift::DriftItem> {
    report
        .items
        .iter()
        .map(|item| {
            let verdict = if item.similarity >= report.coverage_threshold {
                crate::drift::Verdict::Covered
            } else {
                crate::drift::Verdict::Uncovered
            };
            crate::drift::DriftItem {
                subject: item.imperative.clone(),
                verdict,
                category: None,
                suggestion: None,
                evidence: crate::drift::Evidence::Heuristic {
                    score: item.similarity,
                    threshold: report.coverage_threshold,
                    matched_rules: item
                        .best_match
                        .iter()
                        .map(|m| m.rule_id.to_string())
                        .collect(),
                },
            }
        })
        .collect()
}
```

Append to `crates/phronesis-mcp/src/wiki_drift.rs` (outside `mod tests`):

```rust
/// Map this source's native report into the common drift envelope.
///
/// A decision carrying `enforces:` frontmatter produces
/// [`crate::drift::Evidence::Declared`] — an author's declaration is not a
/// measurement, and rendering it as a 1.0 similarity would invite
/// comparison against a Jaccard score that means something else entirely.
pub fn into_items(report: &DriftReport) -> Vec<crate::drift::DriftItem> {
    report
        .items
        .iter()
        .map(|item| {
            let verdict = match item.bucket {
                Bucket::Covered => crate::drift::Verdict::Covered,
                Bucket::LikelyCovered => crate::drift::Verdict::LikelyCovered,
                Bucket::Uncovered => crate::drift::Verdict::Uncovered,
                Bucket::Superseded => crate::drift::Verdict::Superseded,
            };
            let evidence = if item.decision.enforces.is_empty() {
                crate::drift::Evidence::Heuristic {
                    score: item.similarity,
                    threshold: report.coverage_threshold,
                    matched_rules: item
                        .best_match
                        .iter()
                        .map(|m| m.rule_id.to_string())
                        .collect(),
                }
            } else {
                crate::drift::Evidence::Declared {
                    rules: item.decision.enforces.clone(),
                    superseded_by: item.decision.superseded_by.clone(),
                }
            };
            crate::drift::DriftItem {
                subject: format!("{} {}", item.decision.id, item.decision.title),
                verdict,
                category: None,
                suggestion: suggest_rule(item),
                evidence,
            }
        })
        .collect()
}
```

Before writing this, open `src/wiki.rs` and confirm the exact field names on
`Decision` for the id, title, `enforces`, and superseded-by fields. If they
differ from `id`, `title`, `enforces`, `superseded_by`, use the real names.
If `superseded_by` does not exist, pass `None`.

Append to `crates/phronesis-mcp/src/memory_drift.rs` (outside `mod tests`):

```rust
/// Map this source's native report into the common drift envelope.
///
/// `Bucket` becomes [`crate::drift::Category`], NOT part of the verdict:
/// what kind of guidance an entry is and whether a rule covers it are
/// independent questions.
pub fn into_items(report: &DriftReport) -> Vec<crate::drift::DriftItem> {
    report
        .items
        .iter()
        .map(|item| {
            let verdict = if item.similarity >= report.coverage_threshold {
                crate::drift::Verdict::Covered
            } else {
                crate::drift::Verdict::Uncovered
            };
            let category = Some(match item.bucket {
                Bucket::Actionable => crate::drift::Category::Actionable,
                Bucket::Ambient => crate::drift::Category::Ambient,
                Bucket::Personal => crate::drift::Category::Personal,
            });
            let matched_rules = match &item.best_match {
                Some(MatchedTarget::Rule { rule_id, .. }) => vec![rule_id.to_string()],
                Some(MatchedTarget::DurableParagraph { excerpt, .. }) => {
                    vec![format!("durable.md: {excerpt}")]
                }
                None => Vec::new(),
            };
            crate::drift::DriftItem {
                subject: item.entry.name.clone(),
                verdict,
                category,
                suggestion: suggest_rule(item),
                evidence: crate::drift::Evidence::Heuristic {
                    score: item.similarity,
                    threshold: report.coverage_threshold,
                    matched_rules,
                },
            }
        })
        .collect()
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p phronesis-mcp adapter_ 2>&1 | tail -20`
Expected: PASS, 6 tests.

Then confirm no scoring regressed:
Run: `cargo test --workspace 2>&1 | grep -E "^test result"`
Expected: 0 failed.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --workspace -- -D warnings
git add crates/phronesis-mcp/src
git commit -m "feat(drift): map the three sources into the common envelope"
```

---

### Task 3: Registry and aggregation

**Files:**
- Create: `crates/phronesis-mcp/src/drift/registry.rs`
- Modify: `crates/phronesis-mcp/src/drift/mod.rs`

**Interfaces:**
- Consumes: Task 1 types, Task 2 `into_items` functions
- Produces: `registry::SourceInputs`, `registry::run_source(Source, &SourceInputs) -> DriftReport`, `registry::run_all(&[Source], &SourceInputs, usize) -> AggregateReport`

- [ ] **Step 1: Write the failing test**

Create `crates/phronesis-mcp/src/drift/registry.rs` with this test module (implementation in Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::drift::{Availability, MissingReason, Source};

    fn empty_inputs(root: &std::path::Path) -> SourceInputs<'_> {
        SourceInputs {
            project_root: root,
            claude_md: None,
            memory_dir: None,
            wiki_dir: None,
        }
    }

    #[test]
    fn a_missing_corpus_is_reported_not_raised() {
        let d = tempfile::tempdir().expect("tempdir");
        let report = run_source(Source::Wiki, &empty_inputs(d.path()));
        assert!(matches!(
            report.availability,
            Availability::Missing { reason: MissingReason::NoDir }
        ));
        assert_eq!(report.uncovered_count, 0);
        assert!(report.items.is_empty());
    }

    #[test]
    fn code_source_is_missing_until_rule_staleness_lands() {
        let d = tempfile::tempdir().expect("tempdir");
        let report = run_source(Source::Code, &empty_inputs(d.path()));
        assert!(matches!(
            report.availability,
            Availability::Missing { reason: MissingReason::NoGraph }
        ));
    }

    #[test]
    fn run_all_succeeds_when_every_corpus_is_absent() {
        let d = tempfile::tempdir().expect("tempdir");
        let agg = run_all(Source::ALL, &empty_inputs(d.path()), 5);
        assert_eq!(agg.sources.len(), 4);
        assert_eq!(agg.totals.sources_missing, 4);
        assert_eq!(agg.totals.sources_present, 0);
        assert_eq!(agg.totals.uncovered_total, 0);
        assert!(!agg.truncated);
    }

    #[test]
    fn sources_are_returned_in_stable_order() {
        let d = tempfile::tempdir().expect("tempdir");
        let agg = run_all(Source::ALL, &empty_inputs(d.path()), 5);
        let order: Vec<Source> = agg.sources.iter().map(|r| r.source).collect();
        assert_eq!(order, Source::ALL.to_vec());
    }

    #[test]
    fn limit_truncates_items_and_sets_the_flag() {
        let items: Vec<crate::drift::DriftItem> = (0..10)
            .map(|i| crate::drift::DriftItem {
                subject: format!("item-{i}"),
                verdict: crate::drift::Verdict::Uncovered,
                category: None,
                suggestion: None,
                evidence: crate::drift::Evidence::Heuristic {
                    score: 0.0,
                    threshold: 0.15,
                    matched_rules: vec![],
                },
            })
            .collect();
        let (kept, truncated) = apply_limit(items, 3);
        assert_eq!(kept.len(), 3);
        assert!(truncated);
    }

    #[test]
    fn limit_is_clamped_to_fifty() {
        assert_eq!(clamp_limit(0), 1);
        assert_eq!(clamp_limit(5), 5);
        assert_eq!(clamp_limit(9999), 50);
    }

    #[test]
    fn items_are_ordered_by_family_urgency_before_truncation() {
        let mk = |v: crate::drift::Verdict, s: &str| crate::drift::DriftItem {
            subject: s.to_string(),
            verdict: v,
            category: None,
            suggestion: None,
            evidence: crate::drift::Evidence::Heuristic {
                score: 0.0,
                threshold: 0.15,
                matched_rules: vec![],
            },
        };
        let items = vec![
            mk(crate::drift::Verdict::Covered, "covered"),
            mk(crate::drift::Verdict::Uncovered, "uncovered"),
        ];
        let (kept, _) = apply_limit(items, 1);
        assert_eq!(kept[0].subject, "uncovered", "most urgent must survive truncation");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p phronesis-mcp drift::registry 2>&1 | tail -20`
Expected: FAIL — `SourceInputs` not found.

- [ ] **Step 3: Write the implementation**

Prepend to `crates/phronesis-mcp/src/drift/registry.rs`:

```rust
//! Source dispatch and availability resolution.
//! See `docs/specs/SPEC-drift-consolidation.md` §1 and §5.

use std::path::Path;

use super::types::{
    AggregateReport, Availability, DriftItem, DriftReport, MissingReason, Source, Totals,
    uncovered_count,
};

pub const DEFAULT_LIMIT: usize = 5;
pub const MAX_LIMIT: usize = 50;

/// Everything a source might need, built by the caller from request
/// parameters and workspace state. The drift core stays pure over this.
pub struct SourceInputs<'a> {
    pub project_root: &'a Path,
    pub claude_md: Option<&'a Path>,
    pub memory_dir: Option<&'a Path>,
    pub wiki_dir: Option<&'a Path>,
}

pub fn clamp_limit(requested: usize) -> usize {
    requested.clamp(1, MAX_LIMIT)
}

/// Sort by descending triage urgency, then truncate. Sorting first means a
/// truncated response keeps the items that matter, rather than whichever
/// happened to be scanned first.
pub fn apply_limit(mut items: Vec<DriftItem>, limit: usize) -> (Vec<DriftItem>, bool) {
    items.sort_by(|a, b| b.verdict.family().cmp(&a.verdict.family()));
    let truncated = items.len() > limit;
    items.truncate(limit);
    (items, truncated)
}

/// Run one source. Never returns Err: an absent corpus is reported as
/// `Missing`, an unreadable one as `Errored`.
pub fn run_source(source: Source, inputs: &SourceInputs<'_>) -> DriftReport {
    match source {
        Source::ClaudeMd => run_claude_md(inputs),
        Source::Memory => run_memory(inputs),
        Source::Wiki => run_wiki(inputs),
        // Registered by SPEC-rule-staleness; until then there is no graph
        // binding source to consult.
        Source::Code => DriftReport::missing(Source::Code, MissingReason::NoGraph),
    }
}

fn run_claude_md(inputs: &SourceInputs<'_>) -> DriftReport {
    let path = match inputs.claude_md {
        Some(p) if p.exists() => p.to_path_buf(),
        Some(_) => return DriftReport::missing(Source::ClaudeMd, MissingReason::NoFile),
        None => {
            let default = inputs.project_root.join("CLAUDE.md");
            if !default.exists() {
                return DriftReport::missing(Source::ClaudeMd, MissingReason::NoFile);
            }
            default
        }
    };
    let _ = path;
    match crate::claude_md_drift::run(inputs.project_root) {
        Ok(report) => {
            let items = crate::claude_md_drift::into_items(&report);
            DriftReport {
                source: Source::ClaudeMd,
                availability: Availability::Present {
                    scanned: report.items.len(),
                },
                uncovered_count: uncovered_count(&items),
                items,
            }
        }
        Err(e) => DriftReport::errored(Source::ClaudeMd, e.to_string()),
    }
}

fn run_memory(inputs: &SourceInputs<'_>) -> DriftReport {
    let dir = match inputs.memory_dir {
        Some(p) => p.to_path_buf(),
        None => crate::memory_drift::default_memory_dir(inputs.project_root),
    };
    if !dir.is_dir() {
        return DriftReport::missing(Source::Memory, MissingReason::NoDir);
    }
    match crate::memory_drift::run_with_dir(inputs.project_root, &dir) {
        Ok(report) => {
            let items = crate::memory_drift::into_items(&report);
            DriftReport {
                source: Source::Memory,
                availability: Availability::Present {
                    scanned: report.items.len(),
                },
                uncovered_count: uncovered_count(&items),
                items,
            }
        }
        Err(e) => DriftReport::errored(Source::Memory, e.to_string()),
    }
}

fn run_wiki(inputs: &SourceInputs<'_>) -> DriftReport {
    let dir = match inputs.wiki_dir {
        Some(p) => p.to_path_buf(),
        None => crate::wiki::default_wiki_dir(inputs.project_root).join("decisions"),
    };
    if !dir.is_dir() {
        return DriftReport::missing(Source::Wiki, MissingReason::NoDir);
    }
    match crate::wiki_drift::run_with_dir(inputs.project_root, &dir) {
        Ok(report) => {
            let items = crate::wiki_drift::into_items(&report);
            DriftReport {
                source: Source::Wiki,
                availability: Availability::Present {
                    scanned: report.items.len(),
                },
                uncovered_count: uncovered_count(&items),
                items,
            }
        }
        Err(e) => DriftReport::errored(Source::Wiki, e.to_string()),
    }
}

/// Run several sources. One source failing never suppresses the others.
pub fn run_all(sources: &[Source], inputs: &SourceInputs<'_>, limit: usize) -> AggregateReport {
    let limit = clamp_limit(limit);
    let mut totals = Totals::default();
    let mut truncated_any = false;
    let mut reports = Vec::with_capacity(sources.len());

    for &source in sources {
        let mut report = run_source(source, inputs);
        match report.availability {
            Availability::Present { .. } => totals.sources_present += 1,
            Availability::Missing { .. } => totals.sources_missing += 1,
            Availability::Errored { .. } => totals.sources_errored += 1,
        }
        totals.uncovered_total += report.uncovered_count;
        for item in &report.items {
            *totals.by_family.entry(item.verdict.family()).or_insert(0) += 1;
        }
        let (kept, truncated) = apply_limit(std::mem::take(&mut report.items), limit);
        report.items = kept;
        truncated_any |= truncated;
        reports.push(report);
    }

    AggregateReport {
        sources: reports,
        totals,
        truncated: truncated_any,
    }
}
```

Add to `crates/phronesis-mcp/src/drift/mod.rs`:

```rust
pub mod registry;

pub use registry::{DEFAULT_LIMIT, MAX_LIMIT, SourceInputs, run_all, run_source};
```

Confirm `tempfile` is already a dev-dependency of `phronesis-mcp` (it is used
by `context/config.rs` tests). If not, add it under `[dev-dependencies]`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p phronesis-mcp drift::registry 2>&1 | tail -20`
Expected: PASS, 7 tests.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --workspace -- -D warnings && cargo test --workspace 2>&1 | grep -E "^test result"
git add crates/phronesis-mcp/src/drift
git commit -m "feat(drift): add the source registry with availability resolution"
```

---

### Task 4: Renderers

**Files:**
- Create: `crates/phronesis-mcp/src/drift/render.rs`
- Modify: `crates/phronesis-mcp/src/drift/mod.rs`

**Interfaces:**
- Consumes: Task 1 types, Task 3 `AggregateReport`
- Produces: `render::render_table(&AggregateReport) -> String`, `render::render_json(&AggregateReport) -> String`

- [ ] **Step 1: Write the failing test**

Create `crates/phronesis-mcp/src/drift/render.rs` with this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::drift::{
        Availability, DriftItem, DriftReport, Evidence, MissingReason, Source, Totals, Verdict,
    };
    use crate::drift::types::AggregateReport;

    fn agg(sources: Vec<DriftReport>) -> AggregateReport {
        AggregateReport {
            sources,
            totals: Totals::default(),
            truncated: false,
        }
    }

    #[test]
    fn table_labels_each_evidence_kind_distinctly() {
        let report = DriftReport {
            source: Source::Wiki,
            availability: Availability::Present { scanned: 2 },
            uncovered_count: 1,
            items: vec![
                DriftItem {
                    subject: "ADR-007 use rustix".to_string(),
                    verdict: Verdict::Covered,
                    category: None,
                    suggestion: None,
                    evidence: Evidence::Declared {
                        rules: vec!["rule-a".to_string()],
                        superseded_by: None,
                    },
                },
                DriftItem {
                    subject: "ADR-009 something".to_string(),
                    verdict: Verdict::Uncovered,
                    category: None,
                    suggestion: None,
                    evidence: Evidence::Heuristic {
                        score: 0.11,
                        threshold: 0.15,
                        matched_rules: vec![],
                    },
                },
            ],
        };
        let out = render_table(&agg(vec![report]));
        assert!(out.contains("declared"), "got {out}");
        assert!(out.contains("heuristic"), "got {out}");
        assert!(out.contains("0.11"), "score must be visible: {out}");
    }

    #[test]
    fn table_reports_a_missing_source_without_pretending_it_is_clean() {
        let out = render_table(&agg(vec![DriftReport::missing(
            Source::Memory,
            MissingReason::NoDir,
        )]));
        assert!(out.contains("memory"), "got {out}");
        assert!(out.contains("not present"), "got {out}");
    }

    #[test]
    fn table_reports_an_errored_source() {
        let out = render_table(&agg(vec![DriftReport::errored(
            Source::Wiki,
            "bad frontmatter in ADR-003".to_string(),
        )]));
        assert!(out.contains("error"), "got {out}");
        assert!(out.contains("bad frontmatter"), "got {out}");
    }

    #[test]
    fn table_announces_truncation() {
        let mut a = agg(vec![DriftReport::missing(Source::Wiki, MissingReason::NoDir)]);
        a.truncated = true;
        let out = render_table(&a);
        assert!(
            out.contains("truncated"),
            "a silent cap reads as 'nothing more to see': {out}"
        );
    }

    #[test]
    fn json_round_trips_as_an_object() {
        let out = render_json(&agg(vec![DriftReport::missing(
            Source::Wiki,
            MissingReason::NoDir,
        )]));
        let v: serde_json::Value = serde_json::from_str(&out).expect("valid json");
        assert!(v.get("sources").is_some());
        assert!(v.get("totals").is_some());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p phronesis-mcp drift::render 2>&1 | tail -20`
Expected: FAIL — `render_table` not found.

- [ ] **Step 3: Write the implementation**

Prepend to `crates/phronesis-mcp/src/drift/render.rs`:

```rust
//! Table and JSON rendering for drift reports.
//!
//! Evidence formatting lives here rather than as a `Display` impl so the
//! pure types in `types.rs` never import a formatting concern.

use std::fmt::Write as _;

use super::types::{AggregateReport, Availability, Evidence, MissingReason};

fn evidence_compact(evidence: &Evidence) -> String {
    match evidence {
        Evidence::Declared {
            rules,
            superseded_by,
        } => match superseded_by {
            Some(s) => format!("declared enforces={} superseded_by={s}", rules.join(",")),
            None => format!("declared enforces={}", rules.join(",")),
        },
        Evidence::Heuristic {
            score,
            threshold,
            matched_rules,
        } => {
            let cmp = if score >= threshold { ">=" } else { "<" };
            if matched_rules.is_empty() {
                format!("heuristic jaccard={score:.2} {cmp}{threshold:.2}")
            } else {
                format!(
                    "heuristic jaccard={score:.2} {cmp}{threshold:.2} matched={}",
                    matched_rules.join(",")
                )
            }
        }
        Evidence::Structural {
            symbol, resolves, ..
        } => format!("structural symbol={symbol} resolves={resolves}"),
    }
}

fn missing_reason_text(reason: MissingReason) -> &'static str {
    match reason {
        MissingReason::NoFile => "not present (no file)",
        MissingReason::NoDir => "not present (no directory)",
        MissingReason::NoGraph => "not present (no code graph)",
    }
}

pub fn render_table(agg: &AggregateReport) -> String {
    let mut out = String::new();

    for report in &agg.sources {
        match &report.availability {
            Availability::Missing { reason } => {
                let _ = writeln!(
                    out,
                    "{:<10} {}",
                    report.source.as_str(),
                    missing_reason_text(*reason)
                );
                continue;
            }
            Availability::Errored { detail } => {
                let _ = writeln!(out, "{:<10} error: {detail}", report.source.as_str());
                continue;
            }
            Availability::Present { scanned } => {
                let _ = writeln!(
                    out,
                    "{:<10} {} scanned, {} uncovered",
                    report.source.as_str(),
                    scanned,
                    report.uncovered_count
                );
            }
        }
        for item in &report.items {
            let verdict = serde_json::to_value(item.verdict)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| "unknown".to_string());
            let _ = writeln!(
                out,
                "  {:<16} {:<48} {}",
                verdict,
                truncate(&item.subject, 48),
                evidence_compact(&item.evidence)
            );
        }
    }

    let t = &agg.totals;
    let _ = writeln!(
        out,
        "\n{} present, {} missing, {} errored — {} uncovered total",
        t.sources_present, t.sources_missing, t.sources_errored, t.uncovered_total
    );
    if agg.truncated {
        let _ = writeln!(
            out,
            "Items truncated; re-run with a single --source and a higher --limit for detail."
        );
    }
    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let kept: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}

pub fn render_json(agg: &AggregateReport) -> String {
    serde_json::to_string_pretty(agg).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
}
```

Add to `crates/phronesis-mcp/src/drift/mod.rs`:

```rust
pub mod render;

pub use render::{render_json, render_table};
```

Also add `pub use types::AggregateReport;` to `mod.rs` if not already re-exported.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p phronesis-mcp drift::render 2>&1 | tail -20`
Expected: PASS, 5 tests.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --workspace -- -D warnings && cargo test --workspace 2>&1 | grep -E "^test result"
git add crates/phronesis-mcp/src/drift
git commit -m "feat(drift): add unified table and json renderers"
```

---

### Task 5: MCP tool

**Files:**
- Modify: `crates/phronesis-mcp/src/server_params.rs:244-270` (remove three structs, add one)
- Modify: `crates/phronesis-mcp/src/server.rs:962-1060` (remove three tools, add one)
- Modify: `crates/phronesis-mcp/src/server.rs:1640-1655, 1708-1716` (invert the canaries)

**Interfaces:**
- Consumes: Task 3 `run_all`/`SourceInputs`, Task 4 renderers
- Produces: MCP tool `get_drift`

- [ ] **Step 1: Write the failing test**

Replace the body of `drift_detection_tools_are_registered` and
`wiki_drift_tool_is_registered` in `crates/phronesis-mcp/src/server.rs`
with a single test. Delete both old tests and add:

```rust
    /// One drift tool, not three. The three removed names must NOT be
    /// registered — an incomplete removal is the failure this catches, and
    /// it is the same class of SPEC-vs-code gap the previous versions of
    /// these assertions were written to guard against.
    #[test]
    fn drift_is_one_tool_and_the_three_old_names_are_gone() {
        let mcp = EpistemeMcp::new();
        assert!(
            mcp.tool_router.has_route("get_drift"),
            "get_drift tool must be registered (matches `phr-mcp drift` CLI)"
        );
        for gone in [
            "get_claude_md_drift",
            "get_memory_drift",
            "get_wiki_drift",
        ] {
            assert!(
                !mcp.tool_router.has_route(gone),
                "{gone} must be removed — superseded by get_drift(source)"
            );
        }
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p phronesis-mcp drift_is_one_tool 2>&1 | tail -20`
Expected: FAIL — `get_drift` not registered.

- [ ] **Step 3: Write the implementation**

In `crates/phronesis-mcp/src/server_params.rs`, delete `GetClaudeMdDriftParams`,
`GetMemoryDriftParams`, and `GetWikiDriftParams`, and add:

```rust
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetDriftParams {
    /// Which corpus to scan: "claude_md", "memory", "wiki", "code", or
    /// "all" (default).
    #[serde(default)]
    pub source: Option<String>,
    /// Max items per source. Default 5, clamped to 50.
    #[serde(default)]
    pub limit: Option<usize>,
    /// "json" (default) or "table".
    #[serde(default)]
    pub format: Option<String>,
    /// Override the auto-memory directory.
    #[serde(default)]
    pub memory_dir: Option<String>,
    /// Override the wiki decisions directory.
    #[serde(default)]
    pub wiki_dir: Option<String>,
}
```

Match the derive list and attribute style used by the neighbouring param
structs in that file — read one before writing this.

In `crates/phronesis-mcp/src/server.rs`, delete the three
`async fn get_claude_md_drift`, `get_memory_drift`, and `get_wiki_drift`
methods together with their `#[tool(...)]` attributes, and add:

```rust
    #[tool(
        description = "Detect drift between written guidance and enforced rules across every corpus: CLAUDE.md imperatives, Claude Code auto-memory entries, ADR decisions under .phronesis/wiki/decisions/, and (once SPEC-rule-staleness lands) rules naming code the graph no longer defines. Read-only, heuristic, no LLM call — output is a triage list, not ground truth. `source` selects one of \"claude_md\", \"memory\", \"wiki\", \"code\", or \"all\" (default). With \"all\" the response is a bounded summary: use a single source plus a higher `limit` for detail. A corpus that does not exist is reported as unavailable rather than failing the call. Optional: `limit` (default 5, max 50), `format` (\"json\" default or \"table\"), `memory_dir`, `wiki_dir`."
    )]
    async fn get_drift(
        &self,
        Parameters(params): Parameters<GetDriftParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::drift::{self, Source, SourceInputs};

        let root = security::project_root();

        let sources: Vec<Source> = match params.source.as_deref().unwrap_or("all") {
            "all" => Source::ALL.to_vec(),
            "claude_md" => vec![Source::ClaudeMd],
            "memory" => vec![Source::Memory],
            "wiki" => vec![Source::Wiki],
            "code" => vec![Source::Code],
            other => {
                return Err(Self::err(format!(
                    "unknown source {other:?} — expected one of: claude_md, memory, wiki, code, all"
                )));
            }
        };

        let memory_dir = params.memory_dir.as_deref().map(std::path::PathBuf::from);
        let wiki_dir = params.wiki_dir.as_deref().map(std::path::PathBuf::from);
        let inputs = SourceInputs {
            project_root: &root,
            claude_md: None,
            memory_dir: memory_dir.as_deref(),
            wiki_dir: wiki_dir.as_deref(),
        };

        let limit = params.limit.unwrap_or(drift::DEFAULT_LIMIT);
        let agg = drift::run_all(&sources, &inputs, limit);

        Self::log_event("get_drift", |e| {
            e.with("sources_present", agg.totals.sources_present as u64)
                .with("sources_missing", agg.totals.sources_missing as u64)
                .with("sources_errored", agg.totals.sources_errored as u64)
                .with("uncovered_total", agg.totals.uncovered_total as u64)
        });

        let body = if params.format.as_deref() == Some("table") {
            drift::render_table(&agg)
        } else {
            drift::render_json(&agg)
        };
        Ok(CallToolResult::success(vec![Content::text(body)]))
    }
```

Match the exact shape of `Self::log_event` and `Self::err` used by the
neighbouring tools — read `get_stats` in the same file before writing this,
and mirror its call style. Update the `use crate::server_params::{...}`
import list to drop the three removed structs and add `GetDriftParams`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p phronesis-mcp drift_is_one_tool 2>&1 | tail -20`
Expected: PASS.

Run: `cargo test --workspace 2>&1 | grep -E "^test result"`
Expected: 0 failed.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --workspace -- -D warnings
git add crates/phronesis-mcp/src
git commit -m "feat(mcp): replace three drift tools with get_drift(source)"
```

---

### Task 6: CLI

**Files:**
- Modify: `crates/phronesis-mcp/src/main.rs:147` (drop `alias = "drift"`)
- Modify: `crates/phronesis-mcp/src/main.rs` (add `Drift` command + handler)
- Test: `crates/phronesis-mcp/tests/cli_smoke.rs`

**Interfaces:**
- Consumes: Task 3 `run_all`, Task 4 renderers
- Produces: `phr-mcp drift [--source S] [--limit N] [--json]`

- [ ] **Step 1: Write the failing test**

Append to `crates/phronesis-mcp/tests/cli_smoke.rs`, matching the existing
helper style in that file for locating and invoking the binary:

```rust
#[test]
fn drift_defaults_to_all_sources_and_succeeds_on_a_bare_project() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = run_bin(&["drift", "--json"], dir.path());
    assert!(
        out.status.success(),
        "drift must not fail when corpora are absent: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let body = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&body).expect("valid json");
    assert_eq!(
        v["sources"].as_array().map(|a| a.len()),
        Some(4),
        "all four sources must be reported: {body}"
    );
}

#[test]
fn drift_rejects_an_unknown_source() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = run_bin(&["drift", "--source", "nope"], dir.path());
    assert!(!out.status.success(), "unknown source must fail");
}
```

Read `cli_smoke.rs` first and reuse its existing binary-invocation helper.
If it is named something other than `run_bin`, use the real name and
signature.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p phronesis-mcp --test cli_smoke drift_ 2>&1 | tail -20`
Expected: FAIL — unknown subcommand `drift`.

- [ ] **Step 3: Write the implementation**

In `crates/phronesis-mcp/src/main.rs`, change line 147 from:

```rust
    #[command(name = "claude-md-drift", alias = "drift")]
```

to:

```rust
    // `drift` is no longer an alias here: it is now the canonical
    // multi-source command below. Leaving the alias would silently widen
    // `phr-mcp drift` from one corpus to four.
    #[command(name = "claude-md-drift")]
```

Add a new `Drift` variant to the `Command` enum, following the doc-comment
and attribute style of its neighbours:

```rust
    /// Detect drift between written guidance and enforced rules across
    /// every corpus. Read-only; always exits 0 unless `--source` is
    /// invalid.
    Drift {
        /// claude_md | memory | wiki | code | all (default)
        #[arg(long, default_value = "all")]
        source: String,
        /// Max items per source (default 5, max 50).
        #[arg(long, default_value_t = 5)]
        limit: usize,
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
        /// Override the auto-memory directory.
        #[arg(long)]
        memory_dir: Option<PathBuf>,
        /// Override the wiki decisions directory.
        #[arg(long)]
        wiki_dir: Option<PathBuf>,
    },
```

Add the dispatch arm alongside the existing ones:

```rust
        Command::Drift {
            source,
            limit,
            json,
            memory_dir,
            wiki_dir,
        } => handle_drift(source, limit, json, memory_dir, wiki_dir),
```

Add the handler next to `handle_claude_md_drift`:

```rust
fn handle_drift(
    source: String,
    limit: usize,
    json: bool,
    memory_dir: Option<PathBuf>,
    wiki_dir: Option<PathBuf>,
) -> anyhow::Result<()> {
    use phronesis_mcp::drift::{self, Source, SourceInputs};

    let root = phronesis_mcp::security::project_root();
    let sources: Vec<Source> = match source.as_str() {
        "all" => Source::ALL.to_vec(),
        "claude_md" => vec![Source::ClaudeMd],
        "memory" => vec![Source::Memory],
        "wiki" => vec![Source::Wiki],
        "code" => vec![Source::Code],
        other => anyhow::bail!(
            "unknown source {other:?} — expected one of: claude_md, memory, wiki, code, all"
        ),
    };

    let inputs = SourceInputs {
        project_root: &root,
        claude_md: None,
        memory_dir: memory_dir.as_deref(),
        wiki_dir: wiki_dir.as_deref(),
    };
    let agg = drift::run_all(&sources, &inputs, limit);
    if json {
        println!("{}", drift::render_json(&agg));
    } else {
        print!("{}", drift::render_table(&agg));
    }
    Ok(())
}
```

Confirm the crate is imported in `main.rs` under the name the existing
handlers use (`phronesis_mcp::` or a local `use`), and match it.

Leave `claude-md-drift`, `memory-drift`, and `wiki-drift` subcommands and
their handlers exactly as they are.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p phronesis-mcp --test cli_smoke drift_ 2>&1 | tail -20`
Expected: PASS, 2 tests.

Run: `cargo test --workspace 2>&1 | grep -E "^test result"`
Expected: 0 failed.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --workspace -- -D warnings
git add crates/phronesis-mcp
git commit -m "feat(cli): add drift --source and free the drift alias"
```

---

### Task 7: Documentation sweep and the no-stale-names guard

**Files:**
- Modify: `crates/phronesis-mcp/src/init.rs` (durable template drift section)
- Modify: `crates/phronesis-mcp/CLAUDE.md`
- Modify: `crates/phronesis-mcp/README.md`
- Modify: `docs/loop-programming-guide.md`
- Test: `crates/phronesis-mcp/tests/cli_smoke.rs`

**Interfaces:**
- Consumes: everything above
- Produces: no code interfaces; a guard test

- [ ] **Step 1: Write the failing test**

Append to `crates/phronesis-mcp/tests/cli_smoke.rs`:

```rust
/// The three removed MCP tool names must not survive in any shipped
/// artifact. A dead tool name in `durable.md` is re-injected into the
/// model's context every session, so this is the one that matters most.
/// Spec and plan documents are exempt: they describe the migration.
#[test]
fn no_shipped_artifact_names_the_removed_drift_tools() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root");

    let checked = [
        repo.join("crates/phronesis-mcp/src/init.rs"),
        repo.join("crates/phronesis-mcp/CLAUDE.md"),
        repo.join("crates/phronesis-mcp/README.md"),
        repo.join("docs/loop-programming-guide.md"),
    ];

    for path in checked {
        let body = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        for gone in [
            "get_claude_md_drift",
            "get_memory_drift",
            "get_wiki_drift",
        ] {
            assert!(
                !body.contains(gone),
                "{} still names the removed MCP tool {gone}",
                path.display()
            );
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p phronesis-mcp --test cli_smoke no_shipped_artifact 2>&1 | tail -20`
Expected: FAIL — `init.rs` still names them.

- [ ] **Step 3: Update the documentation**

In `crates/phronesis-mcp/src/init.rs`, replace the `## Drift discipline`
section of `DEFAULT_DURABLE_MD` with:

```text
## Drift discipline

`get_drift(source)` surfaces guidance that no rule enforces — `source` is
`claude_md`, `memory`, `wiki`, or `all`. Run it when the user asks about
rules, memory, or project conventions, or says "remember X" / "make a
rule for X".

Scoring is token-overlap Jaccard with no semantic match, so output is
a triage list, not ground truth.
```

In `crates/phronesis-mcp/CLAUDE.md`, replace the three-tool block under
"Drift detection" with a single `get_drift` section documenting the
`source`, `limit`, `format`, `memory_dir`, and `wiki_dir` parameters, the
`all`-is-a-summary behavior, and that a missing corpus is reported rather
than raised. Keep the CLI alias lines, updating them to note they forward
to `drift --source X`. Add `cargo run -- drift` to the command list near
the existing drift entries.

In `crates/phronesis-mcp/README.md` and `docs/loop-programming-guide.md`,
replace every mention of the three MCP tool names with `get_drift`. Leave
CLI subcommand names (`phr-mcp wiki-drift`) intact — they still exist.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p phronesis-mcp --test cli_smoke no_shipped_artifact 2>&1 | tail -20`
Expected: PASS.

Run: `cargo test --workspace 2>&1 | grep -E "^test result"`
Expected: 0 failed.

Verify the durable template still fits its budget:
Run: `cargo run -- init --dry-run 2>&1 | head -20`
Expected: no error.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --workspace -- -D warnings
git add crates docs
git commit -m "docs(drift): point every shipped artifact at get_drift"
```

---

### Task 8: Reconcile the spec

**Files:**
- Modify: `docs/specs/SPEC-drift-consolidation.md`

**Interfaces:**
- Consumes: nothing
- Produces: nothing

- [ ] **Step 1: Apply the corrections**

Two things in the spec turned out wrong once the real types were read.
Update §2.1 to split the axes:

- `Verdict` covers coverage only: `Covered`, `LikelyCovered`, `Uncovered`,
  `Superseded`, `Moved`, `Stale`. Remove `ActionableUncovered`,
  `AmbientUncovered`, and `Personal` from it.
- Add a `Category` enum (`Actionable`, `Ambient`, `Personal`) on
  `DriftItem` as `Option<Category>`, and state that it is orthogonal to
  `Verdict`: `memory_drift::Bucket` classifies what kind of guidance an
  entry is, while coverage is `similarity` vs `coverage_threshold`. An
  `Actionable` entry can be covered, which the fused enum could not
  represent.

Update §3 to state scores are `f32`, matching
`claude_md_drift::DriftItem::similarity` and the other two sources. The
spec currently says `f64`.

Update §2.1's `uncovered_count` paragraph: it excludes `Family::Covered`
and `Family::Superseded`; `Personal` is now a `Category`, not a verdict,
so the exclusion is expressed as "counts `Uncovered` and `Broken`".

- [ ] **Step 2: Verify no other section contradicts the change**

Run: `grep -n "ActionableUncovered\|AmbientUncovered\|f64" docs/specs/SPEC-drift-consolidation.md`
Expected: no matches.

- [ ] **Step 3: Commit**

```bash
git add docs/specs/SPEC-drift-consolidation.md
git commit -m "docs(specs): split coverage from category in the drift envelope"
```

---

## Self-Review

**Spec coverage:**

| Spec section | Task |
|---|---|
| §1 source registry, enum dispatch | 3 |
| §1.1 absent corpus is data | 3 |
| §2 envelope types | 1 |
| §2.1 verdict union + family | 1 (corrected: split into Verdict + Category) |
| §3 three evidence variants | 1, 2 |
| §4 renderers | 4 |
| §5.1 `all` is a bounded summary | 3 (`apply_limit`), 4 (truncation notice) |
| §5.2 one source failing does not fail the call | 3 |
| §6.1 durable.md migration | **not covered — see below** |
| §6.2 surface sweep | 7 |
| §6.3 `drift` alias removal | 6 |
| §7 module boundaries | 1, 3, 4 |
| §8 testing | every task |

**Gap found and accepted:** §6.1's schema-marker migration for *existing*
projects' `durable.md` is not implemented by this plan. Task 7 updates the
shipped template, which covers new projects only. Existing projects keep a
`durable.md` naming three dead tools, re-injected every session.

This is deliberate: the migration needs a `durable.md` schema marker, a
section-matcher that refuses to touch edited files, and a backup path —
enough surface to deserve its own plan, and it is safe to ship after the
tool change rather than with it. **It must not be forgotten**, so it is
recorded here as the required follow-up, and the spec's §6.1 stays
authoritative for it.

**Placeholder scan:** no TBD/TODO. Three steps direct the implementer to
read neighbouring code before writing (`server_params.rs` derives,
`cli_smoke.rs` helper name, `wiki::Decision` field names) rather than
guessing at signatures this plan cannot verify — these are instructions to
verify, not placeholders.

**Type consistency:** `into_items` has the same signature in all three
adapters. `SourceInputs` fields match between Tasks 3, 5, and 6.
`apply_limit`/`clamp_limit` are defined in Task 3 and used only there.
`Family` derives `Ord` in Task 1 because Task 3 uses it as a `BTreeMap`
key and Task 3 sorts on it. Scores are `f32` throughout.

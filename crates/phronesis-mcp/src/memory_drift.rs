//! Detect drift between Claude Code's auto-memory store and the phronesis
//! rule pack / durable directives file.
//!
//! Claude Code maintains a per-project memory directory under
//! `~/.claude/projects/<encoded-cwd>/memory/`. Each `.md` file inside has
//! a small YAML frontmatter with `name`, `description`, and a
//! `metadata.type` of `feedback`, `project`, `user`, or `reference`.
//!
//! Memories of type `feedback` or `project` are candidates for porting:
//! the first as **actionable** phronesis rules that fire at the moment of
//! action, the second as **ambient** prose in `.phronesis/durable.md`
//! that is re-injected each turn. Memories of type `user` or `reference`
//! stay where they are — personal to the operator.
//!
//! This module walks the memory directory, classifies each entry, and
//! scores actionable/ambient entries against the current rule pack and
//! durable.md by token overlap (Jaccard). Entries without a confident
//! match are surfaced as drift candidates that should be ported.
//!
//! Heuristic by design — no LLM call. The output is a triage list, not
//! authoritative ground truth.

use crate::rules_file::{self, DiskRule, RulesFile};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Classification bucket for a memory entry, per `SPEC-memory-to-rules.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bucket {
    /// Names a specific tool / command / code shape. Should become a rule.
    Actionable,
    /// Project-shareable ambient guidance. Should live in durable.md.
    Ambient,
    /// Personal preference about voice / role / cross-project habits.
    /// Stays in MEMORY.md.
    Personal,
}

/// A single memory entry parsed from disk.
#[derive(Debug, Clone)]
pub struct MemoryEntry {
    pub file_path: PathBuf,
    pub name: String,
    pub description: String,
    pub memory_type: String,
    pub body: String,
}

/// Drift assessment for one memory entry.
#[derive(Debug, Clone)]
pub struct DriftItem {
    pub entry: MemoryEntry,
    pub bucket: Bucket,
    pub best_match: Option<MatchedTarget>,
    pub similarity: f32,
}

/// A target the memory might already be covered by — either a phronesis
/// rule or a paragraph in durable.md.
#[derive(Debug, Clone)]
pub enum MatchedTarget {
    Rule {
        rule_id: String,
        shared_terms: Vec<String>,
    },
    DurableParagraph {
        excerpt: String,
        shared_terms: Vec<String>,
    },
}

#[derive(Debug, Clone)]
pub struct DriftReport {
    pub memory_dir: String,
    pub rules_path: String,
    pub durable_md_path: String,
    pub items: Vec<DriftItem>,
    /// Below this, the entry is considered uncovered.
    pub coverage_threshold: f32,
}

#[derive(Debug, thiserror::Error)]
pub enum DriftError {
    #[error("memory directory not found at {0}")]
    MemoryDirMissing(String),
    #[error("failed to read memory file {path}: {source}")]
    MemoryFileIo {
        path: String,
        source: std::io::Error,
    },
    #[error("failed to read rules file: {0}")]
    RulesIo(String),
    #[error("failed to walk memory directory: {0}")]
    DirWalk(#[from] std::io::Error),
}

const COVERAGE_THRESHOLD: f32 = 0.15;

const STOPWORDS: &[&str] = &[
    "the", "a", "an", "of", "to", "for", "in", "on", "at", "by", "is", "are", "be", "and", "or",
    "but", "with", "as", "it", "its", "this", "that", "you", "your", "we", "our", "i", "me",
];

/// Top-level entry. Walks the memory directory for `project_root`,
/// classifies each entry, scores it against the project's rules and
/// durable.md, and returns the report.
pub fn run(project_root: &Path) -> Result<DriftReport, DriftError> {
    let memory_dir = default_memory_dir(project_root);
    run_with_dir(project_root, &memory_dir)
}

/// Same as [`run`], but with an explicit memory-directory path —
/// useful for tests and for callers who don't store memory in the
/// default Claude Code location.
pub fn run_with_dir(project_root: &Path, memory_dir: &Path) -> Result<DriftReport, DriftError> {
    if !memory_dir.exists() {
        return Err(DriftError::MemoryDirMissing(
            memory_dir.display().to_string(),
        ));
    }

    let entries = read_memory_dir(memory_dir)?;
    let rules_path = rules_file::default_path(project_root);
    let rules = rules_file::read(&rules_path).map_err(|e| DriftError::RulesIo(e.to_string()))?;
    let durable_md_path = project_root.join(".phronesis").join("durable.md");
    let durable_md = std::fs::read_to_string(&durable_md_path).unwrap_or_default();

    let items = entries
        .into_iter()
        .map(|e| score_entry(e, &rules, &durable_md))
        .collect();

    Ok(DriftReport {
        memory_dir: memory_dir.display().to_string(),
        rules_path: rules_path.display().to_string(),
        durable_md_path: durable_md_path.display().to_string(),
        items,
        coverage_threshold: COVERAGE_THRESHOLD,
    })
}

/// Compute the default memory directory for `project_root` using the
/// Claude Code encoding (`/` in the absolute path is replaced with `-`,
/// with a leading `-` from the root slash).
///
/// Returns `<home>/.claude/projects/<encoded>/memory/`.
pub fn default_memory_dir(project_root: &Path) -> PathBuf {
    let home = dirs::home_dir().unwrap_or_default();
    let abs = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let encoded = abs.to_string_lossy().replace('/', "-");
    home.join(".claude")
        .join("projects")
        .join(encoded)
        .join("memory")
}

fn read_memory_dir(memory_dir: &Path) -> Result<Vec<MemoryEntry>, DriftError> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(memory_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        // Skip the index file — it is metadata about the store, not a
        // memory entry of its own.
        if path.file_name().and_then(|n| n.to_str()) == Some("MEMORY.md") {
            continue;
        }
        let raw = std::fs::read_to_string(&path).map_err(|source| DriftError::MemoryFileIo {
            path: path.display().to_string(),
            source,
        })?;
        if let Some(parsed) = parse_memory_file(&path, &raw) {
            out.push(parsed);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Parse a memory `.md` file: extract `name`, `description`, and
/// `metadata.type` from the frontmatter; the rest is body. Returns
/// `None` if the file has no frontmatter or no `name` — those are
/// considered malformed for our purposes.
fn parse_memory_file(path: &Path, raw: &str) -> Option<MemoryEntry> {
    let trimmed = raw.trim_start();
    let rest = trimmed.strip_prefix("---\n")?;
    let (frontmatter, body) = rest.split_once("\n---")?;
    let body = body.trim_start_matches(['\n', '\r']);

    let mut name = String::new();
    let mut description = String::new();
    let mut memory_type = String::new();

    // Track whether we're inside a "metadata:" subsection so we read
    // `  type: feedback` correctly. The frontmatter we emit is shallow
    // enough that a tiny line-keyed parser is sufficient — we deliberately
    // avoid a YAML dependency for two field reads.
    let mut in_metadata = false;
    for line in frontmatter.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let indented = line.starts_with(' ') || line.starts_with('\t');
        if !indented {
            in_metadata = false;
        }
        if let Some(rest) = line.strip_prefix("name:") {
            name = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("description:") {
            description = rest.trim().to_string();
        } else if line.starts_with("metadata:") {
            in_metadata = true;
        } else if in_metadata {
            let trimmed = line.trim_start();
            if let Some(rest) = trimmed.strip_prefix("type:") {
                memory_type = rest.trim().to_string();
            }
        }
    }

    if name.is_empty() {
        return None;
    }

    Some(MemoryEntry {
        file_path: path.to_path_buf(),
        name,
        description,
        memory_type,
        body: body.to_string(),
    })
}

fn classify(entry: &MemoryEntry) -> Bucket {
    match entry.memory_type.as_str() {
        "feedback" => Bucket::Actionable,
        "project" => {
            // Project memories that name a specific tool / command count
            // as actionable; otherwise ambient.
            if mentions_tool_call(&entry.body) {
                Bucket::Actionable
            } else {
                Bucket::Ambient
            }
        }
        "user" | "reference" => Bucket::Personal,
        _ => Bucket::Personal,
    }
}

/// Heuristic: does the body mention a specific command or tool name
/// that suggests it's actionable at hook time?
fn mentions_tool_call(body: &str) -> bool {
    const TOOL_HINTS: &[&str] = &[
        "git commit",
        "git push",
        "git rebase",
        "git merge",
        "cargo ",
        "npm ",
        "yarn ",
        "pnpm ",
        "rustfmt",
        "clippy",
        "gh pr",
        "gh issue",
        "Edit",
        "Write",
        "Bash",
        "MultiEdit",
    ];
    let lower = body.to_ascii_lowercase();
    TOOL_HINTS
        .iter()
        .any(|hint| lower.contains(&hint.to_ascii_lowercase()))
}

fn score_entry(entry: MemoryEntry, rules: &RulesFile, durable_md: &str) -> DriftItem {
    let bucket = classify(&entry);

    // Personal memories don't drift against rules / durable.md — they
    // belong where they are. Skip the scoring.
    if bucket == Bucket::Personal {
        return DriftItem {
            entry,
            bucket,
            best_match: None,
            similarity: 0.0,
        };
    }

    let entry_tokens = meaningful_tokens(&format!("{} {}", entry.description, entry.body));
    if entry_tokens.is_empty() {
        return DriftItem {
            entry,
            bucket,
            best_match: None,
            similarity: 0.0,
        };
    }

    let mut best: Option<(f32, MatchedTarget)> = None;

    // Score against every rule.
    for rule in &rules.rules {
        let rule_text = rule_textual_blob(rule);
        let rule_tokens = meaningful_tokens(&rule_text);
        if rule_tokens.is_empty() {
            continue;
        }
        let (jaccard, shared) = jaccard_with_shared(&entry_tokens, &rule_tokens);
        if jaccard > 0.0 {
            let candidate = (
                jaccard,
                MatchedTarget::Rule {
                    rule_id: rule.id.clone(),
                    shared_terms: shared,
                },
            );
            best = better(best, candidate);
        }
    }

    // Score against each non-empty paragraph in durable.md.
    if bucket == Bucket::Ambient {
        for para in durable_md.split("\n\n") {
            let para = para.trim();
            if para.is_empty() {
                continue;
            }
            let para_tokens = meaningful_tokens(para);
            if para_tokens.is_empty() {
                continue;
            }
            let (jaccard, shared) = jaccard_with_shared(&entry_tokens, &para_tokens);
            if jaccard > 0.0 {
                let excerpt = para.chars().take(80).collect::<String>();
                let candidate = (
                    jaccard,
                    MatchedTarget::DurableParagraph {
                        excerpt,
                        shared_terms: shared,
                    },
                );
                best = better(best, candidate);
            }
        }
    }

    match best {
        Some((similarity, target)) => DriftItem {
            entry,
            bucket,
            best_match: Some(target),
            similarity,
        },
        None => DriftItem {
            entry,
            bucket,
            best_match: None,
            similarity: 0.0,
        },
    }
}

fn better(
    current: Option<(f32, MatchedTarget)>,
    candidate: (f32, MatchedTarget),
) -> Option<(f32, MatchedTarget)> {
    match current {
        None => Some(candidate),
        Some((cur_score, _)) if candidate.0 > cur_score => Some(candidate),
        other => other,
    }
}

fn jaccard_with_shared(a: &HashSet<String>, b: &HashSet<String>) -> (f32, Vec<String>) {
    let shared: Vec<String> = a.iter().filter(|t| b.contains(*t)).cloned().collect();
    if shared.is_empty() {
        return (0.0, shared);
    }
    let union: HashSet<&String> = a.iter().chain(b.iter()).collect();
    (shared.len() as f32 / union.len() as f32, shared)
}

fn rule_textual_blob(rule: &DiskRule) -> String {
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

fn meaningful_tokens(s: &str) -> HashSet<String> {
    let stops: HashSet<&str> = STOPWORDS.iter().copied().collect();
    s.to_ascii_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 1 && !stops.contains(t))
        .map(String::from)
        .collect()
}

/// Render the report as a terminal table.
pub fn render_table(report: &DriftReport) -> String {
    let mut out = String::new();
    out.push_str(&format!("Memory directory: {}\n", report.memory_dir));
    out.push_str(&format!("Rules:            {}\n", report.rules_path));
    out.push_str(&format!("Durable:          {}\n\n", report.durable_md_path));

    if report.items.is_empty() {
        out.push_str("No memory entries found.\n");
        return out;
    }

    out.push_str(&format!(
        "{:<32}  {:<11}  {:<10}  Suggestion\n",
        "Memory", "Bucket", "Similarity"
    ));
    out.push_str(&format!(
        "{:-<32}  {:-<11}  {:-<10}  {:-<40}\n",
        "", "", "", ""
    ));

    for item in &report.items {
        let bucket_str = match item.bucket {
            Bucket::Actionable => "actionable",
            Bucket::Ambient => "ambient",
            Bucket::Personal => "personal",
        };
        let suggestion = suggestion_for(item, report.coverage_threshold);
        out.push_str(&format!(
            "{:<32}  {:<11}  {:<10.2}  {}\n",
            truncate(&item.entry.name, 32),
            bucket_str,
            item.similarity,
            suggestion,
        ));
    }
    out
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let head: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{}…", head)
    }
}

fn suggestion_for(item: &DriftItem, threshold: f32) -> String {
    match item.bucket {
        Bucket::Personal => "stays personal".to_string(),
        Bucket::Actionable => {
            if item.similarity >= threshold {
                match &item.best_match {
                    Some(MatchedTarget::Rule { rule_id, .. }) => {
                        format!("→ already covered by rule {}", rule_id)
                    }
                    Some(MatchedTarget::DurableParagraph { .. }) => {
                        "→ overlap with durable.md (rule still preferable)".to_string()
                    }
                    None => "→ port to a rule".to_string(),
                }
            } else {
                "→ port to a rule (no match in rules.json)".to_string()
            }
        }
        Bucket::Ambient => {
            if item.similarity >= threshold {
                match &item.best_match {
                    Some(MatchedTarget::DurableParagraph { .. }) => {
                        "→ already covered by durable.md".to_string()
                    }
                    Some(MatchedTarget::Rule { rule_id, .. }) => {
                        format!("→ overlap with rule {}", rule_id)
                    }
                    None => "→ port to durable.md".to_string(),
                }
            } else {
                "→ port to durable.md (no match)".to_string()
            }
        }
    }
}

/// Render the report as JSON. Stable key order for diffing.
pub fn render_json(report: &DriftReport) -> String {
    let items: Vec<serde_json::Value> = report
        .items
        .iter()
        .map(|item| {
            let bucket = match item.bucket {
                Bucket::Actionable => "actionable",
                Bucket::Ambient => "ambient",
                Bucket::Personal => "personal",
            };
            let best_match = item.best_match.as_ref().map(|m| match m {
                MatchedTarget::Rule {
                    rule_id,
                    shared_terms,
                } => serde_json::json!({
                    "kind": "rule",
                    "rule_id": rule_id,
                    "shared_terms": shared_terms,
                }),
                MatchedTarget::DurableParagraph {
                    excerpt,
                    shared_terms,
                } => serde_json::json!({
                    "kind": "durable_paragraph",
                    "excerpt": excerpt,
                    "shared_terms": shared_terms,
                }),
            });
            serde_json::json!({
                "name": item.entry.name,
                "description": item.entry.description,
                "memory_type": item.entry.memory_type,
                "bucket": bucket,
                "similarity": item.similarity,
                "best_match": best_match,
                "file": item.entry.file_path.display().to_string(),
            })
        })
        .collect();

    serde_json::to_string_pretty(&serde_json::json!({
        "memory_dir": report.memory_dir,
        "rules_path": report.rules_path,
        "durable_md_path": report.durable_md_path,
        "coverage_threshold": report.coverage_threshold,
        "items": items,
    }))
    .unwrap_or_else(|_| String::from("{}"))
}

/// Emit a draft phronesis rule for an actionable memory that does not
/// yet have a confident match in the rule pack. Returns `None` for
/// items where suggestion does not apply.
pub fn suggest_rule(item: &DriftItem) -> Option<String> {
    if item.bucket != Bucket::Actionable || item.similarity >= COVERAGE_THRESHOLD {
        return None;
    }
    let rule_id = format!("memory-{}", item.entry.name);
    let message = item.entry.description.clone();
    let suggestion = serde_json::json!({
        "id": rule_id,
        "phase": "pre",
        "priority": 5,
        "conditions": [
            { "predicate": "new_content_contains", "args": ["// TODO: pick a substring or command to match"] }
        ],
        "actions": [{
            "action_type": "constraint_warning",
            "params": [message]
        }],
        "_source": {
            "memory": item.entry.name.clone(),
            "file": item.entry.file_path.display().to_string()
        }
    });
    Some(serde_json::to_string_pretty(&suggestion).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_memory(dir: &Path, file: &str, content: &str) {
        fs::write(dir.join(file), content).expect("write memory file");
    }

    fn empty_rules_path(project_root: &Path) {
        // Create an empty rules.json so rules_file::read succeeds.
        let phr_dir = project_root.join(".phronesis");
        fs::create_dir_all(&phr_dir).expect("create .phronesis");
        fs::write(phr_dir.join("rules.json"), r#"{"rules":[]}"#).expect("write rules.json");
    }

    #[test]
    fn parses_well_formed_memory_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("feedback_test.md");
        let content = "---\nname: foo-bar\ndescription: A test feedback\nmetadata:\n  type: feedback\n---\n\nBody text here.\n";
        fs::write(&path, content).unwrap();

        let parsed = parse_memory_file(&path, content).expect("should parse");
        assert_eq!(parsed.name, "foo-bar");
        assert_eq!(parsed.description, "A test feedback");
        assert_eq!(parsed.memory_type, "feedback");
        assert!(parsed.body.contains("Body text here."));
    }

    #[test]
    fn returns_none_for_file_without_frontmatter() {
        let path = Path::new("/tmp/fake.md");
        let parsed = parse_memory_file(path, "no frontmatter here\n");
        assert!(parsed.is_none());
    }

    #[test]
    fn classifies_feedback_as_actionable() {
        let entry = MemoryEntry {
            file_path: PathBuf::from("/tmp/x.md"),
            name: "x".into(),
            description: "".into(),
            memory_type: "feedback".into(),
            body: "".into(),
        };
        assert_eq!(classify(&entry), Bucket::Actionable);
    }

    #[test]
    fn classifies_user_as_personal() {
        let entry = MemoryEntry {
            file_path: PathBuf::from("/tmp/x.md"),
            name: "x".into(),
            description: "".into(),
            memory_type: "user".into(),
            body: "".into(),
        };
        assert_eq!(classify(&entry), Bucket::Personal);
    }

    #[test]
    fn classifies_project_with_tool_mention_as_actionable() {
        let entry = MemoryEntry {
            file_path: PathBuf::from("/tmp/x.md"),
            name: "x".into(),
            description: "".into(),
            memory_type: "project".into(),
            body: "Before any git commit, do X.".into(),
        };
        assert_eq!(classify(&entry), Bucket::Actionable);
    }

    #[test]
    fn classifies_project_without_tool_mention_as_ambient() {
        let entry = MemoryEntry {
            file_path: PathBuf::from("/tmp/x.md"),
            name: "x".into(),
            description: "".into(),
            memory_type: "project".into(),
            body: "This project uses card-game framing throughout.".into(),
        };
        assert_eq!(classify(&entry), Bucket::Ambient);
    }

    #[test]
    fn skips_memory_md_index() {
        let dir = TempDir::new().unwrap();
        let memory_dir = dir.path().join("memory");
        fs::create_dir(&memory_dir).unwrap();
        write_memory(&memory_dir, "MEMORY.md", "- index entry\n");
        write_memory(
            &memory_dir,
            "feedback_x.md",
            "---\nname: x\ndescription: x\nmetadata:\n  type: feedback\n---\n\nbody",
        );

        let entries = read_memory_dir(&memory_dir).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "x");
    }

    #[test]
    fn run_with_dir_surfaces_uncovered_actionable_memory() {
        let project_root = TempDir::new().unwrap();
        empty_rules_path(project_root.path());
        let memory_dir = project_root.path().join("memory");
        fs::create_dir(&memory_dir).unwrap();
        write_memory(
            &memory_dir,
            "feedback_commit_timing.md",
            "---\nname: commit-timing\ndescription: Do not git commit during business hours.\nmetadata:\n  type: feedback\n---\n\nBody.\n",
        );

        let report = run_with_dir(project_root.path(), &memory_dir).unwrap();
        assert_eq!(report.items.len(), 1);
        let item = &report.items[0];
        assert_eq!(item.bucket, Bucket::Actionable);
        // No matching rule, so similarity is 0.
        assert_eq!(item.similarity, 0.0);
    }

    #[test]
    fn suggest_rule_emits_template_for_uncovered_actionable() {
        let entry = MemoryEntry {
            file_path: PathBuf::from("/tmp/feedback_x.md"),
            name: "commit-timing".into(),
            description: "Block commits during business hours.".into(),
            memory_type: "feedback".into(),
            body: "git commit during M-F 9-5 should refuse.".into(),
        };
        let item = DriftItem {
            entry,
            bucket: Bucket::Actionable,
            best_match: None,
            similarity: 0.0,
        };
        let suggestion = suggest_rule(&item).expect("should emit a suggestion");
        assert!(suggestion.contains("memory-commit-timing"));
        assert!(suggestion.contains("Block commits during business hours."));
    }

    #[test]
    fn suggest_rule_returns_none_for_covered() {
        let entry = MemoryEntry {
            file_path: PathBuf::from("/tmp/feedback_x.md"),
            name: "x".into(),
            description: "".into(),
            memory_type: "feedback".into(),
            body: "".into(),
        };
        let item = DriftItem {
            entry,
            bucket: Bucket::Actionable,
            best_match: Some(MatchedTarget::Rule {
                rule_id: "some-rule".into(),
                shared_terms: vec!["x".into()],
            }),
            similarity: 0.5,
        };
        assert!(suggest_rule(&item).is_none());
    }

    #[test]
    fn render_table_includes_bucket_labels() {
        let project_root = TempDir::new().unwrap();
        empty_rules_path(project_root.path());
        let memory_dir = project_root.path().join("memory");
        fs::create_dir(&memory_dir).unwrap();
        write_memory(
            &memory_dir,
            "user_role.md",
            "---\nname: role\ndescription: senior engineer\nmetadata:\n  type: user\n---\n\nbody",
        );
        write_memory(
            &memory_dir,
            "feedback_x.md",
            "---\nname: x\ndescription: x\nmetadata:\n  type: feedback\n---\n\ngit push during business hours",
        );

        let report = run_with_dir(project_root.path(), &memory_dir).unwrap();
        let table = render_table(&report);
        assert!(table.contains("personal"));
        assert!(table.contains("actionable"));
    }

    #[test]
    fn render_json_is_valid_json() {
        let project_root = TempDir::new().unwrap();
        empty_rules_path(project_root.path());
        let memory_dir = project_root.path().join("memory");
        fs::create_dir(&memory_dir).unwrap();
        write_memory(
            &memory_dir,
            "feedback_x.md",
            "---\nname: x\ndescription: x\nmetadata:\n  type: feedback\n---\n\ngit commit",
        );
        let report = run_with_dir(project_root.path(), &memory_dir).unwrap();
        let json = render_json(&report);
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("render_json must emit valid JSON");
        assert!(parsed["items"].is_array());
    }
}

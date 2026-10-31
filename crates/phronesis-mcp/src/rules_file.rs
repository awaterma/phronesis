//! Read/write the `.phronesis/rules.json` format shared by the MCP server and
//! the hook subcommands. The disk format carries an extra `phase` field per
//! rule (`"pre"` or `"post"`) that the in-memory `Rule` struct lacks.
//!
//! Parsing, validation, OR expansion, and disk conversions are shared with
//! `phronesis-governance` (`phronesis_governance::policy`): the types and
//! pure functions below are compatibility re-exports, and this module keeps
//! only the filesystem surfaces (read/read_source, atomic write, merge) plus
//! thin wrappers that preserve this crate's public signatures (`String`
//! check errors, `anyhow` unfold errors) and render the structured
//! governance diagnostics as the exact legacy stderr warnings.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::anyhow;
use phr::Rule;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use phronesis_governance::policy::{
    ACTION_VERBS, DiskAction, DiskCondition, DiskRule, RULE_PHASES, RulesFile, WhenClause,
    base_rule_id, rule_from_disk, rule_to_disk,
};
use phronesis_governance::policy::{
    PolicyDiagnostic, PolicyError, SourceRule as GovSourceRule, parse_source,
    unfold_or_with_diagnostics,
};

/// Legacy check-helper wrappers: same `Result<(), String>` signatures and
/// byte-identical messages (the governance `RuleCheckError` display text is
/// the exact message string this crate has always produced).
pub fn check_action_type(rule_id: &str, field: &str, value: &str) -> Result<(), String> {
    phronesis_governance::policy::check_action_type(rule_id, field, value).map_err(|e| e.0)
}

pub fn check_conditions_nonempty(rule_id: &str, field: &str, len: usize) -> Result<(), String> {
    phronesis_governance::policy::check_conditions_nonempty(rule_id, field, len).map_err(|e| e.0)
}

pub fn check_single_action(rule_id: &str, field: &str, len: usize) -> Result<(), String> {
    phronesis_governance::policy::check_single_action(rule_id, field, len).map_err(|e| e.0)
}

/// Expand a SourceRule's OR clauses into flat, OR-free DiskRules via
/// disjunctive-normal-form (DNF) expansion. Each OR position contributes
/// one alternative per (flattened) branch; the cartesian product across
/// all positions yields the output rules. Child ids are suffixed
/// deterministically: `#or0`, `#or1`, or `#or0-or1` for multi-position
/// products. A single-product result (no OR, or a single-element OR)
/// retains the original id unchanged. The logic is
/// [`phronesis_governance::policy::unfold_or`]; this wrapper keeps the
/// legacy `anyhow` error (carrying the bare message) and re-renders the
/// OR-expansion notice on stderr exactly as before.
pub fn unfold_or(source: &SourceRule) -> anyhow::Result<Vec<DiskRule>> {
    let (rules, diagnostics) = unfold_or_with_diagnostics(&GovSourceRule::from(source.clone()))
        .map_err(|e| match &e {
            PolicyError::Unfold { message, .. } => anyhow!(message.clone()),
            other => anyhow!(other.to_string()),
        })?;
    render_legacy_warnings(&diagnostics, Path::new(""));
    Ok(rules)
}

/// Render structured governance diagnostics as the exact legacy stderr
/// warnings: duplicate-rule notices carried a
/// `phronesis: warning: <path> ` prefix, OR-expansion notices a bare
/// `phronesis: ` one. `path` is empty for callers outside a file read.
fn render_legacy_warnings(diagnostics: &[PolicyDiagnostic], path: &Path) {
    for diagnostic in diagnostics {
        match diagnostic {
            PolicyDiagnostic::DuplicateIdenticalRule { .. } => {
                eprintln!(
                    "phronesis: warning: {} {}",
                    path.display(),
                    diagnostic.message()
                );
            }
            PolicyDiagnostic::LargeOrExpansion { .. } => {
                eprintln!("phronesis: {}", diagnostic.message());
            }
        }
    }
}

#[derive(Debug, Error)]
pub enum RulesFileError {
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("rules file at {path} is malformed: {source}")]
    Malformed {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("serialization error: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("rules file at {path} is invalid: {message}")]
    Invalid { path: String, message: String },
    #[error("rules file at {path} could not be expanded: {message}")]
    Unfold { path: String, message: String },
}

impl RulesFileError {
    /// The file this error is about, when it names one.
    pub fn path(&self) -> Option<&str> {
        match self {
            RulesFileError::Io { path, .. }
            | RulesFileError::Malformed { path, .. }
            | RulesFileError::Invalid { path, .. }
            | RulesFileError::Unfold { path, .. } => Some(path),
            RulesFileError::Serialize(_) => None,
        }
    }
}

/// Thin compatibility wrapper over the governance
/// [`phronesis_governance::policy::SourceRule`]: identical public fields and
/// identical serde behavior (parsing, validation, and serialization are
/// delegated — nothing is duplicated here), so this crate keeps its
/// historical public surface. The one behavioral hold-out is
/// [`SourceRule::synthetic_tagger`], which still returns
/// `Result<Self, RulesFileError>` with the legacy wording.
#[derive(Debug, Clone)]
pub struct SourceRule {
    pub id: String,
    pub phase: String,
    pub priority: i32,
    pub when: Vec<WhenClause>,
    pub then: DiskAction,
    pub silent: Option<bool>,
    pub audit: Option<bool>,
    pub doc_excepted: Option<bool>,
    /// Disable code-symbol binding for rules that intentionally name foreign
    /// or removed referents.
    pub binds: Option<bool>,
}

impl From<SourceRule> for GovSourceRule {
    fn from(rule: SourceRule) -> Self {
        GovSourceRule {
            id: rule.id,
            phase: rule.phase,
            priority: rule.priority,
            when: rule.when,
            then: rule.then,
            silent: rule.silent,
            audit: rule.audit,
            doc_excepted: rule.doc_excepted,
            binds: rule.binds,
        }
    }
}

impl From<GovSourceRule> for SourceRule {
    fn from(rule: GovSourceRule) -> Self {
        SourceRule {
            id: rule.id,
            phase: rule.phase,
            priority: rule.priority,
            when: rule.when,
            then: rule.then,
            silent: rule.silent,
            audit: rule.audit,
            doc_excepted: rule.doc_excepted,
            binds: rule.binds,
        }
    }
}

impl Serialize for SourceRule {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        GovSourceRule::from(self.clone()).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SourceRule {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        GovSourceRule::deserialize(deserializer).map(Self::from)
    }
}

impl SourceRule {
    /// Build a `SourceRule` whose `then` action is the sentinel `tag` verb
    /// and whose message is the tag itself. The journey tagger uses this
    /// constructor to ride the existing rule-firing path without any new
    /// matching code: every tagger compiles into one or more flat `DiskRule`s
    /// via `unfold_or`, loads into a throwaway `ReteNetwork`, and emits a
    /// `tag`-action consequence whose `message` is the tag name when its
    /// `when` matches.
    ///
    /// Errors bubble back as `RulesFileError::Unfold` carrying the serde
    /// parse error — keeps the journey config malformed-paths surface a
    /// single, named error variant. Construction is delegated to the
    /// governance core; only the error mapping stays here.
    pub fn synthetic_tagger(tag: &str, when: &[serde_json::Value]) -> Result<Self, RulesFileError> {
        GovSourceRule::synthetic_tagger(tag, when)
            .map(Self::from)
            .map_err(|error| match error {
                PolicyError::Unfold { source_id, message } => RulesFileError::Unfold {
                    path: source_id,
                    message,
                },
                // synthetic_tagger cannot produce any other variant; if one
                // ever does, fail closed onto the same legacy variant.
                other => RulesFileError::Unfold {
                    path: String::new(),
                    message: other.to_string(),
                },
            })
    }
}

/// Project-relative location of the default rules file. `default_path` joins
/// this onto the project root; the layer resolver names the same relative
/// string so the root is joined exactly once and no root bytes are
/// round-tripped through a lossy display string.
pub const DEFAULT_RULES_RELATIVE_PATH: &str = ".phronesis/rules.json";

/// Default state of the rules file under the project root.
pub fn default_path(project_root: &Path) -> PathBuf {
    project_root.join(".phronesis").join("rules.json")
}

/// Read the rules file at `path`. Returns `Ok(RulesFile{rules:vec![]})` when the
/// file does not exist (the "no rules configured" case). Returns `Err` when the
/// file exists but is unreadable or malformed. Parsing and unfolding are
/// delegated to `phronesis_governance::policy`; the structured diagnostics
/// (identical-duplicate notices, large OR expansions) are re-rendered here as
/// the exact legacy stderr warnings, and unfold errors map back onto
/// [`RulesFileError::Unfold`] with the bare message.
pub fn read(path: &Path) -> Result<RulesFile, RulesFileError> {
    let sources = read_source(path)?;
    let mut flat = Vec::new();
    for sr in &sources {
        let (expanded, diagnostics) = unfold_or_with_diagnostics(&GovSourceRule::from(sr.clone()))
            .map_err(|error| RulesFileError::Unfold {
                path: path.display().to_string(),
                message: match &error {
                    PolicyError::Unfold { message, .. } => message.clone(),
                    other => other.to_string(),
                },
            })?;
        render_legacy_warnings(&diagnostics, path);
        flat.extend(expanded);
    }
    Ok(RulesFile { rules: flat })
}

/// Parse the file into OR-bearing `SourceRule`s without unfolding. Used by
/// migration (which preserves OR on disk). Parsing is delegated to
/// `phronesis_governance::policy::parse_source` with the displayed path as
/// the opaque source id; identical-duplicate notices re-render as the exact
/// legacy stderr warnings. The typed serde error behind
/// [`RulesFileError::Malformed`] is recovered by re-parsing rejected
/// content, which fails identically to the delegation's parse.
pub fn read_source(path: &Path) -> Result<Vec<SourceRule>, RulesFileError> {
    if !path.exists() {
        return Ok(vec![]);
    }
    let content = std::fs::read_to_string(path).map_err(|e| RulesFileError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    #[derive(Deserialize)]
    #[allow(dead_code)]
    struct Wrapper {
        rules: Vec<SourceRule>,
    }
    let parsed =
        parse_source(&content, &path.display().to_string()).map_err(|error| match error {
            PolicyError::Malformed { message, .. } => {
                match serde_json::from_str::<Wrapper>(&content) {
                    Err(source) => RulesFileError::Malformed {
                        path: path.display().to_string(),
                        source,
                    },
                    Ok(_) => RulesFileError::Invalid {
                        path: path.display().to_string(),
                        message,
                    },
                }
            }
            PolicyError::Invalid { message, .. } => RulesFileError::Invalid {
                path: path.display().to_string(),
                message,
            },
            // parse_source never unfolds, so these cannot occur; fail closed.
            PolicyError::Unfold { message, .. } => RulesFileError::Invalid {
                path: path.display().to_string(),
                message,
            },
        })?;
    render_legacy_warnings(&parsed.diagnostics, path);
    Ok(parsed.rules.into_iter().map(SourceRule::from).collect())
}

pub fn write_atomic(path: &Path, file: &RulesFile) -> Result<(), RulesFileError> {
    // Refuse rather than overwrite a file that does not load: carrying its
    // metadata forward is impossible, and replacing it would discard the
    // user's rules (plus the `.bak` rotation would lose the last good copy).
    let existing: HashMap<String, SourceRule> = read_source(path)?
        .into_iter()
        .map(|rule| (rule.id.clone(), rule))
        .collect();
    // Backstop: never write a duplicate id, which the loader would reject
    // and lock every hook. The last definition of an id wins, at the
    // position of its first.
    let mut deduped: Vec<&DiskRule> = Vec::with_capacity(file.rules.len());
    let mut position: HashMap<&str, usize> = HashMap::new();
    for rule in &file.rules {
        match position.get(rule.id.as_str()) {
            Some(&at) => deduped[at] = rule,
            None => {
                position.insert(rule.id.as_str(), deduped.len());
                deduped.push(rule);
            }
        }
    }
    let sources: Vec<SourceRule> = deduped
        .into_iter()
        .map(|rule| {
            let mut source = diskrule_to_source(rule);
            if let Some(previous) = existing.get(&rule.id) {
                source.binds = previous.binds;
                source.audit = source.audit.or(previous.audit);
                source.silent = source.silent.or(previous.silent);
                source.doc_excepted = source.doc_excepted.or(previous.doc_excepted);
            }
            source
        })
        .collect();
    write_source(path, &sources)
}

/// Write OR-bearing SourceRules to disk in v2 shape. Used by migration.
pub fn write_source(path: &Path, sources: &[SourceRule]) -> Result<(), RulesFileError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| RulesFileError::Io {
            path: parent.display().to_string(),
            source: e,
        })?;
    }
    if path.exists() {
        let bak = path.with_extension("json.bak");
        std::fs::copy(path, &bak).map_err(|e| RulesFileError::Io {
            path: bak.display().to_string(),
            source: e,
        })?;
    }
    #[derive(Serialize)]
    struct Wrapper<'a> {
        rules: &'a [SourceRule],
    }
    let json = serde_json::to_string_pretty(&Wrapper { rules: sources })?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| RulesFileError::Io {
        path: tmp.display().to_string(),
        source: e,
    })?;
    std::fs::rename(&tmp, path).map_err(|e| RulesFileError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    // Rule persistence and binding persistence are one lifecycle. Without
    // this, a newly added rule has no binding history until an unrelated code
    // edit happens to advance the graph. Keep the rules write authoritative;
    // reconciliation is best-effort and hook-time generation checks fail safe.
    reconcile_bindings_after_write(path);
    Ok(())
}

fn reconcile_bindings_after_write(path: &Path) {
    if path.file_name().and_then(|name| name.to_str()) != Some("rules.json") {
        return;
    }
    let Some(phronesis_dir) = path
        .parent()
        .filter(|parent| parent.file_name().and_then(|name| name.to_str()) == Some(".phronesis"))
    else {
        return;
    };
    let Some(root) = phronesis_dir.parent() else {
        return;
    };
    if let Err(error) = crate::graph::sync::reconcile_rules(root) {
        tracing::debug!("rule write could not reconcile code bindings: {error}");
    }
}

/// Flat DiskRule → SourceRule (all-Leaf when, single then). Inverse of the
/// no-OR case of unfold_or; used by the writer.
fn diskrule_to_source(d: &DiskRule) -> SourceRule {
    SourceRule {
        id: d.id.clone(),
        phase: d.phase.clone(),
        priority: d.priority,
        when: d.conditions.iter().cloned().map(WhenClause::Leaf).collect(),
        then: d.actions.first().cloned().unwrap_or(DiskAction {
            action_type: "log".to_string(),
            params: vec![String::new()],
            data: None,
        }),
        silent: d.silent,
        audit: d.audit,
        doc_excepted: d.doc_excepted,
        binds: None,
    }
}

/// Merge `in_memory` rules with `existing` disk rules.
///
/// - Rules present in both (by ID) → in-memory version wins, with the in-memory
///   phase if known, else the existing disk phase
/// - Rules in `in_memory` only → added, with their associated phase or the
///   provided default
/// - Rules in `existing` only → preserved
///
/// Returns a count summary for reporting back to the caller.
pub struct MergeResult {
    pub merged: RulesFile,
    pub added: usize,
    pub updated: usize,
    pub preserved: usize,
}

/// Apply in-memory rules over the existing disk map: update matching ids,
/// insert new ones. Returns the updated map plus added/updated counts.
fn apply_in_memory(
    existing: &RulesFile,
    in_memory: &[Rule],
    phase_map: &HashMap<String, String>,
    default_phase: &str,
) -> (HashMap<String, DiskRule>, usize, usize) {
    let mut by_id: HashMap<String, DiskRule> = existing
        .rules
        .iter()
        .map(|r| (r.id.clone(), r.clone()))
        .collect();
    let (added, updated) = in_memory.iter().fold((0usize, 0usize), |(a, u), rule| {
        let disk_phase = by_id.get(&rule.id).map(|d| d.phase.clone());
        let phase = phase_map
            .get(&rule.id)
            .cloned()
            .or(disk_phase)
            .unwrap_or_else(|| default_phase.to_string());
        let disk = rule_to_disk(rule, &phase);
        let is_update = by_id.contains_key(&rule.id);
        by_id.insert(rule.id.clone(), disk);
        if is_update { (a, u + 1) } else { (a + 1, u) }
    });
    (by_id, added, updated)
}

/// Rebuild ordered output: existing rules in original order, then new rules
/// (those not in existing) sorted by id for determinism.
fn ordered_merge(existing: &RulesFile, mut by_id: HashMap<String, DiskRule>) -> Vec<DiskRule> {
    let mut merged_rules: Vec<DiskRule> = Vec::with_capacity(by_id.len());
    for r in &existing.rules {
        if let Some(disk) = by_id.remove(&r.id) {
            merged_rules.push(disk);
        }
    }
    // Remaining entries are new rules not in existing — sort for determinism.
    let mut remaining: Vec<DiskRule> = by_id.into_values().collect();
    remaining.sort_by(|a, b| a.id.cmp(&b.id));
    merged_rules.extend(remaining);
    merged_rules
}

pub fn merge(
    existing: &RulesFile,
    in_memory: &[Rule],
    phase_map: &HashMap<String, String>,
    default_phase: &str,
) -> MergeResult {
    let (by_id, added, updated) = apply_in_memory(existing, in_memory, phase_map, default_phase);
    let in_memory_ids: std::collections::HashSet<&str> =
        in_memory.iter().map(|r| r.id.as_str()).collect();
    let preserved = existing
        .rules
        .iter()
        .filter(|r| !in_memory_ids.contains(r.id.as_str()))
        .count();
    let merged_rules = ordered_merge(existing, by_id);
    MergeResult {
        merged: RulesFile {
            rules: merged_rules,
        },
        added,
        updated,
        preserved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phr::{Action, Condition};

    #[test]
    fn v2_action_verbs_via_source_rule() {
        let block: SourceRule = serde_json::from_str(
            r#"{ "id": "t", "when": [{ "file_path_matches": "src/" }], "then": { "block": "no unwrap" } }"#,
        )
        .unwrap();
        assert_eq!(block.then.action_type, "constraint_violation");
        assert_eq!(block.then.params, vec!["no unwrap".to_string()]);
        let warn: SourceRule = serde_json::from_str(
            r#"{ "id": "t", "when": [{ "file_path_matches": "src/" }], "then": { "warn": "m" } }"#,
        )
        .unwrap();
        assert_eq!(warn.then.action_type, "constraint_warning");
        let log: SourceRule = serde_json::from_str(
            r#"{ "id": "t", "when": [{ "file_path_matches": "src/" }], "then": { "log": "m" } }"#,
        )
        .unwrap();
        assert_eq!(log.then.action_type, "log");
    }

    fn rule(id: &str, priority: i32) -> Rule {
        Rule {
            id: id.to_string(),
            priority,
            conditions: vec![Condition {
                predicate: "p".to_string(),
                args: vec!["x".to_string()],
                script: None,
            }],
            actions: vec![Action {
                action_type: "log".to_string(),
                params: vec!["y".to_string()],
                data: None,
            }],
        }
    }

    fn disk(id: &str, phase: &str) -> DiskRule {
        rule_to_disk(&rule(id, 5), phase)
    }

    #[test]
    fn round_trip_rule() {
        let r = rule("test-rule", 7);
        let d = rule_to_disk(&r, "pre");
        let (r2, phase) = rule_from_disk(&d);
        assert_eq!(r2.id, "test-rule");
        assert_eq!(r2.priority, 7);
        assert_eq!(phase, "pre");
    }

    #[test]
    fn merge_adds_new_rules() {
        let existing = RulesFile { rules: vec![] };
        let in_memory = vec![rule("a", 1), rule("b", 2)];
        let phase_map = HashMap::new();
        let result = merge(&existing, &in_memory, &phase_map, "pre");
        assert_eq!(result.added, 2);
        assert_eq!(result.updated, 0);
        assert_eq!(result.preserved, 0);
        assert_eq!(result.merged.rules.len(), 2);
        assert!(result.merged.rules.iter().all(|r| r.phase == "pre"));
    }

    #[test]
    fn merge_updates_existing_by_id() {
        let existing = RulesFile {
            rules: vec![disk("a", "post")],
        };
        let in_memory = vec![rule("a", 99)]; // updated priority
        let phase_map = HashMap::new();
        let result = merge(&existing, &in_memory, &phase_map, "pre");
        assert_eq!(result.added, 0);
        assert_eq!(result.updated, 1);
        assert_eq!(result.merged.rules[0].priority, 99);
        // Phase from disk is preserved when not in phase_map
        assert_eq!(result.merged.rules[0].phase, "post");
    }

    #[test]
    fn merge_preserves_disk_only_rules() {
        let existing = RulesFile {
            rules: vec![disk("a", "pre"), disk("b", "post")],
        };
        let in_memory = vec![rule("a", 99)];
        let phase_map = HashMap::new();
        let result = merge(&existing, &in_memory, &phase_map, "pre");
        assert_eq!(result.preserved, 1);
        assert_eq!(result.merged.rules.len(), 2);
        assert!(result.merged.rules.iter().any(|r| r.id == "b"));
    }

    #[test]
    fn merge_honors_phase_map_override() {
        let existing = RulesFile {
            rules: vec![disk("a", "pre")],
        };
        let in_memory = vec![rule("a", 5)];
        let mut phase_map = HashMap::new();
        phase_map.insert("a".to_string(), "post".to_string());
        let result = merge(&existing, &in_memory, &phase_map, "pre");
        assert_eq!(result.merged.rules[0].phase, "post");
    }

    #[test]
    fn atomic_write_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".phronesis").join("rules.json");
        let file = RulesFile {
            rules: vec![disk("a", "pre"), disk("b", "post")],
        };
        write_atomic(&path, &file).unwrap();
        let reread = read(&path).unwrap();
        assert_eq!(reread.rules.len(), 2);
        assert_eq!(reread.rules[0].id, "a");
        assert_eq!(reread.rules[1].phase, "post");
    }

    #[test]
    fn atomic_write_creates_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.json");
        let v1 = RulesFile {
            rules: vec![disk("a", "pre")],
        };
        let v2 = RulesFile {
            rules: vec![disk("b", "post")],
        };
        write_atomic(&path, &v1).unwrap();
        write_atomic(&path, &v2).unwrap();
        let bak = path.with_extension("json.bak");
        assert!(bak.exists(), "backup file should exist after second write");
        let bak_content = read(&bak).unwrap();
        assert_eq!(bak_content.rules[0].id, "a");
    }

    #[test]
    fn read_missing_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.json");
        let result = read(&path).unwrap();
        assert!(result.rules.is_empty());
    }

    #[test]
    fn read_malformed_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "{not valid").unwrap();
        let result = read(&path);
        assert!(matches!(result, Err(RulesFileError::Malformed { .. })));
    }

    /// DiskRule field round-trip via write_atomic + read (now through SourceRule/v2).
    #[test]
    fn disk_rule_round_trips_silent_field() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.json");
        let rf = RulesFile {
            rules: vec![DiskRule {
                id: "r1".into(),
                phase: "pre".into(),
                priority: 1,
                conditions: vec![DiskCondition {
                    predicate: "p".to_string(),
                    args: vec!["x".to_string()],
                    script: None,
                }],
                actions: vec![DiskAction {
                    action_type: "log".into(),
                    params: vec!["m".into()],
                    ..Default::default()
                }],
                silent: Some(true),
                audit: None,
                doc_excepted: None,
            }],
        };
        write_atomic(&path, &rf).unwrap();
        let reread = read(&path).unwrap();
        assert_eq!(reread.rules[0].silent, Some(true));
    }

    #[test]
    fn disk_rule_without_silent_field_omits_it_on_serialize() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.json");
        let rf = RulesFile {
            rules: vec![DiskRule {
                id: "r1".into(),
                phase: "pre".into(),
                priority: 1,
                conditions: vec![DiskCondition {
                    predicate: "p".to_string(),
                    args: vec!["x".to_string()],
                    script: None,
                }],
                actions: vec![DiskAction {
                    action_type: "log".into(),
                    params: vec!["m".into()],
                    ..Default::default()
                }],
                silent: None,
                audit: None,
                doc_excepted: None,
            }],
        };
        write_atomic(&path, &rf).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains("\"silent\""),
            "absent flag must not appear in re-serialized JSON: {}",
            text
        );
    }

    #[test]
    fn disk_rule_round_trips_audit_field() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.json");
        let rf = RulesFile {
            rules: vec![DiskRule {
                id: "r1".into(),
                phase: "pre".into(),
                priority: 1,
                conditions: vec![DiskCondition {
                    predicate: "p".to_string(),
                    args: vec!["x".to_string()],
                    script: None,
                }],
                actions: vec![DiskAction {
                    action_type: "log".into(),
                    params: vec!["m".into()],
                    ..Default::default()
                }],
                silent: None,
                audit: Some(true),
                doc_excepted: None,
            }],
        };
        write_atomic(&path, &rf).unwrap();
        let reread = read(&path).unwrap();
        assert_eq!(reread.rules[0].audit, Some(true));
    }

    #[test]
    fn atomic_rewrite_preserves_existing_audit_opt_in() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.json");
        let mut original = disk("r1", "audit");
        original.audit = Some(true);
        write_atomic(
            &path,
            &RulesFile {
                rules: vec![original],
            },
        )
        .unwrap();

        let replacement = disk("r1", "audit");
        assert_eq!(replacement.audit, None);
        write_atomic(
            &path,
            &RulesFile {
                rules: vec![replacement],
            },
        )
        .unwrap();

        let reread = read(&path).unwrap();
        assert_eq!(reread.rules[0].audit, Some(true));
    }

    #[test]
    fn disk_rule_without_audit_field_omits_it_on_serialize() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.json");
        let rf = RulesFile {
            rules: vec![DiskRule {
                id: "r1".into(),
                phase: "pre".into(),
                priority: 1,
                conditions: vec![DiskCondition {
                    predicate: "p".to_string(),
                    args: vec!["x".to_string()],
                    script: None,
                }],
                actions: vec![DiskAction {
                    action_type: "log".into(),
                    params: vec!["m".into()],
                    ..Default::default()
                }],
                silent: None,
                audit: None,
                doc_excepted: None,
            }],
        };
        write_atomic(&path, &rf).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("\"audit\""));
    }

    #[test]
    fn v2_leaf_condition_single_string_arg() {
        let json = r#"{ "new_content_contains": ".unwrap()" }"#;
        let clause: WhenClause = serde_json::from_str(json).unwrap();
        match clause {
            WhenClause::Leaf(c) => {
                assert_eq!(c.predicate, "new_content_contains");
                assert_eq!(c.args, vec![".unwrap()".to_string()]);
                assert_eq!(c.script, None);
            }
            _ => panic!("expected Leaf"),
        }
    }

    #[test]
    fn v2_leaf_condition_multi_arg_array() {
        let json = r#"{ "function_param_count_high": ["?file", "?fn", "?count"] }"#;
        let clause: WhenClause = serde_json::from_str(json).unwrap();
        match clause {
            WhenClause::Leaf(c) => {
                assert_eq!(c.predicate, "function_param_count_high");
                assert_eq!(
                    c.args,
                    vec!["?file".to_string(), "?fn".to_string(), "?count".to_string()]
                );
            }
            _ => panic!("expected Leaf"),
        }
    }

    #[test]
    fn v2_leaf_condition_zero_arg_bool() {
        let json = r#"{ "some_zero_arg_predicate": true }"#;
        let clause: WhenClause = serde_json::from_str(json).unwrap();
        match clause {
            WhenClause::Leaf(c) => {
                assert_eq!(c.predicate, "some_zero_arg_predicate");
                assert!(c.args.is_empty());
            }
            _ => panic!("expected Leaf"),
        }
    }

    #[test]
    fn v2_script_condition() {
        let json = r#"{ "__script__": "rank > 5" }"#;
        let clause: WhenClause = serde_json::from_str(json).unwrap();
        match clause {
            WhenClause::Leaf(c) => {
                assert_eq!(c.predicate, "__script__");
                assert_eq!(c.script, Some("rank > 5".to_string()));
            }
            _ => panic!("expected Leaf"),
        }
    }

    #[test]
    fn v2_or_clause() {
        let json = r#"{ "or": [ { "new_content_contains": "cargo test" }, { "new_content_contains": "cargo nextest" } ] }"#;
        let clause: WhenClause = serde_json::from_str(json).unwrap();
        match clause {
            WhenClause::Or(alts) => {
                assert_eq!(alts.len(), 2);
                match &alts[0] {
                    WhenClause::Leaf(c) => {
                        assert_eq!(c.predicate, "new_content_contains");
                        assert_eq!(c.args, vec!["cargo test".to_string()]);
                    }
                    _ => panic!("expected Leaf in or[0]"),
                }
                match &alts[1] {
                    WhenClause::Leaf(c) => assert_eq!(c.args, vec!["cargo nextest".to_string()]),
                    _ => panic!("expected Leaf in or[1]"),
                }
            }
            _ => panic!("expected Or"),
        }
    }

    #[test]
    fn source_rule_parses_v2() {
        let json = r#"{
            "id": "r1", "phase": "pre", "priority": 10, "audit": true, "binds": false,
            "when": [ { "new_content_contains": ".unwrap()" }, { "file_path_matches": "src" } ],
            "then": { "block": "no unwrap" }
        }"#;
        let sr: SourceRule = serde_json::from_str(json).unwrap();
        assert_eq!(sr.id, "r1");
        assert_eq!(sr.when.len(), 2);
        assert_eq!(sr.then.action_type, "constraint_violation");
        assert_eq!(sr.audit, Some(true));
    }

    #[test]
    fn source_rule_parses_v1_legacy() {
        let json = r#"{
            "id": "r1", "phase": "pre", "priority": 10,
            "conditions": [ {"predicate":"new_content_contains","args":[".unwrap()"]} ],
            "actions": [ {"action_type":"constraint_violation","params":["no unwrap"]} ]
        }"#;
        let sr: SourceRule = serde_json::from_str(json).unwrap();
        assert_eq!(sr.id, "r1");
        assert_eq!(sr.when.len(), 1);
        match &sr.when[0] {
            WhenClause::Leaf(c) => assert_eq!(c.predicate, "new_content_contains"),
            _ => panic!("expected leaf"),
        }
        assert_eq!(sr.then.action_type, "constraint_violation");
    }

    #[test]
    fn source_rule_serializes_v2_round_trip() {
        let json = r#"{
            "id": "r1", "phase": "pre", "priority": 10, "audit": true, "binds": false,
            "when": [ { "new_content_contains": ".unwrap()" }, { "file_path_matches": "src" } ],
            "then": { "block": "no unwrap" }
        }"#;
        let sr: SourceRule = serde_json::from_str(json).unwrap();
        let out = serde_json::to_value(&sr).unwrap();
        // Re-parse the serialized form; must be identical SourceRule.
        let sr2: SourceRule = serde_json::from_value(out.clone()).unwrap();
        assert_eq!(sr2.id, "r1");
        assert_eq!(sr2.when.len(), 2);
        assert_eq!(sr2.then.action_type, "constraint_violation");
        assert_eq!(sr2.binds, Some(false));
        // Spot-check the emitted shape is v2, not v1.
        assert!(out.get("when").is_some());
        assert!(out.get("conditions").is_none());
        assert_eq!(out["then"]["block"], "no unwrap");
        assert_eq!(out["when"][0]["new_content_contains"], ".unwrap()");
        assert_eq!(out["binds"], false);
    }

    #[test]
    fn source_rule_serializes_or_clause() {
        let json = r#"{
            "id": "r1", "phase": "pre", "priority": 5,
            "when": [ { "or": [ { "new_content_contains": "a" }, { "new_content_contains": "b" } ] } ],
            "then": { "warn": "m" }
        }"#;
        let sr: SourceRule = serde_json::from_str(json).unwrap();
        let out = serde_json::to_value(&sr).unwrap();
        assert!(out["when"][0]["or"].is_array());
        assert_eq!(out["when"][0]["or"][0]["new_content_contains"], "a");
    }

    #[test]
    fn read_parses_v2_file_to_flat_diskrules() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.json");
        std::fs::write(
            &path,
            r#"{ "rules": [
            { "id": "r1", "phase": "pre", "priority": 10,
              "when": [ { "new_content_contains": ".unwrap()" } ],
              "then": { "block": "no unwrap" } }
        ] }"#,
        )
        .unwrap();
        let file = read(&path).unwrap();
        assert_eq!(file.rules.len(), 1);
        assert_eq!(
            file.rules[0].conditions[0].predicate,
            "new_content_contains"
        );
        assert_eq!(file.rules[0].actions[0].action_type, "constraint_violation");
    }

    #[test]
    fn read_parses_v1_legacy_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.json");
        std::fs::write(
            &path,
            r#"{ "rules": [
            { "id": "r1", "phase": "pre", "priority": 10,
              "conditions": [ {"predicate":"new_content_contains","args":[".unwrap()"]} ],
              "actions": [ {"action_type":"constraint_violation","params":["no unwrap"]} ] }
        ] }"#,
        )
        .unwrap();
        let file = read(&path).unwrap();
        assert_eq!(file.rules.len(), 1);
        assert_eq!(
            file.rules[0].conditions[0].predicate,
            "new_content_contains"
        );
    }

    #[test]
    fn write_atomic_emits_v2() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.json");
        let rf = RulesFile {
            rules: vec![DiskRule {
                id: "r1".into(),
                phase: "pre".into(),
                priority: 10,
                conditions: vec![DiskCondition {
                    predicate: "new_content_contains".into(),
                    args: vec![".unwrap()".into()],
                    script: None,
                }],
                actions: vec![DiskAction {
                    action_type: "constraint_violation".into(),
                    params: vec!["no unwrap".into()],
                    ..Default::default()
                }],
                silent: None,
                audit: Some(true),
                doc_excepted: None,
            }],
        };
        write_atomic(&path, &rf).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"when\""));
        assert!(text.contains("\"then\""));
        assert!(text.contains("\"block\""));
        assert!(!text.contains("\"action_type\""));
    }

    fn leaf(pred: &str, arg: &str) -> WhenClause {
        WhenClause::Leaf(DiskCondition {
            predicate: pred.into(),
            args: vec![arg.into()],
            script: None,
        })
    }
    fn src(id: &str, when: Vec<WhenClause>) -> SourceRule {
        SourceRule {
            id: id.into(),
            phase: "pre".into(),
            priority: 1,
            when,
            then: DiskAction {
                action_type: "log".into(),
                params: vec!["m".into()],
                ..Default::default()
            },
            silent: None,
            audit: None,
            doc_excepted: None,
            binds: None,
        }
    }

    #[test]
    fn unfold_no_or_passthrough() {
        let s = src("r", vec![leaf("a", "1"), leaf("b", "2")]);
        let out = unfold_or(&s).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "r");
        assert_eq!(out[0].conditions.len(), 2);
    }

    #[test]
    fn base_rule_id_strips_or_expansion_suffixes() {
        assert_eq!(base_rule_id("r"), "r");
        assert_eq!(base_rule_id("r#or0"), "r");
        assert_eq!(base_rule_id("r#or0-or1"), "r");
        assert_eq!(base_rule_id("r#or0-or1-or2"), "r");
        assert_eq!(
            base_rule_id("warn-untested-risky-call#or4"),
            "warn-untested-risky-call"
        );
        // Merely containing "#or" is not an expansion suffix.
        assert_eq!(base_rule_id("x#organic"), "x#organic");
        assert_eq!(base_rule_id("r#or"), "r#or");
    }

    #[test]
    fn unfold_single_or_two_alternatives() {
        let s = src(
            "r",
            vec![
                WhenClause::Or(vec![leaf("a", "1"), leaf("b", "2")]),
                leaf("c", "3"),
            ],
        );
        let out = unfold_or(&s).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, "r#or0");
        assert_eq!(out[1].id, "r#or1");
        // Each carries the non-OR leaf c.
        assert!(out[0].conditions.iter().any(|c| c.predicate == "c"));
        assert!(out[0].conditions.iter().any(|c| c.predicate == "a"));
        assert!(out[1].conditions.iter().any(|c| c.predicate == "b"));
    }

    #[test]
    fn unfold_multi_or_cartesian() {
        let s = src(
            "r",
            vec![
                WhenClause::Or(vec![leaf("a", "1"), leaf("b", "2")]),
                WhenClause::Or(vec![leaf("c", "3"), leaf("d", "4")]),
            ],
        );
        let out = unfold_or(&s).unwrap();
        assert_eq!(out.len(), 4);
        let ids: Vec<&str> = out.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["r#or0-or0", "r#or0-or1", "r#or1-or0", "r#or1-or1"]
        );
    }

    #[test]
    fn unfold_nested_or_flattens() {
        let s = src(
            "r",
            vec![WhenClause::Or(vec![
                leaf("a", "1"),
                WhenClause::Or(vec![leaf("b", "2"), leaf("c", "3")]),
            ])],
        );
        let out = unfold_or(&s).unwrap();
        assert_eq!(out.len(), 3); // a, b, c
    }

    #[test]
    fn unfold_empty_or_errors() {
        let s = src("r", vec![WhenClause::Or(vec![])]);
        assert!(unfold_or(&s).is_err());
    }

    #[test]
    fn unfold_single_element_or_degenerates() {
        let s = src("r", vec![WhenClause::Or(vec![leaf("a", "1")])]);
        let out = unfold_or(&s).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "r"); // no suffix when only one product
    }

    #[test]
    fn load_rules_preserves_script_condition() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.json");
        let rf = RulesFile {
            rules: vec![DiskRule {
                id: "s".into(),
                phase: "pre".into(),
                priority: 1,
                conditions: vec![DiskCondition {
                    predicate: "__script__".into(),
                    args: vec![],
                    script: Some("rank > 5".into()),
                }],
                actions: vec![DiskAction {
                    action_type: "constraint_warning".into(),
                    params: vec!["m".into()],
                    ..Default::default()
                }],
                silent: None,
                audit: None,
                doc_excepted: None,
            }],
        };
        write_atomic(&path, &rf).unwrap();
        let loaded = read(&path).unwrap();
        assert_eq!(
            loaded.rules[0].conditions[0].script,
            Some("rank > 5".to_string())
        );
    }
    /// The public constructor must keep its exact legacy return type: a
    /// caller assigning to `Result<SourceRule, RulesFileError>` (as the
    /// journey tagger config surface documents) must compile unchanged.
    /// The `ok` assignment below is the compile-time assertion.
    #[test]
    fn synthetic_tagger_keeps_legacy_error_type_and_display() {
        let ok: Result<SourceRule, RulesFileError> = SourceRule::synthetic_tagger(
            "missing-test",
            &[serde_json::json!({"change_set_has_production_rust": true})],
        );
        let tagger = ok.expect("valid tagger");
        assert_eq!(tagger.id, "tagger:missing-test");
        assert_eq!(tagger.phase, "tag");
        assert_eq!(tagger.then.action_type, "tag");
        assert_eq!(tagger.then.params, vec!["missing-test"]);

        let err = SourceRule::synthetic_tagger("t", &[serde_json::json!({"p": 5})])
            .expect_err("malformed when must be rejected");
        // The legacy Display wording ("rules file at …") is what consumers
        // stringify verbatim into their own config-error types.
        assert_eq!(
            err.to_string(),
            "rules file at tagger:t could not be expanded: predicate value must be string, array, or bool; got 5"
        );
        assert!(matches!(&err, RulesFileError::Unfold { path, .. } if path == "tagger:t"));
        assert_eq!(err.path(), Some("tagger:t"));
    }

    /// The wrapper is a pure delegate: for the same JSON it must parse and
    /// serialize exactly like the governance `SourceRule`, field for field
    /// (source-only `binds` included), and reject the same input with the
    /// same message — the MCP crate's public serde surface must not drift
    /// from the shared core.
    #[test]
    fn source_rule_wrapper_serde_matches_governance_exactly() {
        let json = r#"{
            "id": "r1", "phase": "post", "priority": 7,
            "silent": true, "audit": false, "doc_excepted": true, "binds": true,
            "when": [ { "or": [ { "file_path_matches": "src" },
                                { "file_extension_is": ".rs" } ] } ],
            "then": { "warn": "legacy" }
        }"#;
        let wrapped: SourceRule = serde_json::from_str(json).unwrap();
        let gov: GovSourceRule = serde_json::from_str(json).unwrap();
        assert_eq!(
            serde_json::to_value(&wrapped).unwrap(),
            serde_json::to_value(&gov).unwrap()
        );

        // Every public field crosses the From conversions unchanged,
        // including the source-only `binds` flag flat rules cannot carry.
        assert_eq!(wrapped.binds, Some(true));
        assert_eq!(wrapped.silent, Some(true));
        assert_eq!(wrapped.audit, Some(false));
        assert_eq!(wrapped.doc_excepted, Some(true));
        let back = GovSourceRule::from(wrapped);
        assert_eq!(back.binds, gov.binds);
        assert_eq!(back.phase, gov.phase);
        assert_eq!(back.priority, gov.priority);
        assert_eq!(
            serde_json::to_value(&back).unwrap(),
            serde_json::to_value(&gov).unwrap()
        );

        // Unknown keys fail identically on both paths (shared deserializer).
        let bad =
            r#"{"id":"r1","phase":"pre","priority":1,"when":[],"then":{"block":"x"},"bogus":1}"#;
        let wrap_err = serde_json::from_str::<SourceRule>(bad)
            .unwrap_err()
            .to_string();
        let gov_err = serde_json::from_str::<GovSourceRule>(bad)
            .unwrap_err()
            .to_string();
        assert_eq!(wrap_err, gov_err);
        assert!(wrap_err.contains("unknown key `bogus`"), "{wrap_err}");
    }
}

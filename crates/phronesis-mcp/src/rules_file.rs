//! Read/write the `.phronesis/rules.json` format shared by the MCP server and
//! the hook subcommands. The disk format carries an extra `phase` field per
//! rule (`"pre"` or `"post"`) that the in-memory `Rule` struct lacks.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use phr::{Action, Condition, Rule};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DiskRule {
    pub id: String,
    pub phase: String,
    pub priority: i32,
    pub conditions: Vec<DiskCondition>,
    pub actions: Vec<DiskAction>,
    /// When `Some(true)`, this rule is hidden from `session-context`
    /// output. Escape hatch for noisy packs. The engine itself ignores
    /// this field — it only affects the SessionStart summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub silent: Option<bool>,
    /// When `Some(true)`, this rule participates in whole-tree audits
    /// (`audit_codebase` / `phr-mcp audit`). Rules without it are
    /// skipped — typically the LLM-deflection pack and any diff-only
    /// rule whose predicates don't make sense over current file state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit: Option<bool>,
    /// When `Some(true)`, audit matches whose immediately preceding
    /// non-blank line is a `///` doc-comment are suppressed. Lets a
    /// rule whose message is "either delete or document" honor the
    /// "document" branch — the reader has supplied an explanation, so
    /// the audit doesn't keep flagging it. Only consulted by the audit
    /// engine; ignored at hook time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc_excepted: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DiskCondition {
    pub predicate: String,
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DiskAction {
    pub action_type: String,
    pub params: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RulesFile {
    pub rules: Vec<DiskRule>,
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
}

/// Default state of the rules file under the project root.
pub fn default_path(project_root: &Path) -> PathBuf {
    project_root.join(".phronesis").join("rules.json")
}

/// Read the rules file at `path`. Returns `Ok(RulesFile{rules:vec![]})` when the
/// file does not exist (the "no rules configured" case). Returns `Err` when the
/// file exists but is unreadable or malformed.
pub fn read(path: &Path) -> Result<RulesFile, RulesFileError> {
    if !path.exists() {
        return Ok(RulesFile { rules: vec![] });
    }
    let content = std::fs::read_to_string(path).map_err(|e| RulesFileError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    serde_json::from_str(&content).map_err(|e| RulesFileError::Malformed {
        path: path.display().to_string(),
        source: e,
    })
}

/// Atomically write a rules file to `path`. Creates parent directories if needed
/// and preserves a single `.bak` of the previous contents.
pub fn write_atomic(path: &Path, file: &RulesFile) -> Result<(), RulesFileError> {
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

    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(file)?;
    std::fs::write(&tmp, json).map_err(|e| RulesFileError::Io {
        path: tmp.display().to_string(),
        source: e,
    })?;
    std::fs::rename(&tmp, path).map_err(|e| RulesFileError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    Ok(())
}

/// Convert an in-memory `Rule` plus a phase string into the disk form.
pub fn rule_to_disk(rule: &Rule, phase: &str) -> DiskRule {
    DiskRule {
        id: rule.id.clone(),
        phase: phase.to_string(),
        priority: rule.priority,
        conditions: rule
            .conditions
            .iter()
            .map(|c| DiskCondition {
                predicate: c.predicate.clone(),
                args: c.args.clone(),
                script: c.script.clone(),
            })
            .collect(),
        actions: rule
            .actions
            .iter()
            .map(|a| DiskAction {
                action_type: a.action_type.clone(),
                params: a.params.clone(),
            })
            .collect(),
        silent: None,
        audit: None,
        doc_excepted: None,
    }
}

/// Convert a disk rule back into an in-memory `Rule` plus its phase.
pub fn rule_from_disk(disk: &DiskRule) -> (Rule, String) {
    let rule = Rule {
        id: disk.id.clone(),
        priority: disk.priority,
        conditions: disk
            .conditions
            .iter()
            .map(|c| Condition {
                predicate: c.predicate.clone(),
                args: c.args.clone(),
                script: c.script.clone(),
            })
            .collect(),
        actions: disk
            .actions
            .iter()
            .map(|a| Action {
                action_type: a.action_type.clone(),
                params: a.params.clone(),
            })
            .collect(),
    };
    (rule, disk.phase.clone())
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

pub fn merge(
    existing: &RulesFile,
    in_memory: &[Rule],
    phase_map: &HashMap<String, String>,
    default_phase: &str,
) -> MergeResult {
    let mut by_id: HashMap<String, DiskRule> = existing
        .rules
        .iter()
        .map(|r| (r.id.clone(), r.clone()))
        .collect();

    let mut added = 0usize;
    let mut updated = 0usize;

    for rule in in_memory {
        let phase = phase_map
            .get(&rule.id)
            .cloned()
            .or_else(|| by_id.get(&rule.id).map(|d| d.phase.clone()))
            .unwrap_or_else(|| default_phase.to_string());
        let disk = rule_to_disk(rule, &phase);
        if by_id.contains_key(&rule.id) {
            updated += 1;
        } else {
            added += 1;
        }
        by_id.insert(rule.id.clone(), disk);
    }

    let in_memory_ids: std::collections::HashSet<&str> =
        in_memory.iter().map(|r| r.id.as_str()).collect();
    let preserved = existing
        .rules
        .iter()
        .filter(|r| !in_memory_ids.contains(r.id.as_str()))
        .count();

    // Preserve ordering: existing rules in original order, then new ones
    let mut merged_rules: Vec<DiskRule> = Vec::with_capacity(by_id.len());
    let mut seen = std::collections::HashSet::new();
    for r in &existing.rules {
        if let Some(disk) = by_id.remove(&r.id) {
            seen.insert(disk.id.clone());
            merged_rules.push(disk);
        }
    }
    // Append remaining (new) rules sorted by ID for determinism
    let mut remaining: Vec<DiskRule> = by_id.into_values().collect();
    remaining.sort_by(|a, b| a.id.cmp(&b.id));
    for d in remaining {
        if !seen.contains(&d.id) {
            merged_rules.push(d);
        }
    }

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

    #[test]
    fn disk_rule_round_trips_silent_field() {
        let json =
            r#"{"id":"r1","phase":"pre","priority":1,"conditions":[],"actions":[],"silent":true}"#;
        let r: DiskRule = serde_json::from_str(json).unwrap();
        assert_eq!(r.silent, Some(true));
        // Re-serialize round-trip preserves the field.
        let out = serde_json::to_string(&r).unwrap();
        assert!(out.contains("\"silent\":true"));
    }

    #[test]
    fn disk_rule_without_silent_field_omits_it_on_serialize() {
        let json = r#"{"id":"r1","phase":"pre","priority":1,"conditions":[],"actions":[]}"#;
        let r: DiskRule = serde_json::from_str(json).unwrap();
        assert_eq!(r.silent, None);
        let out = serde_json::to_string(&r).unwrap();
        assert!(
            !out.contains("\"silent\""),
            "absent flag must not appear in re-serialized JSON: {}",
            out
        );
    }

    #[test]
    fn disk_rule_round_trips_audit_field() {
        let json =
            r#"{"id":"r1","phase":"pre","priority":1,"conditions":[],"actions":[],"audit":true}"#;
        let r: DiskRule = serde_json::from_str(json).unwrap();
        assert_eq!(r.audit, Some(true));
        let out = serde_json::to_string(&r).unwrap();
        assert!(out.contains("\"audit\":true"));
    }

    #[test]
    fn disk_rule_without_audit_field_omits_it_on_serialize() {
        let json = r#"{"id":"r1","phase":"pre","priority":1,"conditions":[],"actions":[]}"#;
        let r: DiskRule = serde_json::from_str(json).unwrap();
        assert_eq!(r.audit, None);
        let out = serde_json::to_string(&r).unwrap();
        assert!(!out.contains("\"audit\""));
    }
}

//! Read/write the `.phronesis/rules.json` format shared by the MCP server and
//! the hook subcommands. The disk format carries an extra `phase` field per
//! rule (`"pre"` or `"post"`) that the in-memory `Rule` struct lacks.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::anyhow;
use phr::{Action, Condition, Rule};
use serde::de::{self};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// One clause in a v2 rule's `when` array: a leaf condition or an OR group.
#[derive(Debug, Clone)]
pub enum WhenClause {
    Leaf(DiskCondition),
    Or(Vec<WhenClause>),
}

fn parse_or_clause(val: &serde_json::Value) -> anyhow::Result<WhenClause> {
    let arr = val
        .as_array()
        .ok_or_else(|| anyhow!("\"or\" value must be an array of clauses"))?;
    let alts: anyhow::Result<Vec<WhenClause>> = arr
        .iter()
        .map(|v| serde_json::from_value::<WhenClause>(v.clone()).map_err(|e| anyhow!("{}", e)))
        .collect();
    Ok(WhenClause::Or(alts?))
}

fn parse_leaf_clause(key: &str, val: &serde_json::Value) -> anyhow::Result<WhenClause> {
    let predicate = key.to_string();
    let (args, script) = if predicate == "__script__" {
        let s = val
            .as_str()
            .ok_or_else(|| anyhow!("__script__ value must be a string"))?;
        (Vec::new(), Some(s.to_string()))
    } else {
        let args = match val {
            serde_json::Value::String(s) => vec![s.clone()],
            // A boolean is a zero-arg presence marker: the predicate name is
            // what matters, not the value (true or false). Both yield empty args.
            serde_json::Value::Bool(_) => Vec::new(),
            serde_json::Value::Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                for it in items {
                    let s = it
                        .as_str()
                        .ok_or_else(|| anyhow!("predicate arg array must contain strings"))?;
                    out.push(s.to_string());
                }
                out
            }
            other => {
                anyhow::bail!(
                    "predicate value must be string, array, or bool; got {}",
                    other
                );
            }
        };
        (args, None)
    };
    Ok(WhenClause::Leaf(DiskCondition {
        predicate,
        args,
        script,
    }))
}

impl<'de> Deserialize<'de> for WhenClause {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // A clause is always a single-key JSON object. If the key is "or",
        // it's a disjunction; otherwise the key is a predicate name.
        let value = serde_json::Value::deserialize(deserializer)?;
        let obj = value
            .as_object()
            .ok_or_else(|| de::Error::custom("when-clause must be a JSON object"))?;
        if obj.len() != 1 {
            return Err(de::Error::custom(
                "when-clause must have exactly one key (a predicate name or \"or\")",
            ));
        }
        let (key, val) = obj.iter().next().expect("len checked == 1");
        if key == "or" {
            return parse_or_clause(val).map_err(de::Error::custom);
        }
        parse_leaf_clause(key, val).map_err(de::Error::custom)
    }
}

/// On-disk v2 rule form. OR-bearing; expanded to flat `DiskRule`s by `unfold_or`.
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

/// Map a v2 `then` object (`{"block": "msg"}`) to an internal action.
/// `block`→constraint_violation, `warn`→constraint_warning, `log`→log,
/// anything else passes through as its own action_type (forward-compat).
fn parse_then_action(value: &serde_json::Value) -> anyhow::Result<DiskAction> {
    let obj = value
        .as_object()
        .ok_or_else(|| anyhow!("then must be a JSON object"))?;
    if obj.len() != 1 {
        return Err(anyhow!("then must have exactly one verb key"));
    }
    let (verb, msg_val) = obj.iter().next().expect("len==1");
    let msg = msg_val
        .as_str()
        .ok_or_else(|| anyhow!("then message must be a string"))?
        .to_string();
    let action_type = match verb.as_str() {
        "block" => "constraint_violation",
        "warn" => "constraint_warning",
        "log" => "log",
        other => other,
    }
    .to_string();
    Ok(DiskAction {
        action_type,
        params: vec![msg],
    })
}

/// Inverse of `parse_then_action`: internal action → v2 verb object.
fn action_to_then(action: &DiskAction) -> serde_json::Value {
    let verb = match action.action_type.as_str() {
        "constraint_violation" => "block",
        "constraint_warning" => "warn",
        "log" => "log",
        other => other,
    };
    let msg = action.params.first().cloned().unwrap_or_default();
    serde_json::json!({ verb: msg })
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
    /// single, named error variant.
    pub fn synthetic_tagger(tag: &str, when: &[serde_json::Value]) -> Result<Self, RulesFileError> {
        let id = format!("tagger:{}", tag);
        let when_clauses: Vec<WhenClause> = when
            .iter()
            .map(|v| serde_json::from_value::<WhenClause>(v.clone()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| RulesFileError::Unfold {
                path: id.clone(),
                message: e.to_string(),
            })?;
        Ok(SourceRule {
            id,
            phase: "tag".to_string(),
            priority: 0,
            when: when_clauses,
            then: DiskAction {
                action_type: "tag".to_string(),
                params: vec![tag.to_string()],
            },
            silent: None,
            audit: None,
            doc_excepted: None,
            binds: None,
        })
    }
}

fn parse_when_field(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> anyhow::Result<Vec<WhenClause>> {
    if let Some(when_val) = obj.get("when") {
        let arr = when_val
            .as_array()
            .ok_or_else(|| anyhow!("`when` must be an array"))?;
        arr.iter()
            .map(|c| serde_json::from_value::<WhenClause>(c.clone()).map_err(|e| anyhow!("{}", e)))
            .collect()
    } else if let Some(cond_val) = obj.get("conditions") {
        // v1 legacy: each is {predicate, args, script?}.
        let arr = cond_val
            .as_array()
            .ok_or_else(|| anyhow!("`conditions` must be an array"))?;
        arr.iter()
            .map(|c| {
                let co = c
                    .as_object()
                    .ok_or_else(|| anyhow!("v1 condition must be an object"))?;
                let predicate = co
                    .get("predicate")
                    .and_then(|x| x.as_str())
                    .ok_or_else(|| anyhow!("v1 condition missing `predicate`"))?
                    .to_string();
                let args = co
                    .get("args")
                    .and_then(|x| x.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|s| s.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let script = co.get("script").and_then(|x| x.as_str()).map(String::from);
                Ok(WhenClause::Leaf(DiskCondition {
                    predicate,
                    args,
                    script,
                }))
            })
            .collect()
    } else {
        anyhow::bail!("rule has neither `when` nor `conditions`")
    }
}

fn parse_then_field(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> anyhow::Result<DiskAction> {
    if let Some(then_val) = obj.get("then") {
        parse_then_action(then_val)
    } else if let Some(actions_val) = obj.get("actions") {
        let first = actions_val
            .as_array()
            .and_then(|a| a.first())
            .ok_or_else(|| anyhow!("v1 `actions` must be a non-empty array"))?;
        let ao = first
            .as_object()
            .ok_or_else(|| anyhow!("v1 action must be an object"))?;
        let action_type = ao
            .get("action_type")
            .and_then(|x| x.as_str())
            .ok_or_else(|| anyhow!("v1 action missing `action_type`"))?
            .to_string();
        let params = ao
            .get("params")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        Ok(DiskAction {
            action_type,
            params,
        })
    } else {
        anyhow::bail!("rule has neither `then` nor `actions`")
    }
}

impl<'de> Deserialize<'de> for SourceRule {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let v = serde_json::Value::deserialize(deserializer)?;
        let obj = v
            .as_object()
            .ok_or_else(|| de::Error::custom("rule must be a JSON object"))?;
        let id = obj
            .get("id")
            .and_then(|x| x.as_str())
            .ok_or_else(|| de::Error::custom("rule missing string `id`"))?
            .to_string();
        let phase = obj
            .get("phase")
            .and_then(|x| x.as_str())
            .unwrap_or("pre")
            .to_string();
        let when = parse_when_field(obj).map_err(de::Error::custom)?;
        let then = parse_then_field(obj).map_err(de::Error::custom)?;
        Ok(SourceRule {
            id,
            phase,
            priority: obj.get("priority").and_then(|x| x.as_i64()).unwrap_or(0) as i32,
            when,
            then,
            silent: obj.get("silent").and_then(|x| x.as_bool()),
            audit: obj.get("audit").and_then(|x| x.as_bool()),
            doc_excepted: obj.get("doc_excepted").and_then(|x| x.as_bool()),
            binds: obj.get("binds").and_then(|x| x.as_bool()),
        })
    }
}

impl Serialize for WhenClause {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let v = match self {
            WhenClause::Or(alts) => {
                let arr: Vec<serde_json::Value> = alts
                    .iter()
                    .map(|c| serde_json::to_value(c).unwrap_or(serde_json::Value::Null))
                    .collect();
                serde_json::json!({ "or": arr })
            }
            WhenClause::Leaf(c) => {
                if let Some(script) = &c.script {
                    serde_json::json!({ "__script__": script })
                } else {
                    let value = match c.args.len() {
                        0 => serde_json::Value::Bool(true),
                        1 => serde_json::Value::String(c.args[0].clone()),
                        _ => serde_json::Value::Array(
                            c.args
                                .iter()
                                .cloned()
                                .map(serde_json::Value::String)
                                .collect(),
                        ),
                    };
                    serde_json::json!({ c.predicate.clone(): value })
                }
            }
        };
        v.serialize(serializer)
    }
}

impl Serialize for SourceRule {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        // Pinned key order: id, phase, priority, metadata, when, then.
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("id", &self.id)?;
        map.serialize_entry("phase", &self.phase)?;
        map.serialize_entry("priority", &self.priority)?;
        if let Some(a) = self.audit {
            map.serialize_entry("audit", &a)?;
        }
        if let Some(s) = self.silent {
            map.serialize_entry("silent", &s)?;
        }
        if let Some(d) = self.doc_excepted {
            map.serialize_entry("doc_excepted", &d)?;
        }
        if let Some(b) = self.binds {
            map.serialize_entry("binds", &b)?;
        }
        map.serialize_entry("when", &self.when)?;
        map.serialize_entry("then", &action_to_then(&self.then))?;
        map.end()
    }
}

#[derive(Debug, Clone)]
pub struct DiskRule {
    pub id: String,
    pub phase: String,
    pub priority: i32,
    pub conditions: Vec<DiskCondition>,
    pub actions: Vec<DiskAction>,
    /// When `Some(true)`, this rule is hidden from `session-context`
    /// output. Escape hatch for noisy packs. The engine itself ignores
    /// this field — it only affects the SessionStart summary.
    pub silent: Option<bool>,
    /// When `Some(true)`, this rule participates in whole-tree audits
    /// (`audit_codebase` / `phr-mcp audit`). Rules without it are
    /// skipped — typically the LLM-deflection pack and any diff-only
    /// rule whose predicates don't make sense over current file state.
    pub audit: Option<bool>,
    /// When `Some(true)`, audit matches whose immediately preceding
    /// non-blank line is a `///` doc-comment are suppressed. Lets a
    /// rule whose message is "either delete or document" honor the
    /// "document" branch — the reader has supplied an explanation, so
    /// the audit doesn't keep flagging it. Only consulted by the audit
    /// engine; ignored at hook time.
    pub doc_excepted: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct DiskCondition {
    pub predicate: String,
    pub args: Vec<String>,
    pub script: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DiskAction {
    pub action_type: String,
    pub params: Vec<String>,
}

#[derive(Debug, Clone)]
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
    #[error("rules file at {path} could not be expanded: {message}")]
    Unfold { path: String, message: String },
}

/// Default state of the rules file under the project root.
pub fn default_path(project_root: &Path) -> PathBuf {
    project_root.join(".phronesis").join("rules.json")
}

/// Read the rules file at `path`. Returns `Ok(RulesFile{rules:vec![]})` when the
/// file does not exist (the "no rules configured" case). Returns `Err` when the
/// file exists but is unreadable or malformed.
pub fn read(path: &Path) -> Result<RulesFile, RulesFileError> {
    let sources = read_source(path)?;
    let mut flat = Vec::new();
    for sr in &sources {
        let expanded = unfold_or(sr).map_err(|e| RulesFileError::Unfold {
            path: path.display().to_string(),
            message: e.to_string(),
        })?;
        flat.extend(expanded);
    }
    Ok(RulesFile { rules: flat })
}

/// Parse the file into OR-bearing `SourceRule`s without unfolding. Used by
/// migration (which preserves OR on disk).
pub fn read_source(path: &Path) -> Result<Vec<SourceRule>, RulesFileError> {
    if !path.exists() {
        return Ok(vec![]);
    }
    let content = std::fs::read_to_string(path).map_err(|e| RulesFileError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    #[derive(Deserialize)]
    struct Wrapper {
        rules: Vec<SourceRule>,
    }
    let w: Wrapper = serde_json::from_str(&content).map_err(|e| RulesFileError::Malformed {
        path: path.display().to_string(),
        source: e,
    })?;
    Ok(w.rules)
}

/// Cartesian product of per-position alternative condition sets, tracking the
/// chosen alt index at each OR position so the caller can construct ids.
fn cartesian_product(
    position_alts: &[Vec<Vec<DiskCondition>>],
    is_or_position: &[bool],
) -> Vec<(Vec<usize>, Vec<DiskCondition>)> {
    let mut results: Vec<(Vec<usize>, Vec<DiskCondition>)> = vec![(Vec::new(), Vec::new())];
    for (pos, alts) in position_alts.iter().enumerate() {
        results = {
            let mut next = Vec::new();
            for (idx_path, conds) in &results {
                for (alt_idx, alt_conds) in alts.iter().enumerate() {
                    let new_path: Vec<usize> = if is_or_position[pos] {
                        idx_path
                            .iter()
                            .cloned()
                            .chain(std::iter::once(alt_idx))
                            .collect()
                    } else {
                        idx_path.clone()
                    };
                    let new_conds: Vec<DiskCondition> = conds
                        .iter()
                        .cloned()
                        .chain(alt_conds.iter().cloned())
                        .collect();
                    next.push((new_path, new_conds));
                }
            }
            next
        };
    }
    results
}

/// Expand a SourceRule's OR clauses into flat, OR-free DiskRules via
/// disjunctive-normal-form (DNF) expansion. Each OR position contributes
/// one alternative per (flattened) branch; the cartesian product across
/// all positions yields the output rules. Child ids are suffixed
/// deterministically: `#or0`, `#or1`, or `#or0-or1` for multi-position
/// products. A single-product result (no OR, or a single-element OR)
/// retains the original id unchanged.
pub fn unfold_or(source: &SourceRule) -> anyhow::Result<Vec<DiskRule>> {
    // For each `when` position, compute the list of condition-alternatives.
    // A Leaf has exactly one alternative (no suffix contribution).
    // An Or contributes one alternative per flattened branch.
    fn alternatives(clause: &WhenClause, rule_id: &str) -> anyhow::Result<Vec<Vec<DiskCondition>>> {
        match clause {
            WhenClause::Leaf(c) => Ok(vec![vec![c.clone()]]),
            WhenClause::Or(alts) => {
                if alts.is_empty() {
                    anyhow::bail!("rule `{}` has an empty `or` (unsatisfiable)", rule_id);
                }
                let mut out = Vec::new();
                for alt in alts {
                    // Flatten nested OR by recursion: each branch yields its own alternatives.
                    let branch = alternatives(alt, rule_id)?;
                    out.extend(branch);
                }
                Ok(out)
            }
        }
    }

    // Per-position alternative sets. Leaf positions have one alternative (no
    // index pushed to path); OR positions with >1 branch push an alt index.
    let (position_alts, is_or_position) = {
        let mut pa: Vec<Vec<Vec<DiskCondition>>> = Vec::new();
        let mut ior: Vec<bool> = Vec::new();
        for clause in &source.when {
            let alts = alternatives(clause, &source.id)?;
            ior.push(matches!(clause, WhenClause::Or(_)) && alts.len() > 1);
            pa.push(alts);
        }
        (pa, ior)
    };

    let results = cartesian_product(&position_alts, &is_or_position);

    if results.len() > 32 {
        eprintln!(
            "phronesis: rule `{}` expands to {} rules via OR — consider simplifying",
            source.id,
            results.len()
        );
    }

    let multiple = results.len() > 1;
    Ok(results
        .into_iter()
        .map(|(idx_path, conditions)| {
            let id = if multiple && !idx_path.is_empty() {
                let suffix = idx_path
                    .iter()
                    .map(|i| format!("or{}", i))
                    .collect::<Vec<_>>()
                    .join("-");
                format!("{}#{}", source.id, suffix)
            } else {
                source.id.clone()
            };
            DiskRule {
                id,
                phase: source.phase.clone(),
                priority: source.priority,
                conditions,
                actions: vec![source.then.clone()],
                silent: source.silent,
                audit: source.audit,
                doc_excepted: source.doc_excepted,
            }
        })
        .collect())
}

/// Atomically write a rules file to `path`. Creates parent directories if needed
/// and preserves a single `.bak` of the previous contents. Emits v2 shape.
pub fn write_atomic(path: &Path, file: &RulesFile) -> Result<(), RulesFileError> {
    let existing_binds: HashMap<String, Option<bool>> = read_source(path)
        .unwrap_or_default()
        .into_iter()
        .map(|rule| (rule.id, rule.binds))
        .collect();
    let sources: Vec<SourceRule> = file
        .rules
        .iter()
        .map(|rule| {
            let mut source = diskrule_to_source(rule);
            source.binds = existing_binds.get(&rule.id).copied().flatten();
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
        }),
        silent: d.silent,
        audit: d.audit,
        doc_excepted: d.doc_excepted,
        binds: None,
    }
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
                conditions: vec![],
                actions: vec![DiskAction {
                    action_type: "log".into(),
                    params: vec!["m".into()],
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
                conditions: vec![],
                actions: vec![DiskAction {
                    action_type: "log".into(),
                    params: vec!["m".into()],
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
                conditions: vec![],
                actions: vec![DiskAction {
                    action_type: "log".into(),
                    params: vec!["m".into()],
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
    fn disk_rule_without_audit_field_omits_it_on_serialize() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.json");
        let rf = RulesFile {
            rules: vec![DiskRule {
                id: "r1".into(),
                phase: "pre".into(),
                priority: 1,
                conditions: vec![],
                actions: vec![DiskAction {
                    action_type: "log".into(),
                    params: vec!["m".into()],
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
    fn v2_clause_rejects_malformed_inputs() {
        assert!(serde_json::from_str::<WhenClause>(r#""just_a_string""#).is_err());
        assert!(serde_json::from_str::<WhenClause>(r#"{"a": true, "b": true}"#).is_err());
        assert!(serde_json::from_str::<WhenClause>(r#"{"or": "not_an_array"}"#).is_err());
        assert!(serde_json::from_str::<WhenClause>(r#"{"pred": 42}"#).is_err());
    }

    #[test]
    fn v2_action_block() {
        let json = r#"{ "block": "no unwrap" }"#;
        let a = parse_then_action(&serde_json::from_str(json).unwrap()).unwrap();
        assert_eq!(a.action_type, "constraint_violation");
        assert_eq!(a.params, vec!["no unwrap".to_string()]);
    }

    #[test]
    fn v2_action_warn_and_log() {
        let w = parse_then_action(&serde_json::from_str(r#"{ "warn": "m" }"#).unwrap()).unwrap();
        assert_eq!(w.action_type, "constraint_warning");
        let l = parse_then_action(&serde_json::from_str(r#"{ "log": "m" }"#).unwrap()).unwrap();
        assert_eq!(l.action_type, "log");
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
}

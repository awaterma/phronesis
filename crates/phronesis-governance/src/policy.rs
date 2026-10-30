//! The `.phronesis/rules.json` format: v1/v2 source parsing, strict load
//! validation, OR expansion, disk conversions, and compiled-policy
//! selection.
//!
//! Ported verbatim in behavior from `phronesis-mcp::rules_file`; everything
//! filesystem-related (read/write/default path) stays in the MCP crate,
//! which maps [`PolicyError`] back onto its path-bearing `RulesFileError`.
//! Errors carry opaque `source_id` strings supplied by the caller (`parse_source`
//! / `parse`) or `"compiled"` for [`CompiledPolicy::compile`]; no filesystem
//! path ever crosses this boundary. No stderr: former warnings surface as
//! structured [`PolicyDiagnostic`]s.

use std::collections::HashMap;

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

/// Phases a rules file may name. `pre` blocks at the pre-tool hook, `post`
/// warns after the tool ran, `audit` is only seen by the whole-tree audit,
/// and `none` parks a rule without deleting it. Anything else — including a
/// mis-cased `"Pre"` — is a load error: an unrecognized phase used to load
/// silently and never fire, so the rule allowed everything it was written to
/// stop.
pub const RULE_PHASES: &[&str] = &["pre", "post", "audit", "none"];

/// `then` verbs and the internal action type each one maps to. The single
/// source of truth for which actions a rule may take: the loader, the v1
/// `action_type` check, and the MCP `add_rule` tool all read this table.
/// An unknown verb is a load error, not a forward-compatible pass-through —
/// the hook only acts on these action types, so an unknown one loaded and
/// then did nothing (decision D1).
pub const ACTION_VERBS: &[(&str, &str)] = &[
    ("block", "constraint_violation"),
    ("warn", "constraint_warning"),
    ("log", "log"),
    ("emit_capsule", "emit_capsule"),
];

/// Top-level keys a rule object may carry (v2 and v1 legacy forms). A key
/// outside this set is a load error: the serializer never writes it back, and
/// a mis-cased `"Phase"` would otherwise be ignored while the rule silently
/// took the default phase.
const RULE_KEYS: &[&str] = &[
    "id",
    "phase",
    "priority",
    "audit",
    "silent",
    "doc_excepted",
    "binds",
    "when",
    "then",
    "conditions",
    "actions",
];

/// Comma-separated list, for "allowed: …" messages.
fn listed<'a>(items: impl IntoIterator<Item = &'a str>) -> String {
    items.into_iter().collect::<Vec<_>>().join(", ")
}

/// The internal action type for a `then` verb. The internal names
/// themselves (`constraint_violation`, …) are accepted as verbs too; that is
/// how older files and fixtures spell them, and it is unambiguous.
fn action_type_for_verb(verb: &str) -> Option<&'static str> {
    ACTION_VERBS
        .iter()
        .find(|(v, internal)| *v == verb || *internal == verb)
        .map(|(_, internal)| *internal)
}

/// A field-level rule check failure ([`check_action_type`],
/// [`check_conditions_nonempty`], [`check_single_action`]). The `Display`
/// text is byte-identical to the message strings the strict loader has
/// always produced, so hosts surface them unchanged.
#[derive(Debug, Error, Clone, PartialEq)]
#[error("{0}")]
pub struct RuleCheckError(pub String);

/// Validate an internal action type (the v1 `action_type` field and the MCP
/// `add_rule` tool), returning the load-error message when it is unknown.
pub fn check_action_type(rule_id: &str, field: &str, value: &str) -> Result<(), RuleCheckError> {
    if ACTION_VERBS.iter().any(|(_, internal)| *internal == value) {
        return Ok(());
    }
    Err(RuleCheckError(format!(
        "rule `{rule_id}`: field `{field}` has unknown value `{value}`; allowed: {}",
        listed(ACTION_VERBS.iter().map(|(_, internal)| *internal))
    )))
}

/// Validate a rule's condition count. A rule with no conditions never
/// fires, so an empty `when` loaded and silently allowed everything.
pub fn check_conditions_nonempty(
    rule_id: &str,
    field: &str,
    len: usize,
) -> Result<(), RuleCheckError> {
    if len == 0 {
        return Err(RuleCheckError(format!(
            "rule `{rule_id}`: field `{field}` is empty; a rule needs at least one condition"
        )));
    }
    Ok(())
}

/// Validate a rule's action count. A rule carries exactly one action; a
/// second one used to be dropped without a word.
pub fn check_single_action(rule_id: &str, field: &str, len: usize) -> Result<(), RuleCheckError> {
    if len != 1 {
        return Err(RuleCheckError(format!(
            "rule `{rule_id}`: field `{field}` must hold exactly one action, got {len}"
        )));
    }
    Ok(())
}

fn check_phase(phase: &str) -> anyhow::Result<()> {
    if RULE_PHASES.contains(&phase) {
        return Ok(());
    }
    anyhow::bail!(
        "field `phase` has invalid value `{phase}`; allowed: {}",
        listed(RULE_PHASES.iter().copied())
    )
}

/// Map a v2 `then` object (`{"block": "msg"}`) to an internal action via
/// [`ACTION_VERBS`]. An unknown verb is an error (fail closed at load).
fn parse_then_action(value: &serde_json::Value) -> anyhow::Result<DiskAction> {
    let obj = value
        .as_object()
        .ok_or_else(|| anyhow!("then must be a JSON object"))?;
    if obj.len() != 1 {
        return Err(anyhow!("then must have exactly one verb key"));
    }
    let (verb, msg_val) = obj.iter().next().expect("len==1");
    let action_type = action_type_for_verb(verb)
        .ok_or_else(|| {
            anyhow!(
                "field `then` has unknown verb `{verb}`; allowed: {}",
                listed(ACTION_VERBS.iter().map(|(v, _)| *v))
            )
        })?
        .to_string();
    if action_type == "emit_capsule" {
        if !msg_val.is_object() {
            anyhow::bail!("emit_capsule must be an object");
        }
        crate::capsule::validate_emit_capsule_payload(msg_val)
            .map_err(|error| anyhow!("Invalid capsule: {error}"))?;
    }
    // String-valued verbs (block/warn/log) keep their message as a param;
    // object-valued verbs (emit_capsule) carry the object as structured data.
    if let Some(s) = msg_val.as_str() {
        Ok(DiskAction {
            action_type,
            params: vec![s.to_string()],
            ..Default::default()
        })
    } else {
        Ok(DiskAction {
            action_type,
            params: Vec::new(),
            data: Some(msg_val.clone()),
        })
    }
}

/// Inverse of `parse_then_action`: internal action → v2 verb object.
fn action_to_then(action: &DiskAction) -> serde_json::Value {
    let verb = match action.action_type.as_str() {
        "constraint_violation" => "block",
        "constraint_warning" => "warn",
        "log" => "log",
        other => other,
    };
    match &action.data {
        Some(data) => serde_json::json!({ verb: data }),
        None => {
            let msg = action.params.first().cloned().unwrap_or_default();
            serde_json::json!({ verb: msg })
        }
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
    /// The `tag` verb and `tag` phase sit outside [`ACTION_VERBS`] and
    /// [`RULE_PHASES`]; taggers never pass through [`CompiledPolicy::compile`]
    /// or the strict loader.
    pub fn synthetic_tagger(tag: &str, when: &[serde_json::Value]) -> Result<Self, PolicyError> {
        let id = format!("tagger:{}", tag);
        let when_clauses: Vec<WhenClause> = when
            .iter()
            .map(|v| serde_json::from_value::<WhenClause>(v.clone()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| PolicyError::SyntheticTagger {
                source_id: id.clone(),
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
                ..Default::default()
            },
            silent: None,
            audit: None,
            doc_excepted: None,
            binds: None,
        })
    }
}

/// A v1 string-array field. A non-string element is an error: it used to be
/// dropped by a `filter_map`, which could empty a condition's arguments and
/// leave a rule that never matched.
fn string_array(value: Option<&serde_json::Value>, field: &str) -> anyhow::Result<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let items = value
        .as_array()
        .ok_or_else(|| anyhow!("field `{field}` must be an array of strings, got `{value}`"))?;
    items
        .iter()
        .map(|item| {
            item.as_str().map(String::from).ok_or_else(|| {
                anyhow!("field `{field}` has non-string value `{item}`; args must be strings")
            })
        })
        .collect()
}

fn parse_when_field(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> anyhow::Result<Vec<WhenClause>> {
    if obj.contains_key("when") && obj.contains_key("conditions") {
        anyhow::bail!("rule has both `when` and `conditions`; use one");
    }
    if let Some(when_val) = obj.get("when") {
        let arr = when_val
            .as_array()
            .ok_or_else(|| anyhow!("`when` must be an array"))?;
        if arr.is_empty() {
            anyhow::bail!("field `when` is empty; a rule needs at least one condition");
        }
        arr.iter()
            .map(|c| serde_json::from_value::<WhenClause>(c.clone()).map_err(|e| anyhow!("{}", e)))
            .collect()
    } else if let Some(cond_val) = obj.get("conditions") {
        // v1 legacy: each is {predicate, args, script?}.
        let arr = cond_val
            .as_array()
            .ok_or_else(|| anyhow!("`conditions` must be an array"))?;
        if arr.is_empty() {
            anyhow::bail!("field `conditions` is empty; a rule needs at least one condition");
        }
        arr.iter()
            .enumerate()
            .map(|(i, c)| {
                let co = c
                    .as_object()
                    .ok_or_else(|| anyhow!("v1 condition must be an object"))?;
                if let Some(key) = co
                    .keys()
                    .find(|k| !["predicate", "args", "script"].contains(&k.as_str()))
                {
                    anyhow::bail!(
                        "field `conditions[{i}]` has unknown key `{key}`; allowed: predicate, args, script"
                    );
                }
                let predicate = co
                    .get("predicate")
                    .and_then(|x| x.as_str())
                    .ok_or_else(|| anyhow!("v1 condition missing `predicate`"))?
                    .to_string();
                let args = string_array(co.get("args"), &format!("conditions[{i}].args"))?;
                let script = match co.get("script") {
                    None => None,
                    Some(x) => Some(
                        x.as_str()
                            .ok_or_else(|| {
                                anyhow!("field `conditions[{i}].script` must be a string, got `{x}`")
                            })?
                            .to_string(),
                    ),
                };
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
    if obj.contains_key("then") && obj.contains_key("actions") {
        anyhow::bail!("rule has both `then` and `actions`; use one");
    }
    if let Some(then_val) = obj.get("then") {
        parse_then_action(then_val)
    } else if let Some(actions_val) = obj.get("actions") {
        let actions = actions_val
            .as_array()
            .ok_or_else(|| anyhow!("v1 `actions` must be an array"))?;
        if actions.len() != 1 {
            anyhow::bail!(
                "field `actions` must hold exactly one action, got {}",
                actions.len()
            );
        }
        let ao = actions[0]
            .as_object()
            .ok_or_else(|| anyhow!("v1 action must be an object"))?;
        if let Some(key) = ao
            .keys()
            .find(|k| !["action_type", "params"].contains(&k.as_str()))
        {
            anyhow::bail!(
                "field `actions[0]` has unknown key `{key}`; allowed: action_type, params"
            );
        }
        let action_type = ao
            .get("action_type")
            .and_then(|x| x.as_str())
            .ok_or_else(|| anyhow!("v1 action missing `action_type`"))?
            .to_string();
        // `emit_capsule` needs a structured body, which only the v2 `then`
        // form carries; the v1 form can name the three message actions.
        if action_type == "emit_capsule" || !ACTION_VERBS.iter().any(|(_, t)| *t == action_type) {
            anyhow::bail!(
                "field `actions[0].action_type` has unknown value `{action_type}`; allowed: {} (use the v2 `then` form for emit_capsule)",
                listed(
                    ACTION_VERBS
                        .iter()
                        .map(|(_, t)| *t)
                        .filter(|t| *t != "emit_capsule")
                )
            );
        }
        let params = string_array(ao.get("params"), "actions[0].params")?;
        Ok(DiskAction {
            action_type,
            params,
            ..Default::default()
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
            .filter(|id| !id.is_empty())
            .ok_or_else(|| de::Error::custom("rule missing non-empty string `id`"))?
            .to_string();
        // Every semantic error names the rule, so a hook that refuses the
        // file tells the human exactly which rule and field to fix.
        let in_rule = |e: anyhow::Error| de::Error::custom(format!("rule `{id}`: {e}"));
        parse_source_rule(&id, obj).map_err(in_rule)
    }
}

/// Validate and parse one rule object once its `id` is known. Malformed
/// semantics fail closed here, at load, exactly like malformed JSON: every
/// reader of a rules document (hooks, adapters, audit, MCP tools) goes
/// through this function, so they all reject the same shapes with the same
/// message.
fn parse_source_rule(
    id: &str,
    obj: &serde_json::Map<String, serde_json::Value>,
) -> anyhow::Result<SourceRule> {
    if let Some(key) = obj.keys().find(|k| !RULE_KEYS.contains(&k.as_str())) {
        anyhow::bail!(
            "unknown key `{key}`; allowed keys: {}",
            listed(RULE_KEYS.iter().copied())
        );
    }
    let phase = match obj.get("phase") {
        // A missing phase keeps its documented default.
        None => "pre".to_string(),
        Some(value) => value
            .as_str()
            .ok_or_else(|| anyhow!("field `phase` must be a string, got `{value}`"))?
            .to_string(),
    };
    check_phase(&phase)?;
    let priority = match obj.get("priority") {
        None => 0,
        Some(value) => value
            .as_i64()
            .and_then(|p| i32::try_from(p).ok())
            .ok_or_else(|| anyhow!("field `priority` must be a 32-bit integer, got `{value}`"))?,
    };
    let flag = |key: &str| -> anyhow::Result<Option<bool>> {
        match obj.get(key) {
            None => Ok(None),
            Some(value) => value
                .as_bool()
                .map(Some)
                .ok_or_else(|| anyhow!("field `{key}` must be true or false, got `{value}`")),
        }
    };
    Ok(SourceRule {
        id: id.to_string(),
        phase,
        priority,
        when: parse_when_field(obj)?,
        then: parse_then_field(obj)?,
        silent: flag("silent")?,
        audit: flag("audit")?,
        doc_excepted: flag("doc_excepted")?,
        binds: flag("binds")?,
    })
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

#[derive(Debug, Clone, PartialEq)]
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

#[derive(Debug, Clone, PartialEq)]
pub struct DiskCondition {
    pub predicate: String,
    pub args: Vec<String>,
    pub script: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DiskAction {
    pub action_type: String,
    pub params: Vec<String>,
    /// Structured payload for verbs whose value is a JSON object (e.g.
    /// `emit_capsule`). `None` for string-valued verbs (block/warn/log).
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default)]
pub struct RulesFile {
    pub rules: Vec<DiskRule>,
}

/// Distinct failure kinds for parsing/compiling a rules document. The
/// `source_id` is the opaque label the caller supplied, or `"compiled"` for
/// [`CompiledPolicy::compile`].
#[derive(Debug, Error, Clone, PartialEq)]
pub enum PolicyError {
    /// The document is not valid JSON (or not the expected JSON shape).
    #[error("rules source `{source_id}` is malformed: {message}")]
    Malformed { source_id: String, message: String },
    /// The document parses but violates the strict load contract.
    #[error("rules source `{source_id}` is invalid: {message}")]
    Invalid { source_id: String, message: String },
    /// A rule's `when` could not be expanded to flat disk rules.
    #[error("rules source `{source_id}` could not be expanded: {message}")]
    Unfold { source_id: String, message: String },
    /// [`SourceRule::synthetic_tagger`] construction failure. Same payload
    /// shape as [`PolicyError::Unfold`], but the display keeps the legacy
    /// MCP wording ("rules file at …") because the MCP journey tagger
    /// stringifies this error verbatim into its own config-error type and
    /// that consumer's wording is outside this crate's control.
    #[error("rules file at {source_id} could not be expanded: {message}")]
    SyntheticTagger { source_id: String, message: String },
}

/// Structured replacement for the warnings the former loader printed to
/// stderr. Renderers (hooks, MCP) decide how to present these.
#[derive(Debug, Clone, PartialEq)]
pub enum PolicyDiagnostic {
    /// A source document listed the same rule id more than once with
    /// identical raw JSON; the extra copies were dropped and the first kept
    /// (legacy MCP behavior: "using one copy — delete the extras").
    DuplicateIdenticalRule { rule_id: String, message: String },
    /// A rule's OR expansion produced more than 32 flat rules
    /// ("consider simplifying").
    LargeOrExpansion { rule_id: String, message: String },
}

impl PolicyDiagnostic {
    /// The rule the diagnostic is about.
    pub fn rule_id(&self) -> &str {
        match self {
            PolicyDiagnostic::DuplicateIdenticalRule { rule_id, .. }
            | PolicyDiagnostic::LargeOrExpansion { rule_id, .. } => rule_id,
        }
    }

    /// The rendered warning text (without any transport prefix). Matches the
    /// exact strings the former stderr warnings carried after the
    /// "phronesis: warning: <source> " prefix.
    pub fn message(&self) -> &str {
        match self {
            PolicyDiagnostic::DuplicateIdenticalRule { message, .. }
            | PolicyDiagnostic::LargeOrExpansion { message, .. } => message,
        }
    }
}

/// OR-bearing source rules parsed from one supplied document, before
/// unfolding. `parse_source` does not install rules or read files.
#[derive(Debug, Clone)]
pub struct ParsedSource {
    pub rules: Vec<SourceRule>,
    pub diagnostics: Vec<PolicyDiagnostic>,
}

/// Flat disk rules parsed and unfolded from one supplied document.
#[derive(Debug, Clone)]
pub struct ParsedPolicy {
    pub file: RulesFile,
    pub diagnostics: Vec<PolicyDiagnostic>,
}

/// Parse one supplied document into OR-bearing [`SourceRule`]s without
/// unfolding. Identical duplicate rule objects (same id, identical raw JSON)
/// deduplicate with a [`PolicyDiagnostic::DuplicateIdenticalRule`]; same-id
/// rules with differing definitions are a [`PolicyError::Invalid`]. Neither
/// parse entry point reads files, prints, or compiles a RETE network.
pub fn parse_source(content: &str, source_id: &str) -> Result<ParsedSource, PolicyError> {
    #[derive(Deserialize)]
    struct Wrapper {
        rules: Vec<SourceRule>,
    }
    let w: Wrapper = serde_json::from_str(content).map_err(|e| PolicyError::Malformed {
        source_id: source_id.to_string(),
        message: e.to_string(),
    })?;
    dedupe_identical(content, source_id, w.rules)
}

/// Parse one supplied document into flat [`DiskRule`]s (v1 and v2 accepted),
/// unfolding OR clauses with `#orN` id suffixes. Mirrors the former
/// `rules_file::read` minus the filesystem read.
pub fn parse(content: &str, source_id: &str) -> Result<ParsedPolicy, PolicyError> {
    let parsed = parse_source(content, source_id)?;
    let mut flat = Vec::new();
    let mut diagnostics = parsed.diagnostics;
    for sr in &parsed.rules {
        let (expanded, diags) = unfold_or_with_diagnostics(sr).map_err(|e| match e {
            PolicyError::Unfold { message, .. } => PolicyError::Unfold {
                source_id: source_id.to_string(),
                message,
            },
            other => other,
        })?;
        diagnostics.extend(diags);
        flat.extend(expanded);
    }
    Ok(ParsedPolicy {
        file: RulesFile { rules: flat },
        diagnostics,
    })
}

/// Two rules with one id are ambiguous: which one the engine keeps depends
/// on load order, so differing copies are a load error. (Layer overrides
/// across files are the supported way to replace a rule.) Copies that are
/// identical JSON are not ambiguous — older MCP servers wrote them when a
/// guide was extracted twice — so they load as one rule, with a diagnostic
/// asking for the extras to be deleted.
fn dedupe_identical(
    content: &str,
    source_id: &str,
    rules: Vec<SourceRule>,
) -> Result<ParsedSource, PolicyError> {
    let invalid = |message: String| PolicyError::Invalid {
        source_id: source_id.to_string(),
        message,
    };
    let mut seen = std::collections::HashSet::new();
    if rules.iter().all(|r| seen.insert(r.id.as_str())) {
        return Ok(ParsedSource {
            rules,
            diagnostics: Vec::new(),
        });
    }
    // Only reached with duplicates: compare the raw JSON of each copy.
    #[derive(Deserialize)]
    struct Raw {
        rules: Vec<serde_json::Value>,
    }
    let raw: Raw = serde_json::from_str(content).map_err(|e| PolicyError::Malformed {
        source_id: source_id.to_string(),
        message: e.to_string(),
    })?;
    let mut first: HashMap<&str, &serde_json::Value> = HashMap::new();
    let mut keep = Vec::with_capacity(rules.len());
    let mut diagnostics = Vec::new();
    let mut warned = std::collections::HashSet::new();
    for (rule, value) in rules.iter().zip(&raw.rules) {
        match first.get(rule.id.as_str()) {
            None => {
                first.insert(rule.id.as_str(), value);
                keep.push(rule.clone());
            }
            Some(previous) if *previous == value => {
                if warned.insert(rule.id.as_str()) {
                    diagnostics.push(PolicyDiagnostic::DuplicateIdenticalRule {
                        rule_id: rule.id.clone(),
                        message: format!(
                            "lists rule `{}` more than once with identical content; using one copy — delete the extras",
                            rule.id
                        ),
                    });
                }
            }
            Some(_) => {
                return Err(invalid(format!(
                    "duplicate rule id `{}` with differing definitions; keep the one you want and delete the others (each rule id must appear once per file)",
                    rule.id
                )));
            }
        }
    }
    Ok(ParsedSource {
        rules: keep,
        diagnostics,
    })
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

/// Strip the OR-expansion suffix that `unfold_or` appends to child rule ids
/// (`#or0`, `#or0-or1`, …), returning the base id as authored on disk.
/// Decision pages, drift detectors, and consequence provenance identify
/// rules by this logical id; ids without an expansion suffix — including
/// ids that merely contain `#or` — are returned unchanged.
pub fn base_rule_id(id: &str) -> &str {
    match id.split_once('#') {
        Some((base, suffix)) if is_or_expansion_suffix(suffix) => base,
        _ => id,
    }
}

/// `unfold_or` suffix grammar: `or<digits>` per or-position, joined by `-`
/// for multi-position products (`#or0`, `#or0-or1`, `#or0-or1-or2`, …).
fn is_or_expansion_suffix(suffix: &str) -> bool {
    suffix.split('-').all(|part| {
        part.strip_prefix("or")
            .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// The OR-expansion position a flat rule id encodes relative to its source
/// rule: `r#or0-or1` against source `r` is `Some([0, 1])`; an unexpanded
/// rule (flat id equal to the source id, possibly with a literal `#` in
/// it) is `None`. Ids that do not follow the expansion grammar — not
/// producible by [`unfold_or_with_diagnostics`] — return `None`.
fn expansion_path(flat_id: &str, source_id: &str) -> Option<Vec<usize>> {
    let suffix = flat_id.strip_prefix(source_id)?.strip_prefix('#')?;
    suffix
        .split('-')
        .map(|part| part.strip_prefix("or")?.parse::<usize>().ok())
        .collect()
}

/// Expand a SourceRule's OR clauses into flat, OR-free DiskRules via
/// disjunctive-normal-form (DNF) expansion. Each OR position contributes
/// one alternative per (flattened) branch; the cartesian product across
/// all positions yields the output rules. Child ids are suffixed
/// deterministically: `#or0`, `#or1`, or `#or0-or1` for multi-position
/// products. A single-product result (no OR, or a single-element OR)
/// retains the original id unchanged. Structured diagnostics are dropped
/// here; prefer [`unfold_or_with_diagnostics`].
pub fn unfold_or(source: &SourceRule) -> Result<Vec<DiskRule>, PolicyError> {
    unfold_or_with_diagnostics(source).map(|(rules, _)| rules)
}

/// [`unfold_or`] with the structured diagnostics that replaced the former
/// stderr warning when a rule expands beyond 32 flat rules.
pub fn unfold_or_with_diagnostics(
    source: &SourceRule,
) -> Result<(Vec<DiskRule>, Vec<PolicyDiagnostic>), PolicyError> {
    // For each `when` position, compute the list of condition-alternatives.
    // A Leaf has exactly one alternative (no suffix contribution).
    // An Or contributes one alternative per flattened branch.
    fn alternatives(
        clause: &WhenClause,
        rule_id: &str,
    ) -> Result<Vec<Vec<DiskCondition>>, PolicyError> {
        match clause {
            WhenClause::Leaf(c) => Ok(vec![vec![c.clone()]]),
            WhenClause::Or(alts) => {
                if alts.is_empty() {
                    return Err(PolicyError::Unfold {
                        source_id: rule_id.to_string(),
                        message: format!("rule `{rule_id}` has an empty `or` (unsatisfiable)"),
                    });
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

    let mut diagnostics = Vec::new();
    if results.len() > 32 {
        diagnostics.push(PolicyDiagnostic::LargeOrExpansion {
            rule_id: source.id.clone(),
            message: format!(
                "rule `{}` expands to {} rules via OR — consider simplifying",
                source.id,
                results.len()
            ),
        });
    }

    let multiple = results.len() > 1;
    let rules = results
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
        .collect();
    Ok((rules, diagnostics))
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
                data: a.data.clone(),
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
                data: a.data.clone(),
            })
            .collect(),
    };
    (rule, disk.phase.clone())
}

/// Phases a compiled policy can select rules for at hook time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Pre,
    Post,
    Audit,
    None,
}

impl Phase {
    /// The disk-format phase string for this phase.
    pub fn as_str(&self) -> &'static str {
        match self {
            Phase::Pre => "pre",
            Phase::Post => "post",
            Phase::Audit => "audit",
            Phase::None => "none",
        }
    }

    /// Parse a disk-format phase string. Unknown strings (including
    /// mis-cased `"Pre"`) return `None`; callers fail closed.
    pub fn from_str_lossy(s: &str) -> Option<Phase> {
        match s {
            "pre" => Some(Phase::Pre),
            "post" => Some(Phase::Post),
            "audit" => Some(Phase::Audit),
            "none" => Some(Phase::None),
            _ => None,
        }
    }
}

impl std::fmt::Display for Phase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which rules a consumer wants out of a compiled policy.
///
/// - `Hook(Phase::Pre | Phase::Post | Phase::Audit)` selects rules whose
///   disk phase matches exactly; the `audit` flag is ignored at hook time
///   (source behavior: the hook filters `r.phase == phase`).
/// - `Hook(Phase::None)` selects nothing — a parked rule never runs.
/// - `Audit` selects exactly `audit == Some(true)` regardless of phase,
///   matching `filter_audit_rules` in the MCP audit engine. This preserves
///   an observed baseline discrepancy: a parked `none` rule marked
///   `audit: true` does appear in audits today; the guide prose saying
///   otherwise is a known documentation issue that must not be "fixed" by
///   changing source semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    Hook(Phase),
    Audit,
}

/// Original-source metadata for one flat rule, preserved across unfolding
/// by a private sidecar on [`CompiledPolicy`] instead of widening the
/// public [`DiskRule`] shape (whose literal constructors must keep
/// compiling). Only populated by the source route,
/// [`CompiledPolicy::compile_source`]; the flat
/// [`CompiledPolicy::compile`] route has no source metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceOrigin {
    /// The [`SourceRule`] id this flat rule was unfolded from. Equal to
    /// the flat rule's own id unless the source rule was OR-expanded.
    pub source_rule_id: String,
    /// The source-only `binds` flag: the graph-binding candidate
    /// extraction opt-out that [`DiskRule`] cannot carry.
    pub binds: Option<bool>,
    /// OR-expansion position of this flat rule within its source rule,
    /// as the `#or…` suffix segments in order. `None` when the source
    /// rule produced a single flat rule under its own id.
    pub expansion: Option<Vec<usize>>,
}

/// A validated set of flat disk rules ready for selection and engine
/// compilation.
///
/// Fields are private: the only constructor is [`CompiledPolicy::compile`],
/// which re-validates even programmatically built [`DiskRule`]s (phase,
/// action verb, condition/action counts, capsule payloads, duplicate ids)
/// exactly like the strict loader, so no invalid policy can exist.
#[derive(Debug, Clone)]
pub struct CompiledPolicy {
    rules: Vec<DiskRule>,
    diagnostics: Vec<PolicyDiagnostic>,
    /// Private source-metadata sidecar: per kept flat rule id, the source
    /// rule it was unfolded from. Empty for the flat compile route.
    source_origins: HashMap<String, SourceOrigin>,
}

impl CompiledPolicy {
    /// Validate and compile flat disk rules. Identical duplicate rules
    /// (same id, structurally equal) deduplicate with a
    /// [`PolicyDiagnostic::DuplicateIdenticalRule`], mirroring the loader;
    /// same-id rules with differing definitions are a
    /// [`PolicyError::Invalid`]. Every other check mirrors the strict
    /// source validation, using the same messages with `"conditions"` /
    /// `"actions"` as the field names.
    pub fn compile(rules: Vec<DiskRule>) -> Result<Self, PolicyError> {
        let (keep, diagnostics) = Self::validate_rules(rules, "compiled")?;
        Ok(CompiledPolicy {
            rules: keep,
            diagnostics,
            source_origins: HashMap::new(),
        })
    }

    /// Validate and compile source rules: unfold every OR clause with
    /// [`unfold_or_with_diagnostics`], record the private source sidecar
    /// (original source id, source-only `binds` flag, expansion position)
    /// for each flat rule, then run exactly the validation
    /// [`CompiledPolicy::compile`] runs — identical duplicate ids dedupe
    /// with a [`PolicyDiagnostic::DuplicateIdenticalRule`] and the sidecar
    /// entry of the first copy wins, same-id differing definitions are a
    /// [`PolicyError::Invalid`]. Unfold errors carry the offending
    /// source rule's id as `source_id`; unfold diagnostics (large OR
    /// expansions) surface in [`CompiledPolicy::diagnostics`].
    pub fn compile_source(sources: &[SourceRule]) -> Result<Self, PolicyError> {
        let mut flat = Vec::new();
        let mut sidecar: HashMap<String, SourceOrigin> = HashMap::new();
        let mut unfold_diagnostics = Vec::new();
        for source in sources {
            let (expanded, source_diagnostics) = unfold_or_with_diagnostics(source)?;
            unfold_diagnostics.extend(source_diagnostics);
            for rule in expanded {
                // Validation keeps the first copy of a duplicate id; the
                // sidecar must keep the first copy's origin too.
                sidecar.entry(rule.id.clone()).or_insert(SourceOrigin {
                    source_rule_id: source.id.clone(),
                    binds: source.binds,
                    expansion: expansion_path(&rule.id, &source.id),
                });
                flat.push(rule);
            }
        }
        let (keep, mut diagnostics) = Self::validate_rules(flat, "compiled")?;
        // Dropped identical duplicates share their id with the kept copy,
        // so keeping the kept ids' entries keeps the first copy's origin.
        let source_origins = keep
            .iter()
            .filter_map(|rule| {
                sidecar
                    .get(&rule.id)
                    .map(|origin| (rule.id.clone(), origin.clone()))
            })
            .collect();
        diagnostics.extend(unfold_diagnostics);
        Ok(CompiledPolicy {
            rules: keep,
            diagnostics,
            source_origins,
        })
    }

    /// Validate flat disk rules (strict loader contract) and dedupe
    /// identical duplicates, returning the kept rules and diagnostics.
    fn validate_rules(
        rules: Vec<DiskRule>,
        source_label: &str,
    ) -> Result<(Vec<DiskRule>, Vec<PolicyDiagnostic>), PolicyError> {
        let invalid = |message: String| PolicyError::Invalid {
            source_id: source_label.to_string(),
            message,
        };
        let mut seen: HashMap<&str, &DiskRule> = HashMap::new();
        let mut keep: Vec<DiskRule> = Vec::with_capacity(rules.len());
        let mut diagnostics = Vec::new();
        let mut warned = std::collections::HashSet::new();
        for rule in &rules {
            if rule.id.is_empty() {
                return Err(invalid("rule missing non-empty string `id`".to_string()));
            }
            check_phase(&rule.phase).map_err(|e| invalid(e.to_string()))?;
            check_conditions_nonempty(&rule.id, "conditions", rule.conditions.len())
                .map_err(|e| invalid(e.to_string()))?;
            check_single_action(&rule.id, "actions", rule.actions.len())
                .map_err(|e| invalid(e.to_string()))?;
            for action in &rule.actions {
                check_action_type(&rule.id, "action_type", &action.action_type)
                    .map_err(|e| invalid(e.to_string()))?;
                if action.action_type == "emit_capsule" {
                    match &action.data {
                        Some(data) if data.is_object() => {
                            crate::capsule::validate_emit_capsule_payload(data)
                                .map_err(|e| invalid(format!("Invalid capsule: {e}")))?;
                        }
                        _ => {
                            return Err(invalid("emit_capsule must be an object".to_string()));
                        }
                    }
                }
            }
            match seen.get(rule.id.as_str()) {
                None => {
                    seen.insert(rule.id.as_str(), rule);
                    keep.push(rule.clone());
                }
                Some(previous) if *previous == rule => {
                    if warned.insert(rule.id.as_str()) {
                        diagnostics.push(PolicyDiagnostic::DuplicateIdenticalRule {
                            rule_id: rule.id.clone(),
                            message: format!(
                                "lists rule `{}` more than once with identical content; using one copy — delete the extras",
                                rule.id
                            ),
                        });
                    }
                }
                Some(_) => {
                    return Err(invalid(format!(
                        "duplicate rule id `{}` with differing definitions; keep the one you want and delete the others (each rule id must appear once per file)",
                        rule.id
                    )));
                }
            }
        }
        Ok((keep, diagnostics))
    }

    /// The validated disk rules in input order, with all metadata
    /// (`silent`, `audit`, `doc_excepted`, phase, priority) intact.
    pub fn rules(&self) -> &[DiskRule] {
        &self.rules
    }

    /// Diagnostics produced while compiling (identical-duplicate dedupe).
    pub fn diagnostics(&self) -> &[PolicyDiagnostic] {
        &self.diagnostics
    }

    /// Source-route metadata for one compiled rule: the source rule it
    /// was unfolded from, the source-only `binds` flag, and the
    /// OR-expansion position. `None` when the policy was compiled from
    /// flat [`DiskRule`]s via [`CompiledPolicy::compile`] or the id is
    /// unknown.
    pub fn source_origin(&self, rule_id: &str) -> Option<&SourceOrigin> {
        self.source_origins.get(rule_id)
    }

    /// Select rules for a [`Selection`] and convert them to engine
    /// [`phr::Rule`]s in input order. Engine rules carry the full
    /// conversion metadata (id, priority, conditions, actions); the disk
    /// metadata remains available via [`CompiledPolicy::rules`].
    pub fn select(&self, selection: Selection) -> Vec<Rule> {
        self.rules
            .iter()
            .filter(|r| match selection {
                // Hook(None) selects nothing: a parked rule never runs,
                // even when its disk phase string would equal "none".
                Selection::Hook(Phase::None) => false,
                Selection::Hook(phase) => phase.as_str() == r.phase.as_str(),
                // Baseline behavior (filter_audit_rules): exactly
                // audit == Some(true), regardless of phase.
                Selection::Audit => r.audit == Some(true),
            })
            .map(|r| rule_from_disk(r).0)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(pred: &str, arg: &str) -> WhenClause {
        WhenClause::Leaf(DiskCondition {
            predicate: pred.into(),
            args: vec![arg.into()],
            script: None,
        })
    }

    fn leaf_of(clause: &WhenClause) -> &DiskCondition {
        match clause {
            WhenClause::Leaf(c) => c,
            WhenClause::Or(_) => panic!("expected leaf"),
        }
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

    fn doc(json: &str, source_id: &str) -> ParsedPolicy {
        parse(json, source_id).unwrap_or_else(|e| panic!("parse failed: {e}"))
    }

    fn parse_rule(json: &str) -> Result<SourceRule, String> {
        serde_json::from_str::<SourceRule>(json).map_err(|e| e.to_string())
    }

    fn err_message(err: &PolicyError) -> &str {
        match err {
            PolicyError::Malformed { message, .. }
            | PolicyError::Invalid { message, .. }
            | PolicyError::Unfold { message, .. }
            | PolicyError::SyntheticTagger { message, .. } => message,
        }
    }

    fn err_kind(err: &PolicyError) -> &'static str {
        match err {
            PolicyError::Malformed { .. } => "malformed",
            PolicyError::Invalid { .. } => "invalid",
            PolicyError::Unfold { .. } => "unfold",
            PolicyError::SyntheticTagger { .. } => "synthetic_tagger",
        }
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
            WhenClause::Leaf(c) => assert_eq!(
                c.args,
                vec!["?file".to_string(), "?fn".to_string(), "?count".to_string()]
            ),
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
                assert_eq!(leaf_of(&alts[0]).args, vec!["cargo test".to_string()]);
                assert_eq!(leaf_of(&alts[1]).args, vec!["cargo nextest".to_string()]);
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
    fn when_clause_rejects_non_object_and_key_count() {
        for bad in ["\"str\"", "5", "[]", "null", "true"] {
            let err = serde_json::from_str::<WhenClause>(bad).unwrap_err();
            assert!(err.to_string().contains("JSON object"), "{bad}: {err}");
        }
        assert!(
            serde_json::from_str::<WhenClause>("{}")
                .unwrap_err()
                .to_string()
                .contains("exactly one key")
        );
        assert!(
            serde_json::from_str::<WhenClause>(r#"{"a": "1", "b": "2"}"#)
                .unwrap_err()
                .to_string()
                .contains("exactly one key")
        );
    }

    #[test]
    fn when_clause_rejects_bad_arg_types() {
        for src in [r#"{"p": 5}"#, r#"{"p": {"x": 1}}"#, r#"{"p": null}"#] {
            let err = serde_json::from_str::<WhenClause>(src).unwrap_err();
            assert!(
                err.to_string().contains("string, array, or bool"),
                "{src}: {err}"
            );
        }
        let err = serde_json::from_str::<WhenClause>(r#"{"p": ["ok", 1]}"#).unwrap_err();
        assert!(err.to_string().contains("must contain strings"), "{err}");
    }

    #[test]
    fn when_clause_rejects_bad_or_shapes() {
        let err = serde_json::from_str::<WhenClause>(r#"{"or": "not-an-array"}"#).unwrap_err();
        assert!(err.to_string().contains("array of clauses"), "{err}");
        let err = serde_json::from_str::<WhenClause>(r#"{"or": [{"a": "1"}, 5]}"#).unwrap_err();
        assert!(err.to_string().contains("JSON object"), "{err}");
    }

    #[test]
    fn when_clause_round_trips_through_serialize() {
        for s in [
            r#"{"a":"1"}"#,
            r#"{"b":["x","y"]}"#,
            r#"{"c":true}"#,
            r#"{"__script__":"x > 1"}"#,
            r#"{"or":[{"a":"1"},{"or":[{"b":true},{"__script__":"y"}]}]}"#,
        ] {
            let clause: WhenClause = serde_json::from_str(s).expect("parse");
            let out = serde_json::to_string(&clause).expect("serialize");
            let back: WhenClause = serde_json::from_str(&out).expect("reparse");
            let again = serde_json::to_string(&back).expect("reserialize");
            assert_eq!(out, again, "round trip unstable for {s}");
            let v1: serde_json::Value = serde_json::from_str(s).unwrap();
            let v2: serde_json::Value = serde_json::from_str(&out).unwrap();
            assert_eq!(v1, v2, "semantic drift for {s}");
        }
    }

    #[test]
    fn source_rule_parses_v2() {
        let json = r#"{
            "id": "r1", "phase": "pre", "priority": 10, "audit": true, "binds": false,
            "when": [ { "new_content_contains": ".unwrap()" }, { "file_path_matches": "src" } ],
            "then": { "block": "no unwrap" }
        }"#;
        let sr = parse_rule(json).unwrap();
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
        let sr = parse_rule(json).unwrap();
        assert_eq!(sr.id, "r1");
        assert_eq!(sr.when.len(), 1);
        assert_eq!(leaf_of(&sr.when[0]).predicate, "new_content_contains");
        assert_eq!(sr.then.action_type, "constraint_violation");
    }

    #[test]
    fn source_rule_serializes_v2_round_trip() {
        let json = r#"{
            "id": "r1", "phase": "pre", "priority": 10, "audit": true, "binds": false,
            "when": [ { "new_content_contains": ".unwrap()" }, { "file_path_matches": "src" } ],
            "then": { "block": "no unwrap" }
        }"#;
        let sr = parse_rule(json).unwrap();
        let out = serde_json::to_value(&sr).unwrap();
        let sr2 = parse_rule(&serde_json::to_string(&out).unwrap()).unwrap();
        assert_eq!(sr2.id, "r1");
        assert_eq!(sr2.when.len(), 2);
        assert_eq!(sr2.then.action_type, "constraint_violation");
        assert_eq!(sr2.binds, Some(false));
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
        let sr = parse_rule(json).unwrap();
        let out = serde_json::to_value(&sr).unwrap();
        assert!(out["when"][0]["or"].is_array());
        assert_eq!(out["when"][0]["or"][0]["new_content_contains"], "a");
    }

    #[test]
    fn source_rule_omits_absent_flags_on_serialize() {
        let sr =
            parse_rule(r#"{ "id": "r1", "when": [ { "p": true } ], "then": { "warn": "m" } }"#)
                .unwrap();
        let out = serde_json::to_value(&sr).unwrap();
        assert!(out.get("audit").is_none());
        assert!(out.get("silent").is_none());
        assert!(out.get("doc_excepted").is_none());
        assert!(out.get("binds").is_none());
        assert_eq!(out["phase"], "pre");
        assert_eq!(out["priority"], 0);
    }

    #[test]
    fn emit_capsule_structured_action_round_trips_canonically() {
        let value = serde_json::json!({"emit_capsule": {
            "id": "review-layering", "body": "why: ?reason", "lifecycle": "session",
            "priority": 50, "max_bytes": 512, "expires_after": "7d"
        }});
        let doc = doc(
            &format!(
                r#"{{ "rules": [ {{ "id": "c", "when": [{{"p": true}}], "then": {value} }} ] }}"#
            ),
            "t",
        );
        let action = &doc.file.rules[0].actions[0];
        assert!(action.params.is_empty());
        assert_eq!(action.data.as_ref().unwrap()["body"], "why: ?reason");
        let sr = &doc_file_source(&doc)[0];
        let out = serde_json::to_value(sr).unwrap();
        assert_eq!(out["then"], value);
    }

    // Helper only used above: rebuild a SourceRule from the parsed doc's
    // first flat rule (single-condition, no OR), exercising the inverse
    // action mapping.
    fn doc_file_source(doc: &ParsedPolicy) -> Vec<SourceRule> {
        doc.file
            .rules
            .iter()
            .map(diskrule_to_source_for_tests)
            .collect()
    }

    fn diskrule_to_source_for_tests(d: &DiskRule) -> SourceRule {
        SourceRule {
            id: d.id.clone(),
            phase: d.phase.clone(),
            priority: d.priority,
            when: d.conditions.iter().cloned().map(WhenClause::Leaf).collect(),
            then: d.actions.first().cloned().unwrap_or_default(),
            silent: d.silent,
            audit: d.audit,
            doc_excepted: d.doc_excepted,
            binds: None,
        }
    }

    #[test]
    fn emit_capsule_rejects_invalid_contract_values() {
        for value in [
            serde_json::json!({"emit_capsule":{"id":"?dynamic","body":"x","lifecycle":"session"}}),
            serde_json::json!({"emit_capsule":{"id":"x","body":"x","lifecycle":"forever"}}),
            serde_json::json!({"emit_capsule":{"id":"x","body":"x","lifecycle":"persistent","expires_after":"0d"}}),
            serde_json::json!({"emit_capsule":{"id":"x","body":"x","lifecycle":"session","unknown":true}}),
        ] {
            let json = format!(
                r#"{{ "rules": [ {{ "id": "c", "when": [{{"p": true}}], "then": {value} }} ] }}"#
            );
            let err = parse(&json, "t").unwrap_err();
            assert_eq!(err_kind(&err), "malformed", "accepted {value}");
            assert!(err_message(&err).contains("Invalid capsule:"), "{err}");
        }
    }

    #[test]
    fn emit_capsule_via_v1_actions_is_rejected() {
        let json = r#"{ "rules": [ { "id": "c",
            "when": [ { "p": true } ],
            "actions": [ { "action_type": "emit_capsule", "params": [] } ] } ] }"#;
        let err = parse(json, "t").unwrap_err();
        assert!(
            err_message(&err).contains("use the v2 `then` form for emit_capsule"),
            "{err}"
        );
    }

    #[test]
    fn strict_load_errors_fail_closed_with_rule_names() {
        let cases: Vec<(&str, &str, &str)> = vec![
            (
                r#"{ "rules": [ { "id": "r1", "phase": "Pre", "when": [ { "p": true } ], "then": { "warn": "m" } } ] }"#,
                "malformed",
                "rule `r1`: field `phase` has invalid value `Pre`; allowed: pre, post, audit, none",
            ),
            (
                r#"{ "rules": [ { "id": "r1", "phase": 5, "when": [ { "p": true } ], "then": { "warn": "m" } } ] }"#,
                "malformed",
                "rule `r1`: field `phase` must be a string, got `5`",
            ),
            (
                r#"{ "rules": [ { "id": "r1", "priority": 99999999999, "when": [ { "p": true } ], "then": { "warn": "m" } } ] }"#,
                "malformed",
                "rule `r1`: field `priority` must be a 32-bit integer",
            ),
            (
                r#"{ "rules": [ { "id": "r1", "unknown_key": 1, "when": [ { "p": true } ], "then": { "warn": "m" } } ] }"#,
                "malformed",
                "rule `r1`: unknown key `unknown_key`",
            ),
            (
                r#"{ "rules": [ { "id": "r1", "when": [], "then": { "warn": "m" } } ] }"#,
                "malformed",
                "rule `r1`: field `when` is empty; a rule needs at least one condition",
            ),
            (
                r#"{ "rules": [ { "id": "r1", "when": [ { "p": true } ] } ] }"#,
                "malformed",
                "rule `r1`: rule has neither `then` nor `actions`",
            ),
            (
                r#"{ "rules": [ { "id": "r1", "when": [ { "p": true } ], "then": { "explode": "m" } } ] }"#,
                "malformed",
                "rule `r1`: field `then` has unknown verb `explode`; allowed: block, warn, log, emit_capsule",
            ),
        ];
        for (json, kind, needle) in cases {
            let err = parse(json, "doc").unwrap_err();
            assert_eq!(err_kind(&err), kind, "{json}: {err}");
            if needle != "unused placeholder" {
                assert!(
                    err_message(&err).contains(needle),
                    "message mismatch: {err} (want substring {needle})"
                );
            }
        }
    }

    #[test]
    fn parse_rejects_both_when_and_conditions() {
        let json = r#"{ "rules": [ { "id": "r1", "when": [ { "p": true } ],
            "conditions": [ {"predicate":"q","args":[]} ],
            "then": { "warn": "m" } } ] }"#;
        let err = parse(json, "t").unwrap_err();
        assert!(
            err_message(&err).contains("both `when` and `conditions`"),
            "{err}"
        );
    }

    #[test]
    fn parse_rejects_both_then_and_actions() {
        let json = r#"{ "rules": [ { "id": "r1", "when": [ { "p": true } ],
            "then": { "warn": "m" }, "actions": [ {"action_type":"log","params":["m"]} ] } ] }"#;
        let err = parse(json, "t").unwrap_err();
        assert!(
            err_message(&err).contains("both `then` and `actions`"),
            "{err}"
        );
    }

    #[test]
    fn parse_rejects_malformed_json_and_missing_id() {
        let err = parse("{not valid", "doc").unwrap_err();
        assert_eq!(err_kind(&err), "malformed");
        let err = parse(
            r#"{ "rules": [ { "when": [ { "p": true } ], "then": { "warn": "m" } } ] }"#,
            "doc",
        )
        .unwrap_err();
        assert!(
            err_message(&err).contains("rule missing non-empty string `id`"),
            "{err}"
        );
        let err = parse(
            r#"{ "rules": [ { "id": "", "when": [ { "p": true } ], "then": { "warn": "m" } } ] }"#,
            "doc",
        )
        .unwrap_err();
        assert!(
            err_message(&err).contains("rule missing non-empty string `id`"),
            "{err}"
        );
    }

    #[test]
    fn source_id_is_opaque_in_errors() {
        let err = parse("{not valid", "pack:rust/starter").unwrap_err();
        match &err {
            PolicyError::Malformed { source_id, message } => {
                assert_eq!(source_id, "pack:rust/starter");
                assert!(!message.contains("pack"), "{message}");
            }
            other => panic!("wrong kind: {other}"),
        }
        assert!(
            err.to_string()
                .contains("rules source `pack:rust/starter` is malformed")
        );
    }

    #[test]
    fn v1_non_string_arg_errors() {
        let json = r#"{ "rules": [ { "id": "r1",
            "conditions": [ {"predicate":"q","args":[5]} ],
            "actions": [ {"action_type":"log","params":["m"]} ] } ] }"#;
        let err = parse(json, "t").unwrap_err();
        assert!(err_message(&err).contains("conditions[0].args"), "{err}");
    }

    #[test]
    fn v1_unknown_condition_key_errors() {
        let json = r#"{ "rules": [ { "id": "r1",
            "conditions": [ {"predicate":"q","args":[],"bogus":1} ],
            "actions": [ {"action_type":"log","params":["m"]} ] } ] }"#;
        let err = parse(json, "t").unwrap_err();
        assert!(err_message(&err).contains("unknown key `bogus`"), "{err}");
    }

    #[test]
    fn v1_two_actions_errors() {
        let json = r#"{ "rules": [ { "id": "r1", "when": [ { "p": true } ],
            "actions": [ {"action_type":"log","params":["m"]}, {"action_type":"log","params":["n"]} ] } ] }"#;
        let err = parse(json, "t").unwrap_err();
        assert!(
            err_message(&err).contains("field `actions` must hold exactly one action, got 2"),
            "{err}"
        );
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
    fn unfold_empty_or_errors_unfold_kind() {
        let s = src("r", vec![WhenClause::Or(vec![])]);
        let err = unfold_or(&s).unwrap_err();
        assert_eq!(err_kind(&err), "unfold");
        assert!(
            err_message(&err).contains("has an empty `or` (unsatisfiable)"),
            "{err}"
        );
        // parse() maps the unfold error onto the document's opaque source id.
        let err = parse(
            r#"{ "rules": [ { "id": "r", "when": [ { "or": [] } ], "then": { "warn": "m" } } ] }"#,
            "doc",
        )
        .unwrap_err();
        match &err {
            PolicyError::Unfold { source_id, message } => {
                assert_eq!(source_id, "doc");
                assert!(message.contains("empty `or`"), "{message}");
            }
            other => panic!("wrong kind: {other}"),
        }
    }

    #[test]
    fn unfold_single_element_or_degenerates() {
        let s = src("r", vec![WhenClause::Or(vec![leaf("a", "1")])]);
        let out = unfold_or(&s).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "r"); // no suffix when only one product
    }

    #[test]
    fn large_or_expansion_yields_diagnostic_not_stderr() {
        let alts: Vec<WhenClause> = (0..7)
            .map(|i| {
                WhenClause::Or(vec![
                    leaf(&format!("p{i}a"), "1"),
                    leaf(&format!("p{i}b"), "2"),
                ])
            })
            .collect();
        let s = src("r", alts);
        let (out, diagnostics) = unfold_or_with_diagnostics(&s).unwrap();
        assert_eq!(out.len(), 128);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].rule_id(), "r");
        assert!(
            diagnostics[0]
                .message()
                .contains("expands to 128 rules via OR — consider simplifying"),
            "{}",
            diagnostics[0].message()
        );
        // 32 or fewer: no diagnostic.
        let alts: Vec<WhenClause> = (0..5)
            .map(|i| {
                WhenClause::Or(vec![
                    leaf(&format!("p{i}a"), "1"),
                    leaf(&format!("p{i}b"), "2"),
                ])
            })
            .collect();
        assert_eq!(unfold_or(&src("r", alts.clone())).unwrap().len(), 32);
        let (_, diagnostics) = unfold_or_with_diagnostics(&src("r", alts)).unwrap();
        assert!(diagnostics.is_empty(), "32 is not large");
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
    fn parse_source_identical_duplicates_dedupe_with_diagnostic() {
        let json = r#"{ "rules": [
            { "id": "r1", "when": [ { "p": true } ], "then": { "warn": "m" } },
            { "id": "r1", "when": [ { "p": true } ], "then": { "warn": "m" } },
            { "id": "r2", "when": [ { "q": true } ], "then": { "log": "m" } },
            { "id": "r2", "when": [ { "q": true } ], "then": { "log": "m" } },
            { "id": "r2", "when": [ { "q": true } ], "then": { "log": "m" } }
        ] }"#;
        let parsed = parse_source(json, "doc").unwrap();
        assert_eq!(parsed.rules.len(), 2);
        let dupes: Vec<&str> = parsed.diagnostics.iter().map(|d| d.rule_id()).collect();
        assert_eq!(dupes, vec!["r1", "r2"], "one diagnostic per rule id");
        for d in &parsed.diagnostics {
            assert!(
                d.message().contains(
                    "more than once with identical content; using one copy — delete the extras"
                ),
                "{}",
                d.message()
            );
        }
        // parse() propagates the same diagnostics and keeps one flat rule each.
        let parsed = parse(json, "doc").unwrap();
        assert_eq!(parsed.file.rules.len(), 2);
        assert_eq!(parsed.diagnostics.len(), 2);
    }

    #[test]
    fn parse_source_differing_duplicates_error() {
        let json = r#"{ "rules": [
            { "id": "r1", "when": [ { "p": true } ], "then": { "warn": "m" } },
            { "id": "r1", "when": [ { "p": true } ], "then": { "warn": "different" } }
        ] }"#;
        let err = parse_source(json, "doc").unwrap_err();
        assert_eq!(err_kind(&err), "invalid");
        assert!(
            err_message(&err)
                .contains("duplicate rule id `r1` with differing definitions; keep the one you want and delete the others (each rule id must appear once per file)"),
            "{err}"
        );
    }

    #[test]
    fn parse_keeps_first_position_for_identical_duplicates() {
        let json = r#"{ "rules": [
            { "id": "a", "priority": 1, "when": [ { "p": true } ], "then": { "log": "first" } },
            { "id": "b", "priority": 2, "when": [ { "p": true } ], "then": { "log": "mid" } },
            { "id": "a", "priority": 1, "when": [ { "p": true } ], "then": { "log": "first" } }
        ] }"#;
        let parsed = parse(json, "doc").unwrap();
        let ids: Vec<(&str, i32)> = parsed
            .file
            .rules
            .iter()
            .map(|r| (r.id.as_str(), r.priority))
            .collect();
        assert_eq!(ids, vec![("a", 1), ("b", 2)]);
    }

    #[test]
    fn selection_hook_matches_phase_exactly_ignoring_audit_flag() {
        let mk = |id: &str, phase: &str, audit: Option<bool>| DiskRule {
            id: id.into(),
            phase: phase.into(),
            priority: 1,
            conditions: vec![DiskCondition {
                predicate: "p".into(),
                args: vec![],
                script: None,
            }],
            actions: vec![DiskAction {
                action_type: "log".into(),
                params: vec![],
                ..Default::default()
            }],
            silent: None,
            audit,
            doc_excepted: None,
        };
        let policy = CompiledPolicy::compile(vec![
            mk("pre-a", "pre", None),
            mk("pre-b", "pre", Some(true)),
            mk("post", "post", Some(true)),
            mk("audit-phase", "audit", Some(true)),
            mk("parked-audit", "none", Some(true)),
            mk("parked", "none", None),
        ])
        .unwrap();
        let ids = |s: Selection| -> Vec<String> {
            policy.select(s).iter().map(|r| r.id.clone()).collect()
        };
        // Hook pre: audit flag ignored.
        assert_eq!(ids(Selection::Hook(Phase::Pre)), vec!["pre-a", "pre-b"]);
        assert_eq!(ids(Selection::Hook(Phase::Post)), vec!["post"]);
        assert_eq!(ids(Selection::Hook(Phase::Audit)), vec!["audit-phase"]);
        // None never runs.
        assert!(ids(Selection::Hook(Phase::None)).is_empty());
        // Audit mode: exactly audit == Some(true), regardless of phase —
        // including the parked `none` rule (preserved baseline discrepancy).
        assert_eq!(
            ids(Selection::Audit),
            vec!["pre-b", "post", "audit-phase", "parked-audit"]
        );
    }

    #[test]
    fn compiled_policy_retains_metadata_and_converts_rules() {
        let rule = Rule {
            id: "r1".into(),
            priority: 7,
            conditions: vec![Condition {
                predicate: "new_content_contains".into(),
                args: vec![".unwrap()".into()],
                script: None,
            }],
            actions: vec![Action {
                action_type: "constraint_violation".into(),
                params: vec!["no unwrap".into()],
                data: None,
            }],
        };
        let mut disk = rule_to_disk(&rule, "pre");
        disk.silent = Some(true);
        disk.audit = Some(true);
        disk.doc_excepted = Some(true);
        let policy = CompiledPolicy::compile(vec![disk]).unwrap();
        assert_eq!(policy.rules().len(), 1);
        let kept = &policy.rules()[0];
        assert_eq!(kept.phase, "pre");
        assert_eq!(kept.silent, Some(true));
        assert_eq!(kept.audit, Some(true));
        assert_eq!(kept.doc_excepted, Some(true));
        assert_eq!(kept.priority, 7);
        let (engine_rule, phase) = rule_from_disk(kept);
        assert_eq!(engine_rule.id, "r1");
        assert_eq!(engine_rule.priority, 7);
        assert_eq!(phase, "pre");
        let selected = policy.select(Selection::Hook(Phase::Pre));
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].conditions[0].predicate, "new_content_contains");
    }

    #[test]
    fn rule_to_disk_from_disk_round_trip() {
        let r = Rule {
            id: "t".into(),
            priority: 5,
            conditions: vec![Condition {
                predicate: "p".into(),
                args: vec!["x".into()],
                script: Some("a > 1".into()),
            }],
            actions: vec![Action {
                action_type: "log".into(),
                params: vec!["y".into()],
                data: None,
            }],
        };
        let d = rule_to_disk(&r, "post");
        assert_eq!(d.phase, "post");
        let (r2, phase) = rule_from_disk(&d);
        assert_eq!(r2.id, "t");
        assert_eq!(r2.priority, 5);
        assert_eq!(r2.conditions[0].script.as_deref(), Some("a > 1"));
        assert_eq!(phase, "post");
    }

    #[test]
    fn compile_rejects_programmatic_invalid_rules() {
        let base = DiskRule {
            id: "r1".into(),
            phase: "pre".into(),
            priority: 1,
            conditions: vec![DiskCondition {
                predicate: "p".into(),
                args: vec![],
                script: None,
            }],
            actions: vec![DiskAction {
                action_type: "log".into(),
                params: vec![],
                ..Default::default()
            }],
            silent: None,
            audit: None,
            doc_excepted: None,
        };
        let invalid = |rule: DiskRule| {
            CompiledPolicy::compile(vec![rule])
                .map_err(|e| (err_kind(&e).to_string(), err_message(&e).to_string()))
        };
        let mut r = base.clone();
        r.phase = "Pre".into();
        assert_eq!(
            invalid(r).unwrap_err(),
            (
                "invalid".into(),
                "field `phase` has invalid value `Pre`; allowed: pre, post, audit, none".into()
            )
        );
        let mut r = base.clone();
        r.conditions.clear();
        assert_eq!(
            invalid(r).unwrap_err(),
            (
                "invalid".into(),
                "rule `r1`: field `conditions` is empty; a rule needs at least one condition"
                    .into()
            )
        );
        let mut r = base.clone();
        r.actions.push(DiskAction {
            action_type: "warn".into(),
            params: vec!["m".into()],
            ..Default::default()
        });
        assert_eq!(
            invalid(r).unwrap_err(),
            (
                "invalid".into(),
                "rule `r1`: field `actions` must hold exactly one action, got 2".into()
            )
        );
        let mut r = base.clone();
        r.actions[0].action_type = "explode".into();
        assert_eq!(
            invalid(r).unwrap_err().1,
            "rule `r1`: field `action_type` has unknown value `explode`; allowed: constraint_violation, constraint_warning, log, emit_capsule"
        );
        let mut r = base.clone();
        r.actions[0].action_type = "emit_capsule".into();
        r.actions[0].data = None;
        assert_eq!(invalid(r).unwrap_err().1, "emit_capsule must be an object");
        let mut r = base;
        r.actions[0].action_type = "emit_capsule".into();
        r.actions[0].data = Some(serde_json::json!({"id":"?x","body":"b","lifecycle":"session"}));
        let (_, msg) = invalid(r).unwrap_err();
        assert!(msg.contains("Invalid capsule:"), "{msg}");
    }

    #[test]
    fn compile_dedupes_identical_and_errors_on_differing() {
        let base = DiskRule {
            id: "r1".into(),
            phase: "pre".into(),
            priority: 1,
            conditions: vec![DiskCondition {
                predicate: "p".into(),
                args: vec![],
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
        };
        let ok = CompiledPolicy::compile(vec![base.clone(), base.clone()]).unwrap();
        assert_eq!(ok.rules().len(), 1);
        assert_eq!(ok.diagnostics().len(), 1);
        assert_eq!(ok.diagnostics()[0].rule_id(), "r1");
        let mut diff = base.clone();
        diff.actions[0].params = vec!["other".into()];
        let err = CompiledPolicy::compile(vec![base, diff]).unwrap_err();
        assert!(
            err_message(&err).contains("duplicate rule id `r1` with differing definitions"),
            "{err}"
        );
    }

    #[test]
    fn compile_rejects_duplicate_expanded_base_ids_only_for_full_ids() {
        // OR-expanded ids share a base but differ in full id: both kept.
        let policy = CompiledPolicy::compile(vec![
            DiskRule {
                id: "r#or0".into(),
                phase: "pre".into(),
                priority: 1,
                conditions: vec![DiskCondition {
                    predicate: "a".into(),
                    args: vec![],
                    script: None,
                }],
                actions: vec![DiskAction {
                    action_type: "log".into(),
                    ..Default::default()
                }],
                silent: None,
                audit: None,
                doc_excepted: None,
            },
            DiskRule {
                id: "r#or1".into(),
                phase: "pre".into(),
                priority: 1,
                conditions: vec![DiskCondition {
                    predicate: "b".into(),
                    args: vec![],
                    script: None,
                }],
                actions: vec![DiskAction {
                    action_type: "log".into(),
                    ..Default::default()
                }],
                silent: None,
                audit: None,
                doc_excepted: None,
            },
        ])
        .unwrap();
        assert_eq!(policy.rules().len(), 2);
        assert_eq!(base_rule_id(&policy.rules()[0].id), "r");
    }

    #[test]
    fn parse_doc_to_flat_rules_with_suffixes() {
        let parsed = doc(
            r#"{ "rules": [
            { "id": "r1", "phase": "pre", "priority": 10,
              "when": [ { "or": [ { "new_content_contains": "a" }, { "new_content_contains": "b" } ] } ],
              "then": { "block": "no" } }
        ] }"#,
            "doc",
        );
        assert_eq!(parsed.file.rules.len(), 2);
        assert_eq!(parsed.file.rules[0].id, "r1#or0");
        assert_eq!(
            parsed.file.rules[0].actions[0].action_type,
            "constraint_violation"
        );
        assert_eq!(
            parsed.file.rules[1].conditions[0].predicate,
            "new_content_contains"
        );
    }

    #[test]
    fn phase_enum_maps_disk_strings() {
        for (s, p) in [
            ("pre", Phase::Pre),
            ("post", Phase::Post),
            ("audit", Phase::Audit),
            ("none", Phase::None),
        ] {
            assert_eq!(Phase::from_str_lossy(s), Some(p));
            assert_eq!(p.as_str(), s);
            assert_eq!(p.to_string(), s);
        }
        assert_eq!(Phase::from_str_lossy("Pre"), None);
        assert_eq!(Phase::from_str_lossy("tag"), None);
    }

    #[test]
    fn synthetic_tagger_keeps_tag_verb_and_phase() {
        let tagger = SourceRule::synthetic_tagger(
            "missing-test",
            &[serde_json::json!({"change_set_has_production_rust": true})],
        )
        .unwrap();
        assert_eq!(tagger.id, "tagger:missing-test");
        assert_eq!(tagger.phase, "tag");
        assert_eq!(tagger.then.action_type, "tag");
        assert_eq!(tagger.then.params, vec!["missing-test"]);
        let expanded = unfold_or(&tagger).unwrap();
        assert_eq!(expanded.len(), 1);
        // Tag phase/verb sit outside the strict vocabulary, so compile()
        // (and therefore hooks/audit) never sees them; that is the
        // documented boundary.
        assert_eq!(Phase::from_str_lossy(&expanded[0].phase), None);
        assert!(CompiledPolicy::compile(expanded).is_err());
    }

    #[test]
    fn synthetic_tagger_malformed_when_is_unfold_error() {
        let err = SourceRule::synthetic_tagger("t", &[serde_json::json!({"p": 5})]).unwrap_err();
        assert_eq!(err_kind(&err), "synthetic_tagger");
        assert_eq!(
            err.to_string(),
            "rules file at tagger:t could not be expanded: predicate value must be string, array, or bool; got 5"
        );
        assert!(matches!(
            &err,
            PolicyError::SyntheticTagger { source_id, .. } if source_id == "tagger:t"
        ));
    }

    #[test]
    fn check_helpers_produce_exact_legacy_messages() {
        assert_eq!(
            check_action_type("r", "action_type", "zap").unwrap_err().0,
            "rule `r`: field `action_type` has unknown value `zap`; allowed: constraint_violation, constraint_warning, log, emit_capsule"
        );
        assert_eq!(
            check_conditions_nonempty("r", "conditions", 0)
                .unwrap_err()
                .0,
            "rule `r`: field `conditions` is empty; a rule needs at least one condition"
        );
        assert_eq!(
            check_single_action("r", "actions", 2).unwrap_err().0,
            "rule `r`: field `actions` must hold exactly one action, got 2"
        );
        assert!(check_action_type("r", "action_type", "log").is_ok());
        assert!(check_action_type("r", "action_type", "constraint_violation").is_ok());
        // Verbs that are not internal names ("block") are NOT accepted here;
        // the verb vocabulary is checked via `then` parsing.
        assert!(check_action_type("r", "action_type", "block").is_err());
    }

    #[test]
    fn zero_arg_bool_leaf_serializes_as_true() {
        let c = WhenClause::Leaf(DiskCondition {
            predicate: "flag".into(),
            args: vec![],
            script: None,
        });
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v, serde_json::json!({ "flag": true }));
    }

    #[test]
    fn parse_accepts_missing_phase_defaults_to_pre() {
        let parsed = doc(
            r#"{ "rules": [ { "id": "r1", "when": [ { "p": true } ], "then": { "warn": "m" } } ] }"#,
            "doc",
        );
        assert_eq!(parsed.file.rules[0].phase, "pre");
        assert_eq!(parsed.file.rules[0].priority, 0);
    }

    #[test]
    fn diagnostics_have_no_stderr_and_match_legacy_text() {
        // Duplicate diagnostic text matches the former eprintln content
        // after the "phronesis: warning: {source} " prefix.
        let parsed = parse_source(
            r#"{ "rules": [
                { "id": "r1", "when": [ { "p": true } ], "then": { "warn": "m" } },
                { "id": "r1", "when": [ { "p": true } ], "then": { "warn": "m" } } ] }"#,
            "rules.json",
        )
        .unwrap();
        assert_eq!(
            parsed.diagnostics[0].message(),
            "lists rule `r1` more than once with identical content; using one copy — delete the extras"
        );
    }
}

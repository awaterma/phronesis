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
            .map_err(|e| PolicyError::Unfold {
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
        let invalid = |message: String| PolicyError::Invalid {
            source_id: "compiled".to_string(),
            message,
        };
        let mut seen: HashMap<&str, &DiskRule> = HashMap::new();
        let mut keep: Vec<DiskRule> = Vec::with_capacity(rules.len());
        let mut diagnostics = Vec::new();
        let mut warned = std::collections::HashSet::new();
        for rule in &rules {
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
        Ok(CompiledPolicy {
            rules: keep,
            diagnostics,
        })
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

    /// Select rules for a [`Selection`] and convert them to engine
    /// [`phr::Rule`]s in input order. Engine rules carry the full
    /// conversion metadata (id, priority, conditions, actions); the disk
    /// metadata remains available via [`CompiledPolicy::rules`].
    pub fn select(&self, selection: Selection) -> Vec<Rule> {
        self.rules
            .iter()
            .filter(|r| match selection {
                Selection::Hook(phase) => phase.as_str() == r.phase.as_str(),
                // Baseline behavior (filter_audit_rules): exactly
                // audit == Some(true), regardless of phase.
                Selection::Audit => r.audit == Some(true),
            })
            .map(|r| rule_from_disk(r).0)
            .collect()
    }
}

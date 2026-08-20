use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use phr::{Action, Condition as ReteCondition, Fact, ReteNetwork, Rule};
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use thiserror::Error;

use super::packing::{ContextItem, ItemKind};

const CAPSULE_DIR: &str = ".phronesis/nudges";
const FILE_MAX_BYTES: u64 = 8 * 1024;
const AGGREGATE_MAX_BYTES: u64 = 256 * 1024;

pub const ALLOWED_PREDICATES: &[&str] = &[
    "context_confidence_band",
    "journey_filtered_since_ge",
    "journey_seen",
    "journey_since_ge",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapsuleCondition {
    Leaf {
        predicate: String,
        args: Vec<String>,
    },
    All(Vec<CapsuleCondition>),
    Any(Vec<CapsuleCondition>),
}

#[derive(Debug, Clone)]
pub struct Capsule {
    pub id: String,
    pub priority: i32,
    pub max_bytes: usize,
    pub when: CapsuleCondition,
    pub body: String,
    pub path: PathBuf,
}

impl Capsule {
    pub fn item(&self) -> ContextItem {
        let mut item = ContextItem::new(
            ItemKind::Nudge,
            self.id.clone(),
            format!("{}\n\n[phronesis nudge: {}]", self.body, self.id),
        );
        item.priority = self.priority;
        item
    }
}

#[derive(Debug, Error)]
pub enum CapsuleError {
    #[error("{path}: {message}")]
    Invalid { path: String, message: String },
    #[error("{path}: io error: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

/// Validation failure for one `when` tree. Carries only the message; the
/// enclosing file path is attached by [`CapsuleError::Invalid`] at the call
/// site that knows which file the tree came from.
#[derive(Debug, Error)]
#[error("{0}")]
pub struct ConditionError(String);

impl ConditionError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

/// Validation failure for one capsule file's frontmatter or body. Like
/// [`ConditionError`] it carries only the message; the enclosing file path is
/// attached by [`CapsuleError::Invalid`] at the call site that knows it.
#[derive(Debug, Error)]
#[error("{0}")]
struct FieldError(String);

impl FieldError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    /// Flatten a foreign error (serde's, in practice) to its rendered message,
    /// which is what [`CapsuleError::Invalid`] carries anyway.
    fn from_display(error: impl std::fmt::Display) -> Self {
        Self(error.to_string())
    }
}

#[derive(Debug, Default)]
pub struct CapsuleLoad {
    pub capsules: Vec<Capsule>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug)]
enum StrictValue {
    Null,
    Bool(bool),
    Number(serde_json::Number),
    String(String),
    Array(Vec<StrictValue>),
    Object(Vec<(String, StrictValue)>),
}

impl StrictValue {
    fn into_json(self) -> serde_json::Value {
        match self {
            Self::Null => serde_json::Value::Null,
            Self::Bool(v) => v.into(),
            Self::Number(v) => v.into(),
            Self::String(v) => v.into(),
            Self::Array(v) => v.into_iter().map(Self::into_json).collect(),
            Self::Object(v) => v
                .into_iter()
                .map(|(k, v)| (k, v.into_json()))
                .collect::<serde_json::Map<_, _>>()
                .into(),
        }
    }
}

struct StrictVisitor;

impl<'de> Visitor<'de> for StrictVisitor {
    type Value = StrictValue;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a JSON value without duplicate object keys")
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
        Ok(StrictValue::Bool(v))
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
        Ok(StrictValue::Number(v.into()))
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
        Ok(StrictValue::Number(v.into()))
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
        serde_json::Number::from_f64(v)
            .map(StrictValue::Number)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
        Ok(StrictValue::String(v.to_string()))
    }
    fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
        Ok(StrictValue::String(v))
    }
    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(StrictValue::Null)
    }
    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(StrictValue::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut out = Vec::new();
        while let Some(v) = seq.next_element::<StrictValue>()? {
            out.push(v);
        }
        Ok(StrictValue::Array(out))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut keys = BTreeSet::new();
        let mut out = Vec::new();
        while let Some(key) = map.next_key::<String>()? {
            if !keys.insert(key.clone()) {
                return Err(de::Error::custom(format!("duplicate key `{key}`")));
            }
            out.push((key, map.next_value::<StrictValue>()?));
        }
        Ok(StrictValue::Object(out))
    }
}

impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(StrictVisitor)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CapsuleFrontmatter {
    id: String,
    priority: i32,
    max_bytes: usize,
    when: serde_json::Value,
}

/// Maximum `all`/`any` nesting depth, matching the readable v2 rules parser.
const MAX_CONDITION_DEPTH: usize = 16;
/// Maximum DNF alternatives one capsule may expand to.
const MAX_DNF_ALTERNATIVES: usize = 256;

fn parse_condition(value: &serde_json::Value) -> Result<CapsuleCondition, ConditionError> {
    parse_condition_at(value, 0)
}

fn parse_condition_at(
    value: &serde_json::Value,
    depth: usize,
) -> Result<CapsuleCondition, ConditionError> {
    if depth > MAX_CONDITION_DEPTH {
        return Err(ConditionError::new(format!(
            "condition nesting exceeds {MAX_CONDITION_DEPTH} levels"
        )));
    }
    let obj = value
        .as_object()
        .ok_or_else(|| ConditionError::new("when node must be an object"))?;
    if let Some(predicate) = obj.get("predicate") {
        if obj.keys().any(|k| k != "predicate" && k != "args") {
            return Err(ConditionError::new(
                "leaf condition contains unknown fields",
            ));
        }
        let predicate = predicate
            .as_str()
            .ok_or_else(|| ConditionError::new("predicate must be a string"))?;
        if !ALLOWED_PREDICATES.contains(&predicate) {
            return Err(ConditionError::new(format!(
                "predicate `{predicate}` is not allowlisted"
            )));
        }
        let args = obj
            .get("args")
            .and_then(|v| v.as_array())
            .ok_or_else(|| ConditionError::new("args must be an array"))?
            .iter()
            .map(|v| {
                let s = v
                    .as_str()
                    .ok_or_else(|| ConditionError::new("args must contain strings"))?;
                if s.starts_with('?') || s.len() > 256 || s.chars().any(char::is_control) {
                    return Err(ConditionError::new(
                        "args contain a variable, control character, or oversized string",
                    ));
                }
                Ok(s.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        validate_arity(predicate, &args)?;
        return Ok(CapsuleCondition::Leaf {
            predicate: predicate.to_string(),
            args,
        });
    }
    // Exactly one group operator per node. Checked before destructuring so
    // the single-entry read below cannot fail.
    let mut entries = obj.iter();
    let (Some((kind, children)), None) = (entries.next(), entries.next()) else {
        return Err(ConditionError::new(
            "condition must contain exactly one of predicate, all, or any",
        ));
    };
    if !matches!(kind.as_str(), "all" | "any") {
        return Err(ConditionError::new(format!(
            "unsupported condition operator `{kind}`"
        )));
    }
    let children = children
        .as_array()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| ConditionError::new(format!("{kind} must be a non-empty array")))?
        .iter()
        .map(|value| parse_condition_at(value, depth + 1))
        .collect::<Result<Vec<_>, _>>()?;
    match kind.as_str() {
        "all" => Ok(CapsuleCondition::All(children)),
        _ => Ok(CapsuleCondition::Any(children)),
    }
}

fn validate_arity(predicate: &str, args: &[String]) -> Result<(), ConditionError> {
    let expected = match predicate {
        "journey_seen" | "journey_since_ge" => 2,
        "journey_filtered_since_ge" => 3,
        "context_confidence_band" => 1,
        _ => {
            return Err(ConditionError::new(format!(
                "predicate `{predicate}` is not allowlisted"
            )));
        }
    };
    if args.len() != expected {
        return Err(ConditionError::new(format!(
            "predicate `{predicate}` requires {expected} arguments"
        )));
    }
    if predicate == "context_confidence_band"
        && !matches!(args[0].as_str(), "low" | "medium" | "high")
    {
        return Err(ConditionError::new(
            "context_confidence_band must be low, medium, or high",
        ));
    }
    if matches!(predicate, "journey_since_ge" | "journey_filtered_since_ge") {
        // Canonical positive decimal only: reject "+3", " 3", "03", "3.0".
        let raw = args.last().map(String::as_str).unwrap_or("");
        let canonical = !raw.is_empty()
            && raw.bytes().all(|b| b.is_ascii_digit())
            && (raw == "0" || !raw.starts_with('0'));
        let k = if canonical {
            raw.parse::<u32>().unwrap_or(0)
        } else {
            0
        };
        if k == 0 || k > 10_000 {
            return Err(ConditionError::new(
                "journey threshold must be a canonical positive integer no greater than 10000",
            ));
        }
    }
    Ok(())
}

/// Split a capsule file into its `---json` frontmatter and its body.
///
/// Both delimiters must be whole lines: the opening `---json` and a closing
/// line of exactly `---`. An indented or extended rule (`  ---`, `----`) does
/// not close the block, and only the first closing line does — later `---`
/// lines are ordinary Markdown and stay in the body.
///
/// Either line ending is accepted, and they are not required to agree; a file
/// mixing the two parses. Tightening that would be a behavior change, so it is
/// documented rather than assumed.
fn split_frontmatter(text: &str) -> Result<(&str, &str), FieldError> {
    let rest = text
        .strip_prefix("---json\n")
        .or_else(|| text.strip_prefix("---json\r\n"))
        .ok_or_else(|| FieldError::new("first line must be ---json"))?;
    rest.split_once("\n---\n")
        .or_else(|| rest.split_once("\r\n---\r\n"))
        .ok_or_else(|| FieldError::new("missing exact closing --- line"))
}

/// Deserialize the frontmatter block through [`StrictValue`], which rejects
/// duplicate keys at every depth, and require it to be exactly one JSON value
/// with no trailing input.
fn parse_frontmatter(front: &str) -> Result<CapsuleFrontmatter, FieldError> {
    let strict = {
        let mut de = serde_json::Deserializer::from_str(front);
        let strict = StrictValue::deserialize(&mut de).map_err(FieldError::from_display)?;
        de.end().map_err(FieldError::from_display)?;
        strict
    };
    serde_json::from_value(strict.into_json()).map_err(FieldError::from_display)
}

/// Range checks on the declared frontmatter fields.
fn validate_frontmatter(fm: &CapsuleFrontmatter) -> Result<(), FieldError> {
    if !valid_id(&fm.id) {
        return Err(FieldError::new("id must match [a-z0-9][a-z0-9-]{0,63}"));
    }
    if !(0..=100).contains(&fm.priority) {
        return Err(FieldError::new("priority must be between 0 and 100"));
    }
    if !(64..=1024).contains(&fm.max_bytes) {
        return Err(FieldError::new("max_bytes must be between 64 and 1024"));
    }
    Ok(())
}

/// Trim and vet a capsule body, returning the exact text that will be shown.
///
/// Capsule bodies are static: no fact argument is ever interpolated into one.
/// Rejecting variable and template syntax outright stops an author believing
/// otherwise, which is what keeps a filename or tool payload from becoming a
/// second-order prompt-injection channel.
fn validate_body(raw: &str, max_bytes: usize) -> Result<String, FieldError> {
    let body = raw.trim_end_matches(char::is_whitespace).to_string();
    if body.is_empty() {
        return Err(FieldError::new("body must be non-empty"));
    }
    let variable = body.split_whitespace().any(|word| {
        word.strip_prefix('?')
            .and_then(|rest| rest.chars().next())
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
    });
    if variable || body.contains("{{") || body.contains("}}") || body.contains("${") {
        return Err(FieldError::new(
            "templates and interpolation directives are not supported",
        ));
    }
    if body.len() > max_bytes {
        return Err(FieldError::new(format!(
            "body exceeds declared max_bytes ({max_bytes})"
        )));
    }
    Ok(body)
}

fn parse_file(path: &Path) -> Result<Capsule, CapsuleError> {
    let invalid = |message: String| CapsuleError::Invalid {
        path: path.display().to_string(),
        message,
    };
    let text = {
        let io = |source| CapsuleError::Io {
            path: path.display().to_string(),
            source,
        };
        let metadata = std::fs::metadata(path).map_err(io)?;
        if metadata.len() > FILE_MAX_BYTES {
            return Err(invalid("capsule exceeds 8 KiB".to_string()));
        }
        std::fs::read_to_string(path).map_err(io)?
    };
    let (front, body) = split_frontmatter(&text).map_err(|e| invalid(e.to_string()))?;
    let fm = {
        let fm = parse_frontmatter(front).map_err(|e| invalid(e.to_string()))?;
        validate_frontmatter(&fm).map_err(|e| invalid(e.to_string()))?;
        fm
    };
    let body = validate_body(body, fm.max_bytes).map_err(|e| invalid(e.to_string()))?;
    Ok(Capsule {
        id: fm.id,
        priority: fm.priority,
        max_bytes: fm.max_bytes,
        when: {
            let when = parse_condition(&fm.when).map_err(|e| invalid(e.to_string()))?;
            // Prove the expansion bound at load time so the compile step in
            // `rules()` cannot be surprised by an unbounded capsule.
            dnf(&when).map_err(|e| invalid(e.to_string()))?;
            when
        },
        body,
        path: path.to_path_buf(),
    })
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && (id.as_bytes()[0].is_ascii_lowercase() || id.as_bytes()[0].is_ascii_digit())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Candidate capsule files in `<root>/.phronesis/nudges`, in bytewise
/// filename order.
///
/// A missing directory and an unreadable one both yield no candidates; a
/// directory that resolves outside the project root also records why. The
/// empty vec is not an error condition — [`load`] has nothing to parse either
/// way, so the caller's behavior is identical.
fn capsule_paths(root: &Path, diagnostics: &mut Vec<String>) -> Vec<PathBuf> {
    let dir = root.join(CAPSULE_DIR);
    if !dir.exists() {
        return Vec::new();
    }
    let dir = match crate::security::resolve_safe_path(&dir.display().to_string(), root) {
        Ok(dir) => dir,
        Err(error) => {
            diagnostics.push(error.to_string());
            return Vec::new();
        }
    };
    let Ok(read_dir) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut paths = read_dir
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("md"))
        // `README.md` documents the directory; `init` writes one itself.
        // Parsing it as a capsule would fail on every single hook.
        .filter(|p| {
            !p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.eq_ignore_ascii_case("README.md"))
        })
        .collect::<Vec<_>>();
    paths.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    paths
}

/// Parse each candidate under the aggregate byte cap, grouped by declared id.
///
/// Grouping (rather than inserting) is what lets [`load`] drop *every* copy of
/// a duplicated id instead of letting file order pick a winner.
fn parse_grouped(
    root: &Path,
    paths: Vec<PathBuf>,
    diagnostics: &mut Vec<String>,
) -> BTreeMap<String, Vec<Capsule>> {
    let mut total = 0u64;
    let mut by_id: BTreeMap<String, Vec<Capsule>> = BTreeMap::new();
    for path in paths {
        let path = match crate::security::resolve_safe_path(&path.display().to_string(), root) {
            Ok(path) => path,
            Err(error) => {
                diagnostics.push(error.to_string());
                continue;
            }
        };
        total = total.saturating_add(std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0));
        if total > AGGREGATE_MAX_BYTES {
            diagnostics.push("capsule input exceeds aggregate 256 KiB cap".to_string());
            break;
        }
        match parse_file(&path) {
            Ok(c) => by_id.entry(c.id.clone()).or_default().push(c),
            Err(e) => diagnostics.push(e.to_string()),
        }
    }
    by_id
}

pub fn load(root: &Path) -> CapsuleLoad {
    let mut out = CapsuleLoad::default();
    let by_id = {
        let paths = capsule_paths(root, &mut out.diagnostics);
        parse_grouped(root, paths, &mut out.diagnostics)
    };
    for (id, capsules) in by_id {
        if capsules.len() == 1 {
            out.capsules.extend(capsules);
        } else {
            out.diagnostics
                .push(format!("duplicate capsule id `{id}`; all copies skipped"));
        }
    }
    out.capsules
        .sort_by(|a, b| a.path.file_name().cmp(&b.path.file_name()));
    out
}

fn dnf(condition: &CapsuleCondition) -> Result<Vec<Vec<ReteCondition>>, ConditionError> {
    match condition {
        CapsuleCondition::Leaf { predicate, args } => Ok(vec![vec![ReteCondition {
            predicate: predicate.clone(),
            args: args.clone(),
            script: None,
        }]]),
        CapsuleCondition::Any(children) => {
            let mut out = Vec::new();
            for child in children {
                out.extend(dnf(child)?);
                if out.len() > MAX_DNF_ALTERNATIVES {
                    return Err(ConditionError::new(format!(
                        "condition expands to more than {MAX_DNF_ALTERNATIVES} alternatives"
                    )));
                }
            }
            Ok(out)
        }
        CapsuleCondition::All(children) => {
            let mut products = vec![Vec::new()];
            for child in children {
                let alternatives = dnf(child)?;
                products = products
                    .into_iter()
                    .flat_map(|prefix| {
                        alternatives.iter().map(move |alt| {
                            let mut joined = prefix.clone();
                            joined.extend(alt.clone());
                            joined
                        })
                    })
                    .collect();
                if products.len() > MAX_DNF_ALTERNATIVES {
                    return Err(ConditionError::new(format!(
                        "condition expands to more than {MAX_DNF_ALTERNATIVES} alternatives"
                    )));
                }
            }
            Ok(products)
        }
    }
}

/// Compile validated capsules into match-only RETE rules.
///
/// `dnf` is re-run here rather than cached, and its error is surfaced instead
/// of swallowed: `parse_file` already proved every stored capsule expands
/// within the alternative cap, so a failure at this point is an internal
/// inconsistency the caller should report, not silently drop.
fn rules(capsules: &[Capsule]) -> Result<Vec<Rule>, CapsuleError> {
    let mut out = Vec::new();
    for capsule in capsules {
        let alternatives = dnf(&capsule.when).map_err(|error| CapsuleError::Invalid {
            path: capsule.path.display().to_string(),
            message: error.to_string(),
        })?;
        for (i, conditions) in alternatives.into_iter().enumerate() {
            out.push(Rule {
                id: format!("context:{}:{i}", capsule.id),
                priority: capsule.priority,
                conditions,
                actions: vec![Action {
                    action_type: "context_nudge".to_string(),
                    params: vec![capsule.id.clone()],
                    data: None,
                }],
            });
        }
    }
    Ok(out)
}

/// Result of one capsule-selection pass. Diagnostics record which demanded
/// facts could not be hydrated, so `context inspect` can explain a capsule
/// that failed to match for a reason other than "the facts were false".
#[derive(Debug, Default)]
pub struct MatchOutcome {
    pub items: Vec<ContextItem>,
    pub matched_ids: Vec<String>,
    pub diagnostics: Vec<String>,
}

/// Derive the `journey_*` aggregator facts the compiled rules demand.
///
/// Returns the diagnostic to record, if derivation was unavailable. A failure
/// is not fatal to the pass: non-journey capsules can still match.
async fn hydrate_journey(
    root: &Path,
    rules: &[Rule],
    now_ts: u64,
    network: &mut ReteNetwork,
) -> Option<String> {
    let config = crate::journey::load_config(root).unwrap_or_default();
    let sid = crate::journey::current_sid(root);
    let input = crate::journey::derive::DeriveInput {
        project_root: root,
        rules,
        config: &config,
        scope: crate::journey::derive::WindowScope {
            current_sid: &sid,
            now_ts,
        },
    };
    crate::journey::derive::assert_facts(network, input)
        .await
        .err()
        .map(|error| {
            format!(
                "journey derivation unavailable: {error}; journey-triggered capsules cannot match"
            )
        })
}

/// Assert the `context_confidence_band` projection, if it exists.
///
/// The band fact is a context-only projection that exists only when confidence
/// is configured AND a subject is open. Absence produces no substitute fact —
/// the dependent capsule simply cannot match — so each absent case returns the
/// diagnostic that explains why rather than a falsified band.
async fn hydrate_confidence_band(
    root: &Path,
    now_ts: u64,
    network: &mut ReteNetwork,
) -> Option<String> {
    if !crate::outcomes::enabled(root) {
        return Some(
            "confidence is not configured; `context_confidence_band` capsules cannot match"
                .to_string(),
        );
    }
    let Some(report) = crate::outcomes::report(root, None) else {
        return Some(
            "no open subject; `context_confidence_band` capsules cannot match".to_string(),
        );
    };
    network
        .assert_fact(Fact {
            id: "context:confidence-band".to_string(),
            predicate: "context_confidence_band".to_string(),
            args: vec![report.band.as_str().to_string()],
            timestamp: now_ts,
            source: Some("context".to_string()),
        })
        .await
        .err()
        .map(|error| format!("confidence band fact rejected: {error}"))
}

pub async fn matched(root: &Path, capsules: &[Capsule], now_ts: u64) -> MatchOutcome {
    let mut out = MatchOutcome::default();
    if capsules.is_empty() {
        return out;
    }
    let rules = match rules(capsules) {
        Ok(rules) => rules,
        Err(error) => {
            out.diagnostics.push(error.to_string());
            return out;
        }
    };
    let mut network = ReteNetwork::new();
    for rule in &rules {
        if let Err(error) = network.add_rule(rule.clone()).await {
            out.diagnostics
                .push(format!("rule `{}` rejected by engine: {error}", rule.id));
            return out;
        }
    }
    let demands = |prefix: &str| {
        rules
            .iter()
            .any(|r| r.conditions.iter().any(|c| c.predicate.starts_with(prefix)))
    };
    if demands("journey_") {
        out.diagnostics
            .extend(hydrate_journey(root, &rules, now_ts, &mut network).await);
    }
    if demands("context_confidence_band") {
        out.diagnostics
            .extend(hydrate_confidence_band(root, now_ts, &mut network).await);
    }
    let consequences = match network.fire_all_consequences() {
        Ok(consequences) => consequences,
        Err(error) => {
            out.diagnostics
                .push(format!("capsule evaluation failed: {error}"));
            return out;
        }
    };
    let ids = consequences
        .iter()
        .filter(|c| c.payload.get("action_type").and_then(|v| v.as_str()) == Some("context_nudge"))
        .filter_map(|c| c.payload.get("params")?.as_array()?.first()?.as_str())
        .collect::<BTreeSet<_>>();
    for capsule in capsules.iter().filter(|c| ids.contains(c.id.as_str())) {
        out.matched_ids.push(capsule.id.clone());
        out.items.push(capsule.item());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_capsule(root: &Path, name: &str, body: &str) {
        let dir = root.join(CAPSULE_DIR);
        std::fs::create_dir_all(&dir).expect("mkdir nudges");
        std::fs::write(dir.join(name), body).expect("write capsule");
    }

    /// A well-formed capsule file with the given frontmatter and body.
    fn capsule_text(id: &str, when: &str, body: &str) -> String {
        format!(
            "---json\n{{\"id\":\"{id}\",\"priority\":50,\"max_bytes\":512,\"when\":{when}}}\n---\n{body}\n"
        )
    }

    const LOW_BAND: &str = r#"{"predicate":"context_confidence_band","args":["low"]}"#;

    fn diagnostics_mention(load: &CapsuleLoad, needle: &str) -> bool {
        load.diagnostics.iter().any(|d| d.contains(needle))
    }

    /// A project with confidence configured and an open subject, so the
    /// `context_confidence_band` fact is actually produced.
    fn project_with_confidence() -> tempfile::TempDir {
        let d = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(d.path().join(".phronesis")).expect("mkdir .phronesis");
        std::fs::write(d.path().join(".phronesis/confidence.json"), "{}")
            .expect("write confidence marker");
        crate::outcomes::subject::set(d.path(), "unit").expect("set subject");
        d
    }

    #[test]
    fn duplicate_json_keys_are_rejected() {
        let d = tempfile::tempdir().unwrap();
        write_capsule(
            d.path(),
            "x.md",
            "---json\n{\"id\":\"x\",\"id\":\"y\",\"priority\":1,\"max_bytes\":64,\"when\":{\"predicate\":\"context_confidence_band\",\"args\":[\"low\"]}}\n---\nbody",
        );
        let loaded = load(d.path());
        assert!(loaded.capsules.is_empty());
        assert!(loaded.diagnostics[0].contains("duplicate key"));
    }

    #[test]
    fn disallowed_predicate_is_rejected() {
        let d = tempfile::tempdir().unwrap();
        write_capsule(
            d.path(),
            "x.md",
            "---json\n{\"id\":\"x\",\"priority\":1,\"max_bytes\":64,\"when\":{\"predicate\":\"new_content_contains\",\"args\":[]}}\n---\nbody",
        );
        assert!(load(d.path()).capsules.is_empty());
    }

    #[tokio::test]
    async fn confidence_capsule_matches_low_without_signals() {
        let d = tempfile::tempdir().expect("tempdir");
        // The band fact exists only when confidence is configured; the marker
        // file is what `outcomes::enabled` reads.
        std::fs::create_dir_all(d.path().join(".phronesis")).expect("mkdir .phronesis");
        std::fs::write(d.path().join(".phronesis/confidence.json"), "{}")
            .expect("write confidence marker");
        crate::outcomes::subject::set(d.path(), "unit").expect("set subject");
        let capsule = Capsule {
            id: "low".into(),
            priority: 1,
            max_bytes: 100,
            when: CapsuleCondition::Leaf {
                predicate: "context_confidence_band".into(),
                args: vec!["low".into()],
            },
            body: "verify".into(),
            path: PathBuf::from("low.md"),
        };
        let found = matched(d.path(), &[capsule], 1).await;
        assert_eq!(found.matched_ids, ["low"]);
        assert_eq!(found.items.len(), 1);
    }

    // ── frontmatter strictness ──────────────────────────────────────────

    #[test]
    fn nested_duplicate_json_keys_are_rejected() {
        // The duplicate is inside `when`, not at the root — a visitor that
        // only checks the top level would let this through.
        let d = tempfile::tempdir().expect("tempdir");
        write_capsule(
            d.path(),
            "x.md",
            "---json\n{\"id\":\"x\",\"priority\":1,\"max_bytes\":64,\
             \"when\":{\"predicate\":\"context_confidence_band\",\"args\":[\"low\"],\"args\":[\"high\"]}}\n---\nbody",
        );
        let loaded = load(d.path());
        assert!(loaded.capsules.is_empty());
        assert!(diagnostics_mention(&loaded, "duplicate key"));
    }

    #[test]
    fn trailing_json_after_the_object_is_rejected() {
        let d = tempfile::tempdir().expect("tempdir");
        write_capsule(
            d.path(),
            "x.md",
            "---json\n{\"id\":\"x\",\"priority\":1,\"max_bytes\":64,\"when\":{\"predicate\":\"context_confidence_band\",\"args\":[\"low\"]}} 7\n---\nbody",
        );
        assert!(load(d.path()).capsules.is_empty());
    }

    #[test]
    fn non_object_root_is_rejected() {
        let d = tempfile::tempdir().expect("tempdir");
        write_capsule(d.path(), "x.md", "---json\n[1,2,3]\n---\nbody");
        assert!(load(d.path()).capsules.is_empty());
    }

    #[test]
    fn unknown_frontmatter_field_is_rejected() {
        let d = tempfile::tempdir().expect("tempdir");
        write_capsule(
            d.path(),
            "x.md",
            "---json\n{\"id\":\"x\",\"priority\":1,\"max_bytes\":64,\"surprise\":true,\"when\":{\"predicate\":\"context_confidence_band\",\"args\":[\"low\"]}}\n---\nbody",
        );
        assert!(load(d.path()).capsules.is_empty());
    }

    #[test]
    fn wrong_scalar_type_is_rejected() {
        let d = tempfile::tempdir().expect("tempdir");
        write_capsule(
            d.path(),
            "x.md",
            "---json\n{\"id\":\"x\",\"priority\":\"high\",\"max_bytes\":64,\"when\":{\"predicate\":\"context_confidence_band\",\"args\":[\"low\"]}}\n---\nbody",
        );
        assert!(load(d.path()).capsules.is_empty());
    }

    #[test]
    fn invalid_id_shapes_are_rejected() {
        for id in ["-leading", "Upper", "has_underscore", ""] {
            let d = tempfile::tempdir().expect("tempdir");
            write_capsule(d.path(), "x.md", &capsule_text(id, LOW_BAND, "body"));
            assert!(
                load(d.path()).capsules.is_empty(),
                "id `{id}` should be rejected"
            );
        }
    }

    #[test]
    fn a_crlf_capsule_loads() {
        let d = tempfile::tempdir().expect("tempdir");
        write_capsule(
            d.path(),
            "x.md",
            &format!(
                "---json\r\n{{\"id\":\"x\",\"priority\":50,\"max_bytes\":512,\"when\":{LOW_BAND}}}\r\n---\r\nbody\r\n"
            ),
        );
        assert_eq!(load(d.path()).capsules.len(), 1);
    }

    #[test]
    fn a_decorated_or_indented_rule_does_not_close_the_frontmatter() {
        // The closing delimiter must be a bare `---` on its own line. A
        // horizontal rule inside the JSON block, indented or extended, must
        // not end it early and hand the rest to the body.
        for fake in ["  ---", "----"] {
            let d = tempfile::tempdir().expect("tempdir");
            write_capsule(
                d.path(),
                "x.md",
                &format!(
                    "---json\n{{\"id\":\"x\",\"priority\":50,\n{fake}\n\"max_bytes\":512,\"when\":{LOW_BAND}}}\n---\nbody\n"
                ),
            );
            let loaded = load(d.path());
            assert!(
                loaded.capsules.is_empty(),
                "`{fake}` must not terminate the frontmatter; the JSON is then malformed"
            );
        }
    }

    #[test]
    fn a_bare_rule_inside_the_body_is_kept_verbatim() {
        // Only the *first* `---` line closes the frontmatter; later ones are
        // ordinary Markdown and must survive into the body.
        let d = tempfile::tempdir().expect("tempdir");
        write_capsule(
            d.path(),
            "x.md",
            &format!(
                "---json\n{{\"id\":\"x\",\"priority\":50,\"max_bytes\":512,\"when\":{LOW_BAND}}}\n---\nabove\n---\nbelow\n"
            ),
        );
        let loaded = load(d.path());
        let capsule = loaded.capsules.first().expect("one capsule");
        assert_eq!(capsule.body, "above\n---\nbelow");
    }

    #[test]
    fn missing_frontmatter_delimiters_are_rejected() {
        let d = tempfile::tempdir().expect("tempdir");
        write_capsule(d.path(), "x.md", "just a markdown file\n");
        let loaded = load(d.path());
        assert!(loaded.capsules.is_empty());
        assert!(diagnostics_mention(&loaded, "---json"));
    }

    // ── body contract ───────────────────────────────────────────────────

    #[test]
    fn body_larger_than_declared_max_bytes_is_rejected() {
        // The author asserted 64 bytes. Silently letting a 200-byte body
        // compete would make `max_bytes` a lie.
        let d = tempfile::tempdir().expect("tempdir");
        write_capsule(
            d.path(),
            "x.md",
            &format!(
                "---json\n{{\"id\":\"x\",\"priority\":1,\"max_bytes\":64,\"when\":{LOW_BAND}}}\n---\n{}\n",
                "b".repeat(200)
            ),
        );
        let loaded = load(d.path());
        assert!(loaded.capsules.is_empty());
        assert!(diagnostics_mention(&loaded, "exceeds declared max_bytes"));
    }

    #[test]
    fn empty_body_is_rejected() {
        let d = tempfile::tempdir().expect("tempdir");
        write_capsule(d.path(), "x.md", &capsule_text("x", LOW_BAND, "   \n\n"));
        assert!(load(d.path()).capsules.is_empty());
    }

    #[test]
    fn template_and_variable_bodies_are_rejected() {
        // Nothing is interpolated into a capsule body. Rejecting the syntax
        // stops an author believing otherwise.
        for body in [
            "Check ?file before continuing.",
            "Value is {{ subject }}.",
            "Value is ${subject}.",
        ] {
            let d = tempfile::tempdir().expect("tempdir");
            write_capsule(d.path(), "x.md", &capsule_text("x", LOW_BAND, body));
            let loaded = load(d.path());
            assert!(
                loaded.capsules.is_empty(),
                "body `{body}` should be rejected"
            );
            assert!(diagnostics_mention(&loaded, "templates"));
        }
    }

    #[test]
    fn a_question_mark_in_ordinary_prose_is_not_a_variable() {
        let d = tempfile::tempdir().expect("tempdir");
        write_capsule(
            d.path(),
            "x.md",
            &capsule_text("x", LOW_BAND, "Is the build green? Verify before claiming."),
        );
        assert_eq!(load(d.path()).capsules.len(), 1);
    }

    // ── condition language ──────────────────────────────────────────────

    #[test]
    fn negation_and_empty_groups_are_rejected() {
        for when in [
            r#"{"not":[{"predicate":"context_confidence_band","args":["low"]}]}"#,
            r#"{"unless":[{"predicate":"context_confidence_band","args":["low"]}]}"#,
            r#"{"all":[]}"#,
            r#"{"any":[]}"#,
        ] {
            let d = tempfile::tempdir().expect("tempdir");
            write_capsule(d.path(), "x.md", &capsule_text("x", when, "body"));
            assert!(
                load(d.path()).capsules.is_empty(),
                "`{when}` should be rejected"
            );
        }
    }

    #[test]
    fn variable_arguments_are_rejected() {
        let d = tempfile::tempdir().expect("tempdir");
        write_capsule(
            d.path(),
            "x.md",
            &capsule_text(
                "x",
                r#"{"predicate":"journey_seen","args":["?tag","session"]}"#,
                "body",
            ),
        );
        assert!(load(d.path()).capsules.is_empty());
    }

    #[test]
    fn wrong_arity_is_rejected() {
        let d = tempfile::tempdir().expect("tempdir");
        write_capsule(
            d.path(),
            "x.md",
            &capsule_text(
                "x",
                r#"{"predicate":"journey_seen","args":["rule-blocked"]}"#,
                "body",
            ),
        );
        assert!(load(d.path()).capsules.is_empty());
    }

    #[test]
    fn non_canonical_journey_threshold_is_rejected() {
        for k in ["0", "03", "+3", " 3", "3.0", "20000"] {
            let d = tempfile::tempdir().expect("tempdir");
            write_capsule(
                d.path(),
                "x.md",
                &capsule_text(
                    "x",
                    &format!(r#"{{"predicate":"journey_since_ge","args":["tag","{k}"]}}"#),
                    "body",
                ),
            );
            assert!(
                load(d.path()).capsules.is_empty(),
                "threshold `{k}` should be rejected"
            );
        }
    }

    #[test]
    fn condition_depth_beyond_the_cap_is_rejected() {
        let mut when = LOW_BAND.to_string();
        for _ in 0..=MAX_CONDITION_DEPTH {
            when = format!(r#"{{"all":[{when}]}}"#);
        }
        let d = tempfile::tempdir().expect("tempdir");
        write_capsule(d.path(), "x.md", &capsule_text("x", &when, "body"));
        let loaded = load(d.path());
        assert!(loaded.capsules.is_empty());
        assert!(diagnostics_mention(&loaded, "nesting"));
    }

    #[test]
    fn dnf_expansion_beyond_the_alternative_cap_is_rejected() {
        // Nine nested 2-way `any` branches inside an `all` expand to 512
        // alternatives — over the 256 cap, and within the depth cap.
        let pair = format!(r#"{{"any":[{LOW_BAND},{LOW_BAND}]}}"#);
        let when = format!(
            r#"{{"all":[{}]}}"#,
            std::iter::repeat_n(pair.as_str(), 9)
                .collect::<Vec<_>>()
                .join(",")
        );
        let d = tempfile::tempdir().expect("tempdir");
        write_capsule(d.path(), "x.md", &capsule_text("x", &when, "body"));
        let loaded = load(d.path());
        assert!(loaded.capsules.is_empty());
        assert!(diagnostics_mention(&loaded, "alternatives"));
    }

    #[test]
    fn nested_all_and_any_within_the_caps_load() {
        let when = format!(
            r#"{{"all":[{{"any":[{LOW_BAND},{{"predicate":"context_confidence_band","args":["medium"]}}]}},{LOW_BAND}]}}"#
        );
        let d = tempfile::tempdir().expect("tempdir");
        write_capsule(d.path(), "x.md", &capsule_text("x", &when, "body"));
        assert_eq!(load(d.path()).capsules.len(), 1);
    }

    // ── loading, ordering, and identity ─────────────────────────────────

    #[test]
    fn duplicate_ids_skip_every_copy() {
        let d = tempfile::tempdir().expect("tempdir");
        write_capsule(d.path(), "a.md", &capsule_text("same", LOW_BAND, "first"));
        write_capsule(d.path(), "b.md", &capsule_text("same", LOW_BAND, "second"));
        let loaded = load(d.path());
        assert!(
            loaded.capsules.is_empty(),
            "neither copy may win a duplicate-id contest"
        );
        assert!(diagnostics_mention(&loaded, "duplicate capsule id"));
    }

    #[test]
    fn loading_is_bytewise_filename_ordered_and_deterministic() {
        let d = tempfile::tempdir().expect("tempdir");
        for name in ["10-b.md", "2-a.md", "Z-upper.md", "a-lower.md"] {
            let id = name.trim_end_matches(".md").to_ascii_lowercase();
            write_capsule(d.path(), name, &capsule_text(&id, LOW_BAND, "body"));
        }
        let first = load(d.path());
        let second = load(d.path());
        let ids = |l: &CapsuleLoad| l.capsules.iter().map(|c| c.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&first), ids(&second), "load order must be stable");
        assert_eq!(ids(&first), ["10-b", "2-a", "z-upper", "a-lower"]);
    }

    #[test]
    fn non_markdown_files_are_ignored() {
        let d = tempfile::tempdir().expect("tempdir");
        write_capsule(d.path(), "notes.txt", "---json\ngarbage\n---\nbody");
        write_capsule(d.path(), "x.md", &capsule_text("x", LOW_BAND, "body"));
        let loaded = load(d.path());
        assert_eq!(loaded.capsules.len(), 1);
        assert!(loaded.diagnostics.is_empty());
    }

    #[test]
    fn an_oversized_file_is_rejected_before_parsing() {
        let d = tempfile::tempdir().expect("tempdir");
        write_capsule(
            d.path(),
            "x.md",
            &capsule_text("x", LOW_BAND, &"b".repeat(9 * 1024)),
        );
        let loaded = load(d.path());
        assert!(loaded.capsules.is_empty());
        assert!(diagnostics_mention(&loaded, "8 KiB"));
    }

    #[test]
    fn a_missing_nudges_directory_is_not_an_error() {
        let d = tempfile::tempdir().expect("tempdir");
        let loaded = load(d.path());
        assert!(loaded.capsules.is_empty());
        assert!(loaded.diagnostics.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_escaping_the_project_is_rejected() {
        let outside = tempfile::tempdir().expect("outside tempdir");
        let secret = outside.path().join("secret.md");
        std::fs::write(&secret, capsule_text("escaped", LOW_BAND, "leaked")).expect("write");

        let d = tempfile::tempdir().expect("tempdir");
        let dir = d.path().join(CAPSULE_DIR);
        std::fs::create_dir_all(&dir).expect("mkdir nudges");
        std::os::unix::fs::symlink(&secret, dir.join("link.md")).expect("symlink");

        let loaded = load(d.path());
        assert!(
            loaded.capsules.is_empty(),
            "a capsule resolving outside the project root must not load"
        );
        assert!(!loaded.diagnostics.is_empty(), "and must say why");
    }

    // ── RETE selection ──────────────────────────────────────────────────

    fn band_capsule(id: &str, band: &str) -> Capsule {
        Capsule {
            id: id.to_string(),
            priority: 50,
            max_bytes: 512,
            when: CapsuleCondition::Leaf {
                predicate: "context_confidence_band".into(),
                args: vec![band.into()],
            },
            body: "verify".into(),
            path: PathBuf::from(format!("{id}.md")),
        }
    }

    #[tokio::test]
    async fn an_all_condition_needs_every_fact() {
        let d = project_with_confidence();
        let capsule = Capsule {
            when: CapsuleCondition::All(vec![
                CapsuleCondition::Leaf {
                    predicate: "context_confidence_band".into(),
                    args: vec!["low".into()],
                },
                CapsuleCondition::Leaf {
                    predicate: "context_confidence_band".into(),
                    args: vec!["high".into()],
                },
            ]),
            ..band_capsule("both", "low")
        };
        let found = matched(d.path(), &[capsule], 1).await;
        assert!(
            found.matched_ids.is_empty(),
            "`all` must not match on a partial fact set"
        );
    }

    #[tokio::test]
    async fn an_any_condition_yields_one_candidate_per_capsule() {
        let d = project_with_confidence();
        let capsule = Capsule {
            when: CapsuleCondition::Any(vec![
                CapsuleCondition::Leaf {
                    predicate: "context_confidence_band".into(),
                    args: vec!["low".into()],
                },
                CapsuleCondition::Leaf {
                    predicate: "context_confidence_band".into(),
                    args: vec!["low".into()],
                },
            ]),
            ..band_capsule("either", "low")
        };
        let found = matched(d.path(), &[capsule], 1).await;
        assert_eq!(
            found.matched_ids,
            ["either"],
            "several matching branches still deduplicate to one capsule"
        );
        assert_eq!(found.items.len(), 1);
    }

    #[tokio::test]
    async fn a_capsule_whose_fact_is_false_does_not_match() {
        let d = project_with_confidence();
        // A fresh subject has no passed signals, so the band is `low`.
        let found = matched(d.path(), &[band_capsule("hi", "high")], 1).await;
        assert!(found.matched_ids.is_empty());
    }

    #[tokio::test]
    async fn an_unconfigured_confidence_projection_reports_why_it_cannot_match() {
        let d = tempfile::tempdir().expect("tempdir");
        let found = matched(d.path(), &[band_capsule("low", "low")], 1).await;
        assert!(found.matched_ids.is_empty());
        assert!(
            found
                .diagnostics
                .iter()
                .any(|d| d.contains("confidence is not configured")),
            "absence must be explained, not silently treated as a false fact: {:?}",
            found.diagnostics
        );
    }

    #[tokio::test]
    async fn no_capsules_means_no_hydration_and_no_diagnostics() {
        let d = tempfile::tempdir().expect("tempdir");
        let found = matched(d.path(), &[], 1).await;
        assert!(found.items.is_empty());
        assert!(found.diagnostics.is_empty());
        assert!(
            !d.path().join(".phronesis/journey").exists(),
            "a project with no capsules must do no journey I/O"
        );
    }

    #[tokio::test]
    async fn the_rendered_item_carries_only_the_trusted_capsule_id() {
        let d = project_with_confidence();
        let found = matched(d.path(), &[band_capsule("low-band", "low")], 1).await;
        let item = found.items.first().expect("one item");
        assert!(item.body.contains("[phronesis nudge: low-band]"));
        assert_eq!(item.kind, ItemKind::Nudge);
    }
}

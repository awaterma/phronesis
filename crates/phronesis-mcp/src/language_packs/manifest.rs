//! `pack.json`: the declared shape of a language pack and its validation.

use std::collections::{BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{
    MANIFEST_FILE, PackError, RECORD_KINDS, RecordError, is_identifier, limits,
    relative_path_problem,
};

pub const SCHEMA_VERSION: u32 = 1;
pub const ADAPTER_API: u32 = 1;

/// Grammars compiled into this binary that a manifest may name.
pub const BUNDLED_GRAMMARS: [&str; 8] = [
    "go",
    "java",
    "lua",
    "python",
    "rust",
    "swift",
    "tsx",
    "typescript",
];

/// Names a declared resource may not take: they belong to the host.
const RESERVED_RESOURCES: [&str; 2] = [MANIFEST_FILE, "installed.json"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackManifest {
    pub schema_version: u32,
    pub adapter_api: u32,
    pub id: String,
    pub version: String,
    pub language: String,
    pub extensions: Vec<String>,
    pub grammar: String,
    pub capabilities: Vec<Capability>,
    #[serde(default)]
    pub queries: Vec<QuerySpec>,
    #[serde(default)]
    pub adapter: Option<String>,
    #[serde(default)]
    pub rules: Option<String>,
    #[serde(default)]
    pub toolchains: Option<String>,
    #[serde(default)]
    pub context_files: Vec<String>,
    #[serde(default)]
    pub fixtures: Vec<FixtureSpec>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    StructuralFacts,
    Graph,
    Toolchains,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuerySpec {
    pub id: String,
    pub path: String,
    /// Empty permits Rhai-only interpretation of this query's matches.
    #[serde(default)]
    pub emit: Vec<RecordTemplate>,
}

/// A record-shaped JSON object. Whole-string `${capture}` / `${file}` values
/// substitute; `span_capture` picks the span. Translation happens when the
/// pack is compiled; here only the record kind is checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RecordTemplate(pub Map<String, Value>);

/// Fixture input and its expected canonical records; never a command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureSpec {
    pub path: String,
    pub expected: String,
}

impl PackManifest {
    /// Every declared resource as `(manifest field, path)`, in manifest
    /// order. The manifest itself is not included.
    pub fn resource_paths(&self) -> Vec<(String, &str)> {
        let mut out: Vec<(String, &str)> = Vec::new();
        for (i, q) in self.queries.iter().enumerate() {
            out.push((format!("queries[{i}].path"), &q.path));
        }
        for (field, path) in [
            ("adapter", &self.adapter),
            ("rules", &self.rules),
            ("toolchains", &self.toolchains),
        ] {
            if let Some(path) = path {
                out.push((field.to_string(), path));
            }
        }
        for (i, f) in self.fixtures.iter().enumerate() {
            out.push((format!("fixtures[{i}].path"), &f.path));
            out.push((format!("fixtures[{i}].expected"), &f.expected));
        }
        out
    }

    pub fn has(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }
}

/// Validate everything about a manifest that does not need its assets.
pub fn validate_manifest(m: &PackManifest) -> Result<(), PackError> {
    let pack = || Some(m.id.clone());
    let bad = |field: &str, message: String| PackError::Manifest {
        pack: pack(),
        path: MANIFEST_FILE.to_string(),
        field: field.to_string(),
        message,
    };
    let duplicate = |field: &str, value: &str| PackError::Duplicate {
        pack: pack(),
        field: field.to_string(),
        value: value.to_string(),
    };
    let limit = |what: &str, max: usize| PackError::Limit {
        pack: pack(),
        path: Some(MANIFEST_FILE.to_string()),
        what: what.to_string(),
        max,
    };

    for (field, value, expected) in [
        ("schema_version", m.schema_version, SCHEMA_VERSION),
        ("adapter_api", m.adapter_api, ADAPTER_API),
    ] {
        if value != expected {
            return Err(bad(
                field,
                format!("unsupported {value}; expected {expected}"),
            ));
        }
    }
    for (field, value) in [("id", &m.id), ("language", &m.language)] {
        if !is_identifier(value) {
            return Err(bad(
                field,
                format!("`{value}` must match [a-z][a-z0-9_]{{0,47}}"),
            ));
        }
    }
    // A pack's facts are `pack_<id>_<name>`: an id that opens a host-owned
    // `pack_*` relation could forge it (`file` + `adapter`).
    if let Some(host) = host_pack_relation_under(&m.id) {
        return Err(bad(
            "id",
            format!(
                "`{}` is reserved: its facts (`pack_{}_<name>`) could forge the host relation `{host}`",
                m.id, m.id
            ),
        ));
    }
    let numeric = |c: &str| !c.is_empty() && c.len() <= 9 && c.bytes().all(|b| b.is_ascii_digit());
    if m.version.split('.').count() != 3 || !m.version.split('.').all(numeric) {
        return Err(bad(
            "version",
            format!("`{}` must be three numeric components", m.version),
        ));
    }

    if m.extensions.is_empty() {
        return Err(bad(
            "extensions",
            "at least one extension is required".into(),
        ));
    }
    let mut seen = HashSet::new();
    for ext in &m.extensions {
        if let Some(problem) = extension_problem(ext) {
            return Err(bad("extensions", format!("`{ext}` {problem}")));
        }
        if !seen.insert(ext) {
            return Err(duplicate("extensions", ext));
        }
    }

    if !BUNDLED_GRAMMARS.contains(&m.grammar.as_str()) {
        return Err(PackError::UnknownGrammar {
            pack: pack(),
            grammar: m.grammar.clone(),
            available: BUNDLED_GRAMMARS.iter().map(|g| g.to_string()).collect(),
        });
    }
    let mut seen = HashSet::new();
    for cap in &m.capabilities {
        if !seen.insert(cap) {
            return Err(duplicate("capabilities", &format!("{cap:?}")));
        }
    }

    if m.queries.len() > limits::MAX_QUERIES {
        return Err(limit("queries", limits::MAX_QUERIES));
    }
    let mut seen = HashSet::new();
    for (i, q) in m.queries.iter().enumerate() {
        if !is_identifier(&q.id) {
            return Err(bad(
                &format!("queries[{i}].id"),
                format!("`{}` must match [a-z][a-z0-9_]{{0,47}}", q.id),
            ));
        }
        if !seen.insert(&q.id) {
            return Err(duplicate("queries[].id", &q.id));
        }
        for (j, template) in q.emit.iter().enumerate() {
            check_template(template).map_err(|e| bad(&format!("queries[{i}].emit[{j}]"), e.0))?;
        }
    }

    // Contradictory declarations.
    if m.toolchains.is_some() != m.has(Capability::Toolchains) {
        return Err(bad(
            "toolchains",
            "the `toolchains` resource and the `toolchains` capability must be declared together"
                .into(),
        ));
    }
    let structural = m.has(Capability::StructuralFacts) || m.has(Capability::Graph);
    if structural && m.queries.is_empty() {
        return Err(bad(
            "queries",
            "structural_facts/graph capabilities require at least one query".into(),
        ));
    }
    if !structural && !m.queries.is_empty() {
        return Err(bad(
            "capabilities",
            "queries are declared but neither structural_facts nor graph is".into(),
        ));
    }
    if m.adapter.is_some() && m.queries.is_empty() {
        return Err(bad("adapter", "an adapter script requires queries".into()));
    }

    if m.context_files.len() > limits::MAX_CONTEXT_FILES {
        return Err(bad(
            "context_files",
            format!("at most {} names are allowed", limits::MAX_CONTEXT_FILES),
        ));
    }
    let mut seen = HashSet::new();
    for name in &m.context_files {
        let basename =
            !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0']);
        if !basename {
            return Err(bad("context_files", format!("`{name}` is not a basename")));
        }
        if !seen.insert(name) {
            return Err(duplicate("context_files", name));
        }
    }

    let resources = m.resource_paths();
    if resources.len() + 1 > limits::MAX_ASSETS {
        return Err(limit("assets", limits::MAX_ASSETS));
    }
    // Case-folded so two paths cannot alias on a case-insensitive filesystem.
    let mut seen = BTreeSet::new();
    for (field, path) in resources {
        if let Some(reason) = relative_path_problem(path) {
            return Err(PackError::UnsafePath {
                pack: pack(),
                path: path.to_string(),
                reason: reason.to_string(),
            });
        }
        if RESERVED_RESOURCES.contains(&path) {
            return Err(bad(&field, format!("`{path}` is reserved for the host")));
        }
        if !seen.insert(path.to_lowercase()) {
            return Err(duplicate(&field, path));
        }
    }
    Ok(())
}

fn extension_problem(ext: &str) -> Option<&'static str> {
    if ext.is_empty() {
        Some("is empty")
    } else if ext.starts_with('.') {
        Some("must not start with a dot")
    } else if ext.len() > 32 {
        Some("is longer than 32 bytes")
    } else if ext.contains(['/', '\\']) {
        Some("contains a path separator")
    } else if ext.contains(['*', '?', '[', ']', '{', '}', '!']) {
        Some("contains a glob character")
    } else if ext.chars().any(|c| c.is_whitespace() || c.is_control()) {
        Some("contains whitespace or a control character")
    } else {
        None
    }
}

fn check_template(template: &RecordTemplate) -> Result<(), RecordError> {
    match template.0.get("kind").and_then(Value::as_str) {
        Some(kind) if RECORD_KINDS.contains(&kind) => {}
        Some(kind) => {
            return Err(RecordError(format!(
                "unknown record kind `{kind}`; expected one of {}",
                RECORD_KINDS.join(", ")
            )));
        }
        None => return Err(RecordError("`kind` must be a record kind string".into())),
    }
    match template.0.get("span_capture") {
        None | Some(Value::String(_)) => Ok(()),
        Some(_) => Err(RecordError("`span_capture` must be a capture name".into())),
    }
}

/// The host-owned `pack_*` relation a pack with this id could forge through
/// its `pack_<id>_<name>` facts, if any.
pub(crate) fn host_pack_relation_under(id: &str) -> Option<&'static str> {
    let prefix = format!("pack_{id}_");
    crate::graph::hydrate::GRAPH_RELATIONS
        .iter()
        .copied()
        .find(|relation| relation.starts_with(&prefix))
}

/// Reject duplicate pack IDs, any extension claimed twice (by two packs, or
/// by a pack and a native/built-in owner in `reserved`, as
/// `(extension, owner)`), and two ids where one is the other plus `_...`.
/// Matching is exact and case-sensitive.
///
/// The id rule keeps `pack_<id>_<name>` fact predicates unambiguous: if two
/// different ids produced the same predicate, the shorter id followed by
/// `_` would have to begin the longer one (`a`/`b_c` and `a_b`/`c` both give
/// `pack_a_b_c`).
pub fn check_ownership<'a>(
    manifests: impl IntoIterator<Item = &'a PackManifest>,
    reserved: &[(&str, &str)],
) -> Result<(), PackError> {
    let mut owners: HashMap<&str, String> = reserved
        .iter()
        .map(|(ext, owner)| (*ext, format!("built-in `{owner}`")))
        .collect();
    let mut ids = HashSet::new();
    for m in manifests {
        let pack = Some(m.id.to_string());
        if !ids.insert(m.id.as_str()) {
            return Err(PackError::Duplicate {
                pack,
                field: "id".into(),
                value: m.id.to_string(),
            });
        }
        for ext in &m.extensions {
            if let Some(owner) = owners.get(ext.as_str()) {
                return Err(PackError::Ownership {
                    pack,
                    message: format!("extension `{ext}` is already owned by {owner}"),
                });
            }
            owners.insert(ext, format!("pack `{}`", m.id));
        }
    }
    let mut sorted: Vec<&str> = ids.into_iter().collect();
    sorted.sort_unstable();
    for (i, shorter) in sorted.iter().enumerate() {
        let prefix = format!("{shorter}_");
        // Sorted, so every id extending `shorter` follows it directly.
        if let Some(longer) = sorted[i + 1..].iter().find(|id| id.starts_with(&prefix)) {
            return Err(PackError::Ownership {
                pack: Some((*longer).to_string()),
                message: format!(
                    "pack id `{longer}` extends pack id `{shorter}`, so their `pack_<id>_<name>` facts could collide; rename one"
                ),
            });
        }
    }
    Ok(())
}

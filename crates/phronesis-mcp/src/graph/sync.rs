//! The per-save pipeline and the staleness index.
//!
//! Two tiers, deliberately separated (spec §4.5): **parse** touches only the
//! edited file, while **derive** runs over the whole edge set. Derived facts
//! are therefore correct after every save without reparsing the repository.
//!
//! The index exists because edits can bypass the hook entirely — `git
//! checkout`, `git mv`, a rebase, a plain shell edit. A graph that has
//! silently drifted must not be allowed to block work, so drift is detected
//! and downgrades enforcement to warn.

use super::derive::{canonicalize_function_edges, derive_all};
use super::extract::{DEFAULT_WATCHLIST, extract_rust_at_module, module_path};
use super::helm3;
use super::model::Edge;
use super::store;
use super::unit::{UnitContext, UnitMap};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Location of the staleness index, relative to project root.
pub const INDEX_REL_PATH: &str = ".phronesis/graph.index";

/// Identity scheme the extractor writes. Bumped whenever entity naming
/// changes, because such a change invalidates every edge already on disk
/// while leaving file contents — and therefore content hashes — untouched.
/// Without it, an upgrade silently yields a graph half in the old naming and
/// half in the new, whose `imports` never join to its `declares_module`.
///
/// 5 — `<lang>:<package>[#<target>]::<module path>` (unchanged by rev 5,
/// which added `graph_definition`, `defines`, `element_in_file`,
/// `element_in_module`, `graph_module`, `graph_function`, `graph_test`,
/// `graph_file` and multilingual dialect support; format 4 remains the
/// same scheme without those relation names).
/// Anything earlier is recorded as 0: pre-versioning, bare `crate::…`.
pub const GRAPH_FORMAT: u32 = 16;

/// Header line stamping the format into the index file.
const FORMAT_KEY: &str = "# format";
const GENERATION_KEY: &str = "# generation";

/// Content hashes of every file the graph was built from.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Index {
    /// Identity scheme the graph was built under; 0 for a pre-versioning or
    /// absent index. Only meaningful on load — writes always stamp the
    /// current format, because what we write is by definition current.
    pub format: u32,
    /// Monotonic graph-write generation shared with `bindings.json`.
    pub generation: u64,
    pub entries: BTreeMap<String, u64>,
}

/// Whether the graph still reflects what is on disk.
#[derive(Debug, PartialEq, Eq)]
pub enum Freshness {
    Fresh,
    /// Files whose content no longer matches the index, sorted.
    Stale(Vec<String>),
    /// The graph was built under a different identity scheme. Content hashes
    /// prove nothing here: the files are untouched and every edge is still
    /// wrong. Only a rebuild resolves it.
    Outdated {
        found: u32,
        expected: u32,
    },
}

/// Outcome of a single-file save.
#[derive(Debug, PartialEq, Eq)]
pub struct SaveOutcome {
    /// Base edges for the whole project after compaction.
    pub base: usize,
    /// Derived edges regenerated this pass.
    pub derived: usize,
    /// Items the extractor declined to name.
    pub skipped: usize,
    /// Rules whose deprecated graph predicates were migrated during rebuild.
    pub migrated_rules: usize,
}

const GRAPH_PREDICATE_MIGRATIONS: &[(&str, &str)] = &[
    ("untested", "no_direct_test"),
    ("rhai_exposes_fn", "exposes"),
    ("calls_rhai_fn", "calls"),
];
const MANUAL_GRAPH_PREDICATE_MIGRATIONS: &[&str] = &["rhai_call_resolves_to"];

fn migrate_clause_predicates(clause: &mut crate::rules_file::WhenClause) -> bool {
    match clause {
        crate::rules_file::WhenClause::Leaf(condition) => {
            let Some((_, replacement)) = GRAPH_PREDICATE_MIGRATIONS
                .iter()
                .find(|(deprecated, _)| condition.predicate == *deprecated)
            else {
                return false;
            };
            let deprecated = condition.predicate.clone();
            condition.predicate = (*replacement).to_string();
            if matches!(deprecated.as_str(), "rhai_exposes_fn" | "calls_rhai_fn")
                && let Some(callable) = condition.args.get_mut(1)
                && callable != "*"
                && !callable.starts_with('?')
                && !callable.starts_with("rhai:callable::")
            {
                *callable = format!("rhai:callable::{callable}");
            }
            true
        }
        crate::rules_file::WhenClause::Or(alternatives) => {
            let mut changed = false;
            for alternative in alternatives {
                changed |= migrate_clause_predicates(alternative);
            }
            changed
        }
    }
}

fn collect_deprecated_predicates(
    clause: &crate::rules_file::WhenClause,
    found: &mut BTreeSet<String>,
) {
    match clause {
        crate::rules_file::WhenClause::Leaf(condition) => {
            if GRAPH_PREDICATE_MIGRATIONS
                .iter()
                .any(|(deprecated, _)| condition.predicate == *deprecated)
                || MANUAL_GRAPH_PREDICATE_MIGRATIONS.contains(&condition.predicate.as_str())
            {
                found.insert(condition.predicate.clone());
            }
        }
        crate::rules_file::WhenClause::Or(alternatives) => {
            for alternative in alternatives {
                collect_deprecated_predicates(alternative, found);
            }
        }
    }
}

/// Deprecated graph predicates still referenced by durable project rules.
///
/// This is semantic rule/graph drift: file hashes may be current while the
/// consumer vocabulary no longer matches the graph producer vocabulary.
pub fn deprecated_graph_rule_predicates(root: &Path) -> std::io::Result<Vec<String>> {
    let rules = crate::rules_file::read_source(&crate::rules_file::default_path(root))
        .map_err(std::io::Error::other)?;
    let mut found = BTreeSet::new();
    for rule in &rules {
        for clause in &rule.when {
            collect_deprecated_predicates(clause, &mut found);
        }
    }
    Ok(found.into_iter().collect())
}

fn migrate_graph_rule_predicates(root: &Path) -> std::io::Result<usize> {
    let path = crate::rules_file::default_path(root);
    let mut rules = crate::rules_file::read_source(&path).map_err(std::io::Error::other)?;
    let mut manual = BTreeSet::new();
    for rule in &rules {
        for clause in &rule.when {
            collect_deprecated_predicates(clause, &mut manual);
        }
    }
    manual.retain(|predicate| MANUAL_GRAPH_PREDICATE_MIGRATIONS.contains(&predicate.as_str()));
    if !manual.is_empty() {
        return Err(std::io::Error::other(format!(
            "graph rebuild requires manual rule migration for changed relation semantics: {}",
            manual.into_iter().collect::<Vec<_>>().join(", ")
        )));
    }
    let migrated = rules
        .iter_mut()
        .map(|rule| {
            let mut changed = false;
            for clause in &mut rule.when {
                changed |= migrate_clause_predicates(clause);
            }
            usize::from(changed)
        })
        .sum();
    if migrated > 0 {
        crate::rules_file::write_source(&path, &rules).map_err(std::io::Error::other)?;
    }
    Ok(migrated)
}

fn rule_predicate_edges(root: &Path) -> std::io::Result<Vec<Edge>> {
    fn visit(rule: &str, clause: &crate::rules_file::WhenClause, out: &mut Vec<Edge>) {
        match clause {
            crate::rules_file::WhenClause::Leaf(condition) => out.push(Edge::base(
                "rule_uses_predicate",
                &[rule, &condition.predicate],
                ".phronesis/rules.json",
            )),
            crate::rules_file::WhenClause::Or(alternatives) => {
                for alternative in alternatives {
                    visit(rule, alternative, out);
                }
            }
        }
    }
    let rules = crate::rules_file::read_source(&crate::rules_file::default_path(root))
        .map_err(std::io::Error::other)?;
    let mut out = Vec::new();
    for rule in &rules {
        for clause in &rule.when {
            visit(&rule.id, clause, &mut out);
        }
    }
    Ok(out)
}

/// Deterministic content hash (FNV-1a, 64-bit).
///
/// Not `DefaultHasher`: that is explicitly not stable across Rust releases,
/// and a hash that changes under the reader would mark every file stale after
/// a toolchain upgrade.
pub fn hash_content(content: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in content.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

pub fn index_path(root: &Path) -> PathBuf {
    root.join(INDEX_REL_PATH)
}

/// A missing or unreadable index means "nothing known yet", which callers
/// treat as stale. Failing closed here would take enforcement offline for a
/// recoverable, self-healing condition.
pub fn load_index(path: &Path) -> std::io::Result<Index> {
    let body = match std::fs::read_to_string(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Index::default()),
        Err(e) => return Err(e),
    };
    let mut entries = BTreeMap::new();
    let mut format = 0;
    let mut generation = 0;
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix(FORMAT_KEY) {
            format = rest.trim().parse::<u32>().unwrap_or(0);
            continue;
        }
        if let Some(rest) = line.strip_prefix(GENERATION_KEY) {
            generation = rest.trim().parse::<u64>().unwrap_or(0);
            continue;
        }
        if let Some((hash, rel)) = line.split_once(' ')
            && let Ok(h) = hash.parse::<u64>()
        {
            entries.insert(rel.to_string(), h);
        }
    }
    Ok(Index {
        format,
        generation,
        entries,
    })
}

pub fn save_index(path: &Path, index: &Index) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut body = format!(
        "{FORMAT_KEY} {GRAPH_FORMAT}\n{GENERATION_KEY} {}\n",
        index.generation
    );
    for (rel, hash) in &index.entries {
        body.push_str(&format!("{hash} {rel}\n"));
    }
    let tmp = path.with_extension("index.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)
}

/// Every file under `root`, in a tracked language (Rust, Python,
/// TypeScript, Lua, CUE, JSON, YAML, Helm), that the graph should track, as
/// paths relative to `root`.  Honours `.gitignore` so build output and
/// vendored trees never enter the graph, and prunes `node_modules`
/// unconditionally — `.gitignore` alone cannot be relied on to exclude it,
/// and `is_tracked` must agree with this walk or a sensor-recorded file
/// becomes permanent drift.
fn tracked_files(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for entry in ignore::WalkBuilder::new(root)
        .hidden(true)
        .filter_entry(|e| e.file_name() != "node_modules")
        .build()
        .flatten()
    {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.trim_start_matches('.'))
            .unwrap_or("");
        if !matches!(
            ext,
            "rs" | "py"
                | "ts"
                | "tsx"
                | "mts"
                | "cts"
                | "lua"
                | "rhai"
                | "cue"
                | "json"
                | "yaml"
                | "yml"
                | "tpl"
        ) {
            continue;
        }
        if let Ok(rel) = path.strip_prefix(root)
            && let Some(rel) = rel.to_str()
        {
            out.push(rel.replace('\\', "/"));
        }
    }
    out.sort();
    out
}

#[derive(Clone)]
struct IncludedRustModule {
    module: String,
    unit: UnitContext,
    owner: String,
}

/// Resolve external `#[path = "…"] mod name;` files to the module identity
/// Rust actually compiles them under. Only parser-confirmed module items and
/// existing in-repository files participate; conflicting inclusions are
/// omitted rather than assigned an arbitrary owner.
fn rust_path_inclusions(
    root: &Path,
    files: &[String],
    units: &UnitMap,
) -> BTreeMap<String, IncludedRustModule> {
    use crate::syntax::parsed::ParsedFile;

    let mut candidates: BTreeMap<String, Vec<IncludedRustModule>> = BTreeMap::new();
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    for owner in files.iter().filter(|file| file.ends_with(".rs")) {
        let Ok(content) = std::fs::read_to_string(root.join(owner)) else {
            continue;
        };
        let Some(ParsedFile::Rust { tree, source }) = ParsedFile::parse_rust(&content) else {
            continue;
        };
        let unit = units.context_for(owner);
        let owner_module = module_path(owner, &unit);
        let mut cursor = tree.root_node().walk();
        for node in tree.root_node().named_children(&mut cursor) {
            if node.kind() != "mod_item" {
                continue;
            }
            let mut attributes = node
                .prev_named_sibling()
                .filter(|n| n.kind() == "attribute_item")
                .into_iter()
                .collect::<Vec<_>>();
            let mut children = node.walk();
            attributes.extend(
                node.named_children(&mut children)
                    .filter(|child| child.kind() == "attribute_item"),
            );
            let path_literal = attributes.iter().find_map(|attribute| {
                attribute
                    .utf8_text(source.as_bytes())
                    .ok()?
                    .strip_prefix("#[path")?
                    .split_once('=')
                    .map(|(_, value)| value)?
                    .trim()
                    .strip_suffix(']')?
                    .trim()
                    .strip_prefix('"')?
                    .strip_suffix('"')
            });
            let Some(path_literal) = path_literal else {
                continue;
            };
            let Some(name) = node
                .child_by_field_name("name")
                .and_then(|name| name.utf8_text(source.as_bytes()).ok())
            else {
                continue;
            };
            let owner_dir = root.join(owner).parent().unwrap_or(root).to_path_buf();
            let Ok(target) = owner_dir.join(path_literal).canonicalize() else {
                continue;
            };
            let Ok(relative) = target.strip_prefix(&canonical_root) else {
                continue;
            };
            let Some(relative) = relative.to_str() else {
                continue;
            };
            candidates
                .entry(relative.replace('\\', "/"))
                .or_default()
                .push(IncludedRustModule {
                    module: format!("{owner_module}::{name}"),
                    unit: unit.clone(),
                    owner: owner.clone(),
                });
        }
    }
    candidates
        .into_iter()
        .filter_map(|(path, mut owners)| (owners.len() == 1).then(|| (path, owners.remove(0))))
        .collect()
}

fn decision_input_files(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if root.join(".phronesis/rules.json").is_file() {
        out.push(".phronesis/rules.json".to_string());
    }
    let dir = root.join(".phronesis/wiki/decisions");
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("md")
                || path.file_name().and_then(|name| name.to_str()) == Some("README.md")
            {
                continue;
            }
            if let Ok(rel) = path.strip_prefix(root)
                && let Some(rel) = rel.to_str()
            {
                out.push(rel.replace('\\', "/"));
            }
        }
    }
    out.sort();
    out
}

/// Compare the index against what is on disk under `root`.
pub fn check_freshness(root: &Path, index: &Index) -> Freshness {
    // An empty index is "nothing built yet", which the per-file loop below
    // already reports as wholly stale. Only an index that actually describes
    // a graph can be outdated.
    if !index.entries.is_empty() && index.format != GRAPH_FORMAT {
        return Freshness::Outdated {
            found: index.format,
            expected: GRAPH_FORMAT,
        };
    }
    let mut drifted = Vec::new();
    let mut on_disk = tracked_files(root);
    if root.join(".phronesis/graph.toml").is_file() {
        on_disk.push(".phronesis/graph.toml".to_string());
    }
    on_disk.extend(decision_input_files(root));

    for rel in &on_disk {
        let current = std::fs::read_to_string(root.join(rel))
            .ok()
            .map(|c| hash_content(&c));
        match (current, index.entries.get(rel)) {
            (Some(now), Some(&then)) if now == then => {}
            _ => drifted.push(rel.clone()),
        }
    }
    // Indexed files that have vanished are drift too — their edges are still
    // in the graph and would keep matching rules.
    for rel in index.entries.keys() {
        if !on_disk.contains(rel) {
            drifted.push(rel.clone());
        }
    }

    if drifted.is_empty() {
        Freshness::Fresh
    } else {
        drifted.sort();
        drifted.dedup();
        Freshness::Stale(drifted)
    }
}

/// Every file extension the graph has an extractor for. A file outside this
/// set is not tracked, not indexed, and not counted as drift.
///
/// Covers Rust, Python, TypeScript (and siblings), Lua, CUE, JSON, YAML,
/// and Helm3 template files.
pub const TRACKED_EXTENSIONS: &[&str] = &[
    ".rs", ".py", ".ts", ".tsx", ".mts", ".cts", ".lua", ".rhai", ".cue", ".json", ".yaml", ".yml",
    ".tpl",
];

/// Whether `on_save`/`record_from_disk` should index this file.
///
/// Must agree with `tracked_files`'s walk, which prunes `node_modules`.
/// Without the same exclusion here, the sensor can record an index entry for
/// a path `tracked_files` will never enumerate — a hash `check_freshness`
/// can then never match, so the file reports as permanent drift until a
/// manual `rebuild`. Matched by path *component*, not substring, so a
/// legitimate directory like `my_node_modules_helper` is not excluded.
fn is_tracked(file_path: &str) -> bool {
    if Path::new(file_path)
        .components()
        .any(|c| c.as_os_str() == "node_modules")
    {
        return false;
    }
    TRACKED_EXTENSIONS.iter().any(|e| file_path.ends_with(e))
}

fn chart_for(root: &Path, rel: &str) -> Option<(String, String)> {
    tracked_files(root)
        .into_iter()
        .filter(|file| file.ends_with("Chart.yaml") || file.ends_with("Chart.yml"))
        .filter_map(|file| {
            let directory = file.rsplit_once('/').map_or("", |(directory, _)| directory);
            if !rel.starts_with(directory) {
                return None;
            }
            let body = std::fs::read_to_string(root.join(&file)).ok()?;
            let value: serde_norway::Value = serde_norway::from_str(&body).ok()?;
            let name = value.get("name")?.as_str()?.to_string();
            Some((directory.to_string(), name))
        })
        .max_by_key(|(directory, _)| directory.len())
}

/// Route one file to the extractor for its language.
fn extract_one(
    root: &Path,
    rel: &str,
    content: &str,
    units: &UnitMap,
    cue_index: Option<&super::cue::PackageIndex>,
    rust_inclusions: &BTreeMap<String, IncludedRustModule>,
) -> super::extract::Extracted {
    let inclusion = rust_inclusions.get(rel);
    let unit = inclusion
        .map(|included| included.unit.clone())
        .unwrap_or_else(|| units.context_for(rel));
    // Helm owns templated YAML beneath a chart's templates/ tree. Extension
    // alone is insufficient: routing such a file through generic YAML first
    // would either misparse Go actions or silently discard the file.
    if matches!(super::unit::lang_of_path(rel), Some(super::unit::LANG_YAML))
        && rel.contains("/templates/")
        && content.contains("{{")
        && let Some((chart_root, _chart_name)) = chart_for(root, rel)
    {
        let mut helm_unit = super::unit::UnitContext::unnamed_for(super::unit::LANG_HELM3);
        let chart_identity = if chart_root.is_empty() {
            "project"
        } else {
            chart_root.as_str()
        };
        helm_unit.id = format!("helm3:{chart_identity}");
        return helm3::extract_helm3(rel, content, &helm_unit, Some(&chart_root));
    }
    match super::unit::lang_of_path(rel) {
        Some(super::unit::LANG_PYTHON) => super::python::extract_python(rel, content, &unit),
        Some(super::unit::LANG_TYPESCRIPT) => {
            super::typescript::extract_typescript(rel, content, &unit)
        }
        Some(super::unit::LANG_LUA) => {
            let mut lua_unit = unit;
            lua_unit.lua_files = tracked_files(root);
            super::lua::extract_lua(rel, content, &lua_unit)
        }
        Some(super::unit::LANG_RHAI) => super::rhai::extract_rhai(rel, content, &unit),
        Some(super::unit::LANG_CUE) => {
            if let Some(index) = cue_index {
                let mut cue_unit = super::unit::UnitContext::unnamed_for(super::unit::LANG_CUE);
                cue_unit.id = format!("cue:{}", super::cue::discover_module(root, rel));
                super::cue::extract_cue_with_index(rel, content, &cue_unit, Some(index))
            } else {
                super::cue::extract_cue_at_root(root, rel, content)
            }
        }
        Some(super::unit::LANG_HELM3) => {
            if let Some((chart_root, _chart_name)) = chart_for(root, rel) {
                let mut helm_unit = unit;
                let chart_identity = if chart_root.is_empty() {
                    "project"
                } else {
                    chart_root.as_str()
                };
                helm_unit.id = format!("helm3:{chart_identity}");
                helm3::extract_helm3(rel, content, &helm_unit, Some(&chart_root))
            } else {
                helm3::extract_helm3(rel, content, &unit, None)
            }
        }
        Some(super::unit::LANG_JSON) => {
            let mut json_unit = unit;
            json_unit.files = tracked_files(root);
            super::json_extractor::extract_json(rel, content, &json_unit)
        }
        Some(super::unit::LANG_YAML) => {
            let mut yaml_unit = unit;
            yaml_unit.files = tracked_files(root);
            super::yaml::extract_yaml(rel, content, &yaml_unit)
        }
        _ => extract_rust_at_module(
            rel,
            content,
            DEFAULT_WATCHLIST,
            &unit,
            inclusion.map(|included| included.module.as_str()),
        ),
    }
}

/// Recompute derived edges over `base` and persist both sets.
fn persist(root: &Path, mut base: Vec<Edge>) -> std::io::Result<(usize, usize)> {
    canonicalize_function_edges(&mut base);
    let definition_ids = base
        .iter()
        .filter(|edge| !edge.d && edge.p == "defines_fn")
        .filter_map(|edge| edge.a.get(1))
        .collect::<BTreeSet<_>>();
    if let Some(invalid) = base.iter().find(|edge| {
        !edge.d
            && edge.p == "tested_by"
            && edge
                .a
                .first()
                .is_none_or(|target| !definition_ids.contains(target))
    }) {
        return Err(std::io::Error::other(format!(
            "tested_by target is not a canonical defines_fn identity: {:?}",
            invalid.a.first()
        )));
    }
    let derived = derive_all(&base);
    let (n_base, n_derived) = (base.len(), derived.len());
    let mut all = base;
    all.extend(derived);
    let path = store::graph_path(root);
    store::write_atomic(&path, &all)?;
    let persisted = store::load(&path)?;
    let persisted_base = persisted.iter().filter(|edge| !edge.d).count();
    let persisted_derived = persisted.len().saturating_sub(persisted_base);
    if (persisted_base, persisted_derived) != (n_base, n_derived) {
        return Err(std::io::Error::other(format!(
            "graph persistence verification failed: wrote {n_base} base/{n_derived} derived, read {persisted_base} base/{persisted_derived} derived"
        )));
    }
    Ok((n_base, n_derived))
}

fn now_unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

/// Reconcile bindings after the graph and index generation are durable.
/// Failure deliberately leaves the older generation in place, which causes
/// pre-check to ignore it and retain full enforcement.
fn reconcile_bindings(root: &Path, generation: u64) -> std::io::Result<()> {
    let rules = crate::rules_file::read_source(&crate::rules_file::default_path(root))
        .map_err(std::io::Error::other)?;
    let path = super::bindings::bindings_path(root);
    let persisted = super::bindings::load_recovering(&path)?.unwrap_or_default();
    let edges = store::load(&store::graph_path(root))?;
    let next =
        super::bindings::reconcile(&persisted, &rules, &edges, generation, now_unix_seconds());
    super::bindings::store_atomic(&path, &next)
}

/// Reconcile durable bindings after a rules-file mutation without changing
/// the graph generation. Missing graph state is a safe no-op; once a graph is
/// present, failure leaves the prior binding generation in place so hook-time
/// demotion remains disabled rather than trusting partial evidence.
pub fn reconcile_rules(root: &Path) -> std::io::Result<()> {
    if !store::graph_path(root).is_file() {
        return Ok(());
    }
    let index = load_index(&index_path(root))?;
    reconcile_bindings(root, index.generation)
}

fn reconcile_bindings_best_effort(root: &Path, generation: u64) {
    if let Err(error) = reconcile_bindings(root, generation) {
        tracing::debug!("binding reconciliation skipped: {error}");
    }
}

/// Apply one save: parse the edited file, compact by provenance, re-derive
/// over the whole graph, and write atomically.
///
/// Only the edited file is parsed; derivation runs over the full edge set
/// already on disk. That is what makes whole-repo facts affordable per save.
pub fn on_save(root: &Path, file_path: &str, content: &str) -> std::io::Result<SaveOutcome> {
    let path_module_owner = file_path.ends_with(".rs")
        && (content.contains("#[path")
            || store::load(&store::graph_path(root))?.iter().any(|edge| {
                !edge.d
                    && edge.p == "includes_file"
                    && edge.a.first().is_some_and(|owner| owner == file_path)
            }));
    let data_contract_input = root.join(".phronesis/graph.toml").is_file()
        && matches!(
            Path::new(file_path)
                .extension()
                .and_then(|value| value.to_str()),
            Some("rs" | "json" | "yaml" | "yml")
        );
    if file_path.ends_with(".cue")
        || path_module_owner
        || file_path == ".phronesis/graph.toml"
        || file_path == ".phronesis/rules.json"
        || file_path.starts_with(".phronesis/wiki/decisions/")
        || data_contract_input
    {
        return rebuild(root);
    }
    if !is_tracked(file_path) {
        return Ok(SaveOutcome {
            base: 0,
            derived: 0,
            skipped: 0,
            migrated_rules: 0,
        });
    }
    let ipath = index_path(root);
    let mut index = load_index(&ipath)?;
    // A graph written under an older identity scheme cannot be patched one
    // file at a time: compaction replaces only the edited file's edges, so
    // every other file would keep its old names and the two halves would
    // never join. Rebuild once, then record this edit on top of the result.
    if !index.entries.is_empty() && index.format != GRAPH_FORMAT {
        rebuild(root)?;
        index = load_index(&ipath)?;
    }

    let units = UnitMap::discover(root);
    let files = tracked_files(root);
    let rust_inclusions = rust_path_inclusions(root, &files, &units);
    let extracted = extract_one(root, file_path, content, &units, None, &rust_inclusions);
    let existing = store::load(&store::graph_path(root))?;
    if extracted.parse_failed {
        // Leave the graph and the index exactly as they were. Compacting the
        // empty edge set would erase the file's evidence, and recording the
        // unparseable content's hash would report the result fresh — the
        // harness would then keep enforcing on facts it had just deleted.
        // Leaving the hash stale makes freshness report the file as drifted,
        // which demotes structural rules to warnings: the honest state.
        let base = existing.iter().filter(|e| !e.d).count();
        return Ok(SaveOutcome {
            base,
            derived: existing.len() - base,
            skipped: extracted.skipped,
            migrated_rules: 0,
        });
    }
    let base = store::compact(existing, file_path, extracted.edges);
    let (n_base, n_derived) = persist(root, base)?;

    index.generation = index.generation.saturating_add(1);
    index
        .entries
        .insert(file_path.to_string(), hash_content(content));
    save_index(&ipath, &index)?;
    reconcile_bindings_best_effort(root, index.generation);

    Ok(SaveOutcome {
        base: n_base,
        derived: n_derived,
        skipped: extracted.skipped,
        migrated_rules: 0,
    })
}

/// Hook entry point: record the file's current on-disk content.
///
/// Best-effort and infallible by design. The sensor runs in `PostToolUse`,
/// after the edit has already happened; a graph write that fails must not
/// turn into a hook error that interrupts the user's work. Failures leave the
/// file's hash stale, which the freshness check will catch and report.
pub fn record_from_disk(root: &Path, file_path: &str) {
    // Only maintain a graph the project actually opted into. The sensor runs
    // before rules are loaded — it has to, since the structural pack ships
    // `phase: "pre"` rules exclusively — so it cannot ask whether a graph
    // rule exists. The graph's own presence is the opt-in signal: `init
    // --packs structural` builds it, and `graph rebuild` restores it. Without
    // this, every phronesis project would start building and rewriting a code
    // graph on every save whether or not it has a single structural rule.
    if !store::graph_path(root).exists() && !index_path(root).exists() {
        return;
    }
    // Hosts send absolute paths while the graph is keyed repo-relative, so
    // relativize first. Rejecting an absolute path outright — as this guard
    // once did — turned the sensor off for every real host, and because the
    // sensor is best-effort it failed silently: the graph simply never
    // updated and the structural pack demoted itself to warnings forever.
    let Some(rel) = super::hydrate::repo_relative(root, file_path) else {
        // Outside the project: `repo_relative` is the containment check.
        return;
    };
    // Never follow a path out of the project — the sensor reads whatever it
    // is handed, and a traversal would pull unrelated files into the graph.
    if rel.contains("..") || Path::new(&rel).is_absolute() {
        return;
    }
    let file_path = rel.as_str();
    let content = match std::fs::read_to_string(root.join(file_path)) {
        Ok(content) => content,
        // The file is gone — a `Delete File` patch block, or a delete routed
        // through the hook. Leaving its edges and its index entry behind
        // makes the very next freshness check report drift, which demotes
        // every structural rule to a warning until someone rebuilds by hand.
        // An empty extraction lets provenance-keyed compaction drop them.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Err(e) = on_delete(root, file_path) {
                tracing::debug!("graph sensor could not record deletion of {file_path}: {e}");
            }
            return;
        }
        Err(_) => return,
    };
    if let Err(e) = on_save(root, file_path, &content) {
        tracing::debug!("graph sensor skipped {file_path}: {e}");
    }
}

/// Drop a vanished file from both the graph and the staleness index.
///
/// Compaction is keyed on provenance, so replacing the file's edge set with
/// nothing removes exactly its contribution. The index entry must go too: a
/// hash recorded against a path that no longer exists is stale evidence that
/// `check_freshness` reports as drift forever.
fn on_delete(root: &Path, file_path: &str) -> std::io::Result<()> {
    let data_contract_input = root.join(".phronesis/graph.toml").is_file()
        && matches!(
            Path::new(file_path)
                .extension()
                .and_then(|value| value.to_str()),
            Some("rs" | "json" | "yaml" | "yml")
        );
    if file_path.ends_with(".cue")
        || file_path == ".phronesis/graph.toml"
        || file_path == ".phronesis/rules.json"
        || file_path.starts_with(".phronesis/wiki/decisions/")
        || data_contract_input
    {
        rebuild(root)?;
        return Ok(());
    }
    let existing = store::load(&store::graph_path(root))?;
    let base = store::compact(existing, file_path, Vec::new());
    persist(root, base)?;

    let ipath = index_path(root);
    let mut index = load_index(&ipath)?;
    if index.entries.remove(file_path).is_some() {
        index.generation = index.generation.saturating_add(1);
        save_index(&ipath, &index)?;
        reconcile_bindings_best_effort(root, index.generation);
    }
    Ok(())
}

/// Full rescan of every tracked file (Rust, Python, TypeScript), pruning
/// `node_modules`. The recovery path after the graph has drifted, and the
/// only way edges for deleted files are cleared.
pub fn rebuild(root: &Path) -> std::io::Result<SaveOutcome> {
    // Rules are graph consumers. Migrate their vocabulary before hashing
    // inputs so the rebuild cannot make its own index immediately stale.
    let migrated_rules = migrate_graph_rule_predicates(root)?;
    let mut base = Vec::new();
    let previous_generation = load_index(&index_path(root))
        .map(|index| index.generation)
        .unwrap_or(0);
    let mut index = Index {
        generation: previous_generation.saturating_add(1),
        ..Index::default()
    };
    let mut skipped = 0;
    let units = UnitMap::discover(root);
    let cue_index = super::cue::build_package_index(root);
    let files = tracked_files(root);
    let rust_inclusions = rust_path_inclusions(root, &files, &units);

    for rel in files {
        let Ok(content) = std::fs::read_to_string(root.join(&rel)) else {
            continue;
        };
        let extracted = extract_one(
            root,
            &rel,
            &content,
            &units,
            Some(&cue_index),
            &rust_inclusions,
        );
        skipped += extracted.skipped;
        if extracted.parse_failed {
            // A complete rebuild has observed this exact content and
            // intentionally excluded it. Record the hash so status does not
            // misreport a permanently skipped input as post-build drift.
            // A later edit changes the hash and correctly becomes stale.
            index.entries.insert(rel, hash_content(&content));
            continue;
        }
        base.extend(extracted.edges);
        index.entries.insert(rel, hash_content(&content));
    }
    base.extend(rust_inclusions.iter().map(|(file, included)| {
        Edge::base("includes_file", &[&included.owner, file], &included.owner)
    }));

    super::data_contracts::augment(root, &mut base);
    base.extend(rule_predicate_edges(root)?);
    base.extend(super::decisions::extract(root));
    if let Ok(content) = std::fs::read_to_string(root.join(".phronesis/graph.toml")) {
        index
            .entries
            .insert(".phronesis/graph.toml".to_string(), hash_content(&content));
    }
    for rel in decision_input_files(root) {
        if let Ok(content) = std::fs::read_to_string(root.join(&rel)) {
            index.entries.insert(rel, hash_content(&content));
        }
    }

    let (n_base, n_derived) = persist(root, base)?;
    save_index(&index_path(root), &index)?;
    reconcile_bindings_best_effort(root, index.generation);
    Ok(SaveOutcome {
        base: n_base,
        derived: n_derived,
        skipped,
        migrated_rules,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use tempfile::TempDir;

    fn project() -> TempDir {
        let d = TempDir::new().expect("tempdir");
        std::fs::create_dir_all(d.path().join("src")).expect("mkdir src");
        d
    }

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(p, body).expect("write");
    }

    fn edges(root: &Path) -> Vec<Edge> {
        store::load(&store::graph_path(root)).expect("load")
    }

    #[test]
    fn path_included_tests_use_the_compiled_module_identity_and_resolve_super_globs() {
        let dir = project();
        write(dir.path(), "src/lib.rs", "mod foo;");
        write(
            dir.path(),
            "src/foo.rs",
            "pub mod implementation { pub fn production() {} }\npub use implementation::{production};\n#[cfg(test)]\n#[path = \"../tests/unit/foo_tests.rs\"]\nmod tests;\n",
        );
        write(
            dir.path(),
            "tests/unit/foo_tests.rs",
            "use super::*;\nfn helper() {}\n#[test]\nfn works() { production(); helper(); }\n",
        );

        rebuild(dir.path()).expect("rebuild");
        let graph = edges(dir.path());
        let test = "rust:crate::foo::tests::works";
        assert!(
            graph.iter().any(|edge| {
                edge.p == "defines_test" && edge.a.get(1).map(String::as_str) == Some(test)
            }),
            "defines_test: {:?}",
            graph
                .iter()
                .filter(|edge| edge.p == "defines_test")
                .map(|edge| &edge.a)
                .collect::<Vec<_>>()
        );
        assert!(graph.iter().any(|edge| {
            edge.p == "tested_by" && edge.a == ["rust:crate::foo::implementation::production", test]
        }));
        assert!(!graph.iter().any(|edge| {
            edge.p == "tested_by"
                && edge
                    .a
                    .first()
                    .is_some_and(|target| target.ends_with("::helper"))
        }));
    }

    fn has(root: &Path, p: &str) -> bool {
        edges(root).iter().any(|e| e.p == p)
    }

    fn write_binding_rule(root: &Path) {
        write(
            root,
            ".phronesis/rules.json",
            r#"{"rules":[{"id":"tracks-foo","phase":"pre","when":[{"new_content_contains":"foo("}],"then":{"block":"foo contract"}}]}"#,
        );
    }

    // ─── hashing ────────────────────────────────────────────────────

    #[test]
    fn identical_content_hashes_identically() {
        assert_eq!(hash_content("fn f() {}"), hash_content("fn f() {}"));
    }

    #[test]
    fn different_content_hashes_differently() {
        assert_ne!(hash_content("fn f() {}"), hash_content("fn g() {}"));
    }

    #[test]
    fn graph_sync_reconciles_rule_bindings_in_the_same_generation() {
        let d = project();
        write_binding_rule(d.path());
        write(d.path(), "src/lib.rs", "pub fn foo() {}\n");
        rebuild(d.path()).expect("rebuild");

        let first = super::super::bindings::load(&super::super::bindings::bindings_path(d.path()))
            .expect("load")
            .expect("binding set");
        let index = load_index(&index_path(d.path())).expect("index");
        assert_eq!(first.generation, index.generation);
        assert_eq!(first.bindings.len(), 1);
        assert_eq!(
            first.bindings[0].state,
            super::super::bindings::BindingState::Bound
        );

        let changed = "pub fn replacement() {}\n";
        write(d.path(), "src/lib.rs", changed);
        on_save(d.path(), "src/lib.rs", changed).expect("save");
        let second = super::super::bindings::load(&super::super::bindings::bindings_path(d.path()))
            .expect("load")
            .expect("binding set");
        assert_eq!(
            second.bindings[0].state,
            super::super::bindings::BindingState::Stale
        );
        assert!(second.bindings[0].stale_at.is_some());
    }

    #[test]
    fn writing_rules_reconciles_without_advancing_the_graph() {
        let d = project();
        write(d.path(), "src/lib.rs", "pub fn late_rule_target() {}\n");
        rebuild(d.path()).expect("rebuild");
        let generation = load_index(&index_path(d.path())).expect("index").generation;

        let source: crate::rules_file::SourceRule = serde_json::from_value(serde_json::json!({
            "id": "late-rule",
            "phase": "pre",
            "when": [{"new_content_contains": "late_rule_target("}],
            "then": {"block": "target contract"}
        }))
        .expect("rule");
        crate::rules_file::write_source(&crate::rules_file::default_path(d.path()), &[source])
            .expect("rules write");

        let set = super::super::bindings::load(&super::super::bindings::bindings_path(d.path()))
            .expect("load")
            .expect("bindings");
        assert_eq!(set.generation, generation);
        assert_eq!(set.bindings.len(), 1);
        assert_eq!(set.bindings[0].symbol, "late_rule_target");
    }

    #[test]
    fn graph_sensor_reconciles_a_direct_rules_file_edit() {
        let d = project();
        write(d.path(), "src/lib.rs", "pub fn direct_edit_target() {}\n");
        rebuild(d.path()).expect("rebuild");
        write(
            d.path(),
            ".phronesis/rules.json",
            r#"{"rules":[{"id":"direct-rule","phase":"pre","when":[{"new_content_contains":"direct_edit_target("}],"then":{"block":"target contract"}}]}"#,
        );

        record_from_disk(d.path(), ".phronesis/rules.json");

        let set = super::super::bindings::load(&super::super::bindings::bindings_path(d.path()))
            .expect("load")
            .expect("bindings");
        assert_eq!(set.bindings.len(), 1);
        assert_eq!(set.bindings[0].symbol, "direct_edit_target");
    }

    // ─── index round trip ───────────────────────────────────────────

    #[test]
    fn a_missing_index_loads_as_empty() {
        let d = project();
        assert_eq!(
            load_index(&index_path(d.path())).expect("load"),
            Index::default()
        );
    }

    #[test]
    fn the_index_survives_a_save_load_round_trip() {
        let d = project();
        let mut idx = Index::default();
        idx.entries.insert("src/a.rs".into(), 42);
        let p = index_path(d.path());
        save_index(&p, &idx).expect("save");
        assert_eq!(load_index(&p).expect("load").entries, idx.entries);
    }

    #[test]
    fn a_written_index_is_stamped_with_the_current_format() {
        let d = project();
        let p = index_path(d.path());
        save_index(&p, &Index::default()).expect("save");
        assert_eq!(load_index(&p).expect("load").format, GRAPH_FORMAT);
    }

    #[test]
    fn a_complete_rebuild_records_intentionally_skipped_content_as_observed() {
        let d = project();
        write(d.path(), "config/broken.json", "{not valid json");

        let outcome = rebuild(d.path()).expect("rebuild");
        assert!(outcome.skipped > 0);
        let index = load_index(&index_path(d.path())).expect("index");
        assert_eq!(check_freshness(d.path(), &index), Freshness::Fresh);

        write(d.path(), "config/broken.json", "{still not valid json");
        assert!(matches!(
            check_freshness(d.path(), &index),
            Freshness::Stale { .. }
        ));
    }

    #[test]
    fn rebuild_migrates_deprecated_graph_predicates_without_losing_rule_metadata() {
        let d = project();
        write(d.path(), "src/a.rs", "fn risky() { panic!(); }");
        write(
            d.path(),
            ".phronesis/rules.json",
            r#"{
              "rules": [{
                "id": "legacy-graph-rule",
                "phase": "audit",
                "priority": 17,
                "audit": true,
                "silent": true,
                "doc_excepted": true,
                "binds": false,
                "when": [
                  {"untested": ["?func"]},
                  {"or": [
                    {"untested": ["?other"]},
                    {"calls_api": ["?func", "panic"]}
                  ]}
                ],
                "then": {"warn": "legacy relation"}
              }]
            }"#,
        );
        assert_eq!(
            deprecated_graph_rule_predicates(d.path()).expect("predicate drift"),
            vec!["untested"]
        );

        let outcome = rebuild(d.path()).expect("rebuild");
        assert_eq!(outcome.migrated_rules, 1);
        let migrated = std::fs::read_to_string(d.path().join(".phronesis/rules.json"))
            .expect("migrated rules");
        assert!(!migrated.contains("\"untested\""));
        assert_eq!(migrated.matches("\"no_direct_test\"").count(), 2);
        for metadata in [
            "\"audit\": true",
            "\"silent\": true",
            "\"doc_excepted\": true",
            "\"binds\": false",
        ] {
            assert!(
                migrated.contains(metadata),
                "missing {metadata}: {migrated}"
            );
        }
        let index = load_index(&index_path(d.path())).expect("index");
        assert_eq!(check_freshness(d.path(), &index), Freshness::Fresh);
        assert!(
            deprecated_graph_rule_predicates(d.path())
                .expect("resolved drift")
                .is_empty()
        );

        let second = rebuild(d.path()).expect("idempotent rebuild");
        assert_eq!(second.migrated_rules, 0);
    }

    #[test]
    fn rebuild_migrates_dynamic_boundary_relations_and_literal_callable_ids() {
        let d = project();
        write(
            d.path(),
            ".phronesis/rules.json",
            r#"{"rules":[{"id":"legacy-rhai","phase":"audit","when":[{"rhai_exposes_fn":["?host","state_get_hp"]},{"calls_rhai_fn":["?script","state_*"]}],"then":{"warn":"legacy"}}]}"#,
        );
        let outcome = rebuild(d.path()).expect("rebuild");
        assert_eq!(outcome.migrated_rules, 1);
        let migrated = std::fs::read_to_string(d.path().join(".phronesis/rules.json"))
            .expect("migrated rules");
        assert!(migrated.contains("\"exposes\""));
        assert!(migrated.contains("\"calls\""));
        assert!(migrated.contains("rhai:callable::state_get_hp"));
        assert!(migrated.contains("rhai:callable::state_*"));
    }

    #[test]
    fn rebuild_refuses_a_non_equivalent_resolution_rule_migration() {
        let d = project();
        write(
            d.path(),
            ".phronesis/rules.json",
            r#"{"rules":[{"id":"legacy-resolution","phase":"audit","when":[{"rhai_call_resolves_to":["?script","?target"]}],"then":{"warn":"legacy"}}]}"#,
        );
        let error = rebuild(d.path()).expect_err("manual migration required");
        assert!(error.to_string().contains("manual rule migration"));
        let unchanged = std::fs::read_to_string(d.path().join(".phronesis/rules.json"))
            .expect("rules remain readable");
        assert!(unchanged.contains("rhai_call_resolves_to"));
    }

    #[test]
    fn an_index_without_a_format_header_reads_as_format_zero() {
        // The shape written before identity versioning existed.
        let d = project();
        let p = index_path(d.path());
        write(d.path(), INDEX_REL_PATH, "42 src/a.rs\n");
        let idx = load_index(&p).expect("load");
        assert_eq!(idx.format, 0);
        assert_eq!(idx.entries.get("src/a.rs"), Some(&42));
    }

    // ─── identity-format migration ──────────────────────────────────

    #[test]
    fn a_graph_built_under_an_older_identity_format_is_not_fresh() {
        // Content hashes match exactly — nothing on disk changed. Only the
        // format header betrays that every edge carries the old naming.
        let d = project();
        write(d.path(), "src/a.rs", "fn f() {}");
        let index = Index {
            format: 0,
            generation: 0,
            entries: BTreeMap::from([("src/a.rs".to_string(), hash_content("fn f() {}"))]),
        };
        assert_eq!(
            check_freshness(d.path(), &index),
            Freshness::Outdated {
                found: 0,
                expected: GRAPH_FORMAT,
            }
        );
    }

    #[test]
    fn an_index_that_describes_nothing_is_never_reported_as_outdated() {
        // A project that has never built a graph has format 0 too; calling
        // that a migration would demand a rebuild of an empty graph.
        let d = project();
        assert_ne!(
            check_freshness(d.path(), &Index::default()),
            Freshness::Outdated {
                found: 0,
                expected: GRAPH_FORMAT,
            }
        );
    }

    #[test]
    fn saving_into_an_older_format_graph_rebuilds_every_file() {
        let d = project();
        write(d.path(), "src/a.rs", "fn alpha() {}");
        write(d.path(), "src/b.rs", "fn beta() {}");
        // A graph in the pre-versioning naming, with an index that says it is
        // current for both files.
        store::write_atomic(
            &store::graph_path(d.path()),
            &[
                Edge::base("defines_fn", &["src/a.rs", "crate::a::alpha"], "src/a.rs"),
                Edge::base("defines_fn", &["src/b.rs", "crate::b::beta"], "src/b.rs"),
            ],
        )
        .expect("seed graph");
        save_index(
            &index_path(d.path()),
            &Index {
                format: 0,
                generation: 0,
                entries: BTreeMap::from([
                    ("src/a.rs".to_string(), hash_content("fn alpha() {}")),
                    ("src/b.rs".to_string(), hash_content("fn beta() {}")),
                ]),
            },
        )
        .expect("seed index");
        // Written by `save_index`, which always stamps the current format;
        // force the legacy shape back onto disk.
        std::fs::write(index_path(d.path()), "0 src/a.rs\n0 src/b.rs\n").expect("legacy index");

        on_save(d.path(), "src/a.rs", "fn alpha() {}").expect("save");

        let names: Vec<String> = edges(d.path())
            .into_iter()
            .filter(|e| e.p == "defines_fn")
            .filter_map(|e| e.a.get(1).cloned())
            .collect();
        assert!(
            names.iter().all(|n| n.starts_with("rust:")),
            "the unedited file must be migrated too, not left in the old naming: {names:?}"
        );
        assert!(names.iter().any(|n| n.ends_with("::beta")), "{names:?}");
        assert_eq!(
            load_index(&index_path(d.path())).expect("load").format,
            GRAPH_FORMAT,
            "a migrated graph must stop reporting itself outdated"
        );
    }

    // ─── the per-save pipeline ──────────────────────────────────────

    #[test]
    fn saving_a_file_writes_its_base_edges() {
        let d = project();
        write(d.path(), "src/a.rs", "fn f() {}");
        on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
        assert!(has(d.path(), "defines_fn"));
    }

    #[test]
    fn saving_a_file_derives_untested_in_the_same_pass() {
        let d = project();
        write(d.path(), "src/a.rs", "fn f() {}");
        on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
        assert!(
            has(d.path(), "no_direct_test"),
            "derived facts must be current after every save, not only after rebuild"
        );
    }

    #[test]
    fn a_test_added_later_clears_untested_without_a_rebuild() {
        let d = project();
        write(d.path(), "src/a.rs", "fn fire() {}");
        on_save(d.path(), "src/a.rs", "fn fire() {}").expect("save");
        assert!(has(d.path(), "no_direct_test"));

        let test_src = "#[test]\nfn t() { fire(); }";
        write(d.path(), "tests/a.rs", test_src);
        on_save(d.path(), "tests/a.rs", test_src).expect("save");
        assert!(
            !has(d.path(), "no_direct_test"),
            "coverage is whole-repo: a test elsewhere must clear it"
        );
    }

    #[test]
    fn re_saving_a_file_replaces_its_edges_rather_than_duplicating_them() {
        let d = project();
        for _ in 0..3 {
            on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
        }
        let defines: Vec<_> = edges(d.path())
            .into_iter()
            .filter(|e| e.p == "defines_fn")
            .collect();
        assert_eq!(defines.len(), 1);
    }

    #[test]
    fn removing_a_function_removes_its_edges() {
        let d = project();
        on_save(d.path(), "src/a.rs", "fn f() {}\nfn g() {}").expect("save");
        on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
        let names: Vec<String> = edges(d.path())
            .into_iter()
            .filter(|e| e.p == "defines_fn")
            .filter_map(|e| e.a.get(1).cloned())
            .collect();
        assert_eq!(names, vec!["rust:crate::a::f".to_string()]);
    }

    #[test]
    fn derived_edges_do_not_accumulate_across_saves() {
        let d = project();
        on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
        on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
        let untested: Vec<_> = edges(d.path())
            .into_iter()
            .filter(|e| e.p == "no_direct_test")
            .collect();
        assert_eq!(untested.len(), 1);
    }

    #[test]
    fn saving_records_the_files_hash_in_the_index() {
        let d = project();
        on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
        let idx = load_index(&index_path(d.path())).expect("load");
        assert_eq!(
            idx.entries.get("src/a.rs"),
            Some(&hash_content("fn f() {}"))
        );
    }

    #[test]
    fn a_non_rust_file_is_ignored() {
        let d = project();
        on_save(d.path(), "README.md", "# hi").expect("save");
        assert!(edges(d.path()).is_empty());
    }

    #[test]
    fn saving_a_typescript_file_writes_its_base_edges() {
        let d = project();
        write(d.path(), "package.json", r#"{"name": "myapp"}"#);
        write(
            d.path(),
            "src/billing.ts",
            "export function charge() { return 1 }\n",
        );
        rebuild(d.path()).expect("opt the project into the graph");
        let names: Vec<String> = edges(d.path())
            .into_iter()
            .filter(|e| e.p == "defines_fn")
            .filter_map(|e| e.a.get(1).cloned())
            .collect();
        // No tsconfig.json exists, so there is no baseUrl: `src/` stays part
        // of the module path (see graph/resolve.rs's `strip_module_base` and
        // `graph/unit.rs`'s `context_for`, which only strips a unit's
        // `baseUrl` from the front of a file's path).
        assert!(
            names.contains(&"typescript:myapp::src::billing::charge".to_string()),
            "{names:?}"
        );
    }

    // ─── the hook entry point ───────────────────────────────────────

    #[test]
    fn a_project_that_never_opted_into_the_graph_gets_no_graph() {
        // The sensor runs before rules load, so it cannot key off rule
        // phases. Without an explicit gate it builds a graph in every
        // phronesis project on the first edit — imposing a per-save tree walk
        // and an unasked-for file on users who only wanted the `llm` pack.
        let d = project();
        write(d.path(), "src/a.rs", "fn f() {}");
        record_from_disk(d.path(), "src/a.rs");
        assert!(
            !store::graph_path(d.path()).exists(),
            "no graph existed, so none should be created"
        );
        assert!(!index_path(d.path()).exists());
    }

    #[test]
    fn an_existing_graph_is_still_kept_current() {
        // `init --packs structural` builds the graph; its presence is the
        // opt-in signal the sensor keys off.
        let d = project();
        write(d.path(), "src/a.rs", "fn f() {}");
        rebuild(d.path()).expect("rebuild opts the project in");
        write(d.path(), "src/a.rs", "fn f() {}\nfn added() {}");
        record_from_disk(d.path(), "src/a.rs");
        let names: Vec<String> = edges(d.path())
            .into_iter()
            .filter(|e| e.p == "defines_fn")
            .filter_map(|e| e.a.get(1).cloned())
            .collect();
        assert!(names.iter().any(|n| n.ends_with("::added")), "{names:?}");
    }

    #[test]
    fn recording_from_disk_accepts_the_absolute_path_a_host_sends() {
        // Every real host sends an absolute path. A guard that rejected them
        // silently disabled the sensor everywhere, and because the sensor is
        // best-effort by design nothing reported it.
        let d = project();
        write(d.path(), "src/a.rs", "fn f() {}");
        rebuild(d.path()).expect("opt the project into the graph");
        write(d.path(), "src/a.rs", "fn f() {}\nfn added() {}");
        let absolute = d.path().join("src/a.rs");
        record_from_disk(d.path(), absolute.to_str().expect("utf8"));
        let names: Vec<String> = edges(d.path())
            .into_iter()
            .filter(|e| e.p == "defines_fn")
            .filter_map(|e| e.a.get(1).cloned())
            .collect();
        assert!(
            names.iter().any(|n| n.ends_with("::added")),
            "sensor must record the edit: {names:?}"
        );
    }

    #[test]
    fn a_parse_failure_preserves_the_files_existing_edges() {
        // A malformed mid-edit save must not be read as "this file now
        // defines nothing". Compacting an empty extraction erases every
        // function, risky call and import the file had, and recording the
        // malformed content's hash makes the graph report itself fresh — so
        // the harness keeps enforcing on evidence it silently destroyed.
        let d = project();
        write(d.path(), "src/a.rs", "fn important() {}");
        on_save(d.path(), "src/a.rs", "fn important() {}").expect("save");
        assert!(has(d.path(), "defines_fn"), "precondition");

        let broken = "fn important( { ((( ";
        write(d.path(), "src/a.rs", broken);
        on_save(d.path(), "src/a.rs", broken).expect("save");

        let names: Vec<String> = edges(d.path())
            .into_iter()
            .filter(|e| e.p == "defines_fn")
            .filter_map(|e| e.a.get(1).cloned())
            .collect();
        assert!(
            names.iter().any(|n| n.ends_with("::important")),
            "existing evidence must survive a parse failure: {names:?}"
        );
    }

    #[test]
    fn a_parse_failure_leaves_the_file_reported_as_stale() {
        // Staleness is the honest state: the graph no longer reflects what is
        // on disk, and structural rules must demote to warnings until it does.
        let d = project();
        write(d.path(), "src/a.rs", "fn important() {}");
        on_save(d.path(), "src/a.rs", "fn important() {}").expect("save");

        let broken = "fn important( { ((( ";
        write(d.path(), "src/a.rs", broken);
        on_save(d.path(), "src/a.rs", broken).expect("save");

        let index = load_index(&index_path(d.path())).expect("load index");
        assert_eq!(
            check_freshness(d.path(), &index),
            Freshness::Stale(vec!["src/a.rs".to_string()]),
            "an unparseable file must not be recorded as successfully indexed"
        );
    }

    #[test]
    fn deleting_a_file_removes_its_edges_and_its_index_entry() {
        // A `Delete File` patch block routes through the sensor. Leaving the
        // edges and hash behind makes every later freshness check report
        // drift, demoting structural rules to warnings until a manual
        // rebuild — the exact failure the sensor exists to prevent.
        let d = project();
        write(d.path(), "src/a.rs", "fn alpha() {}");
        write(d.path(), "src/b.rs", "fn beta() {}");
        on_save(d.path(), "src/a.rs", "fn alpha() {}").expect("save a");
        on_save(d.path(), "src/b.rs", "fn beta() {}").expect("save b");

        std::fs::remove_file(d.path().join("src/a.rs")).expect("delete");
        record_from_disk(d.path(), "src/a.rs");

        let names: Vec<String> = edges(d.path())
            .into_iter()
            .filter(|e| e.p == "defines_fn")
            .filter_map(|e| e.a.get(1).cloned())
            .collect();
        assert!(
            names.iter().all(|n| !n.ends_with("::alpha")),
            "the deleted file's edges must go: {names:?}"
        );
        assert!(names.iter().any(|n| n.ends_with("::beta")), "{names:?}");
        assert!(
            !load_index(&index_path(d.path()))
                .expect("load index")
                .entries
                .contains_key("src/a.rs"),
            "a hash for a path that no longer exists is permanent drift"
        );
        assert_eq!(
            check_freshness(d.path(), &load_index(&index_path(d.path())).expect("index")),
            Freshness::Fresh,
            "a recorded deletion must leave the graph fresh"
        );
    }

    #[test]
    fn recording_from_disk_ignores_a_path_outside_the_project() {
        let d = project();
        let outside = TempDir::new().expect("tempdir");
        write(outside.path(), "evil.rs", "fn f() {}");
        let absolute = outside.path().join("evil.rs");
        record_from_disk(d.path(), absolute.to_str().expect("utf8"));
        assert!(
            edges(d.path()).is_empty(),
            "a file outside the project has no graph identity"
        );
    }

    #[test]
    fn recording_from_disk_reads_the_current_file_content() {
        let d = project();
        write(d.path(), "src/a.rs", "fn f() {}");
        rebuild(d.path()).expect("opt the project into the graph");
        write(d.path(), "src/a.rs", "fn f() {}\nfn later() {}");
        record_from_disk(d.path(), "src/a.rs");
        let names: Vec<String> = edges(d.path())
            .into_iter()
            .filter(|e| e.p == "defines_fn")
            .filter_map(|e| e.a.get(1).cloned())
            .collect();
        assert!(names.iter().any(|n| n.ends_with("::later")), "{names:?}");
    }

    #[test]
    fn recording_a_file_outside_the_project_is_a_no_op() {
        // Path traversal must not let the sensor read arbitrary files.
        let d = project();
        record_from_disk(d.path(), "../../etc/hosts.rs");
        assert!(edges(d.path()).is_empty());
    }

    #[test]
    fn recording_a_missing_file_is_a_no_op_rather_than_an_error() {
        let d = project();
        record_from_disk(d.path(), "src/gone.rs");
        assert!(edges(d.path()).is_empty());
    }

    #[test]
    fn recording_a_node_modules_file_is_a_no_op() {
        // `tracked_files` (and thus `rebuild`/`check_freshness`) prunes
        // `node_modules`. If the sensor recorded an index entry for a path
        // under it anyway, that hash could never be matched by the walk and
        // the file would report as drift forever — the exact permanent-
        // demotion failure the sensor exists to prevent, entering through
        // `on_save` instead of `rebuild`. `patch-package`, `prisma
        // generate`, or an agent edit under `node_modules` can all arrive
        // here through `PostToolUse`.
        let d = project();
        write(d.path(), "package.json", r#"{"name": "myapp"}"#);
        write(
            d.path(),
            "src/billing.ts",
            "export function charge() { return 1 }\n",
        );
        rebuild(d.path()).expect("opt the project into the graph");

        write(
            d.path(),
            "node_modules/dep/index.ts",
            "export function vendored() { return 1 }\n",
        );
        record_from_disk(d.path(), "node_modules/dep/index.ts");

        let idx = load_index(&index_path(d.path())).expect("load index");
        assert!(
            !idx.entries.contains_key("node_modules/dep/index.ts"),
            "node_modules must never gain an index entry: {:?}",
            idx.entries.keys().collect::<Vec<_>>()
        );
        let names: Vec<String> = edges(d.path())
            .into_iter()
            .filter(|e| e.p == "defines_fn")
            .filter_map(|e| e.a.get(1).cloned())
            .collect();
        assert!(
            names.iter().all(|n| !n.contains("vendored")),
            "node_modules must not be extracted: {names:?}"
        );
        assert_eq!(
            check_freshness(d.path(), &idx),
            Freshness::Fresh,
            "a node_modules edit must not be reported as drift"
        );
    }

    // ─── staleness ──────────────────────────────────────────────────

    #[test]
    fn an_untouched_project_is_fresh() {
        let d = project();
        write(d.path(), "src/a.rs", "fn f() {}");
        on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
        let idx = load_index(&index_path(d.path())).expect("load");
        assert_eq!(check_freshness(d.path(), &idx), Freshness::Fresh);
    }

    #[test]
    fn an_edit_outside_the_hook_path_is_detected_as_stale() {
        let d = project();
        write(d.path(), "src/a.rs", "fn f() {}");
        on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
        // Simulates git checkout / shell edit: content changes, hook never runs.
        write(d.path(), "src/a.rs", "fn f() {}\nfn sneaky() {}");
        let idx = load_index(&index_path(d.path())).expect("load");
        assert_eq!(
            check_freshness(d.path(), &idx),
            Freshness::Stale(vec!["src/a.rs".to_string()])
        );
    }

    #[test]
    fn a_deleted_file_is_detected_as_stale() {
        let d = project();
        write(d.path(), "src/a.rs", "fn f() {}");
        on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
        std::fs::remove_file(d.path().join("src/a.rs")).expect("rm");
        let idx = load_index(&index_path(d.path())).expect("load");
        assert!(matches!(
            check_freshness(d.path(), &idx),
            Freshness::Stale(_)
        ));
    }

    #[test]
    fn an_untracked_new_file_is_detected_as_stale() {
        let d = project();
        write(d.path(), "src/a.rs", "fn f() {}");
        on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
        write(d.path(), "src/b.rs", "fn g() {}");
        let idx = load_index(&index_path(d.path())).expect("load");
        assert!(matches!(
            check_freshness(d.path(), &idx),
            Freshness::Stale(_)
        ));
    }

    // ─── rebuild ────────────────────────────────────────────────────

    #[test]
    fn rebuild_indexes_every_rust_file_in_the_project() {
        let d = project();
        write(d.path(), "src/a.rs", "fn f() {}");
        write(d.path(), "src/b.rs", "fn g() {}");
        rebuild(d.path()).expect("rebuild");
        let names: Vec<String> = edges(d.path())
            .into_iter()
            .filter(|e| e.p == "defines_fn")
            .filter_map(|e| e.a.get(1).cloned())
            .collect();
        assert_eq!(names.len(), 2, "both files must be scanned");
    }

    #[test]
    fn rebuild_composes_multilingual_modules_and_cross_language_schema_imports() {
        let d = project();
        write(d.path(), "src/lib.rs", "pub fn run() {}");
        write(
            d.path(),
            "scripts/run.lua",
            "function run() return true end",
        );
        write(d.path(), "config/model.cue", "#Model: { name: string }");
        write(
            d.path(),
            "schemas/base.yaml",
            "$schema: https://json-schema.org/draft/2020-12/schema\n$anchor: User\n",
        );
        write(
            d.path(),
            "schemas/user.json",
            r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","$ref":"base.yaml#User"}"#,
        );
        write(
            d.path(),
            "charts/app/Chart.yaml",
            "name: app\nversion: 0.1.0\n",
        );
        write(
            d.path(),
            "charts/app/templates/_helpers.tpl",
            "{{ define \"app.name\" }}app{{ end }}",
        );
        write(
            d.path(),
            "charts/app/templates/deployment.yaml",
            "apiVersion: apps/v1\nmetadata:\n  name: {{ include \"app.name\" . }}\n",
        );

        rebuild(d.path()).expect("rebuild multilingual graph");
        let graph = edges(d.path());

        for prefix in ["rust:", "lua:", "cue:", "json:", "yaml:", "helm3:"] {
            assert!(
                graph.iter().any(|edge| {
                    edge.p == "declares_module"
                        && edge
                            .a
                            .get(1)
                            .is_some_and(|module| module.starts_with(prefix))
                }),
                "missing language-qualified {prefix} module"
            );
        }
        assert!(graph.iter().any(|edge| {
            edge.p == "imports"
                && edge.a
                    == [
                        "json:project::schemas::user".to_string(),
                        "yaml:project::schemas::base::doc:0".to_string(),
                    ]
        }));
        assert!(
            graph.iter().any(|edge| {
                edge.p == "declares_module"
                    && edge.a
                        == [
                            "charts/app/templates/deployment.yaml".to_string(),
                            "helm3:charts/app::templates::deployment".to_string(),
                        ]
            }),
            "templated YAML ownership: {:?}",
            graph
                .iter()
                .filter(|edge| edge.src.contains("deployment.yaml"))
                .collect::<Vec<_>>()
        );
        assert!(
            graph
                .iter()
                .filter(|edge| edge.p == "graph_definition")
                .all(|edge| { edge.a.len() == 1 && edge.a[0].contains(':') })
        );

        let modules = graph
            .iter()
            .filter(|edge| edge.p == "declares_module")
            .filter_map(|edge| edge.a.get(1))
            .collect::<BTreeSet<_>>();
        for edge in graph.iter().filter(|edge| edge.p == "imports") {
            let Some(target) = edge.a.get(1) else {
                continue;
            };
            if ["cue:", "lua:", "json:", "yaml:", "helm3:"]
                .iter()
                .any(|prefix| target.starts_with(prefix))
            {
                assert!(
                    modules.contains(target),
                    "repository-local dependency target is not a graph node: {edge:?}"
                );
            }
        }
        for prefix in ["cue:", "lua:", "json:", "yaml:", "helm3:"] {
            let definitions = graph
                .iter()
                .filter(|edge| edge.p == "graph_definition")
                .filter(|edge| edge.a[0].starts_with(prefix))
                .count();
            assert!(
                definitions <= 8,
                "representative {prefix} fixture emitted excessive definitions: {definitions}"
            );
        }
    }

    #[test]
    fn rebuild_restores_freshness_after_drift() {
        let d = project();
        write(d.path(), "src/a.rs", "fn f() {}");
        on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
        write(d.path(), "src/a.rs", "fn f() {}\nfn sneaky() {}");
        rebuild(d.path()).expect("rebuild");
        let idx = load_index(&index_path(d.path())).expect("load");
        assert_eq!(check_freshness(d.path(), &idx), Freshness::Fresh);
    }

    #[test]
    fn rebuild_excludes_node_modules_from_typescript_tracking() {
        // `tracked_files` drives both `rebuild` and the freshness check. If
        // `node_modules` is not pruned there too — discovery already prunes
        // it, but that is a separate walk — `rebuild` would extract from
        // every dependency's TypeScript, and every one of those files would
        // then show as drift on every subsequent freshness check.
        let d = project();
        write(d.path(), "package.json", r#"{"name": "myapp"}"#);
        write(
            d.path(),
            "src/billing.ts",
            "export function charge() { return 1 }\n",
        );
        write(
            d.path(),
            "node_modules/dep/index.ts",
            "export function vendored() { return 1 }\n",
        );
        rebuild(d.path()).expect("rebuild");

        let names: Vec<String> = edges(d.path())
            .into_iter()
            .filter(|e| e.p == "defines_fn")
            .filter_map(|e| e.a.get(1).cloned())
            .collect();
        assert!(
            names.iter().all(|n| !n.contains("vendored")),
            "node_modules must not be extracted: {names:?}"
        );

        let idx = load_index(&index_path(d.path())).expect("load index");
        assert!(
            !idx.entries.keys().any(|k| k.contains("node_modules")),
            "node_modules must not be indexed: {:?}",
            idx.entries.keys().collect::<Vec<_>>()
        );
        assert_eq!(
            check_freshness(d.path(), &idx),
            Freshness::Fresh,
            "an untracked node_modules file must not be reported as drift"
        );
    }

    #[test]
    fn a_typescript_project_is_fresh_immediately_after_rebuild() {
        // Adding four new tracked extensions means files that were
        // previously invisible to `tracked_files` are now tracked. A
        // TypeScript project must report Fresh right after `rebuild`, not
        // immediately show drift.
        let d = project();
        write(d.path(), "package.json", r#"{"name": "myapp"}"#);
        write(
            d.path(),
            "src/billing.ts",
            "export function charge() { return 1 }\n",
        );
        rebuild(d.path()).expect("rebuild");
        let idx = load_index(&index_path(d.path())).expect("load index");
        assert_eq!(check_freshness(d.path(), &idx), Freshness::Fresh);
    }

    #[test]
    fn rebuild_tracks_and_indexes_rhai_scripts() {
        let d = project();
        write(
            d.path(),
            "scripts/combat.rhai",
            "state_attempt_stunning_strike(actor, target);\n",
        );
        rebuild(d.path()).expect("rebuild");
        assert!(
            edges(d.path())
                .iter()
                .any(|edge| { edge.p == "calls" && edge.src == "scripts/combat.rhai" })
        );
        let index = load_index(&index_path(d.path())).expect("index");
        assert!(index.entries.contains_key("scripts/combat.rhai"));
    }

    #[test]
    fn rebuild_drops_edges_for_files_that_no_longer_exist() {
        let d = project();
        write(d.path(), "src/a.rs", "fn f() {}");
        on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
        std::fs::remove_file(d.path().join("src/a.rs")).expect("rm");
        rebuild(d.path()).expect("rebuild");
        assert!(edges(d.path()).is_empty());
    }
}

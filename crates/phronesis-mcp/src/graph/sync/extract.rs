//! Per-file extraction routing and the Rust-only ownership enrichment that
//! rides alongside it.

use super::index::tracked_files;
use crate::graph;
use crate::graph::extract::{
    DEFAULT_WATCHLIST, extract_rust_at_module_with_ownership, module_path,
};
use crate::graph::helm3;
use crate::graph::model::Edge;
use crate::graph::ownership;
use crate::graph::ownership::config::OwnershipConfig;
use crate::graph::ownership::provider::{
    AnalysisTrigger, OwnershipEvidenceProvider, OwnershipFunction, RustAnalyzerProvider,
    failure_report, incremental_stale_report,
};
use crate::graph::unit::{UnitContext, UnitMap};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Clone)]
pub(super) struct IncludedRustModule {
    pub(super) module: String,
    pub(super) unit: UnitContext,
    pub(super) owner: String,
}

/// Resolve external `#[path = "…"] mod name;` files to the module identity
/// Rust actually compiles them under. Only parser-confirmed module items and
/// existing in-repository files participate; conflicting inclusions are
/// omitted rather than assigned an arbitrary owner.
pub(super) fn rust_path_inclusions(
    root: &Path,
    files: &[String],
    units: &UnitMap,
) -> BTreeMap<String, IncludedRustModule> {
    let mut candidates: BTreeMap<String, Vec<IncludedRustModule>> = BTreeMap::new();
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    for owner in files.iter().filter(|file| file.ends_with(".rs")) {
        collect_owner_inclusions(root, &canonical_root, owner, units, &mut candidates);
    }
    candidates
        .into_iter()
        .filter_map(|(path, mut owners)| (owners.len() == 1).then(|| (path, owners.remove(0))))
        .collect()
}

/// Record every `#[path]` module item in `owner` under the in-repository
/// file it resolves to.
fn collect_owner_inclusions(
    root: &Path,
    canonical_root: &Path,
    owner: &str,
    units: &UnitMap,
    candidates: &mut BTreeMap<String, Vec<IncludedRustModule>>,
) {
    use crate::syntax::parsed::ParsedFile;

    let Ok(content) = std::fs::read_to_string(root.join(owner)) else {
        return;
    };
    let Some(ParsedFile::Rust { tree, source }) = ParsedFile::parse_rust(&content) else {
        return;
    };
    let (unit, owner_module) = {
        let unit = units.context_for(owner);
        let owner_module = module_path(owner, &unit);
        (unit, owner_module)
    };
    for node in tree
        .root_node()
        .named_children(&mut tree.root_node().walk())
    {
        if node.kind() != "mod_item" {
            continue;
        }
        let Some(path_literal) = path_attribute_literal(node, &source) else {
            continue;
        };
        let Some(name) = node
            .child_by_field_name("name")
            .and_then(|name| name.utf8_text(source.as_bytes()).ok())
        else {
            continue;
        };
        let Some(relative) = resolve_inclusion_target(root, canonical_root, owner, path_literal)
        else {
            continue;
        };
        candidates
            .entry(relative)
            .or_default()
            .push(IncludedRustModule {
                module: format!("{owner_module}::{name}"),
                unit: unit.clone(),
                owner: owner.to_string(),
            });
    }
}

/// The string literal of a `#[path = "…"]` attribute on a module item, from
/// either an outer attribute preceding it or an inner one.
fn path_attribute_literal<'s>(node: tree_sitter::Node<'_>, source: &'s str) -> Option<&'s str> {
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
    attributes.iter().find_map(|attribute| {
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
    })
}

/// Canonicalize `path_literal` relative to `owner`'s directory and express it
/// relative to the repository root, if it resolves to an existing file there.
fn resolve_inclusion_target(
    root: &Path,
    canonical_root: &Path,
    owner: &str,
    path_literal: &str,
) -> Option<String> {
    let owner_dir = root.join(owner).parent().unwrap_or(root).to_path_buf();
    let target = owner_dir.join(path_literal).canonicalize().ok()?;
    let relative = target.strip_prefix(canonical_root).ok()?;
    Some(relative.to_str()?.replace('\\', "/"))
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

/// Everything `extract_one` needs beyond the file itself, assembled once per
/// pass by the caller.
pub(super) struct ExtractOneParams<'a> {
    pub(super) root: &'a Path,
    pub(super) rel: &'a str,
    pub(super) content: &'a str,
    pub(super) units: &'a UnitMap,
    pub(super) cue_index: Option<&'a graph::cue::PackageIndex>,
    pub(super) rust_inclusions: &'a BTreeMap<String, IncludedRustModule>,
    /// Loaded **once per pass** by the caller, never per file: it is one read
    /// of `.phronesis/graph.toml`, and re-reading it for every tracked file
    /// would turn a rebuild into thousands of redundant stats and reads. Only
    /// the Rust arm consumes it — the enrichment is Rust-only in Phase One.
    pub(super) ownership: &'a OwnershipConfig,
}

/// Stamp the Helm chart identity onto `unit`.
fn helm_unit_for(chart_root: &str, mut unit: UnitContext) -> UnitContext {
    let chart_identity = if chart_root.is_empty() {
        "project"
    } else {
        chart_root
    };
    unit.id = format!("helm3:{chart_identity}");
    unit
}

/// A CUE unit named after the module `rel` belongs to.
fn cue_unit_for(root: &Path, rel: &str) -> UnitContext {
    let mut cue_unit = UnitContext::unnamed_for(graph::unit::LANG_CUE);
    cue_unit.id = format!("cue:{}", graph::cue::discover_module(root, rel));
    cue_unit
}

/// `unit` with every tracked file attached, for extractors that resolve
/// cross-file references.
fn with_tracked_files(root: &Path, mut unit: UnitContext) -> UnitContext {
    unit.files = tracked_files(root);
    unit
}

/// `unit` with every tracked file attached as Lua resolution candidates.
fn with_lua_files(root: &Path, mut unit: UnitContext) -> UnitContext {
    unit.lua_files = tracked_files(root);
    unit
}

/// Route one file to the extractor for its language.
pub(super) fn extract_one(params: ExtractOneParams<'_>) -> graph::extract::Extracted {
    let ExtractOneParams {
        root,
        rel,
        content,
        units,
        cue_index,
        rust_inclusions,
        ownership,
    } = params;
    let inclusion = rust_inclusions.get(rel);
    let unit = inclusion
        .map(|included| included.unit.clone())
        .unwrap_or_else(|| units.context_for(rel));
    // Helm owns templated YAML beneath a chart's templates/ tree. Extension
    // alone is insufficient: routing such a file through generic YAML first
    // would either misparse Go actions or silently discard the file.
    if matches!(graph::unit::lang_of_path(rel), Some(graph::unit::LANG_YAML))
        && rel.contains("/templates/")
        && content.contains("{{")
        && let Some((chart_root, _chart_name)) = chart_for(root, rel)
    {
        let helm_unit = helm_unit_for(
            &chart_root,
            UnitContext::unnamed_for(graph::unit::LANG_HELM3),
        );
        return helm3::extract_helm3(rel, content, &helm_unit, Some(&chart_root));
    }
    match graph::unit::lang_of_path(rel) {
        Some(graph::unit::LANG_PYTHON) => graph::python::extract_python(rel, content, &unit),
        Some(graph::unit::LANG_TYPESCRIPT) => {
            graph::typescript::extract_typescript(rel, content, &unit)
        }
        Some(graph::unit::LANG_SWIFT) => graph::swift::extract_swift(rel, content, &unit),
        Some(graph::unit::LANG_LUA) => {
            graph::lua::extract_lua(rel, content, &with_lua_files(root, unit))
        }
        Some(graph::unit::LANG_RHAI) => graph::rhai::extract_rhai(rel, content, &unit),
        Some(graph::unit::LANG_CUE) => {
            if let Some(index) = cue_index {
                graph::cue::extract_cue_with_index(
                    rel,
                    content,
                    &cue_unit_for(root, rel),
                    Some(index),
                )
            } else {
                graph::cue::extract_cue_at_root(root, rel, content)
            }
        }
        Some(graph::unit::LANG_HELM3) => {
            if let Some((chart_root, _chart_name)) = chart_for(root, rel) {
                let helm_unit = helm_unit_for(&chart_root, unit);
                helm3::extract_helm3(rel, content, &helm_unit, Some(&chart_root))
            } else {
                helm3::extract_helm3(rel, content, &unit, None)
            }
        }
        Some(graph::unit::LANG_JSON) => {
            graph::json_extractor::extract_json(rel, content, &with_tracked_files(root, unit))
        }
        Some(graph::unit::LANG_YAML) => {
            graph::yaml::extract_yaml(rel, content, &with_tracked_files(root, unit))
        }
        // Ownership enrichment rides the tree this call already parses (§7.1,
        // D13) and is gated inside the extractor on `enabled` plus
        // `matches(rel)`. `rel` comes from `tracked_files`, so include/exclude
        // filter that walk rather than performing one of their own (D16) — an
        // independent walk would index files the freshness check can never
        // match, which is drift nothing heals.
        _ => extract_rust_at_module_with_ownership(
            rel,
            content,
            DEFAULT_WATCHLIST,
            &unit,
            inclusion.map(|included| included.module.as_str()),
            ownership,
        ),
    }
}

/// The functions a compiler-aware provider would be asked about.
///
/// Read back off the freshly extracted base set rather than recomputed, so the
/// ids are byte-identical to the ones `ownership_site_in_function` embeds and
/// `defines_fn` declares — reconstructing them here would be the second
/// identity scheme §5.1 forbids. Only site-bearing functions are included:
/// a status edge for a function the query surface never groups is invisible
/// evidence, and the volume would otherwise scale with the whole repository.
fn ownership_subjects(base: &[Edge]) -> Vec<OwnershipFunction> {
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    let mut out = Vec::new();
    for edge in base
        .iter()
        .filter(|edge| !edge.d && edge.p == ownership::OWNERSHIP_SITE_IN_FUNCTION)
    {
        let Some(function) = edge.a.get(1) else {
            continue;
        };
        if seen.insert((function.clone(), edge.src.clone())) {
            out.push(OwnershipFunction::new(function, &edge.src));
        }
    }
    out
}

/// Run the compiler-aware provider, if this run may have one at all.
///
/// Returns the status edges to append and the run's diagnostics. A provider
/// failure is turned into explicit `failed` observations rather than an error:
/// §8.1 requires the AST extractor to stay usable with no working provider,
/// and a rebuild that aborted on a missing optional tool would make opting in
/// strictly worse than staying out.
pub(super) fn compiler_evidence(
    root: &Path,
    ownership: &OwnershipConfig,
    base: &[Edge],
) -> (Vec<Edge>, Vec<String>) {
    // `for_rebuild` yields `None` for every trigger but this one, so the hook,
    // hydration, and incremental paths have no provider value to call at all
    // (§8.2). Naming the trigger here is the whole enforcement mechanism.
    let Some(provider) =
        RustAnalyzerProvider::for_rebuild(ownership, AnalysisTrigger::ExplicitRebuild)
    else {
        return (Vec::new(), Vec::new());
    };
    let functions = ownership_subjects(base);
    let report = match provider.analyze(root, &functions) {
        Ok(report) => report,
        Err(error) => {
            tracing::warn!("ownership provider failed, recording explicit status: {error}");
            failure_report(&functions, &error)
        }
    };
    (report.to_edges(), report.diagnostics())
}

/// D9: mark this file's compiler evidence stale after an incremental edit.
///
/// The AST edges for the file are re-extracted normally by the same save; only
/// the compiler generation is invalidated, because no provider may run on this
/// path (§8.2) and carrying the previous rebuild's conclusions forward would
/// present them as describing bytes they never saw.
///
/// The edges are appended *after* compaction and carry the file as `src`, so
/// they replace whatever compiler status the last rebuild left for it.
pub(super) fn incremental_compiler_staleness(
    ownership: &OwnershipConfig,
    file_path: &str,
) -> Vec<Edge> {
    if !file_path.ends_with(".rs") || !ownership.enabled || !ownership.matches(file_path) {
        return Vec::new();
    }
    incremental_stale_report(file_path).to_edges()
}

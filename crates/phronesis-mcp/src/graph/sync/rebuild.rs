//! The per-save and full-rebuild pipelines: extract, compact, derive,
//! persist, then stamp the index and reconcile bindings.

use super::extract::{
    ExtractOneParams, IncludedRustModule, compiler_evidence, extract_one,
    incremental_compiler_staleness, rust_path_inclusions,
};
use super::index::{decision_input_files, is_tracked, load_index, save_index, tracked_files};
use super::rules::{
    migrate_graph_rule_predicates, reconcile_bindings_best_effort, rule_predicate_edges,
};
use super::{GRAPH_FORMAT, Index, SaveOutcome, hash_content, index_path};
use crate::graph;
use crate::graph::derive::{canonicalize_function_edges, derive_all};
use crate::graph::model::Edge;
use crate::graph::ownership;
use crate::graph::store;
use crate::graph::unit::UnitMap;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Reject a `tested_by` edge whose target is not a canonical `defines_fn`
/// identity.
fn check_tested_by_targets(base: &[Edge]) -> std::io::Result<()> {
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
    Ok(())
}

/// Read the graph back and confirm the counts written are the counts stored.
fn verify_persisted(path: &Path, n_base: usize, n_derived: usize) -> std::io::Result<()> {
    let persisted = store::load(path)?;
    let persisted_base = persisted.iter().filter(|edge| !edge.d).count();
    let persisted_derived = persisted.len().saturating_sub(persisted_base);
    if (persisted_base, persisted_derived) != (n_base, n_derived) {
        return Err(std::io::Error::other(format!(
            "graph persistence verification failed: wrote {n_base} base/{n_derived} derived, read {persisted_base} base/{persisted_derived} derived"
        )));
    }
    Ok(())
}

/// Recompute derived edges over `base` and persist both sets.
fn persist(root: &Path, mut base: Vec<Edge>) -> std::io::Result<(usize, usize)> {
    canonicalize_function_edges(&mut base);
    check_tested_by_targets(&base)?;
    let derived = derive_all(&base);
    let (n_base, n_derived) = (base.len(), derived.len());
    let all = {
        let mut all = base;
        all.extend(derived);
        all
    };
    let path = store::graph_path(root);
    store::write_atomic(&path, &all)?;
    verify_persisted(&path, n_base, n_derived)?;
    Ok((n_base, n_derived))
}

/// Whether editing `file_path` invalidates the data-contract edges.
///
/// Those edges are attributed to `.phronesis/graph.toml`, not to the artifact,
/// so provenance-keyed compaction leaves them untouched by an ordinary save;
/// only a rebuild recomputes them. The trigger is therefore that the config
/// **names this file** as a generated artifact.
///
/// It deliberately is not "graph.toml exists and this is a `.rs`/`.json`/
/// `.yaml` file", which is what it used to be. Under that rule the mere
/// presence of the config turned every Rust save into a whole-repo rebuild —
/// and since opting into ownership enrichment means creating that file,
/// enabling the feature would silently blow the hook latency budget for every
/// edit in the repository (decision D17).
///
/// The cost of the narrowing: a `.rs` edit no longer refreshes *inferred*
/// bindings, which are heuristics recomputed at the next rebuild.
fn declared_artifact(root: &Path, file_path: &str) -> bool {
    graph::data_contracts::declares_artifact(root, file_path)
}

/// Whether this save cannot be applied incrementally and must rebuild.
fn forces_rebuild(root: &Path, file_path: &str, content: &str) -> std::io::Result<bool> {
    let path_module_owner = file_path.ends_with(".rs")
        && (content.contains("#[path")
            || store::load(&store::graph_path(root))?.iter().any(|edge| {
                !edge.d
                    && edge.p == "includes_file"
                    && edge.a.first().is_some_and(|owner| owner == file_path)
            }));
    let data_contract_input = declared_artifact(root, file_path);
    Ok(file_path.ends_with(".cue")
        || path_module_owner
        || file_path == ".phronesis/graph.toml"
        || file_path == ".phronesis/rules.json"
        || file_path.starts_with(".phronesis/wiki/decisions/")
        || data_contract_input)
}

/// Load the index, rebuilding first if the graph on disk predates the
/// current identity scheme.
///
/// A graph written under an older identity scheme cannot be patched one
/// file at a time: compaction replaces only the edited file's edges, so
/// every other file would keep its old names and the two halves would
/// never join. Rebuild once, then record this edit on top of the result.
fn load_current_index(root: &Path, ipath: &Path) -> std::io::Result<Index> {
    let index = load_index(ipath)?;
    if !index.entries.is_empty() && index.format != GRAPH_FORMAT {
        rebuild(root)?;
        return load_index(ipath);
    }
    Ok(index)
}

/// Parse the edited file alone, with the per-save context it needs.
fn extract_for_save(
    root: &Path,
    file_path: &str,
    content: &str,
) -> (
    graph::extract::Extracted,
    ownership::config::OwnershipConfig,
) {
    let units = UnitMap::discover(root);
    let files = tracked_files(root);
    let rust_inclusions = rust_path_inclusions(root, &files, &units);
    // One read of `.phronesis/graph.toml` for the save, not one per file.
    let ownership = ownership::config::load_or_disabled(root);
    let extracted = extract_one(ExtractOneParams {
        root,
        rel: file_path,
        content,
        units: &units,
        cue_index: None,
        rust_inclusions: &rust_inclusions,
        ownership: &ownership,
    });
    (extracted, ownership)
}

/// The outcome reported when a save leaves the graph untouched.
fn untouched_outcome(existing: &[Edge], skipped: usize) -> SaveOutcome {
    let base = existing.iter().filter(|e| !e.d).count();
    SaveOutcome {
        base,
        derived: existing.len() - base,
        skipped,
        migrated_rules: 0,
        diagnostics: Vec::new(),
    }
}

/// Apply one save: parse the edited file, compact by provenance, re-derive
/// over the whole graph, and write atomically.
///
/// Only the edited file is parsed; derivation runs over the full edge set
/// already on disk. That is what makes whole-repo facts affordable per save.
pub fn on_save(root: &Path, file_path: &str, content: &str) -> std::io::Result<SaveOutcome> {
    if forces_rebuild(root, file_path, content)? {
        return rebuild(root);
    }
    if !is_tracked(file_path) {
        return Ok(SaveOutcome {
            base: 0,
            derived: 0,
            skipped: 0,
            migrated_rules: 0,
            diagnostics: Vec::new(),
        });
    }
    let ipath = index_path(root);
    let mut index = load_current_index(root, &ipath)?;

    let (extracted, ownership) = extract_for_save(root, file_path, content);
    let existing = store::load(&store::graph_path(root))?;
    if extracted.parse_failed {
        // Leave the graph and the index exactly as they were. Compacting the
        // empty edge set would erase the file's evidence, and recording the
        // unparseable content's hash would report the result fresh — the
        // harness would then keep enforcing on facts it had just deleted.
        // Leaving the hash stale makes freshness report the file as drifted,
        // which demotes structural rules to warnings: the honest state.
        return Ok(untouched_outcome(&existing, extracted.skipped));
    }
    let base = {
        let mut base = store::compact(existing, file_path, extracted.edges);
        // After compaction, so these supersede the previous rebuild's compiler
        // status for this file instead of being deleted alongside it (D9, D12).
        base.extend(incremental_compiler_staleness(&ownership, file_path));
        base
    };
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
        diagnostics: Vec::new(),
    })
}

/// Hook entry point: regenerate the graph after a document save.
///
/// Best-effort and infallible by design. The sensor runs in `PostToolUse`,
/// after the edit has already happened; a graph write that fails must not
/// turn into a hook error that interrupts the user's work. A complete rebuild
/// is the default so edits outside the immediately reported file are folded in
/// at the next hooked save. Failures leave hashes stale, which the freshness
/// check will catch and report.
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
    let Some(rel) = graph::hydrate::repo_relative(root, file_path) else {
        // Outside the project: `repo_relative` is the containment check.
        return;
    };
    // Never follow a path out of the project — the sensor reads whatever it
    // is handed, and a traversal would pull unrelated files into the graph.
    if rel.contains("..") || Path::new(&rel).is_absolute() {
        return;
    }
    if let Err(e) = rebuild(root) {
        tracing::debug!("graph sensor could not rebuild after saving {rel}: {e}");
    }
}

/// Project-wide inputs a rebuild resolves once, before walking files.
struct RebuildScan {
    units: UnitMap,
    cue_index: graph::cue::PackageIndex,
    files: Vec<String>,
    rust_inclusions: BTreeMap<String, IncludedRustModule>,
    ownership: ownership::config::OwnershipConfig,
}

impl RebuildScan {
    fn discover(root: &Path) -> Self {
        let units = UnitMap::discover(root);
        let cue_index = graph::cue::build_package_index(root);
        let files = tracked_files(root);
        let rust_inclusions = rust_path_inclusions(root, &files, &units);
        // Loaded once for the whole rebuild. `.phronesis/graph.toml` is a single
        // file read; doing it per tracked file would repeat it thousands of times
        // to reach the same answer.
        let ownership = ownership::config::load_or_disabled(root);
        Self {
            units,
            cue_index,
            files,
            rust_inclusions,
            ownership,
        }
    }
}

/// A fresh index whose generation follows the one on disk.
fn next_index(root: &Path) -> Index {
    let previous_generation = load_index(&index_path(root))
        .map(|index| index.generation)
        .unwrap_or(0);
    Index {
        generation: previous_generation.saturating_add(1),
        ..Index::default()
    }
}

/// Extract every tracked file, recording each one's hash in `index`.
/// Returns the base edges and the number of items the extractor declined
/// to name.
fn extract_tracked(root: &Path, scan: &RebuildScan, index: &mut Index) -> (Vec<Edge>, usize) {
    let mut base = Vec::new();
    let mut skipped = 0;
    for rel in &scan.files {
        let Ok(content) = std::fs::read_to_string(root.join(rel)) else {
            continue;
        };
        let extracted = extract_one(ExtractOneParams {
            root,
            rel,
            content: &content,
            units: &scan.units,
            cue_index: Some(&scan.cue_index),
            rust_inclusions: &scan.rust_inclusions,
            ownership: &scan.ownership,
        });
        skipped += extracted.skipped;
        if extracted.parse_failed {
            // A complete rebuild has observed this exact content and
            // intentionally excluded it. Record the hash so status does not
            // misreport a permanently skipped input as post-build drift.
            // A later edit changes the hash and correctly becomes stale.
            index.entries.insert(rel.clone(), hash_content(&content));
            continue;
        }
        base.extend(extracted.edges);
        index.entries.insert(rel.clone(), hash_content(&content));
    }
    base.extend(scan.rust_inclusions.iter().map(|(file, included)| {
        Edge::base("includes_file", &[&included.owner, file], &included.owner)
    }));
    (base, skipped)
}

/// Hash the non-source inputs (`graph.toml`, rules, decisions) into `index`.
fn record_auxiliary_inputs(root: &Path, index: &mut Index) {
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
}

/// Full rescan of every tracked file (Rust, Python, TypeScript), pruning
/// `node_modules`. The recovery path after the graph has drifted, and the
/// only way edges for deleted files are cleared.
pub fn rebuild(root: &Path) -> std::io::Result<SaveOutcome> {
    // Rules are graph consumers. Migrate their vocabulary before hashing
    // inputs so the rebuild cannot make its own index immediately stale.
    let migrated_rules = migrate_graph_rule_predicates(root)?;
    let mut index = next_index(root);
    let scan = RebuildScan::discover(root);
    let (mut base, skipped) = extract_tracked(root, &scan, &mut index);

    graph::data_contracts::augment(root, &mut base);
    // Compiler enrichment is rebuild-only (§8.2) and runs after AST extraction
    // because its subject list is read back off the extracted edges — the ids
    // must be the ones already in the graph, not a second reconstruction.
    let (compiler_edges, diagnostics) = compiler_evidence(root, &scan.ownership, &base);
    base.extend(compiler_edges);
    base.extend(rule_predicate_edges(root)?);
    base.extend(graph::decisions::extract(root));
    record_auxiliary_inputs(root, &mut index);

    let (n_base, n_derived) = persist(root, base)?;
    save_index(&index_path(root), &index)?;
    reconcile_bindings_best_effort(root, index.generation);
    for diagnostic in &diagnostics {
        tracing::info!("ownership provider limitation: {diagnostic}");
    }
    Ok(SaveOutcome {
        base: n_base,
        derived: n_derived,
        skipped,
        migrated_rules,
        diagnostics,
    })
}

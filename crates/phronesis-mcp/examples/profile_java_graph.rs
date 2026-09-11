//! Reproducible Java corpus report, including cold/warm discovery timings.
//! Run: cargo run -p phronesis-mcp --example profile_java_graph -- /path/to/repo

use phronesis_mcp::graph::{derive, java::project::Project};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("expected a repository path"))?;
    if std::env::args().any(|arg| arg == "--parse-errors") {
        return report_parse_errors(&root);
    }
    let cold = Instant::now();
    let project = Project::discover(&root, None)?;
    let cold_ms = cold.elapsed().as_millis();
    let warm = Instant::now();
    let warm_project = Project::discover(&root, None)?;
    let warm_ms = warm.elapsed().as_millis();
    let mut edges = Vec::new();
    let mut skipped_files = BTreeMap::new();
    let mut skipped_details = BTreeMap::new();
    for file in project.files.keys() {
        let extracted = project.extract(file);
        if extracted.skipped > 0 {
            skipped_files.insert(file, extracted.skipped);
            let entry = &project.files[file];
            let mut unresolved_owners = BTreeMap::new();
            let unresolved = entry
                .source
                .imports
                .iter()
                .filter(|import| {
                    let candidates = std::cell::RefCell::new(Vec::new());
                    let resolution =
                        project
                            .index
                            .resolve(import.as_import(), &entry.owner, |owner| {
                                let visible = entry.sees(owner);
                                candidates.borrow_mut().push(serde_json::json!({
                                    "file": owner.file, "unit": owner.unit,
                                    "context": format!("{:?}", owner.context), "visible": visible,
                                }));
                                visible
                            });
                    let unresolved =
                        resolution == phronesis_mcp::graph::java::index::Resolution::Unresolved;
                    if unresolved {
                        unresolved_owners.insert(format!("{import:?}"), candidates.into_inner());
                    }
                    unresolved
                })
                .map(|import| format!("{import:?}"))
                .collect::<Vec<_>>();
            skipped_details.insert(
                file,
                serde_json::json!({
                    "path_mismatch": entry.path_mismatch, "parse_failed": entry.source.parse_failed,
                    "source_skipped": entry.source.skipped, "unresolved_imports": unresolved,
                    "source_unit": entry.owner.unit, "source_context": format!("{:?}", entry.owner.context),
                    "unresolved_visibility_candidates": unresolved_owners,
                }),
            );
        }
        edges.extend(extracted.edges);
    }
    let warm_edges = warm_project
        .files
        .keys()
        .flat_map(|file| warm_project.extract(file).edges)
        .collect::<Vec<_>>();
    anyhow::ensure!(edges == warm_edges, "warm cache changed the graph");
    let units = project
        .files
        .values()
        .map(|file| &file.owner.unit)
        .collect::<BTreeSet<_>>();
    let modules = project
        .files
        .values()
        .map(|file| &file.owner.module)
        .collect::<BTreeSet<_>>();
    let counts = edges
        .iter()
        .fold(BTreeMap::<&str, usize>::new(), |mut counts, edge| {
            *counts.entry(&edge.p).or_default() += 1;
            counts
        });
    let cycles = derive::in_cycle(&edges);
    let mut counters = BTreeMap::new();
    for name in [
        "unresolved_property",
        "external_parent_skipped",
        "group_id_unresolved",
        "unit_id_collision",
        "unsupported_plugin_source_modifier",
        "inactive_profile_skipped",
        "multi_release_root_ignored",
        "dependency_modifier_ignored",
        "select_branch_unioned",
        "unbound_identifier",
        "unsupported_syntax_skipped",
        "unresolved_label",
        "dual_claimed_file",
        "files_unclaimed",
        "test_class_entry_point",
        "test_class_unresolved",
        "module_info_skipped",
        "module_import_ignored",
        "jpms_unit_unmodelled",
        "split_package",
    ] {
        counters.insert(name.to_string(), project.diagnostics.count(name));
    }
    for (name, objects) in &project.diagnostics.0 {
        counters.insert(name.clone(), objects.len());
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "root": root, "cold_ms": cold_ms, "warm_ms": warm_ms,
            "files": project.files.len(), "units": units.len(), "modules": modules.len(),
            "relations": counts, "counters": counters,
        "skipped": skipped_files.values().sum::<usize>(), "skipped_files": skipped_files, "skipped_details": skipped_details,
            "failed_inputs": project.failed_inputs, "diagnostic_objects": project.diagnostics.0,
            "cycle_members": cycles, "import_edges": edges.iter().filter(|edge| edge.p == "imports").collect::<Vec<_>>()
        }))?
    );
    Ok(())
}

/// Inspect grammar failures independently from graph extraction and caching.
fn report_parse_errors(root: &std::path::Path) -> anyhow::Result<()> {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&phronesis_mcp::graph::java::grammar::language())?;
    let mut failures = BTreeMap::new();
    for file in phronesis_mcp::graph::java::project::input_files(root)
        .into_iter()
        .filter(|file| file.ends_with(".java"))
    {
        let Ok(source) = std::fs::read_to_string(root.join(&file)) else {
            continue;
        };
        let Some(tree) = parser.parse(&source, None) else {
            continue;
        };
        if !tree.root_node().has_error() {
            continue;
        }
        let mut pending = vec![tree.root_node()];
        let mut errors = Vec::new();
        while let Some(node) = pending.pop() {
            if node.is_error() || node.is_missing() {
                errors.push(serde_json::json!({
                    "line": node.start_position().row + 1,
                    "kind": node.kind(), "missing": node.is_missing(),
                    "parent": node.parent().map(|parent| parent.kind()),
                    "text": node.utf8_text(source.as_bytes())?.chars().take(160).collect::<String>(),
                }));
            }
            let mut cursor = node.walk();
            pending.extend(node.children(&mut cursor));
        }
        errors.reverse();
        failures.insert(file, errors);
    }
    println!("{}", serde_json::to_string_pretty(&failures)?);
    Ok(())
}

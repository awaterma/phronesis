//! Loading the durable graph into working memory at `PreToolUse`.
//!
//! Hydration is gated on demand: only relations that some loaded rule
//! actually mentions are asserted. A project with no structural rules pays
//! nothing, which matters because every existing project is in that state and
//! hook latency is a shared budget.

use super::model::Edge;
use super::store;
use super::sync::{Freshness, check_freshness, index_path, load_index};
use phr::{Fact, Rule, RuleId};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Relations this feature can supply. A rule naming none of them needs no
/// graph load at all.
pub const GRAPH_RELATIONS: &[&str] = &[
    "file_type",
    "build_member",
    "defines_fn",
    "defines_method",
    "defines_test",
    "declares_module",
    "calls_api",
    "imports",
    "includes_file",
    "reexports",
    "tested_by",
    "test_reaches",
    "no_direct_test",
    "exposes",
    "calls",
    "loads_rhai_script",
    "resolves_to",
    "runtime_reachable",
    "rhai_binding_diagnostic",
    "rhai_callable_backing",
    "rhai_emits_predicate",
    "rule_uses_predicate",
    "rhai_implements_predicate",
    "in_cycle",
    "generates",
    "consumes_data",
    "deserializes",
    "data_flows_to",
    "data_key",
    "serde_field",
    "emits_key",
    "maps_data_key",
    "unconsumed_data_key",
    "generated_without_consumer",
    "consumed_without_producer",
    "generated_artifact_diagnostic",
    // ADR-to-rule traceability. These are graph predicates, not merely
    // query metadata: projects may govern on missing or stale decision links.
    "graph_decision",
    "decision_enforces",
    "rule_governed_by",
    "decision_missing_rule",
    "proposed_decision_enforces",
    "superseded_decision_enforces",
    "rule_without_decision",
    // Graph-format-5 relations: multilingual dialects, entity-level
    // definitions, and file/module membership.
    "graph_definition",
    "defines",
    "element_in_file",
    "element_in_module",
    "graph_module",
    "graph_function",
    "graph_test",
    "graph_file",
    // Not stored on disk — asserted per invocation, see `EDITED_FILE`.
    EDITED_FILE,
    // Per-file diagnostic predicates emitted by language extractors.
    // These allow structural rules to warn/block on concrete defects
    // detected during extraction without needing cross-file analysis.
    //
    // Lua diagnostics
    "lua_dynamic_code_load",
    // CUE diagnostics
    "cue_import_diagnostic",
    // JSON diagnostics
    "json_schema_unknown_dialect",
    // YAML diagnostics
    "yaml_duplicate_key",
    "yaml_undefined_alias",
    "yaml_merge_key",
    // Helm3 diagnostics
    "helm3_dynamic_tpl",
    "helm3_cluster_lookup",
];

/// The file the current tool call is touching, expressed in the graph's
/// repo-relative form.
///
/// Graph relations describe the whole repository, so a rule written over them
/// alone fires on every edit regardless of what is being edited — the same
/// warnings, every time, until the user disables the pack. Joining against
/// this relation scopes a rule to the work in front of the user.
///
/// The existing `file_path` fact cannot serve: hosts send absolute paths,
/// while the graph is keyed repo-relative, so the two never join.
pub const EDITED_FILE: &str = "edited_file";

/// Which graph relations the loaded rules actually reference.
pub fn needed_relations(rules: &[Rule]) -> BTreeSet<String> {
    let mut needed = BTreeSet::new();
    for rule in rules {
        for cond in &rule.conditions {
            if GRAPH_RELATIONS.contains(&cond.predicate.as_str()) {
                needed.insert(cond.predicate.clone());
            }
        }
    }
    needed
}

/// Ids of rules that depend on the graph, and would therefore be reasoning
/// from stale data if the graph has drifted.
pub fn graph_rule_ids(rules: &[Rule]) -> BTreeSet<RuleId> {
    rules
        .iter()
        .filter(|r| {
            r.conditions
                .iter()
                .any(|c| GRAPH_RELATIONS.contains(&c.predicate.as_str()))
        })
        .map(|r| RuleId::from(r.id.as_str()))
        .collect()
}

/// Express `path` the way the graph keys files: relative to the project root,
/// forward-slashed. Hosts send absolute paths; the graph stores relative ones,
/// and a path outside the project has no graph identity at all.
pub(crate) fn repo_relative(root: &Path, path: &str) -> Option<String> {
    let candidate = Path::new(path);
    let rel = if candidate.is_absolute() {
        strip_root(candidate, root)?
    } else {
        candidate.to_path_buf()
    };
    Some(rel.to_str()?.replace('\\', "/"))
}

/// Strip `root` from an absolute path, tolerating a symlinked root.
///
/// A lexical strip is not enough: `project_root` comes from `current_dir`,
/// which the OS returns already resolved (`/private/var/…` on macOS), while
/// hosts send the unresolved form (`/var/…`). The two name the same file and
/// compare unequal, which silently yields "outside the project" for a file
/// that is squarely inside it.
fn strip_root(candidate: &Path, root: &Path) -> Option<PathBuf> {
    if let Ok(rel) = candidate.strip_prefix(root) {
        return Some(rel.to_path_buf());
    }
    let root = root.canonicalize().ok()?;
    let real = resolve_existing_ancestor(candidate)?;
    real.strip_prefix(&root).ok().map(Path::to_path_buf)
}

/// Canonical form of `path`, resolving through its parent so that a file
/// which does not exist yet — a `Write` creating one — still resolves.
fn resolve_existing_ancestor(path: &Path) -> Option<PathBuf> {
    if let Ok(real) = path.canonicalize() {
        return Some(real);
    }
    let parent = path.parent()?.canonicalize().ok()?;
    Some(parent.join(path.file_name()?))
}

/// Edges to assert, plus whether the graph still matches the working tree.
pub struct Hydration {
    pub facts: Vec<Fact>,
    pub fresh: bool,
    /// Files that drifted, when not fresh.
    pub drifted: Vec<String>,
    /// The graph was built under an older identity scheme rather than having
    /// drifted. Same enforcement consequence, different remedy to report.
    pub outdated: bool,
    /// Rules reasoning over the graph. When `fresh` is false, the harness
    /// downgrades these rules' violations to warnings.
    pub graph_rules: BTreeSet<RuleId>,
    /// Freshness was actually computed against the tree this invocation and
    /// came back `Fresh`. Distinct from `fresh`, which is also true when no
    /// rule needed the graph and nothing was checked at all. Lets later
    /// stages (binding demotion) reuse the tree hash instead of re-paying it.
    pub verified_fresh: bool,
}

/// Select the facts to assert for `rules`.
///
/// Only relations named by some rule are loaded, and only facts *about the
/// code* are asserted. Whether the graph is currently trustworthy is a
/// property of the enforcement machinery, not of the codebase, so it is
/// returned to the caller rather than injected into working memory — rules
/// describe the world, not the health of the tool observing it.
pub fn hydrate(root: &Path, rules: &[Rule], edited_file: Option<&str>) -> Hydration {
    let needed = needed_relations(rules);
    if needed.is_empty() {
        return Hydration {
            facts: Vec::new(),
            fresh: true,
            drifted: Vec::new(),
            outdated: false,
            graph_rules: BTreeSet::new(),
            verified_fresh: false,
        };
    }

    let edges: Vec<Edge> = store::load(&store::graph_path(root)).unwrap_or_default();
    let index = load_index(&index_path(root)).unwrap_or_default();
    let (fresh, drifted, outdated) = match check_freshness(root, &index) {
        Freshness::Fresh => (true, Vec::new(), false),
        Freshness::Stale(files) => (false, files, false),
        // No file drifted; the whole graph speaks an older identity scheme.
        Freshness::Outdated { .. } => (false, Vec::new(), true),
    };

    let mut facts: Vec<Fact> = edges
        .iter()
        .filter(|e| needed.contains(&e.p))
        .map(Edge::to_fact)
        .collect();

    if let Some(rel) = edited_file.and_then(|p| repo_relative(root, p)) {
        facts.push(Fact {
            id: format!("{EDITED_FILE}:{rel}"),
            predicate: EDITED_FILE.to_string(),
            args: vec![rel],
            timestamp: 0,
            source: Some("graph:context".to_string()),
        });
    }

    Hydration {
        facts,
        fresh,
        drifted,
        outdated,
        graph_rules: graph_rule_ids(rules),
        verified_fresh: fresh,
    }
}

#[cfg(test)]
mod tests {
    use super::super::sync;
    use super::*;
    use phr::{Condition, Rule};
    use tempfile::TempDir;

    fn rule_using(predicate: &str) -> Rule {
        Rule {
            id: format!("uses-{predicate}"),
            priority: 0,
            conditions: vec![Condition {
                predicate: predicate.into(),
                args: vec!["?x".into()],
                script: None,
            }],
            actions: vec![],
        }
    }

    #[test]
    fn decision_relations_are_available_to_rete_rules() {
        for predicate in [
            "graph_decision",
            "decision_enforces",
            "rule_governed_by",
            "decision_missing_rule",
            "proposed_decision_enforces",
            "superseded_decision_enforces",
            "rule_without_decision",
        ] {
            assert!(
                GRAPH_RELATIONS.contains(&predicate),
                "{predicate} must be hydratable when a rule consumes it"
            );
            assert!(needed_relations(&[rule_using(predicate)]).contains(predicate));
        }
    }

    fn project_with_graph() -> TempDir {
        let d = TempDir::new().expect("tempdir");
        std::fs::create_dir_all(d.path().join("src")).expect("mkdir");
        std::fs::write(d.path().join("src/a.rs"), "fn f() {}").expect("write");
        sync::rebuild(d.path()).expect("rebuild");
        d
    }

    #[test]
    fn rules_that_mention_no_graph_relation_need_nothing() {
        assert!(needed_relations(&[rule_using("file_path")]).is_empty());
    }

    #[test]
    fn a_rule_mentioning_a_relation_requests_it() {
        let needed = needed_relations(&[rule_using("no_direct_test")]);
        assert!(needed.contains("no_direct_test"));
    }

    #[test]
    fn a_project_without_structural_rules_loads_no_facts() {
        let d = project_with_graph();
        let h = hydrate(d.path(), &[rule_using("file_path")], None);
        assert!(h.facts.is_empty(), "unused graph must cost nothing");
    }

    #[test]
    fn only_the_requested_relations_are_asserted() {
        let d = project_with_graph();
        let h = hydrate(d.path(), &[rule_using("defines_fn")], None);
        assert!(h.facts.iter().any(|f| f.predicate == "defines_fn"));
        assert!(!h.facts.iter().any(|f| f.predicate == "file_type"));
    }

    #[test]
    fn hydration_reports_a_fresh_graph_as_fresh() {
        let d = project_with_graph();
        let h = hydrate(d.path(), &[rule_using("defines_fn")], None);
        assert!(h.fresh);
        assert!(h.drifted.is_empty());
    }

    #[test]
    fn an_edit_outside_the_hook_path_makes_hydration_report_stale() {
        let d = project_with_graph();
        std::fs::write(d.path().join("src/a.rs"), "fn f() {}\nfn sneaky() {}").expect("write");
        let h = hydrate(d.path(), &[rule_using("defines_fn")], None);
        assert!(!h.fresh);
        assert_eq!(h.drifted, vec!["src/a.rs".to_string()]);
    }

    #[test]
    fn only_facts_about_code_reach_working_memory() {
        // Graph health is a property of the enforcement machinery, not of the
        // codebase. It is returned to the caller, never asserted.
        let d = project_with_graph();
        std::fs::write(d.path().join("src/a.rs"), "fn f() {}\nfn sneaky() {}").expect("write");
        let h = hydrate(d.path(), &[rule_using("defines_fn")], None);
        assert!(!h.fresh, "precondition: this graph is stale");
        assert!(
            h.facts
                .iter()
                .all(|f| GRAPH_RELATIONS.contains(&f.predicate.as_str())),
            "only closed-set code relations may be asserted"
        );
    }

    #[test]
    fn a_missing_graph_asserts_nothing() {
        let d = TempDir::new().expect("tempdir");
        let h = hydrate(d.path(), &[rule_using("defines_fn")], None);
        assert!(h.facts.is_empty());
    }

    fn edited_args(h: &Hydration) -> Vec<String> {
        h.facts
            .iter()
            .filter(|f| f.predicate == EDITED_FILE)
            .filter_map(|f| f.args.first().cloned())
            .collect()
    }

    #[test]
    fn the_edited_file_is_asserted_so_rules_can_scope_to_it() {
        let d = project_with_graph();
        let h = hydrate(d.path(), &[rule_using("defines_fn")], Some("src/a.rs"));
        assert_eq!(edited_args(&h), vec!["src/a.rs".to_string()]);
    }

    #[test]
    fn an_absolute_edited_path_is_normalized_to_the_graphs_form() {
        // Hosts send absolute paths; the graph is keyed repo-relative. Without
        // this, the join silently never matches.
        let d = project_with_graph();
        let abs = d.path().join("src/a.rs");
        let h = hydrate(
            d.path(),
            &[rule_using("defines_fn")],
            Some(&abs.to_string_lossy()),
        );
        assert_eq!(edited_args(&h), vec!["src/a.rs".to_string()]);
    }

    #[test]
    fn a_path_outside_the_project_is_not_asserted() {
        let d = project_with_graph();
        let h = hydrate(d.path(), &[rule_using("defines_fn")], Some("/etc/hosts"));
        assert!(edited_args(&h).is_empty());
    }

    #[test]
    fn a_call_with_no_file_asserts_no_edited_file() {
        let d = project_with_graph();
        let h = hydrate(d.path(), &[rule_using("defines_fn")], None);
        assert!(edited_args(&h).is_empty());
    }

    #[test]
    fn rules_reading_the_graph_are_identified_for_downgrade() {
        let ids = graph_rule_ids(&[rule_using("no_direct_test"), rule_using("file_path")]);
        assert_eq!(ids.len(), 1);
        assert!(ids.contains(&"uses-no_direct_test".into()));
    }

    #[test]
    fn a_rule_that_ignores_the_graph_is_never_downgraded() {
        let ids = graph_rule_ids(&[rule_using("file_path")]);
        assert!(ids.is_empty());
    }
}

#[cfg(test)]
mod path_tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn a_relative_path_is_returned_unchanged() {
        assert_eq!(
            repo_relative(Path::new("/proj"), "src/a.rs").as_deref(),
            Some("src/a.rs")
        );
    }

    #[test]
    fn an_absolute_path_inside_the_project_is_relativized() {
        assert_eq!(
            repo_relative(Path::new("/proj"), "/proj/src/a.rs").as_deref(),
            Some("src/a.rs")
        );
    }

    #[test]
    fn a_path_outside_the_project_has_no_relative_form() {
        assert_eq!(repo_relative(Path::new("/proj"), "/elsewhere/a.rs"), None);
    }

    #[test]
    fn a_symlinked_root_still_relativizes() {
        // `project_root` comes from `current_dir`, which the OS returns
        // already resolved, while hosts send the unresolved path. On macOS
        // that is `/private/var/…` against `/var/…` for the same file. A
        // lexical strip alone reports "outside the project" and the fact is
        // silently dropped.
        let real = TempDir::new().expect("tempdir");
        let link_parent = TempDir::new().expect("tempdir");
        let link = link_parent.path().join("link");
        std::os::unix::fs::symlink(real.path(), &link).expect("symlink");
        std::fs::create_dir_all(real.path().join("src")).expect("mkdir");
        std::fs::write(real.path().join("src/a.rs"), "fn f() {}").expect("write");

        let via_link = link.join("src/a.rs");
        assert_eq!(
            repo_relative(real.path(), via_link.to_str().expect("utf8")).as_deref(),
            Some("src/a.rs")
        );
    }

    #[test]
    fn a_file_that_does_not_exist_yet_still_relativizes() {
        // A `Write` creating a new file: the parent exists, the file does not.
        let real = TempDir::new().expect("tempdir");
        let link_parent = TempDir::new().expect("tempdir");
        let link = link_parent.path().join("link");
        std::os::unix::fs::symlink(real.path(), &link).expect("symlink");
        std::fs::create_dir_all(real.path().join("src")).expect("mkdir");

        let via_link = link.join("src/brand_new.rs");
        assert_eq!(
            repo_relative(real.path(), via_link.to_str().expect("utf8")).as_deref(),
            Some("src/brand_new.rs")
        );
    }
}

//! Loading the durable graph into working memory at `PreToolUse`.
//!
//! Hydration is gated on demand: only relations that some loaded rule
//! actually mentions are asserted. A project with no structural rules pays
//! nothing, which matters because every existing project is in that state and
//! hook latency is a shared budget.

use super::model::Edge;
use super::store;
use super::sync::{Freshness, check_freshness, index_path, load_index};
use phr::{Fact, Rule};
use std::collections::BTreeSet;
use std::path::Path;

/// Relations this feature can supply. A rule naming none of them needs no
/// graph load at all.
pub const GRAPH_RELATIONS: &[&str] = &[
    "file_type",
    "defines_fn",
    "calls_api",
    "imports",
    "tested_by",
    "untested",
    "in_cycle",
];

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

/// Edges to assert, plus whether the graph still matches the working tree.
pub struct Hydration {
    pub facts: Vec<Fact>,
    pub fresh: bool,
    /// Files that drifted, when not fresh.
    pub drifted: Vec<String>,
}

/// Select the facts to assert for `rules`.
///
/// Emits a `graph_fresh` fact alongside the structural ones. Freshness is
/// surfaced as a *fact* rather than enforced in the harness so that a rule
/// decides for itself whether it may block on a possibly-stale graph — the
/// same participatory model the rest of phronesis uses.
pub fn hydrate(root: &Path, rules: &[Rule]) -> Hydration {
    let needed = needed_relations(rules);
    if needed.is_empty() {
        return Hydration {
            facts: Vec::new(),
            fresh: true,
            drifted: Vec::new(),
        };
    }

    let edges: Vec<Edge> = store::load(&store::graph_path(root)).unwrap_or_default();
    let index = load_index(&index_path(root)).unwrap_or_default();
    let (fresh, drifted) = match check_freshness(root, &index) {
        Freshness::Fresh => (true, Vec::new()),
        Freshness::Stale(files) => (false, files),
    };

    let mut facts: Vec<Fact> = edges
        .iter()
        .filter(|e| needed.contains(&e.p))
        .map(Edge::to_fact)
        .collect();
    facts.push(Fact {
        id: "graph_fresh".to_string(),
        predicate: "graph_fresh".to_string(),
        args: vec![fresh.to_string()],
        timestamp: 0,
    });

    Hydration {
        facts,
        fresh,
        drifted,
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
            id: "r".into(),
            priority: 0,
            conditions: vec![Condition {
                predicate: predicate.into(),
                args: vec!["?x".into()],
                script: None,
            }],
            actions: vec![],
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
        let needed = needed_relations(&[rule_using("untested")]);
        assert!(needed.contains("untested"));
    }

    #[test]
    fn a_project_without_structural_rules_loads_no_facts() {
        let d = project_with_graph();
        let h = hydrate(d.path(), &[rule_using("file_path")]);
        assert!(h.facts.is_empty(), "unused graph must cost nothing");
    }

    #[test]
    fn only_the_requested_relations_are_asserted() {
        let d = project_with_graph();
        let h = hydrate(d.path(), &[rule_using("defines_fn")]);
        assert!(h.facts.iter().any(|f| f.predicate == "defines_fn"));
        assert!(!h.facts.iter().any(|f| f.predicate == "file_type"));
    }

    #[test]
    fn hydration_reports_a_fresh_graph_as_fresh() {
        let d = project_with_graph();
        let h = hydrate(d.path(), &[rule_using("defines_fn")]);
        assert!(h.fresh);
        assert!(
            h.facts
                .iter()
                .any(|f| f.predicate == "graph_fresh" && f.args == vec!["true".to_string()])
        );
    }

    #[test]
    fn an_edit_outside_the_hook_path_makes_hydration_report_stale() {
        let d = project_with_graph();
        std::fs::write(d.path().join("src/a.rs"), "fn f() {}\nfn sneaky() {}").expect("write");
        let h = hydrate(d.path(), &[rule_using("defines_fn")]);
        assert!(!h.fresh);
        assert_eq!(h.drifted, vec!["src/a.rs".to_string()]);
        assert!(
            h.facts
                .iter()
                .any(|f| f.predicate == "graph_fresh" && f.args == vec!["false".to_string()])
        );
    }

    #[test]
    fn a_missing_graph_yields_only_the_freshness_fact() {
        let d = TempDir::new().expect("tempdir");
        let h = hydrate(d.path(), &[rule_using("defines_fn")]);
        assert_eq!(h.facts.len(), 1);
        assert_eq!(h.facts[0].predicate, "graph_fresh");
    }
}

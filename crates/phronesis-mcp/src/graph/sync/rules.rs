//! Rule-vocabulary migration, rule-provenance edges, and binding
//! reconciliation — the parts of a sync that touch `.phronesis/rules.json`.

use super::{index_path, load_index};
use crate::graph;
use crate::graph::model::Edge;
use crate::graph::store;
use std::collections::BTreeSet;
use std::path::Path;

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

pub(super) fn migrate_graph_rule_predicates(root: &Path) -> std::io::Result<usize> {
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

pub(super) fn rule_predicate_edges(root: &Path) -> std::io::Result<Vec<Edge>> {
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
    let path = graph::bindings::bindings_path(root);
    let persisted = graph::bindings::load_recovering(&path)?.unwrap_or_default();
    let edges = store::load(&store::graph_path(root))?;
    let next =
        graph::bindings::reconcile(&persisted, &rules, &edges, generation, now_unix_seconds());
    graph::bindings::store_atomic(&path, &next)
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

pub(super) fn reconcile_bindings_best_effort(root: &Path, generation: u64) {
    if let Err(error) = reconcile_bindings(root, generation) {
        tracing::debug!("binding reconciliation skipped: {error}");
    }
}

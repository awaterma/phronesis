//! Whole-tree audit of structural rules.
//!
//! The hook answers "is there a problem in the file being edited?" An audit
//! answers "where are all of them?" For graph rules those are the same
//! question with one binding freed, so the audit is expressed as: **assert
//! every file as the edited file, then fire once.**
//!
//! That framing matters more than it looks. The alternative — a bespoke
//! matcher for graph rules inside `audit.rs` — would be a second
//! implementation of joins and variable binding, free to disagree with the
//! engine the hook uses. A rule that blocks at the hook but reports clean in
//! the audit (or vice versa) is worse than no audit at all. Reusing the real
//! network makes disagreement impossible by construction.

use super::hydrate::{EDITED_FILE, GRAPH_RELATIONS};
use super::model::Edge;
use super::ownership;
use super::store;
use phr::{Fact, Rule};
use std::collections::BTreeSet;
use std::path::Path;

/// Relations whose findings are useful for investigation but have not yet
/// earned enforcement trust across representative repositories.
///
/// Keeping this list explicit prevents a heuristic diagnostic from becoming
/// an audit headline merely because a project wrote a rule over it. Promotion
/// requires reviewed corpus results; graph queries remain available meanwhile.
pub(crate) const QUERY_ONLY_RELATIONS: &[&str] = &[
    "cue_import_diagnostic",
    "generated_artifact_diagnostic",
    "generated_without_consumer",
    "consumed_without_producer",
    "rhai_binding_diagnostic",
    "yaml_duplicate_key",
    "yaml_undefined_alias",
    // Ownership evidence is query-only for all of Phase One (spec §11): no
    // packaged rule references it, and no audit finding may be created from
    // it before precision is measured and its false-positive classes are
    // named. Suppression here is audit-only — a project-authored rule can
    // still fire, which Addendum A anticipates.
    ownership::OWNERSHIP_SITE,
    ownership::OWNERSHIP_SITE_IN_FUNCTION,
    ownership::OWNERSHIP_SITE_SPAN,
    ownership::CLONE_SITE,
    ownership::FILTER_SITE,
    ownership::AWAIT_SITE,
    ownership::MUTATION_SITE,
    ownership::SYNC_LOCK_SITE,
    ownership::OWNERSHIP_EVIDENCE,
    ownership::OWNERSHIP_ANALYSIS_STATUS,
    ownership::RESOLVED_TYPE,
    ownership::FILTER_BEFORE_CLONE,
    ownership::CLONE_BEFORE_AWAIT,
    ownership::READ_BEFORE_MUTATION,
    ownership::LOCK_SCOPE_ENDS_BEFORE_AWAIT,
];

/// One structural finding: a rule that matched, and the file it bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphHit {
    pub rule_id: String,
    /// `constraint_violation` or `constraint_warning`.
    pub action_type: String,
    /// The `?file` the rule bound, repo-relative.
    pub file: String,
    /// Compact identification of *what* matched — the bound entities other
    /// than the file, e.g. `crate::init::write_mcp_json · unwrap`.
    ///
    /// Deliberately not the rule's message. Audit lists many hits per rule,
    /// and repeating a paragraph of guidance for each one buries the only
    /// part that differs. The guidance is a property of the rule; the
    /// bindings are the finding.
    pub detail: String,
}

/// True when `rule` reads the structural graph and so cannot be evaluated by
/// the file-scanning audit loop.
pub fn is_graph_rule(rule: &Rule) -> bool {
    rule.conditions
        .iter()
        .any(|c| GRAPH_RELATIONS.contains(&c.predicate.as_str()))
}

/// Whether a structural rule may participate in whole-tree audit output.
pub fn is_audit_eligible_graph_rule(rule: &Rule) -> bool {
    is_graph_rule(rule)
        && !rule
            .conditions
            .iter()
            .any(|condition| QUERY_ONLY_RELATIONS.contains(&condition.predicate.as_str()))
}

/// Render the bindings other than `file` as a compact ` · `-joined label.
///
/// Sorted by variable name so the same finding renders identically on every
/// run — audit output is diffed across snapshots by `phr-mcp trend`, and an
/// unstable label would read as churn.
fn compact_detail(bindings: &std::collections::HashMap<String, String>) -> String {
    let mut parts: Vec<(&str, &str)> = bindings
        .iter()
        .filter(|(k, _)| k.as_str() != "file" && k.as_str() != "?file")
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    parts.sort_unstable();
    parts
        .into_iter()
        .map(|(_, v)| v)
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Every file the graph knows about.
fn files_in_graph(edges: &[Edge]) -> BTreeSet<String> {
    edges
        .iter()
        .filter(|e| e.p == "file_type" || e.p == "defines_fn" || e.p == "declares_module")
        .filter_map(|e| e.a.first().cloned())
        .collect()
}

/// Evaluate every graph rule against the whole graph.
///
/// Returns an empty vector when no rule reads the graph, or when the graph
/// has not been built — an audit reports what it can see, and a missing
/// derived artifact is not a failure worth propagating.
pub async fn audit_graph_rules(root: &Path, rules: &[Rule]) -> Vec<GraphHit> {
    let graph_rules: Vec<&Rule> = rules
        .iter()
        .filter(|r| is_audit_eligible_graph_rule(r))
        .collect();
    if graph_rules.is_empty() {
        return Vec::new();
    }
    let edges = store::load(&store::graph_path(root)).unwrap_or_default();
    if edges.is_empty() {
        return Vec::new();
    }

    let network = crate::net::build_network();
    for rule in &graph_rules {
        if network.add_rule((*rule).clone()).await.is_err() {
            return Vec::new();
        }
    }

    for edge in &edges {
        let _ = network.assert_fact(edge.to_fact()).await;
    }
    // The audit's defining move: every file is the edited file, so rules
    // scoped to the current edit match everywhere instead of nowhere.
    for file in files_in_graph(&edges) {
        let _ = network
            .assert_fact(Fact {
                id: format!("{EDITED_FILE}:{file}"),
                predicate: EDITED_FILE.to_string(),
                args: vec![file],
                timestamp: 0,
                source: Some("graph:context".to_string()),
            })
            .await;
    }

    // Matches must reach the agenda before they can fire; without this the
    // network reports zero consequences with every fact correctly asserted.
    if network.update_agenda().await.is_err() {
        return Vec::new();
    }
    let Ok(consequences) = network.fire_all_consequences() else {
        return Vec::new();
    };

    let mut hits = Vec::new();
    for c in &consequences {
        let Some(logged) = crate::hook_logged::LoggedConsequence::from_consequence(c) else {
            continue;
        };
        if !matches!(
            logged.action_type.as_str(),
            "constraint_violation" | "constraint_warning"
        ) {
            continue;
        }
        // `?file` is the binding every structural rule opens on; without it
        // the finding cannot be attributed to a location.
        let Some(file) = logged.bindings.get("file").or(logged.bindings.get("?file")) else {
            continue;
        };
        hits.push(GraphHit {
            rule_id: logged.rule_id.to_string(),
            action_type: logged.action_type.clone(),
            file: file.clone(),
            detail: compact_detail(&logged.bindings),
        });
    }
    hits.sort_by(|a, b| (&a.rule_id, &a.file, &a.detail).cmp(&(&b.rule_id, &b.file, &b.detail)));
    hits.dedup();
    hits
}

#[cfg(test)]
mod tests {
    use super::super::sync;
    use super::*;
    use phr::{Action, Condition};
    use tempfile::TempDir;

    /// The shipped untested-risky-call rule, in engine form.
    fn untested_rule() -> Rule {
        Rule {
            id: "warn-untested-risky-call".into(),
            priority: 20,
            conditions: vec![
                cond(EDITED_FILE, &["?file"]),
                cond("file_type", &["?file", "production"]),
                cond("defines_fn", &["?file", "?func"]),
                cond("calls_api", &["?func", "?api"]),
                cond("no_direct_test", &["?func"]),
            ],
            actions: vec![Action {
                action_type: "constraint_warning".into(),
                params: vec!["`?func` in ?file calls ?api untested".into()],
                data: None,
            }],
        }
    }

    fn cond(p: &str, args: &[&str]) -> Condition {
        Condition {
            predicate: p.into(),
            args: args.iter().map(|s| (*s).to_string()).collect(),
            script: None,
        }
    }

    fn content_rule() -> Rule {
        Rule {
            id: "content-only".into(),
            priority: 1,
            conditions: vec![cond("new_content_contains", &[".unwrap()"])],
            actions: vec![],
        }
    }

    fn query_only_diagnostic_rule() -> Rule {
        Rule {
            id: "unvalidated-lifecycle-gap".into(),
            priority: 1,
            conditions: vec![cond("generated_without_consumer", &["?artifact"])],
            actions: vec![],
        }
    }

    /// Two flaggable files plus one clean, so a whole-tree sweep has to find
    /// more than the single file a hook would look at.
    fn project() -> TempDir {
        let d = TempDir::new().expect("tempdir");
        std::fs::create_dir_all(d.path().join("src")).expect("mkdir");
        std::fs::write(
            d.path().join("src/one.rs"),
            "pub fn danger_one(v: Vec<u32>) -> u32 { *v.first().expect(\"e\") }\n",
        )
        .expect("one");
        std::fs::write(
            d.path().join("src/two.rs"),
            "pub fn danger_two(v: Vec<u32>) -> u32 { *v.first().expect(\"e\") }\n",
        )
        .expect("two");
        std::fs::write(d.path().join("src/clean.rs"), "pub fn ok() -> u32 { 1 }\n").expect("clean");
        sync::rebuild(d.path()).expect("rebuild");
        d
    }

    #[test]
    fn a_graph_rule_is_recognized() {
        assert!(is_graph_rule(&untested_rule()));
    }

    #[test]
    fn a_content_rule_is_not_a_graph_rule() {
        assert!(!is_graph_rule(&content_rule()));
    }

    #[test]
    fn unvalidated_diagnostics_are_queryable_but_not_audit_eligible() {
        let rule = query_only_diagnostic_rule();
        assert!(is_graph_rule(&rule));
        assert!(!is_audit_eligible_graph_rule(&rule));
    }

    #[tokio::test]
    async fn the_audit_finds_every_offending_file_not_just_one() {
        let d = project();
        let hits = audit_graph_rules(d.path(), &[untested_rule()]).await;
        let files: BTreeSet<&str> = hits.iter().map(|h| h.file.as_str()).collect();
        assert!(files.contains("src/one.rs"), "hits: {hits:?}");
        assert!(files.contains("src/two.rs"), "hits: {hits:?}");
    }

    #[tokio::test]
    async fn a_clean_file_produces_no_hit() {
        let d = project();
        let hits = audit_graph_rules(d.path(), &[untested_rule()]).await;
        assert!(hits.iter().all(|h| h.file != "src/clean.rs"), "{hits:?}");
    }

    #[tokio::test]
    async fn every_hit_carries_the_rule_that_produced_it() {
        let d = project();
        let hits = audit_graph_rules(d.path(), &[untested_rule()]).await;
        assert!(!hits.is_empty());
        assert!(hits.iter().all(|h| h.rule_id == "warn-untested-risky-call"));
    }

    #[tokio::test]
    async fn the_detail_names_what_matched() {
        let d = project();
        let hits = audit_graph_rules(d.path(), &[untested_rule()]).await;
        let one = hits
            .iter()
            .find(|h| h.file == "src/one.rs")
            .expect("hit for one.rs");
        assert!(one.detail.contains("danger_one"), "detail: {}", one.detail);
        assert!(one.detail.contains("expect"), "detail: {}", one.detail);
    }

    #[tokio::test]
    async fn the_detail_omits_the_rule_prose() {
        // Audit lists many hits per rule; repeating the guidance paragraph
        // for each buries the only part that differs.
        let d = project();
        let hits = audit_graph_rules(d.path(), &[untested_rule()]).await;
        assert!(!hits.is_empty());
        for h in &hits {
            assert!(
                !h.detail.contains("no_direct_test"),
                "prose leaked: {}",
                h.detail
            );
            assert!(h.detail.len() < 120, "detail too long: {}", h.detail);
        }
    }

    #[tokio::test]
    async fn the_detail_omits_the_file_it_is_already_keyed_by() {
        let d = project();
        let hits = audit_graph_rules(d.path(), &[untested_rule()]).await;
        let one = hits
            .iter()
            .find(|h| h.file == "src/one.rs")
            .expect("hit for one.rs");
        assert!(!one.detail.contains("src/one.rs"), "detail: {}", one.detail);
    }

    #[tokio::test]
    async fn rules_that_do_not_read_the_graph_are_ignored() {
        let d = project();
        assert!(
            audit_graph_rules(d.path(), &[content_rule()])
                .await
                .is_empty()
        );
    }

    #[tokio::test]
    async fn a_project_with_no_graph_reports_nothing_rather_than_failing() {
        let d = TempDir::new().expect("tempdir");
        assert!(
            audit_graph_rules(d.path(), &[untested_rule()])
                .await
                .is_empty()
        );
    }

    #[tokio::test]
    async fn results_are_deterministic_across_runs() {
        let d = project();
        let a = audit_graph_rules(d.path(), &[untested_rule()]).await;
        let b = audit_graph_rules(d.path(), &[untested_rule()]).await;
        assert_eq!(a, b);
    }
}

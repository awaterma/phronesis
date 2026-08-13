//! ADR-to-rule relationships projected into the structural graph.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use phr::consequence::{Consequence, Provenance};

use super::model::Edge;
use crate::wiki::{self, DecisionStatus};

const DECISIONS_DIR: &str = ".phronesis/wiki/decisions";

/// Build validated decision/rule edges plus explicit lifecycle diagnostics.
pub fn extract(root: &Path) -> Vec<Edge> {
    let rules = crate::rules_file::read(&crate::rules_file::default_path(root))
        .map(|rules| {
            rules
                .rules
                .into_iter()
                .map(|rule| rule.id.to_string())
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    let Ok(decisions) = wiki::walk_decisions(&root.join(DECISIONS_DIR)) else {
        return Vec::new();
    };

    let mut edges = Vec::new();
    let mut governed = BTreeSet::new();
    for decision in decisions {
        let id = decision.frontmatter.id;
        let source = decision
            .path
            .strip_prefix(root)
            .unwrap_or(&decision.path)
            .to_string_lossy()
            .to_string();
        edges.push(Edge::base("graph_decision", &[&id], &source));
        for rule in decision.frontmatter.enforces {
            if !rules.contains(&rule) {
                edges.push(Edge::base("decision_missing_rule", &[&id, &rule], &source));
                continue;
            }
            match decision.frontmatter.status {
                DecisionStatus::Superseded => {
                    edges.push(Edge::base(
                        "superseded_decision_enforces",
                        &[&id, &rule],
                        &source,
                    ));
                    continue;
                }
                DecisionStatus::Proposed => {
                    edges.push(Edge::base(
                        "proposed_decision_enforces",
                        &[&id, &rule],
                        &source,
                    ));
                    continue;
                }
                DecisionStatus::Accepted => {}
            }
            edges.push(Edge::base("decision_enforces", &[&id, &rule], &source));
            edges.push(Edge::base("rule_governed_by", &[&rule, &id], &source));
            governed.insert(rule);
        }
    }
    for rule in rules.difference(&governed) {
        edges.push(Edge::base(
            "rule_without_decision",
            &[rule],
            ".phronesis/rules.json",
        ));
    }
    edges
}

/// Attach accepted governing decision IDs to fired consequences.
pub fn annotate_consequences(root: &Path, consequences: &mut [Consequence]) {
    let by_rule = extract(root)
        .into_iter()
        .filter(|edge| edge.p == "rule_governed_by")
        .filter_map(|edge| Some((edge.a.first()?.clone(), edge.a.get(1)?.clone())))
        .fold(
            BTreeMap::<String, Vec<String>>::new(),
            |mut map, (rule, decision)| {
                map.entry(rule).or_default().push(decision);
                map
            },
        );
    for consequence in consequences {
        let (rule_id, decisions) = match &mut consequence.provenance {
            Provenance::RuleFiring {
                rule_id, decisions, ..
            }
            | Provenance::RuleDrivenLookup {
                rule_id, decisions, ..
            } => (rule_id.as_str(), decisions),
            _ => continue,
        };
        if let Some(linked) = by_rule.get(rule_id) {
            *decisions = linked.clone();
            decisions.sort();
            decisions.dedup();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phr::consequence::{ConsequenceKind, Provenance};
    use tempfile::TempDir;

    fn project() -> TempDir {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(DECISIONS_DIR)).unwrap();
        std::fs::write(
            dir.path().join(".phronesis/rules.json"),
            r#"{"version":2,"rules":[{"id":"r1","when":[{"p":"x"}],"then":{"warn":"x"}}]}"#,
        )
        .unwrap();
        dir
    }

    #[test]
    fn accepted_links_are_bidirectional_and_missing_rules_are_diagnostic() {
        let dir = project();
        std::fs::write(
            dir.path().join(DECISIONS_DIR).join("2026-01-01-choice.md"),
            "---\nid: choice\ndate: 2026-01-01\nstatus: accepted\nenforces:\n  - r1\n  - absent\n---\nDecision.\n",
        )
        .unwrap();
        let edges = extract(dir.path());
        assert!(edges.iter().any(|edge| edge.p == "decision_enforces"));
        assert!(edges.iter().any(|edge| edge.p == "rule_governed_by"));
        assert!(edges.iter().any(|edge| edge.p == "decision_missing_rule"));
        assert!(!edges.iter().any(|edge| edge.p == "rule_without_decision"));
    }

    #[test]
    fn superseded_links_do_not_govern_consequences() {
        let dir = project();
        std::fs::write(
            dir.path().join(DECISIONS_DIR).join("2026-01-01-old.md"),
            "---\nid: old\ndate: 2026-01-01\nstatus: superseded\nenforces:\n  - r1\nsuperseded_by: new\n---\nOld.\n",
        )
        .unwrap();
        let edges = extract(dir.path());
        assert!(
            edges
                .iter()
                .any(|edge| edge.p == "superseded_decision_enforces")
        );
        assert!(edges.iter().any(|edge| edge.p == "rule_without_decision"));
    }

    #[test]
    fn consequence_is_annotated_from_accepted_link() {
        let dir = project();
        std::fs::write(
            dir.path().join(DECISIONS_DIR).join("2026-01-01-choice.md"),
            "---\nid: choice\ndate: 2026-01-01\nstatus: accepted\nenforces:\n  - r1\n---\nDecision.\n",
        )
        .unwrap();
        let mut consequences = vec![Consequence {
            kind: ConsequenceKind::Constraint,
            predicate: "r1".to_string(),
            payload: serde_json::json!({}),
            provenance: Provenance::RuleFiring {
                rule_id: "r1".into(),
                bound_facts: Vec::new(),
                bindings: Default::default(),
                fact_sources: Default::default(),
                decisions: Vec::new(),
            },
        }];
        annotate_consequences(dir.path(), &mut consequences);
        match &consequences[0].provenance {
            Provenance::RuleFiring { decisions, .. } => {
                assert_eq!(decisions, &["choice"]);
            }
            other => panic!("expected rule provenance, got {other:?}"),
        }
    }

    #[test]
    fn rebuild_persists_links_and_tracks_decision_freshness() {
        let dir = project();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "pub fn f() {}\n").unwrap();
        let decision_path = dir.path().join(DECISIONS_DIR).join("2026-01-01-choice.md");
        std::fs::write(
            &decision_path,
            "---\nid: choice\ndate: 2026-01-01\nstatus: accepted\nenforces:\n  - r1\n---\nDecision.\n",
        )
        .unwrap();

        super::super::sync::rebuild(dir.path()).unwrap();
        let edges =
            super::super::store::load(&super::super::store::graph_path(dir.path())).unwrap();
        assert!(
            edges
                .iter()
                .any(|edge| { edge.p == "rule_governed_by" && edge.a == ["r1", "choice"] })
        );

        std::fs::write(
            decision_path,
            "---\nid: choice\ndate: 2026-01-01\nstatus: proposed\nenforces:\n  - r1\n---\nChanged.\n",
        )
        .unwrap();
        let index =
            super::super::sync::load_index(&super::super::sync::index_path(dir.path())).unwrap();
        assert!(matches!(
            super::super::sync::check_freshness(dir.path(), &index),
            super::super::sync::Freshness::Stale(_)
        ));
    }
}

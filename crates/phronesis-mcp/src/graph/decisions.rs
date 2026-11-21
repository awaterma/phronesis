//! ADR-to-rule relationships projected into the structural graph.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use phr::consequence::{Consequence, Provenance};

use super::model::Edge;
use crate::wiki::{self, DecisionStatus};

/// Build validated decision/rule edges plus explicit lifecycle diagnostics.
pub fn extract(root: &Path) -> Vec<Edge> {
    let rules = crate::rules_file::read(&crate::rules_file::default_path(root))
        .map(|rules| {
            rules
                .rules
                .into_iter()
                // `read()` unfolds OR clauses into `base#orN` child ids;
                // decisions govern the logical rule, so match on the base id.
                .map(|rule| crate::rules_file::base_rule_id(&rule.id).to_string())
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    // Graph extraction must stay total (it feeds freshness and hook-time
    // provenance), so duplicates and conflicts become diagnostic edges
    // instead of errors. Canonical copies govern; an internally duplicated
    // id contributes no governance from either location.
    let set = wiki::resolve_decisions_lenient(root);
    let mut edges = Vec::new();
    for duplicate in &set.duplicates {
        let source = duplicate
            .second
            .strip_prefix(root)
            .unwrap_or(&duplicate.second)
            .to_string_lossy()
            .to_string();
        edges.push(Edge::base(
            "decision_duplicate_id",
            &[duplicate.id.as_str()],
            &source,
        ));
    }
    for unreadable in &set.unreadable {
        let source = unreadable
            .path
            .strip_prefix(root)
            .unwrap_or(&unreadable.path)
            .to_string_lossy()
            .to_string();
        let message: String = unreadable.message.chars().take(120).collect();
        edges.push(Edge::base("decision_unreadable", &[&message], &source));
    }
    for conflict in &set.conflicts {
        let source = conflict
            .legacy
            .strip_prefix(root)
            .unwrap_or(&conflict.legacy)
            .to_string_lossy()
            .to_string();
        edges.push(Edge::base(
            "decision_conflicting_copy",
            &[conflict.id.as_str()],
            &source,
        ));
    }
    let mut governed = BTreeSet::new();
    for decision in set.decisions {
        let id = decision.frontmatter.id;
        let source = decision
            .path
            .strip_prefix(root)
            .unwrap_or(&decision.path)
            .to_string_lossy()
            .to_string();
        edges.push(Edge::base("graph_decision", &[&id], &source));
        for rule in decision.frontmatter.enforces {
            // Accept the base id or any `#orN` expansion of it — both name
            // the same logical rule on disk.
            let rule = crate::rules_file::base_rule_id(&rule).to_string();
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
        // The RETE network fires OR-expanded rules under `base#orN` ids;
        // governance links are keyed by the base id.
        if let Some(linked) = by_rule.get(crate::rules_file::base_rule_id(rule_id)) {
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

    const LEGACY_DECISIONS_DIR: &str = crate::wiki::LEGACY_DECISIONS_REL;

    fn project() -> TempDir {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(LEGACY_DECISIONS_DIR)).unwrap();
        std::fs::write(
            dir.path().join(".phronesis/rules.json"),
            r#"{"version":2,"rules":[{"id":"r1","when":[{"p":"x"}],"then":{"warn":"x"}}]}"#,
        )
        .unwrap();
        dir
    }

    /// Rules file with one OR-clause rule: `rules_file::read()` unfolds it
    /// into `compound#or0` / `compound#or1`, mirroring real packs such as
    /// `warn-untested-risky-call`.
    fn or_project() -> TempDir {
        let dir = TempDir::new().expect("tempdir");
        std::fs::create_dir_all(dir.path().join(LEGACY_DECISIONS_DIR))
            .expect("mkdir decisions dir");
        std::fs::write(
            dir.path().join(".phronesis/rules.json"),
            r#"{"version":2,"rules":[{"id":"compound","when":[{"or":[{"p":"a"},{"p":"b"}]}],"then":{"warn":"x"}}]}"#,
        )
        .expect("write rules.json");
        dir
    }

    #[test]
    fn or_expanded_rules_link_to_decisions_by_base_id() {
        let dir = or_project();
        std::fs::write(
            dir.path().join(LEGACY_DECISIONS_DIR).join("2026-01-01-choice.md"),
            "---\nid: choice\ndate: 2026-01-01\nstatus: accepted\nenforces:\n  - compound\n---\nDecision.\n",
        )
        .expect("write decision");
        let edges = extract(dir.path());
        assert!(
            edges
                .iter()
                .any(|edge| edge.p == "decision_enforces" && edge.a == ["choice", "compound"])
        );
        assert!(
            edges
                .iter()
                .any(|edge| edge.p == "rule_governed_by" && edge.a == ["compound", "choice"])
        );
        assert!(!edges.iter().any(|edge| edge.p == "decision_missing_rule"));
        assert!(!edges.iter().any(|edge| edge.p == "rule_without_decision"));
    }

    #[test]
    fn accepted_links_are_bidirectional_and_missing_rules_are_diagnostic() {
        let dir = project();
        std::fs::write(
            dir.path().join(LEGACY_DECISIONS_DIR).join("2026-01-01-choice.md"),
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
            dir.path().join(LEGACY_DECISIONS_DIR).join("2026-01-01-old.md"),
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
            dir.path().join(LEGACY_DECISIONS_DIR).join("2026-01-01-choice.md"),
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
    fn or_expanded_consequence_is_annotated_from_base_id_link() {
        let dir = or_project();
        std::fs::write(
            dir.path().join(LEGACY_DECISIONS_DIR).join("2026-01-01-choice.md"),
            "---\nid: choice\ndate: 2026-01-01\nstatus: accepted\nenforces:\n  - compound\n---\nDecision.\n",
        )
        .expect("write decision");
        // The RETE network fires or-expanded rules under ids like
        // `compound#or0`; the accepted decision names the base id.
        let mut consequences = vec![Consequence {
            kind: ConsequenceKind::Constraint,
            predicate: "compound".to_string(),
            payload: serde_json::json!({}),
            provenance: Provenance::RuleFiring {
                rule_id: "compound#or0".into(),
                bound_facts: Vec::new(),
                bindings: Default::default(),
                fact_sources: Default::default(),
                decisions: Vec::new(),
            },
        }];
        annotate_consequences(dir.path(), &mut consequences);
        match &consequences[0].provenance {
            Provenance::RuleFiring { decisions, .. } => assert_eq!(decisions, &["choice"]),
            other => panic!("expected rule provenance, got {other:?}"),
        }
    }

    #[test]
    fn rebuild_persists_links_and_tracks_decision_freshness() {
        let dir = project();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "pub fn f() {}\n").unwrap();
        let decision_path = dir
            .path()
            .join(LEGACY_DECISIONS_DIR)
            .join("2026-01-01-choice.md");
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

    #[test]
    fn canonical_decisions_govern_from_docs_decisions() {
        let dir = project();
        let canonical = dir.path().join(crate::wiki::CANONICAL_DECISIONS_REL);
        std::fs::create_dir_all(&canonical).unwrap();
        std::fs::write(
            canonical.join("2026-01-01-choice.md"),
            "---\nid: choice\ndate: 2026-01-01\nstatus: accepted\nenforces:\n  - r1\n---\nDecision.\n",
        )
        .unwrap();
        let edges = extract(dir.path());
        assert!(edges.iter().any(|edge| {
            edge.p == "decision_enforces"
                && edge.a == ["choice", "r1"]
                && edge.src == "docs/decisions/2026-01-01-choice.md"
        }));
        assert!(edges.iter().any(|edge| edge.p == "rule_governed_by"));
    }

    #[test]
    fn canonical_wins_and_differing_legacy_copy_is_diagnostic() {
        // Same id in both locations with different bodies: exactly one set
        // of governance edges (canonical), plus the conflict diagnostic.
        let dir = project();
        let canonical = dir.path().join(crate::wiki::CANONICAL_DECISIONS_REL);
        let legacy = dir.path().join(crate::wiki::LEGACY_DECISIONS_REL);
        std::fs::create_dir_all(&canonical).unwrap();
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(
            canonical.join("2026-01-01-choice.md"),
            "---\nid: choice\ndate: 2026-01-01\nstatus: accepted\nenforces:\n  - r1\n---\ncanonical\n",
        )
        .unwrap();
        std::fs::write(
            legacy.join("2026-01-01-choice.md"),
            "---\nid: choice\ndate: 2026-01-01\nstatus: superseded\nenforces:\n  - r1\n---\nlegacy\n",
        )
        .unwrap();
        let edges = extract(dir.path());
        assert!(
            edges
                .iter()
                .any(|edge| edge.p == "decision_conflicting_copy"
                    && edge.a == ["choice"]
                    && edge.src == ".phronesis/wiki/decisions/2026-01-01-choice.md")
        );
        // Canonical status is accepted → governance edges exist once.
        let governed = edges
            .iter()
            .filter(|edge| edge.p == "rule_governed_by" && edge.a == ["r1", "choice"])
            .count();
        assert_eq!(governed, 1, "exactly one canonical governance edge");
        assert!(
            !edges
                .iter()
                .any(|edge| edge.p == "superseded_decision_enforces")
        );
    }

    #[test]
    fn identical_legacy_copy_is_not_diagnostic() {
        let dir = project();
        let canonical = dir.path().join(crate::wiki::CANONICAL_DECISIONS_REL);
        let legacy = dir.path().join(crate::wiki::LEGACY_DECISIONS_REL);
        std::fs::create_dir_all(&canonical).unwrap();
        std::fs::create_dir_all(&legacy).unwrap();
        let content =
            "---\nid: choice\ndate: 2026-01-01\nstatus: accepted\nenforces:\n  - r1\n---\nsame\n";
        std::fs::write(canonical.join("2026-01-01-choice.md"), content).unwrap();
        std::fs::write(legacy.join("2026-01-01-choice.md"), content).unwrap();
        let edges = extract(dir.path());
        assert!(
            !edges
                .iter()
                .any(|edge| edge.p == "decision_conflicting_copy")
        );
        assert!(edges.iter().any(|edge| edge.p == "decision_enforces"));
    }

    #[test]
    fn duplicate_id_within_one_location_drops_governance_and_diagnoses() {
        let dir = project();
        let legacy = dir.path().join(crate::wiki::LEGACY_DECISIONS_REL);
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(
            legacy.join("2026-01-01-a.md"),
            "---\nid: dup\ndate: 2026-01-01\nstatus: accepted\nenforces:\n  - r1\n---\nfirst\n",
        )
        .unwrap();
        std::fs::write(
            legacy.join("2026-01-02-b.md"),
            "---\nid: dup\ndate: 2026-01-02\nstatus: accepted\nenforces:\n  - r1\n---\nsecond\n",
        )
        .unwrap();
        let edges = extract(dir.path());
        assert!(
            edges
                .iter()
                .any(|edge| edge.p == "decision_duplicate_id" && edge.a == ["dup"])
        );
        assert!(
            !edges.iter().any(|edge| edge.p == "decision_enforces"),
            "an ambiguous id must not govern anything"
        );
        assert!(edges.iter().any(|edge| edge.p == "rule_without_decision"));
    }

    #[test]
    fn ambiguous_canonical_id_cannot_fall_back_to_legacy_governance() {
        let dir = project();
        let canonical = dir.path().join(crate::wiki::CANONICAL_DECISIONS_REL);
        let legacy = dir.path().join(crate::wiki::LEGACY_DECISIONS_REL);
        std::fs::create_dir_all(&canonical).unwrap();
        std::fs::create_dir_all(&legacy).unwrap();
        let body =
            "---\nid: dup\ndate: 2026-01-01\nstatus: accepted\nenforces:\n  - r1\n---\nDecision.\n";
        for path in [
            canonical.join("a.md"),
            canonical.join("b.md"),
            legacy.join("a.md"),
        ] {
            std::fs::write(path, body).unwrap();
        }
        let edges = extract(dir.path());
        assert!(edges.iter().any(|edge| edge.p == "decision_duplicate_id"));
        assert!(!edges.iter().any(|edge| edge.p == "decision_enforces"));
    }

    #[test]
    fn canonical_decision_freshness_is_tracked() {
        let dir = project();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "pub fn f() {}\n").unwrap();
        let canonical = dir.path().join(crate::wiki::CANONICAL_DECISIONS_REL);
        std::fs::create_dir_all(&canonical).unwrap();
        let decision_path = canonical.join("2026-01-01-choice.md");
        std::fs::write(
            &decision_path,
            "---\nid: choice\ndate: 2026-01-01\nstatus: accepted\nenforces:\n  - r1\n---\nDecision.\n",
        )
        .unwrap();

        super::super::sync::rebuild(dir.path()).unwrap();
        let index =
            super::super::sync::load_index(&super::super::sync::index_path(dir.path())).unwrap();
        assert!(matches!(
            super::super::sync::check_freshness(dir.path(), &index),
            super::super::sync::Freshness::Fresh
        ));

        std::fs::write(
            &decision_path,
            "---\nid: choice\ndate: 2026-01-01\nstatus: proposed\nenforces:\n  - r1\n---\nChanged.\n",
        )
        .unwrap();
        assert!(matches!(
            super::super::sync::check_freshness(dir.path(), &index),
            super::super::sync::Freshness::Stale(_)
        ));
    }

    #[test]
    fn provenance_annotation_uses_canonical_copy() {
        let dir = project();
        let canonical = dir.path().join(crate::wiki::CANONICAL_DECISIONS_REL);
        let legacy = dir.path().join(crate::wiki::LEGACY_DECISIONS_REL);
        std::fs::create_dir_all(&canonical).unwrap();
        std::fs::create_dir_all(&legacy).unwrap();
        // Canonical copy is accepted; the differing legacy copy claims the
        // same id as superseded. Governance must come from the canonical
        // copy only.
        std::fs::write(
            canonical.join("2026-01-01-choice.md"),
            "---\nid: choice\ndate: 2026-01-01\nstatus: accepted\nenforces:\n  - r1\n---\ncanonical\n",
        )
        .unwrap();
        std::fs::write(
            legacy.join("2026-01-01-choice.md"),
            "---\nid: choice\ndate: 2026-01-01\nstatus: superseded\nenforces:\n  - r1\n---\nlegacy\n",
        )
        .unwrap();
        let mut consequences = vec![Consequence {
            kind: ConsequenceKind::Event,
            predicate: "p".into(),
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
        let Provenance::RuleFiring { decisions, .. } = &consequences[0].provenance else {
            panic!("expected RuleFiring");
        };
        assert_eq!(decisions.as_slice(), &["choice".to_string()]);
    }

    #[test]
    fn unique_legacy_decision_still_governs() {
        let dir = project();
        std::fs::write(
            dir.path().join(crate::wiki::LEGACY_DECISIONS_REL).join("2026-01-01-choice.md"),
            "---\nid: choice\ndate: 2026-01-01\nstatus: accepted\nenforces:\n  - r1\n---\nDecision.\n",
        )
        .unwrap();
        let edges = extract(dir.path());
        assert!(
            edges
                .iter()
                .any(|edge| edge.p == "decision_enforces" && edge.a == ["choice", "r1"])
        );
    }
}

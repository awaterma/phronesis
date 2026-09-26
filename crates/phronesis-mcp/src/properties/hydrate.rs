//! Demand-gated property-fact production at hook fire — the `coverage/hydrate.rs`
//! pattern applied to the property ontology (SPEC-property-ontology.md §2).

use std::collections::HashSet;
use std::path::Path;

use crate::properties::store::{load_properties, load_results};

/// Every relation this module can assert (spec §2 — the closed relation set).
/// The hook demand-gates on this set, exactly as coverage does.
pub const RELATIONS: &[&str] = &[
    "property",
    "property_subject",
    "property_kind",
    "property_depends_on",
    "property_source",
    "property_status",
    "property_corroborated_by",
    "property_encoding",
    "verification_result",
    "result_revision",
    "stale_evidence",
    "property_obligation",
];

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PropertyFact {
    pub predicate: String,
    pub args: Vec<String>,
}

pub struct EditedFile<'a> {
    pub path: String,
    pub old: Option<&'a str>,
    pub new: &'a str,
}

pub struct PropertyHydrationInput<'a> {
    pub root: &'a Path,
    pub rule_relations: HashSet<String>,
    pub edited: Vec<EditedFile<'a>>,
    pub head_sha: Option<String>,
}

fn fact(predicate: &str, args: Vec<String>) -> PropertyFact {
    PropertyFact {
        predicate: predicate.to_string(),
        args,
    }
}

pub fn facts_for_event(input: &PropertyHydrationInput) -> anyhow::Result<Vec<PropertyFact>> {
    let wants = |rel: &str| input.rule_relations.contains(rel);

    if !RELATIONS.iter().any(|r| wants(r)) {
        return Ok(Vec::new());
    }

    let mut facts: Vec<PropertyFact> = Vec::new();

    let properties = load_properties(input.root)?;
    let results = load_results(input.root)?;

    for p in &properties {
        let id = p.id.clone();
        if wants("property") {
            facts.push(fact("property", vec![id.clone()]));
        }
        if wants("property_subject") {
            facts.push(fact(
                "property_subject",
                vec![id.clone(), p.subject.clone()],
            ));
        }
        if wants("property_kind") {
            facts.push(fact("property_kind", vec![id.clone(), p.kind.clone()]));
        }
        for region in &p.depends_on {
            if wants("property_depends_on") {
                facts.push(fact(
                    "property_depends_on",
                    vec![id.clone(), region.clone()],
                ));
            }
        }
        if wants("property_source") {
            let source = serde_json::to_value(p.source)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            facts.push(fact("property_source", vec![id.clone(), source]));
        }
        if wants("property_status") {
            let status = serde_json::to_value(p.status)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            facts.push(fact("property_status", vec![id.clone(), status]));
        }
        for c in &p.corroborated_by {
            if wants("property_corroborated_by") {
                facts.push(fact(
                    "property_corroborated_by",
                    vec![id.clone(), c.clone()],
                ));
            }
        }
        for e in &p.encodings {
            if wants("property_encoding") {
                facts.push(fact(
                    "property_encoding",
                    vec![
                        id.clone(),
                        e.language.clone(),
                        e.verifier.clone(),
                        e.artifact.clone(),
                    ],
                ));
            }
        }
    }

    // Results at their recorded revision (spec §2: verification_result,
    // result_revision — provenance via Fact.source, not RETE args).
    for r in &results {
        if wants("verification_result") {
            facts.push(fact(
                "verification_result",
                vec![r.property.clone(), r.verifier.clone(), r.status.clone()],
            ));
        }
        if wants("result_revision") {
            facts.push(fact(
                "result_revision",
                vec![r.property.clone(), r.verifier.clone(), r.revision.clone()],
            ));
        }
    }

    // Staleness is host-derived (spec §5): the engine cannot order SHAs, so
    // the resolver asserts stale_evidence when the recorded result revision
    // differs from the current head AND a dependent region changed in this
    // event. Without an edit event, staleness stays derivable-only — the
    // facts above still feed rules that want them.
    let wants_stale = wants("stale_evidence");
    if wants_stale {
        let mut changed: HashSet<String> = HashSet::new();
        for edit in &input.edited {
            if let Ok(regions) =
                crate::coverage::region_map::changed_regions(edit.old.unwrap_or(""), edit.new)
            {
                changed.extend(regions.functions.iter().cloned());
                changed.extend(regions.branches.iter().cloned());
            }
        }
        let head = input.head_sha.clone().unwrap_or_default();
        let mut stale: Vec<PropertyFact> = Vec::new();
        for r in &results {
            let depends_on_changed = properties
                .iter()
                .filter(|p| p.id == r.property)
                .flat_map(|p| p.depends_on.iter())
                .any(|region| changed.contains(region));
            if depends_on_changed && !r.revision.is_empty() && r.revision != head {
                stale.push(fact(
                    "stale_evidence",
                    vec![r.property.clone(), r.verifier.clone()],
                ));
            }
        }
        facts.append(&mut stale);
    }

    // First-proof obligation (acceptance C9): the obligation is an OR —
    // (no verification_result at HEAD) OR (stale). An accepted property whose
    // dependent region changed and that has NO result record at all must
    // still fire: the first proof is reachable. Gated separately from the
    // staleness branch so the OR never collapses to an AND.
    if wants("property_obligation") {
        let mut changed: HashSet<String> = HashSet::new();
        for edit in &input.edited {
            if let Ok(regions) =
                crate::coverage::region_map::changed_regions(edit.old.unwrap_or(""), edit.new)
            {
                changed.extend(regions.functions.iter().cloned());
                changed.extend(regions.branches.iter().cloned());
            }
        }
        let head = input.head_sha.clone().unwrap_or_default();
        for p in &properties {
            if !matches!(
                p.status,
                crate::properties::store::PropertyStatus::Accepted
                    | crate::properties::store::PropertyStatus::Verified
            ) {
                continue;
            }
            let depends_on_changed = p.depends_on.iter().any(|region| changed.contains(region));
            let has_result = results
                .iter()
                .any(|r| r.property == p.id && r.revision == head);
            if depends_on_changed && !has_result {
                facts.push(fact(
                    "property_obligation",
                    vec![p.id.clone(), "first_proof".to_string()],
                ));
            }
        }
    }

    facts.sort();
    facts.dedup();
    Ok(facts)
}

use std::collections::HashSet;
use std::path::Path;

use crate::coverage::region_map::changed_regions;
use crate::coverage::store::{StoreCorruption, StoreState, is_stale, load_store};

/// Every relation this module can assert. The hook demand-gates on this
/// set: a relation is asserted only when some loaded rule mentions it (the
/// `graph/hydrate.rs` precedent).
pub const RELATIONS: &[&str] = &[
    "test_hits_region",
    "test_hits_branch",
    "coverage_revision",
    "coverage_stale",
    "changed_region",
    "changed_function",
    "head_revision",
    "region_without_dynamic_evidence",
    "region_without_formal_evidence",
    "store_corrupt",
];

/// Relations whose derivation reads the coverage store. The store is read
/// (and verified) at most once per event, and only when one is demanded.
const STORE_RELATIONS: &[&str] = &[
    "test_hits_region",
    "test_hits_branch",
    "coverage_revision",
    "coverage_stale",
    "region_without_dynamic_evidence",
    "store_corrupt",
];

/// `(predicate, args)` in the clock_facts / outcomes::facts shape — the hook
/// wraps each one in a RETE `Fact` with the same machinery those use.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CoverageFact {
    pub predicate: String,
    pub args: Vec<String>,
}

pub struct EditedFile<'a> {
    pub path: String,
    pub old: Option<&'a str>,
    pub new: &'a str,
}

pub struct HydrationInput<'a> {
    pub root: &'a Path,
    pub rule_relations: HashSet<String>,
    pub edited: Vec<EditedFile<'a>>,
    pub head_sha: Option<String>,
}

fn fact(predicate: &str, args: Vec<String>) -> CoverageFact {
    CoverageFact {
        predicate: predicate.to_string(),
        args,
    }
}

/// Hydration result: the facts, plus the store corruption (if any) so the
/// hook can report it on stderr whether or not a rule demanded
/// `store_corrupt`.
#[derive(Debug, Default)]
pub struct Hydration {
    pub facts: Vec<CoverageFact>,
    pub store_corrupt: Option<StoreCorruption>,
}

pub fn facts_for_event(input: &HydrationInput) -> anyhow::Result<Vec<CoverageFact>> {
    hydrate(input).map(|h| h.facts)
}

/// Derive this event's coverage facts. A corrupt store is not an error: it
/// yields `store_corrupt(coverage, <reason>)` (when demanded), no hit or
/// revision facts, and gap facts computed as if the store held no evidence.
/// Errors are reserved for failures outside the store (region mapping).
pub fn hydrate(input: &HydrationInput) -> anyhow::Result<Hydration> {
    let wants = |rel: &str| input.rule_relations.contains(rel);

    // Demand gate: no loaded rule mentions any coverage relation -> nothing.
    if !RELATIONS.iter().any(|r| wants(r)) {
        return Ok(Hydration::default());
    }

    let mut facts: Vec<CoverageFact> = Vec::new();

    let store = if STORE_RELATIONS.iter().any(|r| wants(r)) {
        load_store(input.root)
    } else {
        StoreState::Missing
    };
    let (index, hits, store_corrupt) = match store {
        StoreState::Loaded { index, hits } => (Some(index), hits, None),
        StoreState::Missing => (None, Vec::new(), None),
        StoreState::Corrupt(c) => (None, Vec::new(), Some(c)),
    };
    if wants("store_corrupt")
        && let Some(c) = &store_corrupt
    {
        facts.push(fact(
            "store_corrupt",
            vec!["coverage".to_string(), c.reason.to_string()],
        ));
    }
    // D3: only evidence verified current for HEAD may suppress a gap.
    let stale = index
        .as_ref()
        .is_some_and(|idx| is_stale(idx, input.head_sha.as_deref()));

    if wants("head_revision")
        && let Some(sha) = &input.head_sha
    {
        facts.push(fact("head_revision", vec![sha.clone()]));
    }

    if wants("coverage_revision")
        && let Some(idx) = &index
    {
        facts.push(fact("coverage_revision", vec![idx.revision.clone()]));
    }
    if wants("coverage_stale") && stale {
        facts.push(fact("coverage_stale", Vec::new()));
    }

    // Closed-world gap evidence, bounded by the change: collect the changed
    // region identities first (the `no_direct_test` precedent — the host
    // computes absence, since RETE conditions do not implement
    // negation-as-failure).
    let mut changed_regions_out: Vec<String> = Vec::new();
    if wants("changed_region")
        || wants("changed_function")
        || wants("region_without_dynamic_evidence")
        || wants("region_without_formal_evidence")
    {
        let change_id = input
            .head_sha
            .as_ref()
            .map(|sha| format!("head:{}", &sha[..12]))
            .unwrap_or_else(|| "head:unknown".to_string());

        for edit in &input.edited {
            let regions = changed_regions(edit.old.unwrap_or(""), edit.new)?;
            for region in regions.functions.iter().chain(regions.branches.iter()) {
                changed_regions_out.push(region.clone());
                if wants("changed_region") {
                    facts.push(fact(
                        "changed_region",
                        vec![change_id.clone(), region.clone()],
                    ));
                }
                if wants("changed_function") && region.starts_with("fn:") {
                    facts.push(fact(
                        "changed_function",
                        vec![change_id.clone(), region.clone()],
                    ));
                }
            }
        }
        changed_regions_out.sort();
        changed_regions_out.dedup();

        if wants("region_without_dynamic_evidence") {
            // Closed world over the WHOLE store (not the change-scoped
            // subset): "has any test ever executed this region?" is a
            // question about the imported evidence, not about this event.
            // Stale or corrupt evidence counts as no evidence: it says
            // nothing about the code at HEAD.
            let hit_regions: HashSet<&str> = if stale {
                HashSet::new()
            } else {
                hits.iter().map(|h| h.region.as_str()).collect()
            };
            for region in &changed_regions_out {
                if !hit_regions.contains(region.as_str()) {
                    facts.push(fact(
                        "region_without_dynamic_evidence",
                        vec![region.clone()],
                    ));
                }
            }
        }
        if wants("region_without_formal_evidence") {
            // SPEC-property-ontology seam: until its results index lands,
            // formal evidence is unconditionally absent — the resolver has
            // no proof results to query. The spec makes this seam explicit
            // rather than silent (§6, gap detection precedent).
            for region in &changed_regions_out {
                facts.push(fact("region_without_formal_evidence", vec![region.clone()]));
            }
        }
    }

    if wants("test_hits_region") || wants("test_hits_branch") {
        // Change scope: only hits whose file is part of this event.
        let edited_paths: HashSet<&str> = input.edited.iter().map(|e| e.path.as_str()).collect();
        for hit in hits {
            if !edited_paths.contains(hit.file.as_str()) {
                continue;
            }
            // Generic join relation (spec §5.1): every hit participates in
            // test_hits_region, branch sites included — the relevant-test
            // join must distinguish branch_relevant from function_relevant
            // through the region identity, not through a separate predicate.
            if wants("test_hits_region") {
                facts.push(fact(
                    "test_hits_region",
                    vec![hit.test.clone(), hit.region.clone()],
                ));
            }
            // Branch specialization for branch-only rules (spec §5.1's
            // `test_hits_branch` family, e.g. selection and gap rules).
            if wants("test_hits_branch") && hit.hit_kind == "branch" {
                facts.push(fact("test_hits_branch", vec![hit.test, hit.region]));
            }
        }
    }

    facts.sort();
    facts.dedup();
    Ok(Hydration {
        facts,
        store_corrupt,
    })
}

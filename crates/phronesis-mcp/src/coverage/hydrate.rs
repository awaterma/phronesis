use std::collections::HashSet;
use std::path::Path;

use crate::coverage::region_map::{changed_regions, is_qualified_region_id, repo_relative_path};
use crate::coverage::store::{load_hits, load_index};

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

pub fn facts_for_event(input: &HydrationInput) -> anyhow::Result<Vec<CoverageFact>> {
    let wants = |rel: &str| input.rule_relations.contains(rel);

    // Demand gate: no loaded rule mentions any coverage relation -> nothing.
    if !RELATIONS.iter().any(|r| wants(r)) {
        return Ok(Vec::new());
    }

    let mut facts: Vec<CoverageFact> = Vec::new();

    // Region ids and hit records name files repo-relative (SPEC §3.2); the
    // hook passes the host's path, which may be absolute. An edit outside
    // the root names nothing in this project and drops out here.
    let edited: Vec<(String, &EditedFile)> = input
        .edited
        .iter()
        .filter_map(|e| repo_relative_path(input.root, &e.path).map(|rel| (rel, e)))
        .collect();

    if wants("head_revision")
        && let Some(sha) = &input.head_sha
    {
        facts.push(fact("head_revision", vec![sha.clone()]));
    }

    let index = load_index(input.root);
    if wants("coverage_revision")
        && let Some(idx) = &index
    {
        facts.push(fact("coverage_revision", vec![idx.revision.clone()]));
    }
    let revision_stale = matches!(
        (&index, &input.head_sha),
        (Some(idx), Some(sha)) if idx.revision != *sha
    );
    // A store imported before region ids were qualified per site
    // (SPEC-coverage-evidence §3.2) carries leaf-name ids (`fn:new`) that
    // name no single site. It is stale evidence whatever its revision: its
    // hits are never joined (below), and rules see `coverage_stale` so the
    // remedy — re-collect — is visible rather than a silent mis-join.
    let wants_hits = wants("region_without_dynamic_evidence")
        || wants("test_hits_region")
        || wants("test_hits_branch");
    let hits = if wants_hits || wants("coverage_stale") {
        load_hits(input.root)?
    } else {
        Vec::new()
    };
    let legacy_ids = hits.iter().any(|h| !is_qualified_region_id(&h.region));
    if wants("coverage_stale") && (revision_stale || legacy_ids) {
        facts.push(fact("coverage_stale", Vec::new()));
    }
    let hits: Vec<_> = hits
        .into_iter()
        .filter(|h| is_qualified_region_id(&h.region))
        .collect();

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

        for (rel, edit) in &edited {
            let regions = changed_regions(rel, edit.old.unwrap_or(""), edit.new)?;
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
            let hit_regions: HashSet<&str> = hits.iter().map(|h| h.region.as_str()).collect();
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
        let edited_paths: HashSet<&str> = edited.iter().map(|(rel, _)| rel.as_str()).collect();
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
    Ok(facts)
}

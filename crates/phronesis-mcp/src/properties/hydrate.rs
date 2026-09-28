//! Demand-gated property-fact production at hook fire — the `coverage/hydrate.rs`
//! pattern applied to the property ontology (SPEC-property-ontology.md §2).

use std::collections::HashSet;
use std::path::Path;

use crate::properties::allowlist::{AllowlistFile, PrincipalKind, approval_kind};
use crate::properties::store::{
    LEGACY_RESULTS_FORMAT, Property, PropertyStoreError, ResultRecord, load_properties,
    load_results,
};

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
    "agent_verification_result",
    "result_principal",
    "result_revision",
    "result_tier",
    "unbound_evidence",
    "stale_evidence",
    "property_obligation",
    "store_corrupt",
];

/// The confinement tiers a bound result may name (`ConfinementTier`,
/// snake_case). `refused` never produces a result.
const RESULT_TIERS: &[&str] = &["devcontainer", "sandbox_exec", "raw"];

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PropertyFact {
    pub predicate: String,
    pub args: Vec<String>,
}

pub struct EditedFile<'a> {
    pub path: String,
    pub old: Option<&'a str>,
    pub new: &'a str,
    /// The file changed but its content could not be read (or was over the
    /// read cap): it maps to one coarse whole-file region, never to none.
    pub whole_file: bool,
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

/// Region ids changed by this event. Edited paths are made repo-relative
/// first (hooks pass Claude Code's absolute `file_path`); an edit outside
/// the root changes no region of this project.
fn changed_region_ids(input: &PropertyHydrationInput) -> HashSet<String> {
    let mut changed = HashSet::new();
    for edit in &input.edited {
        let Some(rel) = crate::coverage::region_map::repo_relative_path(input.root, &edit.path)
        else {
            continue;
        };
        let regions = if edit.whole_file {
            Ok(crate::coverage::region_map::ChangedRegions::whole_file(
                &rel,
            ))
        } else {
            crate::coverage::region_map::changed_regions(&rel, edit.old.unwrap_or(""), edit.new)
        };
        if let Ok(regions) = regions {
            changed.extend(regions.all().cloned());
        }
    }
    changed
}

/// Whether a `depends_on` entry names a region changed in this event.
/// Entries are region ids (SPEC-coverage-evidence §3.2); an entry written
/// before ids were qualified per site matches every changed site with its
/// leaf name, so an unregenerated property store over-reports obligations
/// instead of never matching.
fn depends_on_changed(reference: &str, changed: &HashSet<String>) -> bool {
    changed.contains(reference)
        || changed
            .iter()
            .any(|region| crate::coverage::region_map::reference_matches(reference, region))
}

/// Hydration result: the facts, plus the store corruption (if any) so the
/// hook can report it on stderr whether or not a rule demanded
/// `store_corrupt` — the `coverage::hydrate::Hydration` shape.
#[derive(Debug, Default)]
pub struct PropertyHydration {
    pub facts: Vec<PropertyFact>,
    pub store_corrupt: Option<PropertyStoreError>,
}

fn is_hex_of_len(value: &str, len: usize) -> bool {
    value.len() == len && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Whether a result record is evidence (D9). `Ok(())` is a bound result;
/// `Err(reason)` names the first missing or mismatched binding, and the
/// record hydrates as `unbound_evidence(property, verifier, reason)`:
///
/// - `legacy_record` — a v1 record (no tier, no artifact hash);
/// - `missing_revision` / `invalid_revision` — not a 40-hex commit id;
/// - `missing_tier` / `invalid_tier` — not a confinement tier that runs;
/// - `missing_artifact` / `invalid_artifact` — not a SHA-256 digest;
/// - `draft_template` — rendered from `verification/template-drafts/`
///   (`phr-mcp verify run --allow-drafts`): dev evidence, never a
///   verification; `invalid_template_origin` — an origin outside the set;
/// - `unknown_property` — no curated property with that id;
/// - `no_encoding` — the property has no encoding for that verifier;
/// - `allowlist_unreadable` / `artifact_not_approved` — the artifact hash is
///   not an approved artifact for that property (S3 allowlist).
///
/// A bound record carries the approving entry's principal kind (D10):
/// `Human` wins when both kinds approve the same bytes.
fn binding(
    r: &ResultRecord,
    properties: &[Property],
    approved: &Result<AllowlistFile, ()>,
) -> Result<PrincipalKind, &'static str> {
    if r.v == LEGACY_RESULTS_FORMAT {
        return Err("legacy_record");
    }
    if r.revision.is_empty() {
        return Err("missing_revision");
    }
    if !is_hex_of_len(&r.revision, 40) {
        return Err("invalid_revision");
    }
    match r.tier.as_deref() {
        None | Some("") => return Err("missing_tier"),
        Some(tier) if !RESULT_TIERS.contains(&tier) => return Err("invalid_tier"),
        Some(_) => {}
    }
    let artifact = match r.artifact_sha256.as_deref() {
        None | Some("") => return Err("missing_artifact"),
        Some(a) if !(is_hex_of_len(a, 64) && a.bytes().all(|b| !b.is_ascii_uppercase())) => {
            return Err("invalid_artifact");
        }
        Some(a) => a,
    };
    match r.template_origin.as_deref() {
        None | Some("templates") => {}
        Some("template_drafts") => return Err("draft_template"),
        Some(_) => return Err("invalid_template_origin"),
    }
    let Some(property) = properties.iter().find(|p| p.id == r.property) else {
        return Err("unknown_property");
    };
    if !property.encodings.iter().any(|e| e.verifier == r.verifier) {
        return Err("no_encoding");
    }
    let Ok(approved) = approved else {
        return Err("allowlist_unreadable");
    };
    approval_kind(approved, artifact, &r.property).ok_or("artifact_not_approved")
}

/// One bound result and the principal kind that approved its artifact.
#[derive(Debug, Clone)]
pub struct BoundResult {
    pub record: ResultRecord,
    pub principal: PrincipalKind,
}

/// Every bound result in the sidecar (D9 binding, D10 principal kind) —
/// the evidence `set_property_status` checks before `verified` /
/// `agent_verified`. Fails when either store is corrupt: a transition must
/// not be justified by evidence that cannot be read.
pub fn bound_results(root: &Path) -> Result<Vec<BoundResult>, PropertyStoreError> {
    let properties = load_properties(root)?;
    let results = load_results(root)?;
    let approved = load_approvals(root, results.is_empty());
    Ok(results
        .into_iter()
        .filter_map(|r| {
            let principal = binding(&r, &properties, &approved).ok()?;
            Some(BoundResult {
                record: r,
                principal,
            })
        })
        .collect())
}

/// The allowlist, read only when there is a result to bind.
fn load_approvals(root: &Path, no_results: bool) -> Result<AllowlistFile, ()> {
    if no_results {
        return Ok(AllowlistFile {
            version: crate::properties::allowlist::ALLOWLIST_FORMAT,
            entries: Vec::new(),
        });
    }
    crate::properties::allowlist::load(root).map_err(|_| ())
}

pub fn facts_for_event(input: &PropertyHydrationInput) -> anyhow::Result<Vec<PropertyFact>> {
    hydrate(input).map(|h| h.facts)
}

/// Derive this event's property facts. A corrupt store (properties.json or
/// the results sidecar) is not an error: it yields
/// `store_corrupt(properties, <reason>)` (when demanded), and everything
/// else is derived as if the corrupt file held nothing — unreadable results
/// are no evidence, so obligations still fire (D8).
pub fn hydrate(input: &PropertyHydrationInput) -> anyhow::Result<PropertyHydration> {
    let wants = |rel: &str| input.rule_relations.contains(rel);

    if !RELATIONS.iter().any(|r| wants(r)) {
        return Ok(PropertyHydration::default());
    }

    let mut facts: Vec<PropertyFact> = Vec::new();

    let mut store_corrupt: Option<PropertyStoreError> = None;
    let properties = load_properties(input.root).unwrap_or_else(|e| {
        store_corrupt = Some(e);
        Vec::new()
    });
    let results = load_results(input.root).unwrap_or_else(|e| {
        store_corrupt.get_or_insert(e);
        Vec::new()
    });
    if wants("store_corrupt")
        && let Some(c) = &store_corrupt
    {
        facts.push(fact(
            "store_corrupt",
            vec!["properties".to_string(), c.reason().to_string()],
        ));
    }

    // D9: split results into bound evidence and unbound records. The
    // allowlist is read only when there is a result to bind.
    let approved = load_approvals(input.root, results.is_empty());
    let mut bound: Vec<(&ResultRecord, PrincipalKind)> = Vec::new();
    for r in &results {
        match binding(r, &properties, &approved) {
            Ok(principal) => bound.push((r, principal)),
            Err(reason) => {
                if wants("unbound_evidence") {
                    facts.push(fact(
                        "unbound_evidence",
                        vec![r.property.clone(), r.verifier.clone(), reason.to_string()],
                    ));
                }
            }
        }
    }
    let results = bound;

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

    // Bound results at their recorded revision (spec §2: verification_result,
    // result_revision, result_tier — provenance via Fact.source, not RETE
    // args). Only bound records reach here. D10: a human-approved result is
    // `verification_result`; an agent-quorum-approved one is
    // `agent_verification_result` and never `verification_result`.
    for (r, principal) in &results {
        let relation = match principal {
            PrincipalKind::Human => "verification_result",
            PrincipalKind::AgentQuorum => "agent_verification_result",
        };
        if wants(relation) {
            facts.push(fact(
                relation,
                vec![r.property.clone(), r.verifier.clone(), r.status.clone()],
            ));
        }
        if wants("result_principal") {
            facts.push(fact(
                "result_principal",
                vec![
                    r.property.clone(),
                    r.verifier.clone(),
                    principal.as_str().to_string(),
                ],
            ));
        }
        if wants("result_revision") {
            facts.push(fact(
                "result_revision",
                vec![r.property.clone(), r.verifier.clone(), r.revision.clone()],
            ));
        }
        if wants("result_tier")
            && let Some(tier) = &r.tier
        {
            facts.push(fact(
                "result_tier",
                vec![r.property.clone(), r.verifier.clone(), tier.clone()],
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
        changed.extend(changed_region_ids(input));
        let mut stale: Vec<PropertyFact> = Vec::new();
        // An unknown HEAD cannot prove staleness (the coverage rule); the
        // obligation below stays conservative instead.
        for (r, _) in results.iter().filter(|_| input.head_sha.is_some()) {
            let depends_on_changed = properties
                .iter()
                .filter(|p| p.id == r.property)
                .flat_map(|p| p.depends_on.iter())
                .any(|region| depends_on_changed(region, &changed));
            if depends_on_changed && !at_head(r, input.head_sha.as_deref()) {
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
        changed.extend(changed_region_ids(input));
        for p in &properties {
            if !p.generation_eligible() {
                continue;
            }
            let depends_on_changed = p
                .depends_on
                .iter()
                .any(|region| depends_on_changed(region, &changed));
            // Only a human-approved result discharges the obligation (D10):
            // agent-quorum evidence never satisfies what human evidence does.
            let has_result = results.iter().any(|(r, principal)| {
                *principal == PrincipalKind::Human
                    && r.property == p.id
                    && at_head(r, input.head_sha.as_deref())
            });
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
    Ok(PropertyHydration {
        facts,
        store_corrupt,
    })
}

/// A bound result ran against the current HEAD. An unknown HEAD matches
/// nothing: no result can be shown current, so none suppresses an obligation.
fn at_head(r: &ResultRecord, head_sha: Option<&str>) -> bool {
    head_sha.is_some_and(|head| r.revision.eq_ignore_ascii_case(head))
}

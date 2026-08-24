//! Evidence gathering: the borrowed edge index, per-function grouping, and
//! byte-offset → line resolution.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::graph::model::Edge;
use crate::graph::ownership::{
    ANALYSIS_CAPABILITIES, AWAIT_SITE, CLONE_BEFORE_AWAIT, CLONE_SITE, FILTER_BEFORE_CLONE,
    FILTER_SITE, LOCK_SCOPE_ENDS_BEFORE_AWAIT, MUTATION_SITE, OWNERSHIP_ANALYSIS_STATUS,
    OWNERSHIP_EVIDENCE, OWNERSHIP_SITE, OWNERSHIP_SITE_IN_FUNCTION, OWNERSHIP_SITE_SPAN,
    READ_BEFORE_MUTATION, RESOLVED_TYPE, SYNC_LOCK_SITE,
};
use crate::security;

use super::render::describe_site;
use super::{
    AnalysisStatus, EvidenceRef, FunctionEvidence, OwnershipReport, Relationship, SiteEvidence,
    SourceLocation, SupportRef,
};

/// Relations this surface explains, with the words for their two site
/// arguments and the limit that must always accompany them.
const RELATIONSHIPS: &[(&str, &str, &str, &str)] = &[
    (
        FILTER_BEFORE_CLONE,
        "filter site",
        "clone site",
        "a shared expression chain is structural evidence; it is not runtime cost evidence, and it does not observe UFCS iterator calls",
    ),
    (
        CLONE_BEFORE_AWAIT,
        "clone site",
        "await site",
        "lexical ordering only; it does not establish that the cloned value is live across the suspension point",
    ),
    (
        READ_BEFORE_MUTATION,
        "read site",
        "mutation site",
        "lexical ordering over a syntactic root place; aliasing is not analysed and an early return between the two sites is invisible",
    ),
    (
        LOCK_SCOPE_ENDS_BEFORE_AWAIT,
        "lock site",
        "await site",
        "lexical scope is not general control-flow or borrow-liveness proof",
    ),
];

// ── indexing ────────────────────────────────────────────────────────────

/// Borrowed lookup tables over the edge set. Built once per query; every map
/// is ordered so two runs over the same graph render byte-identically.
pub(super) struct Index<'a> {
    declared: BTreeSet<&'a str>,
    span: BTreeMap<&'a str, (&'a str, &'a str, &'a str)>,
    kind: BTreeMap<&'a str, (&'static str, &'a str, &'a str)>,
    evidence: BTreeMap<&'a str, BTreeSet<(&'a str, &'a str)>>,
    resolved_types: BTreeMap<&'a str, BTreeSet<&'a str>>,
    status: BTreeMap<&'a str, BTreeSet<(&'a str, &'a str, &'a str)>>,
    pub(super) sites_by_function: BTreeMap<&'a str, BTreeSet<&'a str>>,
    pub(super) relationships: BTreeMap<&'a str, BTreeSet<(&'a str, &'a str, &'a str)>>,
}

fn arg(edge: &Edge, i: usize) -> Option<&str> {
    edge.a.get(i).map(String::as_str)
}

impl<'a> Index<'a> {
    pub(super) fn build(edges: &'a [Edge]) -> Self {
        let mut index = Index {
            declared: BTreeSet::new(),
            span: BTreeMap::new(),
            kind: BTreeMap::new(),
            evidence: BTreeMap::new(),
            resolved_types: BTreeMap::new(),
            status: BTreeMap::new(),
            sites_by_function: BTreeMap::new(),
            relationships: BTreeMap::new(),
        };
        for edge in edges {
            match edge.p.as_str() {
                OWNERSHIP_SITE => {
                    if let Some(site) = arg(edge, 0) {
                        index.declared.insert(site);
                    }
                }
                OWNERSHIP_SITE_IN_FUNCTION => {
                    if let (Some(site), Some(function)) = (arg(edge, 0), arg(edge, 1)) {
                        index
                            .sites_by_function
                            .entry(function)
                            .or_default()
                            .insert(site);
                    }
                }
                OWNERSHIP_SITE_SPAN => {
                    if let (Some(site), Some(file), Some(start), Some(end)) =
                        (arg(edge, 0), arg(edge, 1), arg(edge, 2), arg(edge, 3))
                    {
                        index.span.insert(site, (file, start, end));
                    }
                }
                CLONE_SITE | MUTATION_SITE | SYNC_LOCK_SITE => {
                    if let (Some(site), Some(operation), Some(operand)) =
                        (arg(edge, 0), arg(edge, 1), arg(edge, 2))
                    {
                        index
                            .kind
                            .insert(site, (site_kind(&edge.p), operation, operand));
                    }
                }
                FILTER_SITE => {
                    if let (Some(site), Some(operand)) = (arg(edge, 0), arg(edge, 1)) {
                        index.kind.insert(site, ("filter", "filter", operand));
                    }
                }
                AWAIT_SITE => {
                    if let Some(site) = arg(edge, 0) {
                        index.kind.insert(site, ("await", "await", ""));
                    }
                }
                OWNERSHIP_EVIDENCE => {
                    if let (Some(subject), Some(level), Some(provider)) =
                        (arg(edge, 0), arg(edge, 1), arg(edge, 2))
                    {
                        index
                            .evidence
                            .entry(subject)
                            .or_default()
                            .insert((level, provider));
                    }
                }
                RESOLVED_TYPE => {
                    if let (Some(site), Some(ty)) = (arg(edge, 0), arg(edge, 1)) {
                        index.resolved_types.entry(site).or_default().insert(ty);
                    }
                }
                OWNERSHIP_ANALYSIS_STATUS => {
                    if let (Some(subject), Some(capability), Some(status), Some(reason)) =
                        (arg(edge, 0), arg(edge, 1), arg(edge, 2), arg(edge, 3))
                    {
                        index
                            .status
                            .entry(subject)
                            .or_default()
                            .insert((capability, status, reason));
                    }
                }
                other => {
                    if RELATIONSHIPS.iter().any(|(name, ..)| *name == other)
                        && let (Some(function), Some(left), Some(right)) =
                            (arg(edge, 0), arg(edge, 1), arg(edge, 2))
                    {
                        // `other` borrows the edge, so the relation name is
                        // stored alongside its sites rather than as a key.
                        index
                            .relationships
                            .entry(function)
                            .or_default()
                            .insert((other, left, right));
                    }
                }
            }
        }
        index
    }

    fn evidence_of(&self, subject: &str) -> Vec<EvidenceRef> {
        self.evidence
            .get(subject)
            .map(|set| {
                set.iter()
                    .map(|(level, provider)| EvidenceRef {
                        level: (*level).to_string(),
                        provider: (*provider).to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn status_of(&self, subject: &str) -> Vec<AnalysisStatus> {
        self.status
            .get(subject)
            .map(|set| {
                set.iter()
                    .map(|(capability, status, reason)| AnalysisStatus {
                        subject: subject.to_string(),
                        capability: (*capability).to_string(),
                        status: (*status).to_string(),
                        reason: (*reason).to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether the graph knows anything at all about this site ID.
    fn knows(&self, site: &str) -> bool {
        self.declared.contains(site) || self.span.contains_key(site) || self.kind.contains_key(site)
    }

    fn site(&self, site: &str) -> SiteEvidence {
        let (kind, operation, operand) =
            self.kind
                .get(site)
                .copied()
                .unwrap_or(("unclassified", "", ""));
        let location = self
            .span
            .get(site)
            .map(|(file, start, end)| SourceLocation {
                file: (*file).to_string(),
                start_byte: (*start).to_string(),
                end_byte: (*end).to_string(),
                line: None,
            });
        let evidence = self.evidence_of(site);
        let resolved_types = self
            .resolved_types
            .get(site)
            .map(|set| set.iter().map(|t| (*t).to_string()).collect())
            .unwrap_or_default();

        let mut notes = Vec::new();
        if location.is_none() {
            notes.push(
                "no source span indexed for this site — the location is unattributed".to_string(),
            );
        }
        if evidence.is_empty() {
            notes.push(
                "no evidence level or provider recorded for this site — treat it as unattributed, not as confirmed"
                    .to_string(),
            );
        }
        if kind == "clone" && operation == "collect" {
            notes.push(
                "a `collect` was observed; that does not establish that ownership was produced"
                    .to_string(),
            );
        }
        if kind == "sync_lock" && operand.is_empty() {
            notes.push(
                "the guard is an unbound temporary, so no lock-scope conclusion is drawn"
                    .to_string(),
            );
        }

        SiteEvidence {
            site: site.to_string(),
            kind: kind.to_string(),
            operation: operation.to_string(),
            operand: operand.to_string(),
            location,
            evidence,
            resolved_types,
            analysis: self.status_of(site),
            notes,
        }
    }
}

fn site_kind(relation: &str) -> &'static str {
    match relation {
        CLONE_SITE => "clone",
        MUTATION_SITE => "mutation",
        SYNC_LOCK_SITE => "sync_lock",
        FILTER_SITE => "filter",
        AWAIT_SITE => "await",
        _ => "unclassified",
    }
}

pub(super) fn function_evidence(index: &Index<'_>, function: &str) -> FunctionEvidence {
    let sites: Vec<SiteEvidence> = {
        let mut sites: Vec<SiteEvidence> = index
            .sites_by_function
            .get(function)
            .map(|set| set.iter().map(|site| index.site(site)).collect())
            .unwrap_or_default();
        sites.sort_by_key(span_order);
        sites
    };

    let files: Vec<String> = {
        let mut files: Vec<String> = sites
            .iter()
            .filter_map(|site| site.location.as_ref().map(|l| l.file.clone()))
            .collect::<BTreeSet<String>>()
            .into_iter()
            .collect();
        files.sort();
        files
    };

    let relationships: Vec<Relationship> = index
        .relationships
        .get(function)
        .map(|set| {
            set.iter()
                .map(|(relation, left, right)| relationship(index, relation, left, right))
                .collect()
        })
        .unwrap_or_default();

    // Capability results attach to the function, but the extractor also
    // records file-scoped ones (a site cap, a stale compiler generation), and
    // hiding those would make a partial analysis look complete.
    let analysis = {
        let mut analysis = index.status_of(function);
        for file in &files {
            analysis.extend(index.status_of(file));
        }
        analysis.sort();
        analysis.dedup();
        analysis
    };

    let analysis_not_reported = capabilities_not_reported(&analysis);

    let limits = limits_for(&sites, &relationships, &analysis, &analysis_not_reported);

    FunctionEvidence {
        function: function.to_string(),
        files,
        sites,
        relationships,
        analysis,
        analysis_not_reported,
        limits,
    }
}

/// Capabilities with no recorded status among `analysis`.
fn capabilities_not_reported(analysis: &[AnalysisStatus]) -> Vec<String> {
    let reported: BTreeSet<&str> = analysis.iter().map(|s| s.capability.as_str()).collect();
    ANALYSIS_CAPABILITIES
        .iter()
        .filter(|capability| !reported.contains(**capability))
        .map(|capability| (*capability).to_string())
        .collect()
}

/// Sort key: file, then numeric start byte, then site ID. Sorting the raw
/// decimal string would put byte 100 before byte 99.
fn span_order(site: &SiteEvidence) -> (String, u64, String) {
    match &site.location {
        Some(location) => (
            location.file.clone(),
            location.start_byte.parse().unwrap_or(u64::MAX),
            site.site.clone(),
        ),
        None => (String::new(), u64::MAX, site.site.clone()),
    }
}

fn relationship(index: &Index<'_>, relation: &str, left: &str, right: &str) -> Relationship {
    let (left_role, right_role, limit) = RELATIONSHIPS
        .iter()
        .find(|(name, ..)| *name == relation)
        .map(|(_, l, r, limit)| (*l, *r, *limit))
        .unwrap_or(("site", "site", "structural evidence with an unstated limit"));

    let mut evidence: BTreeSet<EvidenceRef> = BTreeSet::new();
    let mut supported_by = Vec::new();
    for (role, site) in [(left_role, left), (right_role, right)] {
        let resolved = index.knows(site);
        let detail = index.site(site);
        evidence.extend(detail.evidence.iter().cloned());
        supported_by.push(SupportRef {
            role: role.to_string(),
            site: site.to_string(),
            summary: if resolved {
                describe_site(&detail)
            } else {
                "no indexed evidence for this site — unattributed".to_string()
            },
            resolved,
        });
    }

    Relationship {
        relation: relation.to_string(),
        supported_by,
        evidence: evidence.into_iter().collect(),
        limit: limit.to_string(),
    }
}

fn limits_for(
    sites: &[SiteEvidence],
    relationships: &[Relationship],
    analysis: &[AnalysisStatus],
    not_reported: &[String],
) -> Vec<String> {
    let mut limits = vec![
        "Ownership relations are observations with stated limits, not verdicts about correctness or cost.".to_string(),
    ];
    let has_relation = |name: &str| relationships.iter().any(|r| r.relation == name);

    if sites
        .iter()
        .any(|site| site.kind == "clone" && site.operation == "collect")
    {
        limits.push(
            "A `collect` site records that a collect was observed; whether it produced ownership is a type-level claim this evidence does not make."
                .to_string(),
        );
    }
    if has_relation(READ_BEFORE_MUTATION) {
        limits.push(
            "`read_before_mutation` groups by a syntactic root place, so every place hanging off `self` is over-grouped."
                .to_string(),
        );
    }
    if has_relation(LOCK_SCOPE_ENDS_BEFORE_AWAIT) {
        limits.push(
            "`lock_scope_ends_before_await` matches an explicit `drop(guard)` by name; a locally shadowed `drop` would make it wrong."
                .to_string(),
        );
    }
    if sites
        .iter()
        .any(|site| site.kind == "filter" || site.kind == "clone")
    {
        limits.push(
            "`filter_before_clone` does not observe UFCS iterator calls, so its absence is an incompleteness, not a claim of cleanliness."
                .to_string(),
        );
    }
    if sites.iter().any(|site| site.kind == "sync_lock")
        && !has_relation(LOCK_SCOPE_ENDS_BEFORE_AWAIT)
    {
        limits.push(
            "No lock-scope conclusion was drawn for at least one lock site; `lock_scope_may_cross_await` is never emitted from AST evidence."
                .to_string(),
        );
    }
    if !not_reported.is_empty() {
        limits.push(format!(
            "No provider reported {}; absence of a capability result is not a clean result.",
            not_reported.join(", ")
        ));
    }
    if analysis.iter().any(|s| {
        matches!(
            s.status.as_str(),
            "unavailable" | "partial" | "failed" | "stale"
        )
    }) {
        limits.push(
            "At least one capability is partial, stale, failed, or unavailable; AST observations are not upgraded by its absence."
                .to_string(),
        );
    }
    limits
}

// ── line resolution ─────────────────────────────────────────────────────

/// Turn byte offsets into 1-based line numbers by reading each referenced
/// file once.
///
/// Best-effort by design: an unreadable file or an offset past EOF leaves
/// `line` as `None` and the caller renders the byte span instead. Both are
/// real states — the graph can outlive an edit to the file it describes.
pub fn resolve_lines(root: &Path, report: &mut OwnershipReport) {
    let mut cache: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut resolved: BTreeMap<(String, String), Option<usize>> = BTreeMap::new();

    for function in &mut report.functions {
        for site in &mut function.sites {
            let Some(location) = &mut site.location else {
                continue;
            };
            location.line = line_for(root, &mut cache, &mut resolved, location);
        }
        // The relationship summaries were rendered from the same sites before
        // lines existed; re-render so both places agree.
        refresh_support_summaries(function);
    }
}

/// Line of `location.start_byte`, memoised per `(file, start_byte)` and
/// reading each file at most once.
fn line_for(
    root: &Path,
    cache: &mut BTreeMap<String, Option<String>>,
    resolved: &mut BTreeMap<(String, String), Option<usize>>,
    location: &SourceLocation,
) -> Option<usize> {
    let key = (location.file.clone(), location.start_byte.clone());
    match resolved.get(&key) {
        Some(line) => *line,
        None => {
            let body = cache
                .entry(location.file.clone())
                .or_insert_with(|| read_tracked(root, &location.file));
            let line = body
                .as_deref()
                .and_then(|body| line_of(body, &location.start_byte));
            resolved.insert(key, line);
            line
        }
    }
}

fn refresh_support_summaries(function: &mut FunctionEvidence) {
    let by_site: BTreeMap<&str, &SiteEvidence> = function
        .sites
        .iter()
        .map(|site| (site.site.as_str(), site))
        .collect();
    for relation in &mut function.relationships {
        for support in &mut relation.supported_by {
            if let Some(site) = by_site.get(support.site.as_str()) {
                support.summary = describe_site(site);
            }
        }
    }
}

fn read_tracked(root: &Path, file: &str) -> Option<String> {
    let path = security::resolve_safe_path(file, root).ok()?;
    security::read_file_capped(&path).ok()
}

pub(super) fn line_of(body: &str, start_byte: &str) -> Option<usize> {
    let offset: usize = start_byte.parse().ok()?;
    if offset > body.len() {
        return None;
    }
    Some(
        1 + body.as_bytes()[..offset]
            .iter()
            .filter(|b| **b == b'\n')
            .count(),
    )
}

//! Grouped ownership explanation query (spec §10, Addendum A.1/A.4).
//!
//! Backs both `phr-mcp graph ownership <function-id-or-glob>` and the
//! `query_ownership_evidence` MCP tool. Both surfaces call [`load`] and one of
//! [`render_table`] / [`render_json`]; neither shapes a result of its own.
//! That is deliberate — `graph query` grew two subtly different JSON envelopes
//! (the CLI reports `{total, returned, results}`, MCP adds `truncated`)
//! because each surface built its own, and §13.2 requires these two to return
//! *identical* grouped evidence.
//!
//! `graph::query::query` returns a flat `Vec<&Edge>` with no grouping and no
//! join, so the grouping here is a separate pass over the edge set: sites are
//! collected per function, then each derived relationship is resolved back to
//! its supporting sites, and each site back to its span, evidence level, and
//! provider. That traversal is the Addendum A.1 lineage requirement.
//!
//! Three absences are reported as three different things, because
//! `store::load` returns an empty vector for a missing file and so cannot
//! distinguish them by itself:
//!
//! - [`OwnershipState::Disabled`] — `[ownership.rust]` is absent or off.
//! - [`OwnershipState::NoGraph`] — no graph has been built yet.
//! - [`OwnershipState::NoMatch`] — a graph exists and nothing matched.
//!
//! The last one renders as "no indexed ownership evidence found", never as
//! evidence that the matched code is clean (§10, Addendum A.4).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use serde::Serialize;

use crate::graph::model::Edge;
use crate::graph::ownership::config;
use crate::graph::ownership::{
    ANALYSIS_CAPABILITIES, AWAIT_SITE, CLONE_BEFORE_AWAIT, CLONE_SITE, FILTER_BEFORE_CLONE,
    FILTER_SITE, LOCK_SCOPE_ENDS_BEFORE_AWAIT, MUTATION_SITE, OWNERSHIP_ANALYSIS_STATUS,
    OWNERSHIP_EVIDENCE, OWNERSHIP_SITE, OWNERSHIP_SITE_IN_FUNCTION, OWNERSHIP_SITE_SPAN,
    READ_BEFORE_MUTATION, RESOLVED_TYPE, SYNC_LOCK_SITE,
};
use crate::graph::query::glob_matches;
use crate::graph::store;
use crate::security;

/// Functions rendered when the caller supplies no limit.
pub const DEFAULT_FUNCTION_LIMIT: usize = 20;

/// Longest operand/place/guard text interpolated into a table row. The graph
/// caps the stored value at 240 bytes (D7), which is still far too wide for a
/// terminal column.
const TABLE_TEXT_WIDTH: usize = 72;

/// Why a report carries no functions — or that it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnershipState {
    /// `[ownership.rust]` is missing, `enabled = false`, or malformed.
    Disabled,
    /// `.phronesis/graph.jsonl` does not exist.
    NoGraph,
    /// A graph exists and ownership is enabled; the pattern matched nothing.
    NoMatch,
    /// At least one function matched.
    Matched,
}

impl OwnershipState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::NoGraph => "no_graph",
            Self::NoMatch => "no_match",
            Self::Matched => "matched",
        }
    }
}

/// What [`build`] could not work out for itself: whether the feature is on,
/// and whether a graph file exists at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Availability {
    pub ownership_enabled: bool,
    pub graph_present: bool,
}

/// A source span, with a line number when the file could still be read and
/// the offset still lands inside it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceLocation {
    pub file: String,
    /// Decimal byte offsets, verbatim from the edge — every graph argument is
    /// a `String`, and a malformed offset is shown rather than zeroed.
    pub start_byte: String,
    pub end_byte: String,
    /// 1-based line of `start_byte`, or `None` when the file could not be
    /// read or has since shrunk past the offset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
}

/// One `ownership_evidence(subject, level, provider)` fact.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct EvidenceRef {
    pub level: String,
    pub provider: String,
}

/// One `ownership_analysis_status(subject, capability, status, reason)` fact.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct AnalysisStatus {
    pub subject: String,
    pub capability: String,
    pub status: String,
    pub reason: String,
}

/// One observed site, with everything needed to judge how strong it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SiteEvidence {
    pub site: String,
    /// `clone` | `filter` | `await` | `mutation` | `sync_lock` | `unclassified`.
    pub kind: String,
    /// The literal observed operation name (`clone`, `cloned`, `collect`,
    /// `lock`, `push`, …). Never mapped to a type or a cost.
    pub operation: String,
    /// Operand, place, or guard text. Empty when the relation carries none
    /// (an await) or when the lock guard was an unbound temporary.
    pub operand: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<SourceLocation>,
    pub evidence: Vec<EvidenceRef>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub resolved_types: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub analysis: Vec<AnalysisStatus>,
    /// Gaps in this site's own lineage, rendered rather than hidden.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// One supporting site of a relationship, resolved back through the graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SupportRef {
    /// Which argument position this site filled, in words.
    pub role: String,
    pub site: String,
    /// Single-line human summary with its source location.
    pub summary: String,
    /// False when the graph holds no site edges for this ID at all. Addendum
    /// A.4 requires an empty evidence path to render as unattributed rather
    /// than vanish.
    pub resolved: bool,
}

/// A derived ordering/scope relationship and the sites that support it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Relationship {
    pub relation: String,
    pub supported_by: Vec<SupportRef>,
    /// Union of the supporting sites' evidence. Never stronger than what the
    /// sites themselves carry — a relationship has no evidence of its own.
    pub evidence: Vec<EvidenceRef>,
    pub limit: String,
}

/// Every ownership fact indexed for one function.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FunctionEvidence {
    pub function: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    pub sites: Vec<SiteEvidence>,
    pub relationships: Vec<Relationship>,
    /// Capability results for this function and for every file it has sites
    /// in — including `partial`, `stale`, `failed`, and `unavailable`.
    pub analysis: Vec<AnalysisStatus>,
    /// Capabilities with no recorded status at all. Rendering this is not
    /// optional: a silently missing capability makes AST-only evidence look
    /// like it was corroborated (Addendum A.4).
    pub analysis_not_reported: Vec<String>,
    pub limits: Vec<String>,
}

/// The whole answer, shared verbatim by the CLI and the MCP tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OwnershipReport {
    pub pattern: String,
    pub state: OwnershipState,
    /// Prose explanation of `state`. Deliberately free of absolute paths so
    /// the two surfaces cannot diverge on how the project root was spelled.
    pub message: String,
    /// Set when `.phronesis/graph.toml` exists but its `[ownership.rust]`
    /// section could not be parsed — "disabled" and "misconfigured" are not
    /// the same answer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_error: Option<String>,
    pub matched_functions: usize,
    pub returned_functions: usize,
    pub functions: Vec<FunctionEvidence>,
}

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

/// Read config and graph, group the evidence, and resolve line numbers.
///
/// A missing graph file is not an error here — it is [`OwnershipState::NoGraph`],
/// which is a different answer from "nothing matched".
pub fn load(root: &Path, function_pattern: &str, limit: usize) -> std::io::Result<OwnershipReport> {
    let (enabled, config_error) = match config::load(root) {
        Ok(config) => (config.enabled, None),
        Err(error) => (false, Some(error.to_string())),
    };
    let graph_path = store::graph_path(root);
    let graph_present = graph_path.exists();
    let edges = if graph_present {
        store::load(&graph_path)?
    } else {
        Vec::new()
    };

    let mut report = build(
        &edges,
        function_pattern,
        limit,
        Availability {
            ownership_enabled: enabled,
            graph_present,
        },
    );
    report.config_error = config_error;
    if let Some(error) = &report.config_error {
        report.message = format!(
            "Ownership evidence is disabled: {error}\nFix the section in {}, then run `phr-mcp graph rebuild`.",
            config::CONFIG_REL_PATH
        );
    }
    resolve_lines(root, &mut report);
    Ok(report)
}

/// Group `edges` into the rendered report. Pure: `location.line` is always
/// `None` here, because a line number needs the source file. [`load`] fills
/// it in afterwards via [`resolve_lines`].
pub fn build(
    edges: &[Edge],
    function_pattern: &str,
    limit: usize,
    availability: Availability,
) -> OwnershipReport {
    let empty = |state: OwnershipState, message: String| OwnershipReport {
        pattern: function_pattern.to_string(),
        state,
        message,
        config_error: None,
        matched_functions: 0,
        returned_functions: 0,
        functions: Vec::new(),
    };

    if !availability.ownership_enabled {
        return empty(OwnershipState::Disabled, disabled_message());
    }
    if !availability.graph_present {
        return empty(OwnershipState::NoGraph, no_graph_message());
    }

    let index = Index::build(edges);
    let mut names: BTreeSet<&str> = BTreeSet::new();
    for name in index.sites_by_function.keys() {
        names.insert(name);
    }
    for name in index.relationships.keys() {
        names.insert(name);
    }
    let matched: Vec<&str> = names
        .into_iter()
        .filter(|name| glob_matches(function_pattern, name))
        .collect();

    if matched.is_empty() {
        return empty(OwnershipState::NoMatch, no_match_message(function_pattern));
    }

    let matched_functions = matched.len();
    let shown = if limit == 0 {
        matched.as_slice()
    } else {
        &matched[..matched.len().min(limit)]
    };
    let functions: Vec<FunctionEvidence> = shown
        .iter()
        .map(|name| function_evidence(&index, name))
        .collect();

    OwnershipReport {
        pattern: function_pattern.to_string(),
        state: OwnershipState::Matched,
        message: format!(
            "{matched_functions} function(s) with indexed ownership evidence match `{function_pattern}`."
        ),
        config_error: None,
        matched_functions,
        returned_functions: functions.len(),
        functions,
    }
}

fn disabled_message() -> String {
    format!(
        "Ownership evidence is not enabled for this project.\n\
         Add to {}:\n\n  \
         [ownership.rust]\n  enabled = true\n  include = [\"src/**/*.rs\"]\n\n\
         then run `phr-mcp graph rebuild`.",
        config::CONFIG_REL_PATH
    )
}

fn no_graph_message() -> String {
    format!(
        "No code graph at {}. Run `phr-mcp graph rebuild` first.",
        store::GRAPH_REL_PATH
    )
}

fn no_match_message(pattern: &str) -> String {
    format!(
        "No indexed ownership evidence found for `{pattern}`.\n\
         This is an absence of indexed evidence, not proof that the matched code has no ownership concern."
    )
}

// ── indexing ────────────────────────────────────────────────────────────

/// Borrowed lookup tables over the edge set. Built once per query; every map
/// is ordered so two runs over the same graph render byte-identically.
struct Index<'a> {
    declared: BTreeSet<&'a str>,
    span: BTreeMap<&'a str, (&'a str, &'a str, &'a str)>,
    kind: BTreeMap<&'a str, (&'static str, &'a str, &'a str)>,
    evidence: BTreeMap<&'a str, BTreeSet<(&'a str, &'a str)>>,
    resolved_types: BTreeMap<&'a str, BTreeSet<&'a str>>,
    status: BTreeMap<&'a str, BTreeSet<(&'a str, &'a str, &'a str)>>,
    sites_by_function: BTreeMap<&'a str, BTreeSet<&'a str>>,
    relationships: BTreeMap<&'a str, BTreeSet<(&'a str, &'a str, &'a str)>>,
}

fn arg(edge: &Edge, i: usize) -> Option<&str> {
    edge.a.get(i).map(String::as_str)
}

impl<'a> Index<'a> {
    fn build(edges: &'a [Edge]) -> Self {
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

fn function_evidence(index: &Index<'_>, function: &str) -> FunctionEvidence {
    let mut sites: Vec<SiteEvidence> = index
        .sites_by_function
        .get(function)
        .map(|set| set.iter().map(|site| index.site(site)).collect())
        .unwrap_or_default();
    sites.sort_by_key(span_order);

    let mut files: Vec<String> = sites
        .iter()
        .filter_map(|site| site.location.as_ref().map(|l| l.file.clone()))
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect();
    files.sort();

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
    let mut analysis = index.status_of(function);
    for file in &files {
        analysis.extend(index.status_of(file));
    }
    analysis.sort();
    analysis.dedup();

    let reported: BTreeSet<&str> = analysis.iter().map(|s| s.capability.as_str()).collect();
    let analysis_not_reported: Vec<String> = ANALYSIS_CAPABILITIES
        .iter()
        .filter(|capability| !reported.contains(**capability))
        .map(|capability| (*capability).to_string())
        .collect();

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
            let key = (location.file.clone(), location.start_byte.clone());
            let line = match resolved.get(&key) {
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
            };
            location.line = line;
        }
        // The relationship summaries were rendered from the same sites before
        // lines existed; re-render so both places agree.
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
}

fn read_tracked(root: &Path, file: &str) -> Option<String> {
    let path = security::resolve_safe_path(file, root).ok()?;
    security::read_file_capped(&path).ok()
}

fn line_of(body: &str, start_byte: &str) -> Option<usize> {
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

// ── rendering ───────────────────────────────────────────────────────────

/// Collapse every run of whitespace to a single space.
///
/// Operand text is capped at 240 bytes but not at one line, and a multi-line
/// expression interpolated into a table row splits it mid-value and breaks
/// alignment for everything after it. Applied to every interpolated value,
/// not only the ones known to wrap today.
fn one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            pending_space = !out.is_empty();
        } else {
            if pending_space {
                out.push(' ');
                pending_space = false;
            }
            out.push(ch);
        }
    }
    out
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}

fn cell(text: &str) -> String {
    truncate(&one_line(text), TABLE_TEXT_WIDTH)
}

fn kind_phrase(kind: &str) -> &'static str {
    match kind {
        "clone" => "clone",
        "filter" => "filter",
        "await" => "await",
        "mutation" => "mutation",
        "sync_lock" => "sync lock acquired",
        _ => "ownership site",
    }
}

fn operand_label(kind: &str) -> &'static str {
    match kind {
        "mutation" => "place",
        "sync_lock" => "guard",
        _ => "operand",
    }
}

fn location_text(location: Option<&SourceLocation>) -> String {
    match location {
        Some(location) => match location.line {
            Some(line) => format!(
                "{}:{line} (bytes {}..{})",
                location.file, location.start_byte, location.end_byte
            ),
            None => format!(
                "{} bytes {}..{}",
                location.file, location.start_byte, location.end_byte
            ),
        },
        None => "location not indexed".to_string(),
    }
}

/// One-line human summary of a site: what was observed, and where.
fn describe_site(site: &SiteEvidence) -> String {
    let phrase = kind_phrase(&site.kind);
    let mut text = phrase.to_string();
    // Name the operation only when the phrase does not already contain it —
    // "sync lock acquired `lock`" is noise, but `cloned`, `collect`, `read`,
    // and `push` are exactly the distinctions D5 and §7.6 exist to keep.
    if !site.operation.is_empty() && !phrase.contains(site.operation.as_str()) {
        let _ = write!(text, " `{}`", cell(&site.operation));
    }
    if !site.operand.is_empty() {
        let _ = write!(
            text,
            " ({} `{}`)",
            operand_label(&site.kind),
            cell(&site.operand)
        );
    }
    let _ = write!(text, " at {}", location_text(site.location.as_ref()));
    text
}

fn evidence_text(evidence: &[EvidenceRef]) -> String {
    if evidence.is_empty() {
        return "unattributed (no evidence relation recorded)".to_string();
    }
    evidence
        .iter()
        .map(|e| format!("{} ({})", e.level, e.provider))
        .collect::<Vec<_>>()
        .join(", ")
}

fn capability_label(capability: &str) -> String {
    match capability {
        "ast_extraction" => "AST".to_string(),
        "type_inference" => "type inference".to_string(),
        "mir_lowering" => "MIR".to_string(),
        other => one_line(other),
    }
}

/// Render the §10 grouped explanation.
pub fn render_table(report: &OwnershipReport) -> String {
    let mut out = String::new();
    if report.state != OwnershipState::Matched {
        let _ = writeln!(out, "{}", report.message);
        return out;
    }

    for function in &report.functions {
        let _ = writeln!(out, "Function: {}", one_line(&function.function));
        let _ = writeln!(out);

        let _ = writeln!(out, "Observed:");
        if function.sites.is_empty() {
            let _ = writeln!(out, "  (no indexed sites for this function)");
        }
        for site in &function.sites {
            let _ = writeln!(out, "  {}", describe_site(site));
            let _ = writeln!(out, "    evidence: {}", evidence_text(&site.evidence));
            if !site.resolved_types.is_empty() {
                let _ = writeln!(
                    out,
                    "    resolved type: {}",
                    cell(&site.resolved_types.join(", "))
                );
            }
            for status in &site.analysis {
                let _ = writeln!(
                    out,
                    "    {}: {} ({})",
                    capability_label(&status.capability),
                    one_line(&status.status),
                    one_line(&status.reason)
                );
            }
            for note in &site.notes {
                let _ = writeln!(out, "    note: {note}");
            }
        }

        let _ = writeln!(out);
        let _ = writeln!(out, "Relationships:");
        if function.relationships.is_empty() {
            let _ = writeln!(out, "  (no derived relationships indexed)");
        }
        for relation in &function.relationships {
            let _ = writeln!(out, "  {}", one_line(&relation.relation));
            for support in &relation.supported_by {
                let _ = writeln!(out, "    {}: {}", support.role, support.summary);
            }
            let _ = writeln!(out, "    evidence: {}", evidence_text(&relation.evidence));
            let _ = writeln!(out, "    limit: {}", relation.limit);
        }

        let _ = writeln!(out);
        let _ = writeln!(out, "Evidence:");
        for capability in ANALYSIS_CAPABILITIES {
            let rows: Vec<&AnalysisStatus> = function
                .analysis
                .iter()
                .filter(|status| status.capability == *capability)
                .collect();
            if rows.is_empty() {
                let _ = writeln!(
                    out,
                    "  {}: not reported (no provider recorded a result)",
                    capability_label(capability)
                );
                continue;
            }
            for status in rows {
                // Only name the subject when it is not the function itself —
                // a file-scoped status (a site cap, a stale compiler
                // generation) means something different from a function one.
                let scope = if status.subject == function.function {
                    String::new()
                } else {
                    format!(" for {}", cell(&status.subject))
                };
                let _ = writeln!(
                    out,
                    "  {}: {} ({}){scope}",
                    capability_label(capability),
                    one_line(&status.status),
                    one_line(&status.reason),
                );
            }
        }
        for status in &function.analysis {
            if !ANALYSIS_CAPABILITIES.contains(&status.capability.as_str()) {
                let _ = writeln!(
                    out,
                    "  {}: {} ({})",
                    capability_label(&status.capability),
                    one_line(&status.status),
                    one_line(&status.reason)
                );
            }
        }

        let _ = writeln!(out);
        let _ = writeln!(out, "Limit:");
        for limit in &function.limits {
            let _ = writeln!(out, "  {limit}");
        }
        let _ = writeln!(out);
    }

    if report.returned_functions < report.matched_functions {
        let _ = writeln!(
            out,
            "{} of {} matching function(s); raise --limit for more.",
            report.returned_functions, report.matched_functions
        );
    } else {
        let _ = writeln!(out, "{} matching function(s).", report.matched_functions);
    }
    out
}

/// Render the machine-readable form. Both surfaces serialize the same struct,
/// so there is no envelope for them to disagree about.
pub fn render_json(report: &OwnershipReport) -> String {
    serde_json::to_string_pretty(report).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::ownership::extract;

    fn edge(relation: &str, args: &[&str]) -> Edge {
        Edge::base(relation, args, "src/scheduler.rs")
    }

    const F: &str = "rust:demo::llm::scheduler::Scheduler::acquire";
    const LOCK: &str = "rust:demo::llm::scheduler::Scheduler::acquire#ownership:lock:1200";
    const AWAIT: &str = "rust:demo::llm::scheduler::Scheduler::acquire#ownership:await:2400";

    fn scheduler_graph() -> Vec<Edge> {
        vec![
            edge(OWNERSHIP_SITE, &[LOCK]),
            edge(OWNERSHIP_SITE_IN_FUNCTION, &[LOCK, F]),
            edge(
                OWNERSHIP_SITE_SPAN,
                &[LOCK, "src/scheduler.rs", "1200", "1240"],
            ),
            edge(SYNC_LOCK_SITE, &[LOCK, "lock", "guard"]),
            edge(OWNERSHIP_EVIDENCE, &[LOCK, "ast", "tree_sitter_rust"]),
            edge(OWNERSHIP_SITE, &[AWAIT]),
            edge(OWNERSHIP_SITE_IN_FUNCTION, &[AWAIT, F]),
            edge(
                OWNERSHIP_SITE_SPAN,
                &[AWAIT, "src/scheduler.rs", "2400", "2420"],
            ),
            edge(AWAIT_SITE, &[AWAIT]),
            edge(OWNERSHIP_EVIDENCE, &[AWAIT, "ast", "tree_sitter_rust"]),
            edge(LOCK_SCOPE_ENDS_BEFORE_AWAIT, &[F, LOCK, AWAIT]),
            edge(
                OWNERSHIP_ANALYSIS_STATUS,
                &[F, "ast_extraction", "available", extract::REASON_COMPLETE],
            ),
            edge(
                OWNERSHIP_ANALYSIS_STATUS,
                &[F, "type_inference", "available", "rust_analyzer"],
            ),
            edge(
                OWNERSHIP_ANALYSIS_STATUS,
                &[F, "mir_lowering", "unavailable", "async_lowering"],
            ),
        ]
    }

    fn on() -> Availability {
        Availability {
            ownership_enabled: true,
            graph_present: true,
        }
    }

    fn build_all(edges: &[Edge], pattern: &str) -> OwnershipReport {
        build(edges, pattern, 0, on())
    }

    // Pins the three empty states apart. `store::load` returns an empty vector
    // for a missing file, so "no graph" and "empty graph" are the same value —
    // conflating them would tell a user to rebuild a graph they already have,
    // or worse, read as "your code is clean".
    #[test]
    fn the_three_empty_states_are_distinguishable_and_never_read_as_clean() {
        let disabled = build(
            &scheduler_graph(),
            "*",
            0,
            Availability {
                ownership_enabled: false,
                graph_present: true,
            },
        );
        assert_eq!(
            disabled.state,
            OwnershipState::Disabled,
            "a project without [ownership.rust] must report disabled"
        );
        assert!(
            disabled.message.contains("[ownership.rust]"),
            "the disabled message must say how to enable it: {}",
            disabled.message
        );

        let no_graph = build(
            &[],
            "*",
            0,
            Availability {
                ownership_enabled: true,
                graph_present: false,
            },
        );
        assert_eq!(
            no_graph.state,
            OwnershipState::NoGraph,
            "an absent graph file must not read as an empty result"
        );
        assert!(
            no_graph.message.contains("graph rebuild"),
            "the no-graph message must name the fix: {}",
            no_graph.message
        );

        let no_match = build(&scheduler_graph(), "rust:other::*", 0, on());
        assert_eq!(
            no_match.state,
            OwnershipState::NoMatch,
            "a graph that matched nothing is its own state"
        );
        assert!(
            no_match
                .message
                .contains("No indexed ownership evidence found"),
            "empty must render as absence of evidence: {}",
            no_match.message
        );
        assert!(
            no_match.message.contains("not proof"),
            "empty must never read as proof the code is clean: {}",
            no_match.message
        );
    }

    // The extractor writes the `reason` argument and this renderer prints it
    // verbatim, so the two agree only by convention. They disagreed for the
    // whole time nothing was wired: the extractor emitted `complete` while
    // every fixture here said `extracted`, and the sample rendering in §10 of
    // the spec read `AST: available (extracted)`. `complete` won. This test is
    // the join — it renders the constant the extractor actually emits and
    // asserts the exact string a user sees, so a rename on either side fails
    // here rather than shipping a value no fixture matches.
    #[test]
    fn the_successful_ast_status_reason_renders_as_the_exact_string_complete() {
        assert_eq!(
            extract::REASON_COMPLETE,
            "complete",
            "the successful ast_extraction reason is the wire value `complete`"
        );
        let mut edges = scheduler_graph();
        edges.push(edge(
            OWNERSHIP_ANALYSIS_STATUS,
            &[
                "src/scheduler.rs",
                extract::CAPABILITY_AST_EXTRACTION,
                extract::STATUS_AVAILABLE,
                extract::REASON_COMPLETE,
            ],
        ));
        let table = render_table(&build_all(&edges, F));
        assert!(
            table.contains("AST: available (complete) for src/scheduler.rs"),
            "the extractor's own reason constant must render verbatim: {table}"
        );
        assert!(
            !table.contains("extracted"),
            "`extracted` is the retired spelling and must not reappear: {table}"
        );
    }

    // §9 records the site cap and D9's stale compiler generation against the
    // *file*, not the function. Those must surface on every function in that
    // file, and must say which subject they are about — a file-scoped partial
    // means something different from a function-scoped one.
    #[test]
    fn file_scoped_partial_and_stale_statuses_surface_on_the_functions_in_that_file() {
        let mut edges = scheduler_graph();
        edges.push(edge(
            OWNERSHIP_ANALYSIS_STATUS,
            &["src/scheduler.rs", "ast_extraction", "partial", "site_cap"],
        ));
        edges.push(edge(
            OWNERSHIP_ANALYSIS_STATUS,
            &[
                "src/scheduler.rs",
                "type_inference",
                "stale",
                "incremental_edit",
            ],
        ));
        let report = build_all(&edges, F);
        let table = render_table(&report);
        assert!(
            table.contains("AST: partial (site_cap) for src/scheduler.rs"),
            "a file-scoped site cap must render and name its subject: {table}"
        );
        assert!(
            table.contains("type inference: stale (incremental_edit) for src/scheduler.rs"),
            "D9's stale status must be visible in the output: {table}"
        );
        assert!(
            table.contains("AST: available (complete)\n"),
            "a function-scoped status must not be cluttered with its own name: {table}"
        );
        assert!(
            report.functions[0]
                .limits
                .iter()
                .any(|l| l.contains("partial, stale, failed, or unavailable")),
            "a degraded capability must be named in the limits"
        );
    }

    // Pins Addendum A.1: the traversal from a derived relationship to its
    // supporting sites, and from each site to span, evidence level, and
    // provider. Losing any hop leaves a relationship the user cannot check.
    #[test]
    fn a_relationship_names_its_supporting_sites_with_span_and_provider() {
        let report = build_all(&scheduler_graph(), F);
        let function = &report.functions[0];
        let relation = &function.relationships[0];
        assert_eq!(
            relation.relation, LOCK_SCOPE_ENDS_BEFORE_AWAIT,
            "the scheduler fixture derives one lock-scope relation"
        );
        assert_eq!(
            relation.supported_by.len(),
            2,
            "both site arguments must be named"
        );
        assert_eq!(
            relation.supported_by[0].site, LOCK,
            "the first argument is the lock site"
        );
        assert!(
            relation.supported_by.iter().all(|s| s.resolved),
            "both supporting sites resolve to indexed evidence"
        );
        assert!(
            relation.supported_by[0]
                .summary
                .contains("src/scheduler.rs"),
            "a supporting site must carry its source location: {}",
            relation.supported_by[0].summary
        );
        assert_eq!(
            relation.evidence,
            vec![EvidenceRef {
                level: "ast".to_string(),
                provider: "tree_sitter_rust".to_string(),
            }],
            "relationship evidence is the union of its sites' evidence, never stronger"
        );
    }

    // Pins Addendum A.4: an unavailable capability must be rendered next to
    // the positive findings. Dropping it makes AST-only evidence look
    // corroborated, which is the single worst failure this feature can have.
    #[test]
    fn unavailable_capabilities_render_alongside_the_positive_facts() {
        let table = render_table(&build_all(&scheduler_graph(), F));
        assert!(
            table.contains("AST: available"),
            "positive AST status must render: {table}"
        );
        assert!(
            table.contains("MIR: unavailable (async_lowering)"),
            "the unavailable MIR capability must render with its reason: {table}"
        );
        assert!(
            table.contains("lexical scope is not general control-flow or borrow-liveness proof"),
            "the §10 limit line must render: {table}"
        );
    }

    // A capability with no status edge at all is the quietest way to make weak
    // evidence look strong: nothing is wrong, there is simply no line.
    #[test]
    fn a_capability_with_no_recorded_status_renders_as_not_reported() {
        let edges: Vec<Edge> = scheduler_graph()
            .into_iter()
            .filter(|e| e.a.get(1).map(String::as_str) != Some("mir_lowering"))
            .collect();
        let report = build_all(&edges, F);
        assert_eq!(
            report.functions[0].analysis_not_reported,
            vec!["mir_lowering".to_string()],
            "a capability with no status edge must be listed as unreported"
        );
        let table = render_table(&report);
        assert!(
            table.contains("MIR: not reported"),
            "an unreported capability must still occupy a line: {table}"
        );
    }

    // Pins that an exact function ID and an embedded glob both select, the
    // same way every other graph query behaves (§13.2).
    #[test]
    fn exact_and_embedded_glob_function_queries_both_select() {
        let edges = scheduler_graph();
        let exact = build_all(&edges, F);
        assert_eq!(
            exact.matched_functions, 1,
            "an exact function ID selects it"
        );

        let glob = build_all(&edges, "rust:demo::llm::*::acquire");
        assert_eq!(
            glob.matched_functions, 1,
            "an embedded glob selects the same function"
        );
        assert_eq!(
            glob.functions, exact.functions,
            "glob and exact selection must produce identical grouped evidence"
        );

        let wrong = build_all(&edges, "rust:demo::llm::*::release");
        assert_eq!(
            wrong.matched_functions, 0,
            "a non-matching glob selects none"
        );
    }

    // A relationship whose supporting site has no site edges must render as
    // unattributed rather than silently disappearing (Addendum A.4).
    #[test]
    fn a_relationship_over_an_unindexed_site_renders_as_unattributed() {
        let mut edges = scheduler_graph();
        edges.retain(|e| e.a.first().map(String::as_str) != Some(AWAIT));
        edges.push(edge(LOCK_SCOPE_ENDS_BEFORE_AWAIT, &[F, LOCK, AWAIT]));
        let report = build_all(&edges, F);
        let support = &report.functions[0].relationships[0].supported_by[1];
        assert!(
            !support.resolved,
            "a site with no indexed edges must be marked unresolved"
        );
        assert!(
            support.summary.contains("unattributed"),
            "an empty evidence path renders as unattributed: {}",
            support.summary
        );
    }

    // Operand text is capped at 240 bytes but not at one line, and it may
    // contain the characters a table uses for alignment.
    #[test]
    fn multi_line_operand_text_cannot_break_table_alignment() {
        let site = "f#ownership:clone:10";
        let edges = vec![
            edge(OWNERSHIP_SITE, &[site]),
            edge(OWNERSHIP_SITE_IN_FUNCTION, &[site, "rust:demo::f"]),
            edge(OWNERSHIP_SITE_SPAN, &[site, "src/a.rs", "10", "20"]),
            edge(
                CLONE_SITE,
                &[site, "clone", "self\n  .items\n\t.first()\n.unwrap()"],
            ),
            edge(OWNERSHIP_EVIDENCE, &[site, "ast", "tree_sitter_rust"]),
        ];
        let table = render_table(&build_all(&edges, "rust:demo::f"));
        let observed: Vec<&str> = table
            .lines()
            .filter(|line| line.contains("self .items"))
            .collect();
        assert_eq!(
            observed.len(),
            1,
            "the operand must collapse onto one row: {table}"
        );
        assert!(
            !table.contains("self\n"),
            "no raw newline may survive into a table cell: {table}"
        );
    }

    // The digest marker and the operand cap protect fact IDs; the renderer
    // must not reintroduce the separator into displayed text either.
    #[test]
    fn rendered_text_never_carries_the_fact_id_separator() {
        let table = render_table(&build_all(&scheduler_graph(), "*"));
        assert!(
            !table.contains('\u{1f}'),
            "U+001F joins fact-id arguments and must never reach rendered text"
        );
    }

    // A bare `collect` is a clone site by D5, but the rendering must not
    // describe it as having produced ownership — that is a type-level claim.
    #[test]
    fn a_collect_site_is_not_described_as_producing_ownership() {
        let site = "rust:demo::g#ownership:clone:44";
        let edges = vec![
            edge(OWNERSHIP_SITE, &[site]),
            edge(OWNERSHIP_SITE_IN_FUNCTION, &[site, "rust:demo::g"]),
            edge(OWNERSHIP_SITE_SPAN, &[site, "src/g.rs", "44", "70"]),
            edge(CLONE_SITE, &[site, "collect", "rows.iter().collect()"]),
            edge(OWNERSHIP_EVIDENCE, &[site, "ast", "tree_sitter_rust"]),
        ];
        let report = build_all(&edges, "rust:demo::g");
        let table = render_table(&report);
        assert!(
            table.contains("does not establish that ownership was produced"),
            "a collect site must carry its type-level caveat: {table}"
        );
        assert!(
            report.functions[0]
                .limits
                .iter()
                .any(|l| l.contains("type-level claim")),
            "the function limits must name the collect caveat"
        );
    }

    // Sites must order by numeric byte offset. String ordering would put
    // byte 1200 before byte 240 and scramble the narrative.
    #[test]
    fn sites_order_by_numeric_byte_offset_not_string_order() {
        let mut edges = scheduler_graph();
        let early = "rust:demo::llm::scheduler::Scheduler::acquire#ownership:clone:240";
        edges.push(edge(OWNERSHIP_SITE, &[early]));
        edges.push(edge(OWNERSHIP_SITE_IN_FUNCTION, &[early, F]));
        edges.push(edge(
            OWNERSHIP_SITE_SPAN,
            &[early, "src/scheduler.rs", "240", "260"],
        ));
        edges.push(edge(CLONE_SITE, &[early, "clone", "cfg"]));
        let report = build_all(&edges, F);
        let order: Vec<&str> = report.functions[0]
            .sites
            .iter()
            .map(|s| s.site.as_str())
            .collect();
        assert_eq!(
            order,
            vec![early, LOCK, AWAIT],
            "byte 240 precedes byte 1200"
        );
    }

    // The function limit must be visible in the shared struct, so the two
    // surfaces cannot disagree about whether the answer was truncated.
    #[test]
    fn truncation_is_reported_in_the_shared_report_not_per_surface() {
        let mut edges = Vec::new();
        for i in 0..5 {
            let function = format!("rust:demo::f{i}");
            let site = format!("{function}#ownership:await:{i}");
            edges.push(edge(OWNERSHIP_SITE, &[&site]));
            edges.push(edge(OWNERSHIP_SITE_IN_FUNCTION, &[&site, &function]));
            edges.push(edge(AWAIT_SITE, &[&site]));
        }
        let report = build(&edges, "rust:demo::*", 2, on());
        assert_eq!(report.matched_functions, 5, "the total ignores the limit");
        assert_eq!(
            report.returned_functions, 2,
            "the limit caps what is rendered"
        );
        assert!(
            render_table(&report).contains("2 of 5 matching function(s)"),
            "truncation must be visible in the table"
        );
    }

    // Line numbers come from the file on disk, and the graph can outlive an
    // edit to it. A stale offset must degrade to the byte span, not to a
    // confidently wrong line.
    #[test]
    fn an_offset_past_the_end_of_the_file_degrades_to_a_byte_span() {
        assert_eq!(
            line_of("one\ntwo\nthree", "4"),
            Some(2),
            "the byte after the first newline is on line 2"
        );
        assert_eq!(line_of("one\ntwo", "0"), Some(1), "offset 0 is line 1");
        assert_eq!(
            line_of("short", "9999"),
            None,
            "an offset past EOF yields no line rather than a wrong one"
        );
        assert_eq!(
            line_of("short", "not-a-number"),
            None,
            "a malformed offset yields no line"
        );
    }

    // The JSON form must be an object: `EpistemeMcp::ok_json` rejects a
    // top-level array at runtime, and a bare list of functions would be one.
    #[test]
    fn the_json_form_is_an_object_so_the_mcp_envelope_accepts_it() {
        let json = render_json(&build_all(&scheduler_graph(), F));
        let value: serde_json::Value =
            serde_json::from_str(&json).expect("rendered JSON must parse");
        assert!(value.is_object(), "structured MCP results must be objects");
        assert!(
            value.get("functions").is_some_and(|f| f.is_array()),
            "functions must be an array under an object key"
        );
        assert_eq!(
            value["state"], "matched",
            "the state must be machine-readable"
        );
    }
}

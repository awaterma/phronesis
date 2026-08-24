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

mod evidence;
mod render;
#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::path::Path;

use serde::Serialize;

use crate::graph::model::Edge;
use crate::graph::ownership::config;
use crate::graph::query::glob_matches;
use crate::graph::store;

pub use evidence::resolve_lines;
use evidence::{Index, function_evidence};
pub use render::{render_json, render_table};

/// Functions rendered when the caller supplies no limit.
pub const DEFAULT_FUNCTION_LIMIT: usize = 20;

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

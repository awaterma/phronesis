//! Parameter types for the MCP tool surface implemented in `server.rs`.
//!
//! Extracted from `server.rs` to keep that file focused on the tool
//! implementations themselves. All types are `pub` (the MCP tool macros
//! and external callers reference them by name) and serialize as plain
//! JSON via `schemars` so the wire shape is unchanged.

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct AddRuleParams {
    pub id: String,
    pub priority: i32,
    pub conditions: Vec<ConditionInput>,
    pub actions: Vec<ActionInput>,
    /// Hook phase this rule applies to: "pre" (block before edit) or "post"
    /// (warn after edit). Defaults to "pre" when omitted.
    #[serde(default)]
    pub phase: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct SaveRulesParams {
    /// Default phase to assign to in-memory rules that have no recorded phase.
    /// When omitted, defaults to "pre".
    #[serde(default)]
    pub phase: Option<String>,
    /// When true (default), merges in-memory rules with the existing on-disk
    /// rules file by ID. When false, the existing file is replaced.
    #[serde(default = "default_true")]
    pub merge: bool,
    /// When true, return the diff/summary without writing.
    #[serde(default)]
    pub dry_run: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct LoadRulesFileParams {}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct ConditionInput {
    pub predicate: String,
    pub args: Vec<String>,
    #[serde(default)]
    pub script: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct ActionInput {
    pub action_type: String,
    pub params: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct RuleIdParam {
    pub rule_id: phr::RuleId,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct AddPredicateProviderParams {
    /// Provider filename without the `.rhai` extension.
    pub name: String,
    /// Sandboxed Rhai source using `event` and `emit_fact(predicate, args)`.
    pub script: String,
    /// Explicitly replace an existing provider with the same name.
    #[serde(default)]
    pub replace: bool,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct PredicateProviderNameParam {
    pub name: String,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct ListPredicateProvidersParams {}

#[derive(Debug, Default, Deserialize, Serialize, schemars::JsonSchema)]
pub struct PredicateProviderEventInput {
    #[serde(default)]
    pub phase: String,
    #[serde(default)]
    pub tool_name: String,
    #[serde(default)]
    pub file_path: String,
    /// All project-relative paths in a batch operation. Empty for per-file events.
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub old_content: String,
    #[serde(default)]
    pub new_content: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub output: String,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct TestPredicateProviderParams {
    pub script: String,
    #[serde(default)]
    pub event: PredicateProviderEventInput,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct AssertFactParams {
    pub id: String,
    pub predicate: String,
    pub args: Vec<String>,
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct FactIdParam {
    pub fact_id: phr::FactId,
}

/// One positional-argument constraint for a fact query: the fact's arg at
/// `index` must exactly equal `value`.
#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct ArgFilter {
    /// Zero-based position in the fact's `args`.
    pub index: usize,
    /// Required value at that position (exact match).
    pub value: String,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct PredicateFilter {
    /// Match facts with this single predicate. Omit (and omit `predicates`)
    /// to return every fact.
    #[serde(default)]
    pub predicate: Option<String>,
    /// Match facts whose predicate is in this set (membership). Takes
    /// precedence over `predicate`/`arg_filters` when given.
    #[serde(default)]
    pub predicates: Option<Vec<String>>,
    /// Positional argument constraints; all must match. Applied together with
    /// a single `predicate` (ignored when `predicates` is used).
    #[serde(default)]
    pub arg_filters: Option<Vec<ArgFilter>>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct KindFilter {
    #[serde(default)]
    pub kind: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct FilePathParam {
    pub file_path: String,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct ClearSectionContextParams {}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct GetActionLogParams {
    /// Maximum number of entries to return (most recent first). Default 100.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Only return entries with `ts >= since` (Unix epoch seconds).
    #[serde(default)]
    pub since: Option<u64>,
    /// Filter by entry kind: `"hook"` or `"mcp"`.
    #[serde(default)]
    pub kind: Option<String>,
    /// Filter by event name (e.g. `"pre_check"`, `"add_rule"`, `"fire_rules"`).
    #[serde(default)]
    pub event: Option<String>,
    /// When true, return only entries with non-zero `exit` — useful for
    /// "show me what's been blocked recently".
    #[serde(default)]
    pub only_nonzero_exit: bool,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct SetSectionContextParams {
    /// The source file the section belongs to. Must match the `args[0]` of
    /// the extracted rules' `markdown_rule` conditions (typically a path
    /// like "docs/RUST-PATTERNS-GUIDE.md").
    pub file: String,
    /// Section name as it appeared in the markdown (e.g. "Error Handling").
    pub section: String,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct GetStatsParams {
    /// Time window like `30m`, `24h`, `7d`, `2w`. Omit (or pass an
    /// unrecognized value) for an all-time aggregate. Matches the
    /// `phr-mcp stats --since` CLI flag.
    #[serde(default)]
    pub since: Option<String>,
    /// When set, restrict aggregation to a single rule id. Same semantics
    /// as `phr-mcp stats --rule`.
    #[serde(default)]
    pub rule: Option<String>,
    /// Rendering format. `"json"` (default) returns the structured Stats
    /// payload — best for programmatic callers. `"table"` returns a
    /// human-readable terminal table.
    #[serde(default)]
    pub format: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct AuditCodebaseParams {
    /// Restrict to a single rule id (e.g. `"no-unwrap-in-src"`). When set,
    /// per-file detail with line numbers is included in table output.
    #[serde(default)]
    pub rule: Option<String>,
    /// Restrict scan to a subdirectory of the project root (e.g. `"src/parser"`).
    /// Absolute paths are accepted as-is. Defaults to the project root.
    #[serde(default)]
    pub path: Option<String>,
    /// `"json"` (default) returns the structured AuditReport payload — best
    /// for programmatic use. `"table"` returns a human-readable summary.
    #[serde(default)]
    pub format: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct GetDebtTrendParams {
    /// Number of most-recent snapshots to include. Default 5. Ignored when `since` is set.
    #[serde(default)]
    pub last: Option<u32>,
    /// Time window like `30m`, `24h`, `7d`. Overrides `last` when set.
    /// Same values as `get_stats --since`.
    #[serde(default)]
    pub since: Option<String>,
    /// Restrict to a single rule id.
    #[serde(default)]
    pub rule: Option<String>,
    /// `"json"` (default) or `"table"`.
    #[serde(default)]
    pub format: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct GetDriftParams {
    /// Which corpus to scan: "claude_md", "memory", "wiki", "code", or
    /// "all" (default).
    #[serde(default)]
    pub source: Option<String>,
    /// Max items per source. Default 5, clamped to 50.
    #[serde(default)]
    pub limit: Option<usize>,
    /// "json" (default) or "table".
    #[serde(default)]
    pub format: Option<String>,
    /// Override the auto-memory directory.
    #[serde(default)]
    pub memory_dir: Option<String>,
    /// Override the wiki decisions directory.
    #[serde(default)]
    pub wiki_dir: Option<String>,
    /// Include a draft rule per item. Off by default — a draft is several
    /// hundred bytes of escaped JSON and would dominate the response.
    #[serde(default)]
    pub suggest: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct GetConfidenceParams {
    /// Report on a specific subject id instead of the open work unit.
    #[serde(default)]
    pub subject: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct GetJourneyParams {
    /// Optional rule id; when set, only return facts that specific rule
    /// references. Mirrors the `phr-mcp journey --explain` CLI flag.
    #[serde(default)]
    pub explain_rule: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct SubmitSuggestionParams {
    /// The work-unit id to open/declare (e.g. a translation: `"xlate-7"`).
    /// Subsequent build/test runs accrue confidence signals to it.
    pub subject: String,
    /// Optional human-readable summary of the suggestion.
    #[serde(default)]
    pub summary: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct QueryCodeGraphParams {
    /// Relation to match, e.g. `tested_by`, `defines_fn`, `imports`,
    /// `no_direct_test`, `in_cycle`, `calls_api`, `file_type`,
    /// `declares_module`.
    /// Omit to list the available relations and their edge counts.
    #[serde(default)]
    pub relation: Option<String>,
    /// Positional argument constraints, in order. Use `"*"` to leave a
    /// position unconstrained — e.g. `["*", "crate::wme"]` on `imports`
    /// asks "what depends on crate::wme?".
    #[serde(default)]
    pub args: Option<Vec<String>>,
    /// Maximum rows returned. Defaults to 50; `0` means no limit. The
    /// response always reports the unlimited total so truncation is visible.
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct OwnershipEvidenceParams {
    /// Canonical Rust function ID, e.g.
    /// `rust:mycrate::llm::scheduler::Scheduler::acquire`, or a glob over
    /// one (`rust:mycrate::llm::*`). Same `*` / `?` semantics as
    /// `query_code_graph`.
    pub function: String,
    /// Maximum functions returned. Defaults to 20; `0` means no limit. The
    /// response always reports the unlimited total so truncation is visible.
    ///
    /// There is deliberately no path field: the project root is
    /// server-controlled, as it is for every other graph tool.
    #[serde(default)]
    pub limit: Option<usize>,
}

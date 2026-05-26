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
pub struct AssertFactParams {
    pub id: String,
    pub predicate: String,
    pub args: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct FactIdParam {
    pub fact_id: phr::FactId,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct PredicateFilter {
    #[serde(default)]
    pub predicate: Option<String>,
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
pub struct GetValuesParams {
    /// Time window like `30m`, `24h`, `7d`, `2w`. Omit (or pass an
    /// unrecognized value) for an all-time aggregate. Matches the
    /// `phr-mcp values --since` CLI flag.
    #[serde(default)]
    pub since: Option<String>,
    /// When set, restrict aggregation to a single rule id. Same semantics
    /// as `phr-mcp values --rule`.
    #[serde(default)]
    pub rule: Option<String>,
    /// Rendering format. `"json"` (default) returns the structured Values
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
    /// Same values as `get_values --since`.
    #[serde(default)]
    pub since: Option<String>,
    /// Restrict to a single rule id.
    #[serde(default)]
    pub rule: Option<String>,
    /// `"json"` (default) or `"table"`.
    #[serde(default)]
    pub format: Option<String>,
}


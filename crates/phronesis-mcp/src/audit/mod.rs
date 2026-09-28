//! Whole-tree audit: walk the project, run opted-in rules' predicates
//! against full file contents, report per-rule violation counts. Pure
//! functions for the eval/aggregation/render path; I/O lives in `run`.
//!
//! Split into cohesive modules; see individual files.

mod diagnostics;
mod engine;
mod graph;
mod render;
mod run;
mod trend;
mod types;

pub use diagnostics::empty_result_diagnostic;
pub use engine::{rule_matches_filter, script_diagnostics};
pub use graph::{graph_scope_prefix, merge_graph_hits};
pub use render::{
    render_json, render_table, render_trend_json, render_trend_table, short_iso_date,
};
pub use run::{AuditSectionTimes, discover_files, run, run_profiled};
pub use trend::{DebtTrend, RuleTrend, TrendOpts, TrendPoint, compute_trend};
pub use types::{
    AuditOpts, AuditReport, FileAudit, Level, RuleAudit, audit_snapshot_entry, resolve_scan_root,
};

#[cfg(test)]
mod tests {
    mod ast_tests;
    mod doc_excepted_tests;
    mod engine_tests;
    mod graph_tests;
    mod render_tests;
    mod script_tests;
    mod trend_tests;
}

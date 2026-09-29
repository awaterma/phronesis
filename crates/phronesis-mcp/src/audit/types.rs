//! Audit public types and small helpers. Split from the original `audit.rs`;
//! functions moved verbatim. Exemption carried from the god-file surface.
//!
//! phronesis-allow: enforce-no-result-string-error (verbatim move from audit.rs)

use std::path::{Path, PathBuf};

use phr::RuleId;

// ── Public types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Block,
    Warn,
}

impl Level {
    pub(super) fn from_action_type(s: &str) -> Option<Self> {
        match s {
            "constraint_violation" => Some(Level::Block),
            "constraint_warning" => Some(Level::Warn),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Level::Block => "block",
            Level::Warn => "warn",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuditOpts {
    pub project_root: PathBuf,
    /// Defaults to `project_root` when constructed by the CLI/MCP handler.
    pub scan_root: PathBuf,
    pub rule_filter: Option<String>,
}

/// Resolve the audit scan root: an absolute path is used as-is; a relative
/// path is joined onto `project_root`; `None` defaults to `project_root`.
///
/// Both the MCP `audit_codebase` handler (server.rs, receives `Option<&str>`
/// from tool params) and the CLI `handle_audit` (main.rs, converts
/// `Option<PathBuf>` to `Option<&str>` at the call site) use this. The
/// call-site conversion keeps the signature uniform here.
pub fn resolve_scan_root(param: Option<&str>, project_root: &Path) -> PathBuf {
    match param {
        Some(p) => {
            let pb = PathBuf::from(p);
            if pb.is_absolute() {
                pb
            } else {
                project_root.join(pb)
            }
        }
        None => project_root.to_path_buf(),
    }
}

/// Build the `audit_codebase` log-snapshot entry by annotating `e` with
/// per-rule hit counts, totals, and the files-scanned count.
///
/// Both `server::EpistemeMcp::audit_codebase` (MCP tool) and
/// `main::handle_audit` (CLI) call this to write their snapshot. Field
/// names and integer widths here must stay in sync with the field reads
/// inside `compute_trend` — that function is the sole reader of these
/// snapshots, and divergence would silently produce zeroed trend rows.
pub fn audit_snapshot_entry(
    e: crate::action_log::LogEntry,
    report: &AuditReport,
) -> crate::action_log::LogEntry {
    let mut per_rule = serde_json::Map::new();
    for r in &report.per_rule {
        per_rule.insert(
            r.rule_id.as_str().to_string(),
            serde_json::json!({ "level": r.level.as_str(), "hits": r.hits }),
        );
    }
    let blocked: u32 = report
        .per_rule
        .iter()
        .filter(|r| r.level == Level::Block)
        .map(|r| r.hits)
        .sum();
    let warned: u32 = report
        .per_rule
        .iter()
        .filter(|r| r.level == Level::Warn)
        .map(|r| r.hits)
        .sum();
    e.with("files_scanned", report.files_scanned as u64)
        .with("blocked_total", blocked as u64)
        .with("warned_total", warned as u64)
        .with("per_rule", serde_json::Value::Object(per_rule))
}

#[derive(Debug, Clone)]
pub struct AuditReport {
    pub generated_at: u64,
    pub scan_duration_ms: u64,
    pub files_scanned: u32,
    /// Sorted by `(level desc, hits desc, rule_id asc)`.
    pub per_rule: Vec<RuleAudit>,
}

#[derive(Debug, Clone)]
pub struct RuleAudit {
    pub rule_id: RuleId,
    pub level: Level,
    pub hits: u32,
    pub files: Vec<FileAudit>,
}

#[derive(Debug, Clone)]
pub struct FileAudit {
    pub path: PathBuf,
    pub lines: Vec<u32>,
    /// Per-hit human-readable detail, parallel to `lines`. Populated for
    /// AST-predicate hits (e.g. `"ladder (8 let bindings)"`) where the
    /// line number is a placeholder; empty for content/whole-file hits,
    /// which carry meaningful line numbers in `lines` instead.
    pub details: Vec<String>,
}

/// Accumulator for one (rule, file) pair during the scan. `lines` and
/// `details` stay parallel: one entry per hit. Collapsed into a
/// `FileAudit` at the end of the scan.
#[derive(Debug, Clone, Default)]
pub(super) struct PerFileHits {
    pub(super) lines: Vec<u32>,
    pub(super) details: Vec<String>,
}

impl PerFileHits {
    pub(super) fn push_line(&mut self, line: u32) {
        self.lines.push(line);
        self.details.push(String::new());
    }
    pub(super) fn push_detail(&mut self, detail: String) {
        // AST hits don't know a real line span yet; line 1 is the
        // documented placeholder. Keep the two vecs the same length so
        // `hits` (derived from `lines.len()`) stays accurate.
        self.lines.push(1);
        self.details.push(detail);
    }
    pub(super) fn extend_lines(&mut self, lines: Vec<u32>) {
        self.lines.extend(lines.iter().copied());
        self.details
            .extend(std::iter::repeat_with(String::new).take(lines.len()));
    }
}

//! Audit run/scan orchestration. Split from the original `audit.rs`;
//! functions moved verbatim.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::rules_file::RulesFile;

use super::engine::{ScanFileInput, build_per_rule, filter_audit_rules, scan_file_into_accum};
use super::types::{AuditOpts, AuditReport, Level, PerFileHits};

/// Shared scan core used by both [`run`] and [`run_profiled`].
/// When `times` is `Some`, timing points are recorded; when `None`
/// the `Instant::now()` calls still execute (unconditionally) but
/// stores are gated so there is no semantic difference for the caller.
pub(crate) fn run_core(
    rules: &RulesFile,
    opts: &AuditOpts,
    mut times: Option<&mut AuditSectionTimes>,
) -> AuditReport {
    let total_start = Instant::now();

    let audit_rules = filter_audit_rules(rules, opts.rule_filter.as_deref());
    if let Some(ref mut t) = times {
        t.audit_rules = audit_rules.len() as u32;
    }

    // For v1, audit every file the walker accepts. Most rules don't carry
    // an explicit file_pattern condition; default to scanning everything
    // and let the predicates self-filter.
    let (files, files_scanned) = {
        let t = Instant::now();
        let f = if audit_rules.is_empty() {
            Vec::new()
        } else {
            discover_files(&opts.scan_root, &["*"])
        };
        let n = f.len() as u32;
        if let Some(ref mut t2) = times {
            t2.discover = t.elapsed();
            t2.files_scanned = n;
        }
        (f, n)
    };

    // per_rule[rule_id] -> (level, BTreeMap<path -> PerFileHits>)
    let mut accum: BTreeMap<String, (Level, BTreeMap<PathBuf, PerFileHits>)> = BTreeMap::new();

    for path in &files {
        let t = Instant::now();
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => {
                if let Some(ref mut t2) = times {
                    t2.read_files += t.elapsed();
                }
                continue;
            }
        };
        if let Some(ref mut t2) = times {
            t2.read_files += t.elapsed();
        }
        scan_file_into_accum(ScanFileInput {
            project_root: &opts.project_root,
            path,
            content: &content,
            rules: &audit_rules,
            accum: &mut accum,
            times: times.as_deref_mut(),
        });
    }

    let (per_rule, total) = {
        let t = Instant::now();
        let r = build_per_rule(accum);
        let total = total_start.elapsed();
        if let Some(ref mut t2) = times {
            t2.report_build = t.elapsed();
            t2.total = total;
        }
        (r, total)
    };

    AuditReport {
        generated_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        scan_duration_ms: total.as_millis() as u64,
        files_scanned,
        per_rule,
    }
}

/// Per-section timing breakdown for `run_profiled`. All fields are
/// cumulative across the scan unless noted.
#[derive(Debug, Default, Clone, Copy)]
pub struct AuditSectionTimes {
    /// Time inside `discover_files` (walking the tree).
    pub discover: std::time::Duration,
    /// Sum of `fs::read_to_string` over every file (I/O + decode + alloc).
    pub read_files: std::time::Duration,
    /// Sum of `rust_test_block_keep_mask_for` over every Rust file.
    pub keep_mask: std::time::Duration,
    /// Sum of the rule × condition × line inner loop (substring matching,
    /// the suspected hot spot).
    pub match_loop: std::time::Duration,
    /// Final aggregation/sort/render setup.
    pub report_build: std::time::Duration,
    /// Total wall time of `run_profiled`.
    pub total: std::time::Duration,

    pub files_scanned: u32,
    pub audit_rules: u32,
    /// Total `line.matches(needle)` invocations performed.
    pub line_matches_evaluated: u64,
}

/// Run the audit over `opts.scan_root` using `rules`. Reads files, runs each
/// opted-in rule's predicates against the file contents, returns an
/// `AuditReport`. Never panics; unreadable files are skipped silently.
pub fn run(rules: &RulesFile, opts: &AuditOpts) -> AuditReport {
    run_core(rules, opts, None)
}

/// Profiling variant of [`run`] — same logic, returns per-section wall
/// times via [`AuditSectionTimes`]. Kept in tree as a permanent diagnostic
/// (analogous to the criterion bench in `phronesis`); no behavior change vs
/// `run`. Call this from a probe binary; production callers use `run`.
pub fn run_profiled(rules: &RulesFile, opts: &AuditOpts) -> (AuditReport, AuditSectionTimes) {
    let mut times = AuditSectionTimes::default();
    let report = run_core(rules, opts, Some(&mut times));
    (report, times)
}

/// Walk `root` and return all files whose extension matches one of
/// `extensions`. Respects .gitignore and other standard ignore files
/// via the `ignore` crate. Symlinks not followed.
///
/// `extensions` should be passed without the leading dot (e.g. `["rs", "swift"]`).
/// A wildcard (`["*"]`) returns every file the walker accepts.
pub fn discover_files(root: &Path, extensions: &[&str]) -> Vec<PathBuf> {
    use ignore::WalkBuilder;
    let mut out = Vec::new();
    let wildcard = extensions.contains(&"*");
    let mut builder = WalkBuilder::new(root);
    builder.follow_links(false);
    // `.phronesisignore` (gitignore-values) lets projects exclude paths from
    // audit without affecting git tracking. Honored at root and at any
    // descendant directory level.
    builder.add_custom_ignore_filename(".phronesisignore");
    for result in builder.build() {
        let entry = match result {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let path = entry.into_path();
        if wildcard {
            out.push(path);
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if extensions.contains(&ext) {
            out.push(path);
        }
    }
    out
}

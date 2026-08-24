//! Whole-tree audit: walk the project, run opted-in rules' predicates
//! against full file contents, report per-rule violation counts. Pure
//! functions for the eval/aggregation/render path; I/O lives in `run`.
//!
//! Holds the engine, the public types (`AuditReport`, `RuleAudit`,
//! `FileAudit`, `DebtTrend`, `RuleTrend`), the trend computation, and
//! the table/JSON renderers in one file because they form a single
//! cohesive surface — splitting them across `audit/{run,types,render,
//! trend}.rs` would scatter related code for the sake of a line-count
//! threshold rather than for any independent reason.
//!
//! phronesis-allow: audit-file-loc-high (cohesive audit-engine surface)

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::rules_file::{DiskRule, RulesFile};
use crate::syntax;
use phr::{Fact, RuleId};

// ── Public types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Block,
    Warn,
}

impl Level {
    fn from_action_type(s: &str) -> Option<Self> {
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
struct PerFileHits {
    lines: Vec<u32>,
    details: Vec<String>,
}

impl PerFileHits {
    fn push_line(&mut self, line: u32) {
        self.lines.push(line);
        self.details.push(String::new());
    }
    fn push_detail(&mut self, detail: String) {
        // AST hits don't know a real line span yet; line 1 is the
        // documented placeholder. Keep the two vecs the same length so
        // `hits` (derived from `lines.len()`) stays accurate.
        self.lines.push(1);
        self.details.push(detail);
    }
    fn extend_lines(&mut self, lines: Vec<u32>) {
        self.lines.extend(lines.iter().copied());
        self.details
            .extend(std::iter::repeat_with(String::new).take(lines.len()));
    }
}

// ── Core engine ─────────────────────────────────────────────────────────────

/// Returns `true` if all gate predicates in `rule` pass for `path`, and every
/// condition uses a predicate that audit can evaluate. Returns `false` if any
/// gate predicate fails or if any condition uses an unsupported predicate
/// (one that requires diff context, e.g. `function_added`). Content predicates
/// (`new_content_contains`) and AST predicates emitted by `SyntaxFacts::all_facts`
/// are both considered "supported" here and skipped — they're evaluated in the
/// scan loop.
fn rule_applies_to_file(rule: &DiskRule, path: &Path, line_count: usize) -> bool {
    for cond in &rule.conditions {
        match cond.predicate.as_str() {
            "new_content_contains" => {
                // Content predicate — evaluated separately in the scan loop.
                continue;
            }
            "file_path_matches" => {
                let needle = match cond.args.first() {
                    Some(s) => s,
                    None => return false,
                };
                if !path.to_string_lossy().contains(needle.as_str()) {
                    return false;
                }
            }
            "file_extension_is" => {
                let wanted = match cond.args.first() {
                    Some(s) => s.as_str(),
                    None => return false,
                };
                let got = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                if got != wanted {
                    return false;
                }
            }
            "file_line_count_above" => {
                let threshold: usize = match cond.args.first().and_then(|s| s.parse().ok()) {
                    Some(n) => n,
                    None => return false,
                };
                if line_count <= threshold {
                    return false;
                }
            }
            other if is_ast_predicate(other) => {
                // AST predicate — evaluated separately in the scan loop by
                // matching against facts from `SyntaxFacts::all_facts(path)`.
                continue;
            }
            _ => {
                // Any other predicate (e.g. diff-only predicates like
                // `function_added`) — audit can't evaluate it; skip the rule.
                return false;
            }
        }
    }
    true
}

/// Predicates emitted by `SyntaxFacts::all_facts`. Membership is the
/// criterion for "audit can evaluate this rule via the syntax extractor."
/// The source of truth is `SyntaxFacts::PREDICATES`; a test in
/// `syntax::facts::tests::predicates_const_matches_all_facts_emission_set`
/// guards against drift between the const and the emission blocks.
fn is_ast_predicate(predicate: &str) -> bool {
    crate::syntax::facts::SyntaxFacts::PREDICATES.contains(&predicate)
}

/// True if `rule` has at least one condition whose predicate is an AST
/// predicate evaluated via `SyntaxFacts`. Used to decide whether the audit
/// loop needs to lazily parse the file's syntax.
fn rule_has_ast_predicate(rule: &DiskRule) -> bool {
    rule.conditions
        .iter()
        .any(|c| is_ast_predicate(&c.predicate))
}

/// Build a human-readable per-hit detail string from an AST fact's args.
/// `args[0]` is the file path (already shown by the renderer); `args[1]`
/// is the function/entity name; for count predicates `args[2]` is the
/// threshold count. Returns `None` only for a shapeless fact (none of the
/// current AST predicates are shapeless — guard anyway).
fn ast_hit_detail(predicate: &str, args: &[String]) -> Option<String> {
    // Count predicates: render "name (N unit)". The unit label is
    // predicate-specific so the line reads naturally, e.g.
    // "ladder (8 let bindings)" rather than a bare number.
    let unit = match predicate {
        "function_let_binding_count_high" => Some("let bindings"),
        "function_let_mut_count_high" => Some("let mut decls"),
        "function_param_count_high"
        | "python_function_param_count_high"
        | "ts_function_param_count_high" => Some("params"),
        "function_clone_count" | "function_clone_count_high" => Some("clones"),
        _ => None,
    };
    if let Some(unit) = unit {
        let name = args.get(1)?;
        let count = args.get(2).map(|s| s.as_str()).unwrap_or("?");
        return Some(format!("{name} ({count} {unit})"));
    }
    // Everything else: surface the name plus any trailing args (param
    // name, type, trait) so the hit still points at something grep-able.
    // `args[0]` is the path; skip it.
    let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
    if rest.is_empty() {
        None
    } else {
        Some(rest.join(" "))
    }
}

/// True if `rule` has no content-matching predicates — only gates. For
/// such rules the audit emits a single "whole-file" hit at line 1 when
/// the gates pass, rather than scanning lines.
fn is_whole_file_rule(rule: &DiskRule) -> bool {
    rule.conditions
        .iter()
        .all(|c| c.predicate != "new_content_contains")
}

/// True if the file's top-level `//!` doc-comment carries an exemption
/// marker for `rule_id`. Looks for a line of the form
/// `//! phronesis-allow: <rule-id>[ <free-form reason>]` anywhere in the
/// leading run of `//!` doc-comment lines (allowing blank lines between).
/// Stops scanning at the first non-blank, non-`//!` line.
fn file_exempts_rule(lines: &[&str], rule_id: &str) -> bool {
    for line in lines {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("//!") {
            let body = rest.trim();
            if let Some(after_marker) = body.strip_prefix("phronesis-allow:") {
                // Match exemption rule-id, treating whatever comes after
                // (space or end-of-line) as a separator. Allows trailing
                // free-form reason text.
                let after_marker = after_marker.trim_start();
                if after_marker == rule_id
                    || after_marker
                        .strip_prefix(rule_id)
                        .map(|tail| tail.starts_with(|c: char| c.is_whitespace()))
                        .unwrap_or(false)
                {
                    return true;
                }
            }
        } else {
            // First non-blank, non-`//!` line — end of the leading
            // doc-comment block; nothing more to check.
            return false;
        }
    }
    false
}

/// True if the lines above index `i` end with a `///` doc-comment block,
/// after skipping past blank lines and other stacked `#[...]` attribute
/// lines. Lets a documented `#[allow(...)]` survive even when interleaved
/// with siblings like `#[serde(default)]`. Used by rules that opt into
/// `doc_excepted: true`.
fn line_preceded_by_doc_comment(lines: &[&str], i: usize) -> bool {
    for j in (0..i).rev() {
        let trimmed = lines[j].trim_start();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("#[") {
            continue;
        }
        return trimmed.starts_with("///");
    }
    false
}

/// True if `id` is selected by `--rule <filter>`. Matches the id exactly,
/// or any `or`-branch expansion of it (`<filter>#or0`, `<filter>#or1`,
/// `<filter>#or0-or1`, ...) — a rule whose `when` carries an `or` clause is
/// stored on disk as one rule per branch (see `rules_file::unfold_or`), and
/// users name the rule, not the branch.
pub fn rule_matches_filter(id: &str, filter: &str) -> bool {
    if id == filter {
        return true;
    }
    let Some(rest) = id.strip_prefix(filter).and_then(|r| r.strip_prefix('#')) else {
        return false;
    };
    rest.split('-').all(|seg| {
        seg.strip_prefix("or")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// Filter `rules` to those opted into the audit, honoring `rule_filter`.
fn filter_audit_rules<'a>(rules: &'a RulesFile, rule_filter: Option<&str>) -> Vec<&'a DiskRule> {
    rules
        .rules
        .iter()
        .filter(|r| r.audit == Some(true))
        .filter(|r| rule_filter.is_none_or(|f| rule_matches_filter(&r.id, f)))
        .collect()
}

/// Evaluate the AST-predicate branch for a single rule on a single file.
/// Returns `Some(hits)` if any AST facts matched, `None` if no hits.
/// Lazily populates `ast_facts` on first call per file.
fn eval_ast_rule(
    rule: &DiskRule,
    path_str: &str,
    content: &str,
    ast_facts: &mut Option<Vec<Fact>>,
) -> Option<PerFileHits> {
    let facts =
        ast_facts.get_or_insert_with(|| syntax::extract(path_str, content).all_facts(path_str));
    let mut hits = PerFileHits::default();
    for cond in &rule.conditions {
        if !is_ast_predicate(&cond.predicate) {
            continue;
        }
        let matching_facts: Vec<&Fact> = facts
            .iter()
            .filter(|fact| {
                fact.predicate == cond.predicate
                    && fact.args.len() == cond.args.len()
                    && cond
                        .args
                        .iter()
                        .zip(&fact.args)
                        .all(|(wanted, got)| wanted.starts_with('?') || wanted == got)
            })
            .collect();
        if matching_facts.is_empty() {
            return None;
        }
        for fact in matching_facts {
            // Per-fact line spans aren't tracked yet; line 1 is the
            // placeholder. The fact's args carry the function name (and
            // count, for count predicates), which `ast_hit_detail` renders
            // into the per-hit detail string the renderer surfaces.
            let detail = ast_hit_detail(&cond.predicate, &fact.args);
            match detail {
                Some(d) => hits.push_detail(d),
                None => hits.push_line(1),
            }
        }
    }
    if hits.lines.is_empty() {
        None
    } else {
        Some(hits)
    }
}

/// Context for `eval_content_rule` so the function stays at two logical
/// parameters instead of five.
struct ContentEvalCtx<'a> {
    lines: &'a [&'a str],
    keep_mask: &'a Option<Vec<bool>>,
    doc_excepted: bool,
    times: Option<&'a mut AuditSectionTimes>,
}

fn eval_content_rule(needle: &str, mut ctx: ContentEvalCtx<'_>) -> Vec<u32> {
    let mut hit_lines: Vec<u32> = Vec::new();
    for (i, line) in ctx.lines.iter().enumerate() {
        if let Some(mask) = ctx.keep_mask
            && !mask.get(i).copied().unwrap_or(true)
        {
            continue;
        }
        if let Some(t) = ctx.times.as_deref_mut() {
            t.line_matches_evaluated += 1;
        }
        let count = line.matches(needle).count();
        if count > 0 && ctx.doc_excepted && line_preceded_by_doc_comment(ctx.lines, i) {
            continue;
        }
        for _ in 0..count {
            hit_lines.push((i + 1) as u32);
        }
    }
    hit_lines
}

/// Context bundling per-file data shared by `evaluate_rule_for_file`.
/// Keeps the public audit surface small; evaluation needs just
/// `EvalCtx` + `DiskRule` + accumulator.
struct EvalCtx<'a> {
    path: &'a Path,
    path_str: &'a str,
    content: &'a str,
    lines: &'a [&'a str],
    keep_mask: &'a Option<Vec<bool>>,
    effective_line_count: usize,
    ast_facts: &'a mut Option<Vec<Fact>>,
    times: Option<&'a mut AuditSectionTimes>,
}

/// Apply one rule's actions against one file's pre-parsed data, writing any
/// hits into `accum`.
fn evaluate_rule_for_file(
    rule: &DiskRule,
    ctx: &mut EvalCtx<'_>,
    accum: &mut BTreeMap<String, (Level, BTreeMap<PathBuf, PerFileHits>)>,
) {
    if !rule_applies_to_file(rule, ctx.path, ctx.effective_line_count) {
        return;
    }
    if rule.doc_excepted.unwrap_or(false) && file_exempts_rule(ctx.lines, &rule.id) {
        return;
    }
    for action in &rule.actions {
        let Some(level) = Level::from_action_type(&action.action_type) else {
            continue;
        };

        if rule_has_ast_predicate(rule) {
            if let Some(hits) = eval_ast_rule(rule, ctx.path_str, ctx.content, ctx.ast_facts) {
                let slot = accum
                    .entry(rule.id.clone())
                    .or_insert_with(|| (level, BTreeMap::new()))
                    .1
                    .entry(ctx.path.to_path_buf())
                    .or_default();
                slot.lines.extend(hits.lines);
                slot.details.extend(hits.details);
            }
            continue;
        }

        if is_whole_file_rule(rule) {
            accum
                .entry(rule.id.clone())
                .or_insert_with(|| (level, BTreeMap::new()))
                .1
                .entry(ctx.path.to_path_buf())
                .or_default()
                .push_line(1);
            continue;
        }

        for cond in &rule.conditions {
            if cond.predicate != "new_content_contains" {
                continue;
            }
            let Some(needle) = cond.args.first() else {
                continue;
            };
            let cctx = ContentEvalCtx {
                lines: ctx.lines,
                keep_mask: ctx.keep_mask,
                doc_excepted: rule.doc_excepted.unwrap_or(false),
                times: ctx.times.as_deref_mut(),
            };
            let hit_lines = eval_content_rule(needle.as_str(), cctx);
            if hit_lines.is_empty() {
                continue;
            }
            accum
                .entry(rule.id.clone())
                .or_insert_with(|| (level, BTreeMap::new()))
                .1
                .entry(ctx.path.to_path_buf())
                .or_default()
                .extend_lines(hit_lines);
        }
    }
}

/// Per-file scan body: runs all `rules` against `content`, accumulating hits
/// into `accum`.
struct ScanFileInput<'a> {
    path: &'a Path,
    content: &'a str,
    rules: &'a [&'a DiskRule],
    accum: &'a mut BTreeMap<String, (Level, BTreeMap<PathBuf, PerFileHits>)>,
    times: Option<&'a mut AuditSectionTimes>,
}

fn scan_file_into_accum(input: ScanFileInput<'_>) {
    let ScanFileInput {
        path,
        content,
        rules,
        accum,
        mut times,
    } = input;
    let lines: Vec<&str> = content.lines().collect();

    let keep_mask: Option<Vec<bool>> = {
        let started = Instant::now();
        let mask = if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            Some(crate::diff_extract::rust_test_block_keep_mask_for(content))
        } else {
            None
        };
        if let Some(t) = times.as_deref_mut() {
            t.keep_mask += started.elapsed();
        }
        mask
    };

    let effective_line_count = match &keep_mask {
        Some(mask) => mask.iter().filter(|&&keep| keep).count(),
        None => lines.len(),
    };

    let mut ast_facts: Option<Vec<Fact>> = None;
    let path_str = path.to_string_lossy().to_string();

    let match_started = Instant::now();
    {
        let mut ctx = EvalCtx {
            path,
            path_str: &path_str,
            content,
            lines: &lines,
            keep_mask: &keep_mask,
            effective_line_count,
            ast_facts: &mut ast_facts,
            times: times.as_deref_mut(),
        };

        for rule in rules {
            evaluate_rule_for_file(rule, &mut ctx, accum);
        }
    }
    if let Some(t) = times {
        t.match_loop += match_started.elapsed();
    }
}

/// Collapse the per-file accumulator into a sorted `Vec<RuleAudit>`.
fn build_per_rule(
    accum: BTreeMap<String, (Level, BTreeMap<PathBuf, PerFileHits>)>,
) -> Vec<RuleAudit> {
    let mut per_rule: Vec<RuleAudit> = accum
        .into_iter()
        .map(|(rule_id, (level, by_path))| {
            let files: Vec<FileAudit> = by_path
                .into_iter()
                .map(|(path, hits)| FileAudit {
                    path,
                    lines: hits.lines,
                    details: hits.details,
                })
                .collect();
            let hits: u32 = files.iter().map(|f| f.lines.len() as u32).sum();
            RuleAudit {
                rule_id: rule_id.into(),
                level,
                hits,
                files,
            }
        })
        .collect();
    per_rule.sort_by(rule_report_order);
    per_rule
}

/// Report ordering: blocks before warnings, then most hits, then rule id.
///
/// Shared by the file-scan pass and the structural merge so a merged report
/// cannot end up sorted two different ways.
fn rule_report_order(a: &RuleAudit, b: &RuleAudit) -> std::cmp::Ordering {
    let lvl = match (a.level, b.level) {
        (Level::Block, Level::Warn) => std::cmp::Ordering::Less,
        (Level::Warn, Level::Block) => std::cmp::Ordering::Greater,
        _ => std::cmp::Ordering::Equal,
    };
    lvl.then_with(|| b.hits.cmp(&a.hits))
        .then_with(|| a.rule_id.cmp(&b.rule_id))
}

/// Shared scan core used by both [`run`] and [`run_profiled`].
/// When `times` is `Some`, timing points are recorded; when `None`
/// the `Instant::now()` calls still execute (unconditionally) but
/// stores are gated so there is no semantic difference for the caller.
fn run_core(
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

// ── Wrapper diagnostics ─────────────────────────────────────────────────────

/// Explain a no-hits audit result when a recoverable misconfiguration is the
/// likely cause. Returns `Some(message)` for two cases:
///
/// 1. `audit_tagged_count == 0` — rules exist on disk but none carry
///    `audit: true`, so the walker never starts. The audit's JSON shape
///    (`{files_scanned: 0, rules: []}`) looks identical to a tool failure
///    from the outside; this message names the recoverable cause.
///
/// 2. `audit_tagged_count > 0 && files_scanned == 0` — rules are opted in
///    but the walker found nothing under `scan_root`. Almost always an
///    over-broad `.gitignore` / `.phronesisignore` or a misrouted `path`
///    argument.
///
/// 3. `rule_filter` names a rule no opted-in rule matches (exactly or via
///    an `#orN` expansion) — the walker never starts because the rule set
///    is empty, which would otherwise be misreported as case 2.
///
/// Returns `None` when the report has hits, or when zero hits is the
/// honest answer (rules opted in, files scanned, nothing matched).
///
/// Wrappers route the message to stderr (CLI) or prepend to the response
/// body (MCP). Audit's structured shape stays unchanged.
pub fn empty_result_diagnostic(
    report: &AuditReport,
    rules: &RulesFile,
    rule_filter: Option<&str>,
    scan_root: &Path,
) -> Option<String> {
    if !report.per_rule.is_empty() {
        return None;
    }
    let opted_in: Vec<&str> = rules
        .rules
        .iter()
        .filter(|r| r.audit == Some(true))
        .map(|r| r.id.as_str())
        .collect();
    let audit_tagged_count = opted_in.len();
    if let Some(filter) = rule_filter
        && audit_tagged_count > 0
        && !opted_in.iter().any(|id| rule_matches_filter(id, filter))
    {
        let near: Vec<String> = near_miss_rule_ids(&opted_in, filter);
        let hint = if near.is_empty() {
            String::new()
        } else {
            format!(" Did you mean: {}?", near.join(", "))
        };
        return Some(format!(
            "phronesis: no opted-in rule matches `{filter}` ({audit_tagged_count} rule(s) \
             carry `audit: true`).{hint}"
        ));
    }
    if audit_tagged_count == 0 {
        return Some(
            "phronesis: no rules have `audit: true` on disk. rules.json holds rules, \
             but none are opted into the whole-tree audit — so the walker never starts. \
             Add `\"audit\": true` to the rules you want surfaced here, or re-run \
             `phr-mcp init --rules-only --force` to refresh the starter pack."
                .to_string(),
        );
    }
    if report.files_scanned == 0 {
        return Some(format!(
            "phronesis: {} opted-in rule(s) on disk but walked 0 files under {}. \
             Check `.gitignore` / `.phronesisignore` for over-broad patterns, \
             or verify the `path` argument resolves to your source tree.",
            audit_tagged_count,
            scan_root.display(),
        ));
    }
    None
}

/// Cheap near-miss suggestions for an unmatched `--rule` filter: opted-in
/// ids (with any `#orN` suffix stripped, deduplicated) where one of the
/// two strings contains the other, case-insensitively. Capped at five.
fn near_miss_rule_ids(opted_in: &[&str], filter: &str) -> Vec<String> {
    let needle = filter.to_ascii_lowercase();
    let mut out: Vec<String> = Vec::new();
    for id in opted_in {
        let base = id.split_once("#or").map_or(*id, |(b, _)| b);
        let hay = base.to_ascii_lowercase();
        if (hay.contains(&needle) || needle.contains(&hay)) && !out.iter().any(|o| o == base) {
            out.push(base.to_string());
        }
    }
    out.sort();
    out.truncate(5);
    out
}

// ── Trend types and compute_trend ───────────────────────────────────────────

use crate::action_log::LogEntry;

#[derive(Debug, Clone, Default)]
pub struct TrendOpts {
    /// Most-recent N snapshots. Default: all available.
    pub last: Option<usize>,
    /// Window in seconds (overrides `last` when set).
    pub since_secs: Option<u64>,
    pub rule_filter: Option<String>,
    /// Unix seconds, injected so tests are deterministic.
    pub now_secs: u64,
}

#[derive(Debug, Clone)]
pub struct DebtTrend {
    pub generated_at: u64,
    pub snapshots_considered: u32,
    pub first_snapshot_ts: u64,
    pub last_snapshot_ts: u64,
    /// Sorted by `net_change` ascending (biggest improvements first).
    pub rules: Vec<RuleTrend>,
}

#[derive(Debug, Clone)]
pub struct RuleTrend {
    pub rule_id: RuleId,
    pub level: Level,
    pub history: Vec<TrendPoint>,
    pub first_hits: u32,
    pub last_hits: u32,
    /// `last_hits - first_hits`. Negative = improvement.
    pub net_change: i32,
}

#[derive(Debug, Clone)]
pub struct TrendPoint {
    pub ts: u64,
    /// `None` when the rule wasn't present in this snapshot. Renderers
    /// show this as `–`.
    pub hits: Option<u32>,
}

/// Accumulate per-rule trend data from a windowed set of audit snapshots.
/// Respects `rule_filter`; returns sorted `Vec<RuleTrend>` (improvements first).
fn rule_trends(snapshots: &[&LogEntry], rule_filter: Option<&str>) -> Vec<RuleTrend> {
    let mut rule_ids: BTreeMap<String, Level> = BTreeMap::new();
    for snap in snapshots {
        let Some(per_rule) = snap.data.get("per_rule").and_then(|v| v.as_object()) else {
            continue;
        };
        for (id, v) in per_rule {
            if let Some(filter) = rule_filter
                && id != filter
            {
                continue;
            }
            let rank_str = v.get("level").and_then(|x| x.as_str()).unwrap_or("");
            let level = match rank_str {
                "warn" => Level::Warn,
                _ => Level::Block,
            };
            rule_ids.entry(id.clone()).or_insert(level);
        }
    }
    let mut rules: Vec<RuleTrend> = rule_ids
        .into_iter()
        .map(|(rule_id, level)| {
            let history: Vec<TrendPoint> = snapshots
                .iter()
                .map(|snap| {
                    let hits = snap
                        .data
                        .get("per_rule")
                        .and_then(|v| v.as_object())
                        .and_then(|m| m.get(&rule_id))
                        .and_then(|v| v.get("hits"))
                        .and_then(|v| v.as_u64())
                        .map(|n| n as u32);
                    TrendPoint { ts: snap.ts, hits }
                })
                .collect();
            // first_hits = first non-None in history; last_hits = last non-None.
            let first_hits = history.iter().find_map(|p| p.hits).unwrap_or(0);
            let last_hits = history.iter().rev().find_map(|p| p.hits).unwrap_or(0);
            let net_change = (last_hits as i32) - (first_hits as i32);
            RuleTrend {
                rule_id: rule_id.into(),
                level,
                history,
                first_hits,
                last_hits,
                net_change,
            }
        })
        .collect();
    // Sort: improvements first (most-negative net_change), then by rule id.
    rules.sort_by(|a, b| {
        a.net_change
            .cmp(&b.net_change)
            .then_with(|| a.rule_id.cmp(&b.rule_id))
    });
    rules
}

/// Compute a debt-over-time view from a slice of audit log entries.
/// `entries` may contain non-`audit_codebase` events; they're skipped.
/// Snapshots are taken in chronological order; `last`/`since_secs` slice
/// the most recent window. Rules absent from a snapshot get a `None`
/// `TrendPoint` at that timestamp (no zero-substitution).
pub fn compute_trend(entries: &[LogEntry], opts: &TrendOpts) -> DebtTrend {
    // Resolve wall-clock `now`, filter to audit snapshots, and apply the
    // window (since_secs or last-N) in one block so the temporaries
    // (`cutoff`, `skip`) don't leak into the outer scope.
    let (now, snapshots) = {
        let now = if opts.now_secs == 0 {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        } else {
            opts.now_secs
        };
        let mut snaps: Vec<&LogEntry> = entries
            .iter()
            .filter(|e| e.event == "audit_codebase")
            .collect();
        snaps.sort_by_key(|e| e.ts);
        if let Some(since) = opts.since_secs {
            let cutoff = now.saturating_sub(since);
            snaps.retain(|e| e.ts >= cutoff);
        } else if let Some(n) = opts.last
            && snaps.len() > n
        {
            let skip = snaps.len() - n;
            snaps.drain(0..skip);
        }
        (now, snaps)
    };

    if snapshots.is_empty() {
        return DebtTrend {
            generated_at: now,
            snapshots_considered: 0,
            first_snapshot_ts: 0,
            last_snapshot_ts: 0,
            rules: Vec::new(),
        };
    }

    let first_ts = snapshots
        .first()
        .expect("snapshots non-empty: early-return above guards this")
        .ts;
    let last_ts = snapshots
        .last()
        .expect("snapshots non-empty: early-return above guards this")
        .ts;
    let rules = rule_trends(&snapshots, opts.rule_filter.as_deref());

    DebtTrend {
        generated_at: now,
        snapshots_considered: snapshots.len() as u32,
        first_snapshot_ts: first_ts,
        last_snapshot_ts: last_ts,
        rules,
    }
}

// ── Renderers ────────────────────────────────────────────────────────────────

use serde_json::json;

/// Render an `AuditReport` as a human-readable terminal table.
/// `expand` switches from per-rule summary to per-file detail with line numbers.
pub fn render_table(report: &AuditReport, expand: bool) -> String {
    if report.per_rule.is_empty() {
        return format!(
            "no audit violations found ({} files scanned in {}ms)\n",
            report.files_scanned, report.scan_duration_ms
        );
    }

    let id_width = report
        .per_rule
        .iter()
        .map(|r| r.rule_id.as_str().len())
        .max()
        .unwrap_or(0)
        .max("Rule".len());

    let mut out = String::new();
    out.push_str(&format!(
        "{:<id_width$}  Level  Hits  Files\n",
        "Rule",
        id_width = id_width
    ));
    for r in &report.per_rule {
        out.push_str(&format!(
            "{:<id_width$}  {:<5}  {:>4}  {:>5}\n",
            r.rule_id,
            r.level.as_str(),
            r.hits,
            r.files.len(),
            id_width = id_width,
        ));
        if expand {
            for f in &r.files {
                // AST-predicate hits carry a per-function detail and a
                // meaningless placeholder line of `1` per hit; surface
                // the names instead of `lines: 1, 1, ...`. Content and
                // whole-file hits have empty details and real line
                // numbers, so they keep the `lines:` form.
                let has_details = f.details.iter().any(|d| !d.is_empty());
                if has_details {
                    let details_str = f
                        .details
                        .iter()
                        .filter(|d| !d.is_empty())
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ");
                    out.push_str(&format!(
                        "    {} \u{2014} {}\n",
                        f.path.display(),
                        details_str
                    ));
                } else {
                    let lines_str = f
                        .lines
                        .iter()
                        .map(|n| n.to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    out.push_str(&format!(
                        "    {} \u{2014} lines: {}\n",
                        f.path.display(),
                        lines_str
                    ));
                }
            }
        }
    }

    let total_blocked: u32 = report
        .per_rule
        .iter()
        .filter(|r| r.level == Level::Block)
        .map(|r| r.hits)
        .sum();
    let total_warned: u32 = report
        .per_rule
        .iter()
        .filter(|r| r.level == Level::Warn)
        .map(|r| r.hits)
        .sum();
    out.push('\n');
    out.push_str(&format!(
        "Total: {} blocked, {} warned across {} rules, {} files scanned in {}ms\n",
        total_blocked,
        total_warned,
        report.per_rule.len(),
        report.files_scanned,
        report.scan_duration_ms,
    ));
    out
}

/// Render an `AuditReport` as JSON. Stable shape: `{generated_at,
/// scan_duration_ms, files_scanned, totals:{blocked,warned,rules},
/// rules:[{rule_id, level, hits, files:[{path,lines,details}]}]}`.
/// `details` is a per-hit array parallel to `lines`; entries are empty
/// strings for content/whole-file hits (which carry real line numbers)
/// and human-readable strings like `"ladder (9 let bindings)"` for
/// AST-predicate hits.
pub fn render_json(report: &AuditReport) -> String {
    let total_blocked: u32 = report
        .per_rule
        .iter()
        .filter(|r| r.level == Level::Block)
        .map(|r| r.hits)
        .sum();
    let total_warned: u32 = report
        .per_rule
        .iter()
        .filter(|r| r.level == Level::Warn)
        .map(|r| r.hits)
        .sum();
    let rules: Vec<_> = report
        .per_rule
        .iter()
        .map(|r| {
            let files: Vec<_> = r
                .files
                .iter()
                .map(|f| {
                    json!({
                        "path": f.path.display().to_string(),
                        "lines": f.lines,
                        "details": f.details,
                    })
                })
                .collect();
            json!({
                "rule_id": r.rule_id,
                "level": r.level.as_str(),
                "hits": r.hits,
                "files": files,
            })
        })
        .collect();
    let payload = json!({
        "generated_at": report.generated_at,
        "scan_duration_ms": report.scan_duration_ms,
        "files_scanned": report.files_scanned,
        "totals": {
            "blocked": total_blocked,
            "warned": total_warned,
            "rules": report.per_rule.len(),
        },
        "rules": rules,
    });
    payload.to_string()
}

/// Render a `DebtTrend` as a human-readable terminal table.
///
/// Columns: rule id, one column per snapshot (ISO date), then delta.
/// Missing values render as an em-dash. Zero net change shows as `0 ·`.
pub fn render_trend_table(trend: &DebtTrend) -> String {
    if trend.snapshots_considered == 0 {
        return "no audit snapshots recorded yet; run audit_codebase to take the first one\n"
            .to_string();
    }
    if trend.rules.is_empty() {
        return "no rules tracked in any snapshot\n".to_string();
    }

    let id_width = trend
        .rules
        .iter()
        .map(|r| r.rule_id.as_str().len())
        .max()
        .unwrap_or(0)
        .max("Rule".len());

    // Column headers: ISO short date for each snapshot timestamp.
    let date_labels: Vec<String> = trend.rules[0]
        .history
        .iter()
        .map(|p| short_iso_date(p.ts))
        .collect();

    let mut out = String::new();
    out.push_str(&format!("{:<id_width$}", "Rule", id_width = id_width));
    for label in &date_labels {
        out.push_str(&format!("  {:>10}", label));
    }
    out.push_str("  \u{0394}\n");

    for r in &trend.rules {
        out.push_str(&format!("{:<id_width$}", r.rule_id, id_width = id_width));
        for p in &r.history {
            match p.hits {
                Some(h) => out.push_str(&format!("  {:>10}", h)),
                None => out.push_str(&format!("  {:>10}", "\u{2013}")),
            }
        }
        let arrow = if r.net_change < 0 {
            " \u{2193}"
        } else if r.net_change > 0 {
            " \u{2191}"
        } else {
            " \u{00b7}"
        };
        let signed = if r.net_change > 0 {
            format!("+{}", r.net_change)
        } else {
            r.net_change.to_string()
        };
        out.push_str(&format!("  {}{}\n", signed, arrow));
    }

    out.push('\n');
    if trend.snapshots_considered < 2 {
        out.push_str("(need at least 2 snapshots to compute trend)\n");
    } else {
        out.push_str(&format!(
            "{} snapshots considered, {} \u{2192} {}\n",
            trend.snapshots_considered,
            short_iso_date(trend.first_snapshot_ts),
            short_iso_date(trend.last_snapshot_ts),
        ));
    }
    out
}

/// Render a `DebtTrend` as JSON.
pub fn render_trend_json(trend: &DebtTrend) -> String {
    let rules: Vec<_> = trend
        .rules
        .iter()
        .map(|r| {
            let history: Vec<_> = r
                .history
                .iter()
                .map(|p| {
                    json!({
                        "ts": p.ts,
                        "hits": p.hits,
                    })
                })
                .collect();
            json!({
                "rule_id": r.rule_id,
                "level": r.level.as_str(),
                "history": history,
                "first_hits": r.first_hits,
                "last_hits": r.last_hits,
                "net_change": r.net_change,
            })
        })
        .collect();
    let payload = json!({
        "generated_at": trend.generated_at,
        "snapshots_considered": trend.snapshots_considered,
        "first_snapshot_ts": trend.first_snapshot_ts,
        "last_snapshot_ts": trend.last_snapshot_ts,
        "rules": rules,
    });
    payload.to_string()
}

pub(crate) fn short_iso_date(ts: u64) -> String {
    // YYYY-MM-DD without pulling chrono.
    let days = (ts / 86_400) as i64;
    let (y, m, d) = days_to_ymd(days);
    format!("{:04}-{:02}-{:02}", y, m, d)
}

// Civil calendar conversion from Howard Hinnant's date algorithms.
// http://howardhinnant.github.io/date_algorithms.html#civil_from_days
fn days_to_ymd(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let (y, mp, d) = {
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = (z - era * 146_097) as u64; // [0, 146096]
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
        let y = yoe as i64 + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
        (y, mp, d)
    };
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules_file::{DiskAction, DiskCondition, DiskRule, RulesFile};

    fn rule(id: &str, content_match: &str, action: &str) -> DiskRule {
        DiskRule {
            id: id.to_string(),
            phase: "pre".to_string(),
            priority: 10,
            conditions: vec![DiskCondition {
                predicate: "new_content_contains".to_string(),
                args: vec![content_match.to_string()],
                script: None,
            }],
            actions: vec![DiskAction {
                action_type: action.to_string(),
                params: vec![format!("{} fired", id)],
                ..Default::default()
            }],
            silent: None,
            audit: Some(true),
            doc_excepted: None,
        }
    }

    #[test]
    fn run_finds_content_matches_in_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.rs"),
            "fn main() { let x = foo.unwrap(); }",
        )
        .unwrap();
        let rules = RulesFile {
            rules: vec![rule("no-unwrap", ".unwrap()", "constraint_violation")],
        };
        let report = run(
            &rules,
            &AuditOpts {
                project_root: dir.path().to_path_buf(),
                scan_root: dir.path().to_path_buf(),
                rule_filter: None,
            },
        );
        assert_eq!(report.per_rule.len(), 1);
        assert_eq!(report.per_rule[0].rule_id, "no-unwrap");
        assert_eq!(report.per_rule[0].hits, 1);
        assert_eq!(report.per_rule[0].level, Level::Block);
        assert_eq!(report.per_rule[0].files.len(), 1);
        assert_eq!(report.per_rule[0].files[0].lines, vec![1]);
    }

    #[test]
    fn run_skips_non_audit_rules() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), ".unwrap()").unwrap();
        let mut r = rule("no-unwrap", ".unwrap()", "constraint_violation");
        r.audit = None; // not opted in
        let rules = RulesFile { rules: vec![r] };
        let report = run(
            &rules,
            &AuditOpts {
                project_root: dir.path().to_path_buf(),
                scan_root: dir.path().to_path_buf(),
                rule_filter: None,
            },
        );
        assert!(report.per_rule.is_empty());
    }

    #[test]
    fn run_respects_rule_filter() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), ".unwrap() panic!()").unwrap();
        let rules = RulesFile {
            rules: vec![
                rule("no-unwrap", ".unwrap()", "constraint_violation"),
                rule("no-panic", "panic!(", "constraint_violation"),
            ],
        };
        let report = run(
            &rules,
            &AuditOpts {
                project_root: dir.path().to_path_buf(),
                scan_root: dir.path().to_path_buf(),
                rule_filter: Some("no-panic".to_string()),
            },
        );
        assert_eq!(report.per_rule.len(), 1);
        assert_eq!(report.per_rule[0].rule_id, "no-panic");
    }

    #[test]
    fn rule_filter_matches_or_expansions() {
        assert!(rule_matches_filter(
            "warn-untested-risky-call",
            "warn-untested-risky-call"
        ));
        assert!(rule_matches_filter(
            "warn-untested-risky-call#or0",
            "warn-untested-risky-call"
        ));
        assert!(rule_matches_filter(
            "warn-untested-risky-call#or12",
            "warn-untested-risky-call"
        ));
        assert!(rule_matches_filter(
            "warn-untested-risky-call#or0-or1",
            "warn-untested-risky-call"
        ));
        assert!(!rule_matches_filter(
            "warn-untested-risky-call#or0-",
            "warn-untested-risky-call"
        ));
        assert!(!rule_matches_filter(
            "warn-untested-risky-call#or",
            "warn-untested-risky-call"
        ));
        assert!(!rule_matches_filter(
            "warn-untested-risky-call-v2",
            "warn-untested-risky-call"
        ));
        assert!(!rule_matches_filter(
            "warn-untested",
            "warn-untested-risky-call"
        ));
    }

    #[test]
    fn run_rule_filter_selects_all_or_branches() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), ".unwrap() panic!()").unwrap();
        let rules = RulesFile {
            rules: vec![
                rule("risky#or0", ".unwrap()", "constraint_violation"),
                rule("risky#or1", "panic!(", "constraint_violation"),
                rule("other", "panic!(", "constraint_violation"),
            ],
        };
        let report = run(
            &rules,
            &AuditOpts {
                project_root: dir.path().to_path_buf(),
                scan_root: dir.path().to_path_buf(),
                rule_filter: Some("risky".to_string()),
            },
        );
        let mut ids: Vec<&str> = report.per_rule.iter().map(|r| r.rule_id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, vec!["risky#or0", "risky#or1"]);
    }

    #[test]
    fn empty_diagnostic_names_unmatched_rule_filter() {
        let rules = RulesFile {
            rules: vec![
                rule("warn-untested-risky-call#or0", "x", "warning"),
                rule("warn-untested-risky-call#or1", "x", "warning"),
                rule("no-unwrap", "x", "warning"),
            ],
        };
        let report = AuditReport {
            generated_at: 0,
            scan_duration_ms: 0,
            files_scanned: 0,
            per_rule: vec![],
        };
        let diag = empty_result_diagnostic(
            &report,
            &rules,
            Some("warn-untested-risky"),
            Path::new("/src"),
        )
        .expect("diagnostic");
        assert!(
            diag.contains("no opted-in rule matches `warn-untested-risky`"),
            "{diag}"
        );
        assert!(
            diag.contains("Did you mean: warn-untested-risky-call?"),
            "{diag}"
        );
        assert!(!diag.contains("walked 0 files"), "{diag}");

        // A matching filter (via #orN) with zero files scanned still reports
        // the walker diagnostic, not the unmatched-rule one.
        let diag = empty_result_diagnostic(
            &report,
            &rules,
            Some("warn-untested-risky-call"),
            Path::new("/src"),
        )
        .expect("diagnostic");
        assert!(diag.contains("walked 0 files"), "{diag}");
    }

    #[test]
    fn doc_excepted_rule_skips_matches_preceded_by_doc_comment() {
        let dir = tempfile::tempdir().unwrap();
        // Two `#[allow(dead_code)]` attributes: the first carries a
        // `///` doc-comment justification immediately above (should be
        // exempt); the second does not (should be flagged).
        std::fs::write(
            dir.path().join("a.rs"),
            "/// Documented exception: planned API surface.\n\
             #[allow(dead_code)]\n\
             struct A;\n\
             \n\
             #[allow(dead_code)]\n\
             struct B;\n",
        )
        .unwrap();
        let mut r = rule(
            "audit-allow-dead-code-in-src",
            "#[allow(dead_code)]",
            "constraint_warning",
        );
        r.doc_excepted = Some(true);
        let rules = RulesFile { rules: vec![r] };
        let report = run(
            &rules,
            &AuditOpts {
                project_root: dir.path().to_path_buf(),
                scan_root: dir.path().to_path_buf(),
                rule_filter: None,
            },
        );
        assert_eq!(report.per_rule.len(), 1);
        // Only one hit: the undocumented `#[allow(dead_code)]` on line 5.
        assert_eq!(report.per_rule[0].hits, 1);
        assert_eq!(report.per_rule[0].files[0].lines, vec![5]);
    }

    #[test]
    fn doc_excepted_rule_skips_past_stacked_attributes() {
        let dir = tempfile::tempdir().unwrap();
        // A `///` doc-comment block, followed by another attribute
        // (`#[serde(default)]`), then the `#[allow(dead_code)]`. The
        // exception walker should skip past the intermediate attribute
        // to find the doc-comment.
        std::fs::write(
            dir.path().join("a.rs"),
            "/// Documented exception.\n\
             #[serde(default)]\n\
             #[allow(dead_code)]\n\
             struct A;\n",
        )
        .unwrap();
        let mut r = rule(
            "audit-allow-dead-code-in-src",
            "#[allow(dead_code)]",
            "constraint_warning",
        );
        r.doc_excepted = Some(true);
        let rules = RulesFile { rules: vec![r] };
        let report = run(
            &rules,
            &AuditOpts {
                project_root: dir.path().to_path_buf(),
                scan_root: dir.path().to_path_buf(),
                rule_filter: None,
            },
        );
        assert!(
            report.per_rule.is_empty(),
            "doc-comment above stacked attributes should still exempt"
        );
    }

    #[test]
    fn doc_excepted_rule_with_blank_line_between_still_exempts() {
        let dir = tempfile::tempdir().unwrap();
        // A `///` doc-comment with one blank line between it and the
        // `#[allow(...)]` should still count as documentation — the
        // helper walks past blanks to find the nearest non-blank line.
        std::fs::write(
            dir.path().join("a.rs"),
            "/// Documented.\n\
             \n\
             #[allow(dead_code)]\n\
             struct A;\n",
        )
        .unwrap();
        let mut r = rule(
            "audit-allow-dead-code-in-src",
            "#[allow(dead_code)]",
            "constraint_warning",
        );
        r.doc_excepted = Some(true);
        let rules = RulesFile { rules: vec![r] };
        let report = run(
            &rules,
            &AuditOpts {
                project_root: dir.path().to_path_buf(),
                scan_root: dir.path().to_path_buf(),
                rule_filter: None,
            },
        );
        assert!(
            report.per_rule.is_empty(),
            "documented exception should be skipped even with blank line"
        );
    }

    #[test]
    fn doc_excepted_rule_does_not_exempt_when_flag_false() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.rs"),
            "/// Documented.\n#[allow(dead_code)]\nstruct A;\n",
        )
        .unwrap();
        // doc_excepted defaults to None/false → rule fires anyway.
        let r = rule(
            "audit-allow-dead-code-in-src",
            "#[allow(dead_code)]",
            "constraint_warning",
        );
        let rules = RulesFile { rules: vec![r] };
        let report = run(
            &rules,
            &AuditOpts {
                project_root: dir.path().to_path_buf(),
                scan_root: dir.path().to_path_buf(),
                rule_filter: None,
            },
        );
        assert_eq!(report.per_rule.len(), 1);
        assert_eq!(report.per_rule[0].hits, 1);
    }

    #[test]
    fn run_skips_lines_inside_test_blocks_for_rust_files() {
        let dir = tempfile::tempdir().unwrap();
        // Two hits: one in production (line 1) and one inside a #[cfg(test)]
        // module. Only the production hit should be reported.
        let content =
            "fn prod() { x.clone(); }\n\n#[cfg(test)]\nmod tests {\n    fn t() { y.clone(); }\n}\n";
        std::fs::write(dir.path().join("a.rs"), content).unwrap();
        let rules = RulesFile {
            rules: vec![rule("warn-clone", ".clone()", "constraint_warning")],
        };
        let report = run(
            &rules,
            &AuditOpts {
                project_root: dir.path().to_path_buf(),
                scan_root: dir.path().to_path_buf(),
                rule_filter: None,
            },
        );
        assert_eq!(report.per_rule.len(), 1, "rule must fire on production hit");
        assert_eq!(report.per_rule[0].hits, 1, "test-block hit must be skipped");
        assert_eq!(
            report.per_rule[0].files[0].lines,
            vec![1],
            "reported line should be the production line, not shifted by stripping"
        );
    }

    #[test]
    fn whole_file_rule_fires_once_per_matching_file_via_line_count_gate() {
        // file_line_count_above is a gate; with no content predicate the rule
        // is "whole-file" and should emit exactly one hit per matching file
        // at line 1.
        let dir = tempfile::tempdir().unwrap();
        let big = "x\n".repeat(100);
        let small = "x\n".repeat(5);
        std::fs::write(dir.path().join("big.rs"), &big).unwrap();
        std::fs::write(dir.path().join("small.rs"), &small).unwrap();

        let r = DiskRule {
            id: "audit-too-big".to_string(),
            phase: "audit".to_string(),
            priority: 3,
            conditions: vec![
                DiskCondition {
                    predicate: "file_extension_is".to_string(),
                    args: vec!["rs".to_string()],
                    script: None,
                },
                DiskCondition {
                    predicate: "file_line_count_above".to_string(),
                    args: vec!["50".to_string()],
                    script: None,
                },
            ],
            actions: vec![DiskAction {
                action_type: "constraint_warning".to_string(),
                params: vec!["too big".to_string()],
                ..Default::default()
            }],
            silent: None,
            audit: Some(true),
            doc_excepted: None,
        };
        let rules = RulesFile { rules: vec![r] };
        let report = run(
            &rules,
            &AuditOpts {
                project_root: dir.path().to_path_buf(),
                scan_root: dir.path().to_path_buf(),
                rule_filter: None,
            },
        );
        assert_eq!(report.per_rule.len(), 1);
        assert_eq!(report.per_rule[0].hits, 1, "only big.rs should fire");
        assert_eq!(report.per_rule[0].files.len(), 1);
        assert!(
            report.per_rule[0].files[0]
                .path
                .to_string_lossy()
                .ends_with("big.rs")
        );
        assert_eq!(report.per_rule[0].files[0].lines, vec![1]);
    }

    #[test]
    fn doc_excepted_whole_file_rule_skips_files_with_exemption_marker() {
        // A Rust file whose top-of-file `//!` doc-comment block carries a
        // `phronesis-allow: <rule-id>` marker should be exempt from the
        // named whole-file (gate-only) rule when the rule opts in.
        let dir = tempfile::tempdir().unwrap();
        let exempt_content = format!(
            "//! Module doc.\n//!\n//! phronesis-allow: audit-too-big (intentional god-file)\n\nfn x() {{}}\n{}",
            "let _ = 1;\n".repeat(100)
        );
        let plain_content = "let _ = 1;\n".repeat(102);
        std::fs::write(dir.path().join("exempt.rs"), &exempt_content).unwrap();
        std::fs::write(dir.path().join("plain.rs"), &plain_content).unwrap();

        let r = DiskRule {
            id: "audit-too-big".to_string(),
            phase: "audit".to_string(),
            priority: 3,
            conditions: vec![
                DiskCondition {
                    predicate: "file_extension_is".to_string(),
                    args: vec!["rs".to_string()],
                    script: None,
                },
                DiskCondition {
                    predicate: "file_line_count_above".to_string(),
                    args: vec!["50".to_string()],
                    script: None,
                },
            ],
            actions: vec![DiskAction {
                action_type: "constraint_warning".to_string(),
                params: vec!["too big".to_string()],
                ..Default::default()
            }],
            silent: None,
            audit: Some(true),
            doc_excepted: Some(true),
        };
        let rules = RulesFile { rules: vec![r] };
        let report = run(
            &rules,
            &AuditOpts {
                project_root: dir.path().to_path_buf(),
                scan_root: dir.path().to_path_buf(),
                rule_filter: None,
            },
        );
        // Only plain.rs should fire; exempt.rs carries the marker.
        assert_eq!(report.per_rule.len(), 1);
        assert_eq!(report.per_rule[0].hits, 1);
        assert!(
            report.per_rule[0].files[0]
                .path
                .to_string_lossy()
                .ends_with("plain.rs")
        );
    }

    #[test]
    fn doc_excepted_whole_file_marker_must_match_rule_id() {
        // An exemption marker naming a DIFFERENT rule must not exempt
        // the file from this rule.
        let dir = tempfile::tempdir().unwrap();
        let content = format!(
            "//! phronesis-allow: some-other-rule\n\n{}",
            "let _ = 1;\n".repeat(100)
        );
        std::fs::write(dir.path().join("a.rs"), &content).unwrap();
        let r = DiskRule {
            id: "audit-too-big".to_string(),
            phase: "audit".to_string(),
            priority: 3,
            conditions: vec![
                DiskCondition {
                    predicate: "file_extension_is".to_string(),
                    args: vec!["rs".to_string()],
                    script: None,
                },
                DiskCondition {
                    predicate: "file_line_count_above".to_string(),
                    args: vec!["50".to_string()],
                    script: None,
                },
            ],
            actions: vec![DiskAction {
                action_type: "constraint_warning".to_string(),
                params: vec!["too big".to_string()],
                ..Default::default()
            }],
            silent: None,
            audit: Some(true),
            doc_excepted: Some(true),
        };
        let rules = RulesFile { rules: vec![r] };
        let report = run(
            &rules,
            &AuditOpts {
                project_root: dir.path().to_path_buf(),
                scan_root: dir.path().to_path_buf(),
                rule_filter: None,
            },
        );
        assert_eq!(
            report.per_rule.len(),
            1,
            "marker for a different rule must not exempt this one"
        );
    }

    #[test]
    fn file_line_count_above_counts_production_lines_only_for_rust() {
        // A Rust file with 100 lines, half of which sit inside a
        // #[cfg(test)] mod tests block. With a threshold of 75, the rule
        // should NOT fire — production line count is 50.
        let dir = tempfile::tempdir().unwrap();
        let prod_lines = "let _ = 1;\n".repeat(50);
        let test_block = format!(
            "#[cfg(test)]\nmod tests {{\n{}\n}}\n",
            "    let _ = 1;\n".repeat(50)
        );
        std::fs::write(
            dir.path().join("a.rs"),
            format!("{}{}", prod_lines, test_block),
        )
        .unwrap();

        let r = DiskRule {
            id: "audit-file-loc-high".to_string(),
            phase: "audit".to_string(),
            priority: 3,
            conditions: vec![
                DiskCondition {
                    predicate: "file_extension_is".to_string(),
                    args: vec!["rs".to_string()],
                    script: None,
                },
                DiskCondition {
                    predicate: "file_line_count_above".to_string(),
                    args: vec!["75".to_string()],
                    script: None,
                },
            ],
            actions: vec![DiskAction {
                action_type: "constraint_warning".to_string(),
                params: vec!["too big".to_string()],
                ..Default::default()
            }],
            silent: None,
            audit: Some(true),
            doc_excepted: None,
        };
        let rules = RulesFile { rules: vec![r] };
        let report = run(
            &rules,
            &AuditOpts {
                project_root: dir.path().to_path_buf(),
                scan_root: dir.path().to_path_buf(),
                rule_filter: None,
            },
        );
        assert!(
            report.per_rule.is_empty(),
            "rule should not fire when prod LOC is under threshold even if total exceeds it"
        );
    }

    #[test]
    fn run_does_not_strip_test_blocks_for_non_rust_files() {
        // The mask is only applied to .rs files. For other extensions the
        // content goes through as-is (the test-block convention is
        // Rust-specific).
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.py"), "#[cfg(test)]\n.clone()").unwrap();
        let rules = RulesFile {
            rules: vec![rule("warn-clone", ".clone()", "constraint_warning")],
        };
        let report = run(
            &rules,
            &AuditOpts {
                project_root: dir.path().to_path_buf(),
                scan_root: dir.path().to_path_buf(),
                rule_filter: None,
            },
        );
        assert_eq!(report.per_rule[0].hits, 1);
    }

    #[test]
    fn run_distinguishes_warn_from_block() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), ".clone() .clone()").unwrap();
        let rules = RulesFile {
            rules: vec![rule("warn-clone", ".clone()", "constraint_warning")],
        };
        let report = run(
            &rules,
            &AuditOpts {
                project_root: dir.path().to_path_buf(),
                scan_root: dir.path().to_path_buf(),
                rule_filter: None,
            },
        );
        assert_eq!(report.per_rule[0].level, Level::Warn);
        assert_eq!(report.per_rule[0].hits, 2);
    }

    #[test]
    fn run_honors_file_path_matches_gate() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::create_dir_all(dir.path().join("target")).unwrap();
        std::fs::write(dir.path().join("src").join("a.rs"), ".unwrap()").unwrap();
        std::fs::write(dir.path().join("target").join("b.rs"), ".unwrap()").unwrap();

        let r = DiskRule {
            id: "no-unwrap-src".to_string(),
            phase: "pre".to_string(),
            priority: 10,
            conditions: vec![
                DiskCondition {
                    predicate: "new_content_contains".to_string(),
                    args: vec![".unwrap()".to_string()],
                    script: None,
                },
                DiskCondition {
                    predicate: "file_path_matches".to_string(),
                    args: vec!["src".to_string()],
                    script: None,
                },
            ],
            actions: vec![DiskAction {
                action_type: "constraint_violation".to_string(),
                params: vec!["no unwrap".to_string()],
                ..Default::default()
            }],
            silent: None,
            audit: Some(true),
            doc_excepted: None,
        };
        let rules = RulesFile { rules: vec![r] };
        let report = run(
            &rules,
            &AuditOpts {
                project_root: dir.path().to_path_buf(),
                scan_root: dir.path().to_path_buf(),
                rule_filter: None,
            },
        );
        assert_eq!(report.per_rule.len(), 1);
        assert_eq!(report.per_rule[0].hits, 1);
        // Only the src/ file should fire; target/ is filtered out by the gate.
        let path = &report.per_rule[0].files[0].path;
        assert!(path.to_string_lossy().contains("src"), "got {:?}", path);
        assert!(!path.to_string_lossy().contains("target"));
    }

    #[test]
    fn run_honors_file_extension_is_gate() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "print(\"hi\")").unwrap();
        std::fs::write(dir.path().join("b.rhai"), "print(\"hi\")").unwrap();

        let r = DiskRule {
            id: "no-rhai-print".to_string(),
            phase: "pre".to_string(),
            priority: 10,
            conditions: vec![
                DiskCondition {
                    predicate: "new_content_contains".to_string(),
                    args: vec!["print(".to_string()],
                    script: None,
                },
                DiskCondition {
                    predicate: "file_extension_is".to_string(),
                    args: vec!["rhai".to_string()],
                    script: None,
                },
            ],
            actions: vec![DiskAction {
                action_type: "constraint_violation".to_string(),
                params: vec!["no print in rhai".to_string()],
                ..Default::default()
            }],
            silent: None,
            audit: Some(true),
            doc_excepted: None,
        };
        let rules = RulesFile { rules: vec![r] };
        let report = run(
            &rules,
            &AuditOpts {
                project_root: dir.path().to_path_buf(),
                scan_root: dir.path().to_path_buf(),
                rule_filter: None,
            },
        );
        assert_eq!(report.per_rule.len(), 1);
        assert_eq!(report.per_rule[0].hits, 1);
        let path = &report.per_rule[0].files[0].path;
        assert!(path.to_string_lossy().ends_with("b.rhai"), "got {:?}", path);
    }

    #[test]
    fn run_skips_rule_with_unsupported_predicate() {
        // A rule mixing new_content_contains with a predicate that audit
        // can't evaluate (here `function_added`, which is a diff-context
        // predicate without a corresponding fact in SyntaxFacts::all_facts)
        // must be skipped entirely — not fire on content match alone, which
        // would be unsafe.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), ".unwrap()").unwrap();
        let r = DiskRule {
            id: "mixed".to_string(),
            phase: "pre".to_string(),
            priority: 10,
            conditions: vec![
                DiskCondition {
                    predicate: "new_content_contains".to_string(),
                    args: vec![".unwrap()".to_string()],
                    script: None,
                },
                DiskCondition {
                    predicate: "function_added".to_string(),
                    args: vec!["?file".to_string(), "?fn".to_string()],
                    script: None,
                },
            ],
            actions: vec![DiskAction {
                action_type: "constraint_violation".to_string(),
                params: vec!["nope".to_string()],
                ..Default::default()
            }],
            silent: None,
            audit: Some(true),
            doc_excepted: None,
        };
        let rules = RulesFile { rules: vec![r] };
        let report = run(
            &rules,
            &AuditOpts {
                project_root: dir.path().to_path_buf(),
                scan_root: dir.path().to_path_buf(),
                rule_filter: None,
            },
        );
        assert!(
            report.per_rule.is_empty(),
            "rule with unsupported predicate must be skipped: {:?}",
            report.per_rule
        );
    }

    // ── AST predicate evaluation in audit (Phase 3.5) ─────────────────────────

    fn ast_let_binding_rule() -> DiskRule {
        DiskRule {
            id: "audit-rust-let-binding-count-high".to_string(),
            phase: "audit".to_string(),
            priority: 3,
            conditions: vec![DiskCondition {
                predicate: "function_let_binding_count_high".to_string(),
                args: vec!["?file".to_string(), "?fn".to_string(), "?count".to_string()],
                script: None,
            }],
            actions: vec![DiskAction {
                action_type: "constraint_warning".to_string(),
                params: vec!["`?fn` in ?file has ?count outer-scope `let` bindings.".to_string()],
                ..Default::default()
            }],
            silent: None,
            audit: Some(true),
            doc_excepted: None,
        }
    }

    /// Mirror of `ast_let_binding_rule` for the let-mut predicate.
    /// Lets the audit tests exercise the let_mut variant of the
    /// block-pattern rule pair end-to-end through the AST branch.
    fn ast_let_mut_rule() -> DiskRule {
        DiskRule {
            id: "audit-rust-let-mut-count-high".to_string(),
            phase: "audit".to_string(),
            priority: 3,
            conditions: vec![DiskCondition {
                predicate: "function_let_mut_count_high".to_string(),
                args: vec!["?file".to_string(), "?fn".to_string(), "?count".to_string()],
                script: None,
            }],
            actions: vec![DiskAction {
                action_type: "constraint_warning".to_string(),
                params: vec![
                    "`?fn` in ?file has ?count outer-scope `let mut` declarations.".to_string(),
                ],
                ..Default::default()
            }],
            silent: None,
            audit: Some(true),
            doc_excepted: None,
        }
    }

    #[test]
    fn run_evaluates_python_and_typescript_ast_predicates() {
        // End-to-end: the .py/.ts extension dispatch reaches the new
        // tree-sitter extractors and their predicates audit like the
        // Rust ones do.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("svc.py"),
            "def fetch(url=load_default()):\n    \"\"\"F.\"\"\"\n    print(url)\n    try:\n        go(url)\n    except:\n        pass\n    try:\n        go(url)\n    except ValueError:\n        pass\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("svc.ts"),
            "function load(a: any): any { return a; }\n",
        )
        .unwrap();
        let mk = |id: &str, predicate: &str, args: Vec<&str>| DiskRule {
            id: id.to_string(),
            phase: "audit".to_string(),
            priority: 3,
            conditions: vec![DiskCondition {
                predicate: predicate.to_string(),
                args: args.into_iter().map(String::from).collect(),
                script: None,
            }],
            actions: vec![DiskAction {
                action_type: "constraint_warning".to_string(),
                params: vec![format!("{} hit in ?file", predicate)],
                ..Default::default()
            }],
            silent: None,
            audit: Some(true),
            doc_excepted: None,
        };
        let rules = RulesFile {
            rules: vec![
                mk(
                    "audit-python-bare-except",
                    "python_bare_except",
                    vec!["?file", "?fn"],
                ),
                mk(
                    "audit-python-print-call",
                    "python_print_call",
                    vec!["?file", "?fn"],
                ),
                mk(
                    "audit-python-call-default",
                    "python_call_in_default_arg",
                    vec!["?file", "?fn", "?param", "?callee"],
                ),
                mk(
                    "audit-python-handler-pass",
                    "python_exception_handler_passes",
                    vec!["?file", "?fn", "?exception"],
                ),
                mk(
                    "audit-ts-explicit-any",
                    "ts_explicit_any",
                    vec!["?file", "?fn", "?count"],
                ),
            ],
        };
        let report = run(
            &rules,
            &AuditOpts {
                project_root: dir.path().to_path_buf(),
                scan_root: dir.path().to_path_buf(),
                rule_filter: None,
            },
        );
        let ids: Vec<&str> = report.per_rule.iter().map(|r| r.rule_id.as_str()).collect();
        assert!(
            ids.contains(&"audit-python-bare-except"),
            "python predicate must audit; got {ids:?}"
        );
        assert!(
            ids.contains(&"audit-python-print-call"),
            "python print predicate must audit; got {ids:?}"
        );
        assert!(
            ids.contains(&"audit-python-call-default"),
            "python default-call predicate must audit; got {ids:?}"
        );
        assert!(
            ids.contains(&"audit-python-handler-pass"),
            "python handler predicate must audit; got {ids:?}"
        );
        assert!(
            ids.contains(&"audit-ts-explicit-any"),
            "typescript predicate must audit; got {ids:?}"
        );
    }

    #[test]
    fn run_evaluates_ast_predicate_function_let_binding_count_high() {
        // Positive case: a function with 8+ outer-scope `let` bindings should
        // produce one audit hit on the let-binding rule.
        let dir = tempfile::tempdir().unwrap();
        let src = "\
fn ladder() {
    let a = 1;
    let b = 2;
    let c = 3;
    let d = 4;
    let e = 5;
    let f = 6;
    let g = 7;
    let h = 8;
    let _ = (a, b, c, d, e, f, g, h);
}
";
        std::fs::write(dir.path().join("a.rs"), src).unwrap();
        let rules = RulesFile {
            rules: vec![ast_let_binding_rule()],
        };
        let report = run(
            &rules,
            &AuditOpts {
                project_root: dir.path().to_path_buf(),
                scan_root: dir.path().to_path_buf(),
                rule_filter: None,
            },
        );
        assert_eq!(
            report.per_rule.len(),
            1,
            "expected one rule with hits, got {:?}",
            report.per_rule,
        );
        assert_eq!(
            report.per_rule[0].rule_id,
            "audit-rust-let-binding-count-high"
        );
        assert_eq!(report.per_rule[0].hits, 1);
        assert_eq!(report.per_rule[0].level, Level::Warn);
        assert_eq!(report.per_rule[0].files.len(), 1);
        assert!(
            report.per_rule[0].files[0]
                .path
                .to_string_lossy()
                .ends_with("a.rs")
        );
    }

    #[test]
    fn run_ast_predicate_hit_carries_function_name_and_count() {
        // The audit must name the offending function and its count, not
        // just emit a placeholder line `1` per hit. `ladder` has 9
        // outer-scope `let` bindings (a..h plus the trailing `let _ =`),
        // which clears the 8-binding threshold; the per-hit detail should
        // read "ladder (9 let bindings)" — the *actual* count, not the
        // threshold.
        let dir = tempfile::tempdir().unwrap();
        let src = "\
fn ladder() {
    let a = 1;
    let b = 2;
    let c = 3;
    let d = 4;
    let e = 5;
    let f = 6;
    let g = 7;
    let h = 8;
    let _ = (a, b, c, d, e, f, g, h);
}
";
        std::fs::write(dir.path().join("a.rs"), src).unwrap();
        let rules = RulesFile {
            rules: vec![ast_let_binding_rule()],
        };
        let report = run(
            &rules,
            &AuditOpts {
                project_root: dir.path().to_path_buf(),
                scan_root: dir.path().to_path_buf(),
                rule_filter: None,
            },
        );
        assert_eq!(report.per_rule.len(), 1);
        let details = &report.per_rule[0].files[0].details;
        assert!(
            details.iter().any(|d| d == "ladder (9 let bindings)"),
            "expected named detail 'ladder (9 let bindings)', got {details:?}"
        );
    }

    #[test]
    fn run_silences_ast_predicate_on_block_pattern_adopter() {
        // Silence case (LOAD-BEARING): a function using the block pattern —
        // `let x = { let a; let b; ...; tmp }` — has 8+ total `let`s but only
        // one OUTER-scope let. The extractor halts at the child block, so
        // SyntaxFacts contains no fact for this function, and the audit must
        // report zero hits. This is the spec's core property; the entire
        // reason the feature exists.
        let dir = tempfile::tempdir().unwrap();
        let src = "\
fn block_adopter() {
    let result = {
        let a = 1;
        let b = 2;
        let c = 3;
        let d = 4;
        let e = 5;
        let f = 6;
        let g = 7;
        let h = 8;
        (a, b, c, d, e, f, g, h)
    };
    let _ = result;
}
";
        std::fs::write(dir.path().join("a.rs"), src).unwrap();
        let rules = RulesFile {
            rules: vec![ast_let_binding_rule()],
        };
        let report = run(
            &rules,
            &AuditOpts {
                project_root: dir.path().to_path_buf(),
                scan_root: dir.path().to_path_buf(),
                rule_filter: None,
            },
        );
        assert!(
            report.per_rule.is_empty(),
            "block-pattern adopter must NOT fire let-binding rule (spec property), got {:?}",
            report.per_rule,
        );
    }

    #[test]
    fn run_ast_predicate_does_not_fire_on_non_rust_files() {
        // SyntaxFacts::extract only parses Rust and Swift; other extensions
        // get a default (empty) SyntaxFacts. A `.py` file with many `let`-ish
        // lines must not produce hits from a Rust AST predicate.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.py"), "let a = 1\nlet b = 2\nlet c = 3\n").unwrap();
        let rules = RulesFile {
            rules: vec![ast_let_binding_rule()],
        };
        let report = run(
            &rules,
            &AuditOpts {
                project_root: dir.path().to_path_buf(),
                scan_root: dir.path().to_path_buf(),
                rule_filter: None,
            },
        );
        assert!(report.per_rule.is_empty());
    }

    #[test]
    fn run_ast_and_content_rules_coexist_on_same_file() {
        // Regression: adding AST evaluation must not break content predicates.
        // A file with both a heavy ladder() function AND a `.unwrap()` should
        // fire both rules independently.
        let dir = tempfile::tempdir().unwrap();
        let src = "\
fn ladder() {
    let a = 1;
    let b = 2;
    let c = 3;
    let d = 4;
    let e = 5;
    let f = 6;
    let g = 7;
    let h = 8;
    let _ = foo.unwrap();
}
";
        std::fs::write(dir.path().join("a.rs"), src).unwrap();
        let rules = RulesFile {
            rules: vec![
                ast_let_binding_rule(),
                rule("no-unwrap", ".unwrap()", "constraint_violation"),
            ],
        };
        let report = run(
            &rules,
            &AuditOpts {
                project_root: dir.path().to_path_buf(),
                scan_root: dir.path().to_path_buf(),
                rule_filter: None,
            },
        );
        let rule_ids: Vec<&str> = report.per_rule.iter().map(|r| r.rule_id.as_str()).collect();
        assert!(
            rule_ids.contains(&"audit-rust-let-binding-count-high"),
            "let-binding rule should fire: {:?}",
            rule_ids
        );
        assert!(
            rule_ids.contains(&"no-unwrap"),
            "content rule should still fire: {:?}",
            rule_ids
        );
    }

    #[test]
    fn run_evaluates_ast_predicate_function_let_mut_count_high() {
        // Mirrors run_evaluates_ast_predicate_function_let_binding_count_high
        // for the let-mut variant. Closes a previously-untested gap: the audit
        // branch was generic but only the let-binding rule had end-to-end
        // coverage through the AST path. A function with 3+ outer-scope
        // `let mut`s should produce one hit on the let-mut rule.
        let dir = tempfile::tempdir().unwrap();
        let src = "\
fn mut_ladder() {
    let mut a = vec![];
    let mut b = String::new();
    let mut c = 0;
    a.push(1);
    b.push_str(\"x\");
    c += 1;
    let _ = (a, b, c);
}
";
        std::fs::write(dir.path().join("a.rs"), src).unwrap();
        let rules = RulesFile {
            rules: vec![ast_let_mut_rule()],
        };
        let report = run(
            &rules,
            &AuditOpts {
                project_root: dir.path().to_path_buf(),
                scan_root: dir.path().to_path_buf(),
                rule_filter: None,
            },
        );
        assert_eq!(
            report.per_rule.len(),
            1,
            "expected one rule with hits, got {:?}",
            report.per_rule,
        );
        assert_eq!(report.per_rule[0].rule_id, "audit-rust-let-mut-count-high");
        assert_eq!(report.per_rule[0].hits, 1);
        assert_eq!(report.per_rule[0].level, Level::Warn);
    }

    #[test]
    fn run_silences_ast_let_mut_rule_on_block_pattern_adopter() {
        // Mirror of the binding-rule silence test, but for the let-mut
        // variant. A function that scopes its `let mut`s inside a block
        // expression must NOT fire the let-mut rule — the walker halts at
        // the child block, so SyntaxFacts contains no fact.
        let dir = tempfile::tempdir().unwrap();
        let src = "\
fn mut_adopter() {
    let result = {
        let mut a = vec![];
        let mut b = String::new();
        let mut c = 0;
        a.push(1);
        b.push_str(\"x\");
        c += 1;
        (a, b, c)
    };
    let _ = result;
}
";
        std::fs::write(dir.path().join("a.rs"), src).unwrap();
        let rules = RulesFile {
            rules: vec![ast_let_mut_rule()],
        };
        let report = run(
            &rules,
            &AuditOpts {
                project_root: dir.path().to_path_buf(),
                scan_root: dir.path().to_path_buf(),
                rule_filter: None,
            },
        );
        assert!(
            report.per_rule.is_empty(),
            "block-pattern adopter must NOT fire let-mut rule (spec property), got {:?}",
            report.per_rule,
        );
    }

    #[test]
    fn run_mixed_ast_and_content_rule_uses_ast_predicate_only() {
        // Pins the documented behavior in the AST-branch comment: when a
        // single rule's `when` clause combines an AST predicate AND a
        // content predicate, the AST branch handles the rule and the
        // `continue` drops the content predicate. Effectively, AST takes
        // priority and the content predicate is ignored.
        //
        // Two sub-cases prove the property:
        //  (a) AST signal present, content signal absent → rule still fires
        //      (proves the content predicate is not required)
        //  (b) Content signal present, AST signal absent → rule does NOT fire
        //      (proves the rule doesn't fall through to content evaluation)
        //
        // No shipped rule currently mixes the two predicate kinds. This test
        // exists so the behavior can't drift unnoticed if one ever does, and
        // so a future change to AND-semantics (instead of AST-priority)
        // forces an explicit test update.
        let mixed_rule = || DiskRule {
            id: "mixed-rule".to_string(),
            phase: "audit".to_string(),
            priority: 3,
            conditions: vec![
                DiskCondition {
                    predicate: "function_let_binding_count_high".to_string(),
                    args: vec!["?file".to_string(), "?fn".to_string(), "?count".to_string()],
                    script: None,
                },
                DiskCondition {
                    predicate: "new_content_contains".to_string(),
                    args: vec!["MARKER_NEVER_PRESENT_IN_FIXTURE".to_string()],
                    script: None,
                },
            ],
            actions: vec![DiskAction {
                action_type: "constraint_warning".to_string(),
                params: vec!["mixed rule fired".to_string()],
                ..Default::default()
            }],
            silent: None,
            audit: Some(true),
            doc_excepted: None,
        };

        // Sub-case (a): AST signal hits (8 outer-scope lets), content marker
        // is deliberately absent. AST-priority semantics → rule fires.
        let dir_a = tempfile::tempdir().unwrap();
        std::fs::write(
            dir_a.path().join("a.rs"),
            "\
fn ladder() {
    let a = 1; let b = 2; let c = 3; let d = 4;
    let e = 5; let f = 6; let g = 7; let h = 8;
    let _ = (a, b, c, d, e, f, g, h);
}
",
        )
        .unwrap();
        let report_a = run(
            &RulesFile {
                rules: vec![mixed_rule()],
            },
            &AuditOpts {
                project_root: dir_a.path().to_path_buf(),
                scan_root: dir_a.path().to_path_buf(),
                rule_filter: None,
            },
        );
        assert_eq!(
            report_a.per_rule.len(),
            1,
            "AST predicate alone should fire the mixed rule (content predicate is ignored); got {:?}",
            report_a.per_rule,
        );
        assert_eq!(report_a.per_rule[0].rule_id, "mixed-rule");

        // Sub-case (b): content marker present, AST signal absent (short fn).
        // AST-priority semantics → rule does NOT fire.
        let dir_b = tempfile::tempdir().unwrap();
        std::fs::write(
            dir_b.path().join("b.rs"),
            "\
// MARKER_NEVER_PRESENT_IN_FIXTURE
fn short() {
    let _ = 1;
}
",
        )
        .unwrap();
        let report_b = run(
            &RulesFile {
                rules: vec![mixed_rule()],
            },
            &AuditOpts {
                project_root: dir_b.path().to_path_buf(),
                scan_root: dir_b.path().to_path_buf(),
                rule_filter: None,
            },
        );
        assert!(
            report_b.per_rule.is_empty(),
            "content predicate alone must NOT fire a mixed rule (AST takes priority); got {:?}",
            report_b.per_rule,
        );
    }

    #[test]
    fn rank_recognizes_constraint_warning_action_type() {
        // The hook + init.rs use `constraint_warning` (not `warn_violation`)
        // for warning actions. Make sure audit maps it correctly.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "dbg!(x)").unwrap();
        let r = DiskRule {
            id: "warn-dbg".to_string(),
            phase: "pre".to_string(),
            priority: 5,
            conditions: vec![DiskCondition {
                predicate: "new_content_contains".to_string(),
                args: vec!["dbg!(".to_string()],
                script: None,
            }],
            actions: vec![DiskAction {
                action_type: "constraint_warning".to_string(),
                params: vec!["no dbg".to_string()],
                ..Default::default()
            }],
            silent: None,
            audit: Some(true),
            doc_excepted: None,
        };
        let rules = RulesFile { rules: vec![r] };
        let report = run(
            &rules,
            &AuditOpts {
                project_root: dir.path().to_path_buf(),
                scan_root: dir.path().to_path_buf(),
                rule_filter: None,
            },
        );
        assert_eq!(report.per_rule.len(), 1);
        assert_eq!(report.per_rule[0].level, Level::Warn);
    }

    #[test]
    fn run_records_multiple_line_numbers_per_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.rs"),
            "line one\n.unwrap()\nline three\n.unwrap()\n",
        )
        .unwrap();
        let rules = RulesFile {
            rules: vec![rule("no-unwrap", ".unwrap()", "constraint_violation")],
        };
        let report = run(
            &rules,
            &AuditOpts {
                project_root: dir.path().to_path_buf(),
                scan_root: dir.path().to_path_buf(),
                rule_filter: None,
            },
        );
        assert_eq!(report.per_rule[0].hits, 2);
        assert_eq!(report.per_rule[0].files[0].lines, vec![2, 4]);
    }

    #[test]
    fn run_returns_empty_when_no_audit_rules_in_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn clean(){}").unwrap();
        let rules = RulesFile {
            rules: vec![rule("no-unwrap", ".unwrap()", "constraint_violation")],
        };
        let report = run(
            &rules,
            &AuditOpts {
                project_root: dir.path().to_path_buf(),
                scan_root: dir.path().to_path_buf(),
                rule_filter: None,
            },
        );
        assert!(report.per_rule.is_empty());
        assert!(report.files_scanned >= 1);
    }

    #[test]
    fn run_groups_hits_across_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), ".unwrap()").unwrap();
        std::fs::write(dir.path().join("b.rs"), ".unwrap()\n.unwrap()").unwrap();
        let rules = RulesFile {
            rules: vec![rule("no-unwrap", ".unwrap()", "constraint_violation")],
        };
        let report = run(
            &rules,
            &AuditOpts {
                project_root: dir.path().to_path_buf(),
                scan_root: dir.path().to_path_buf(),
                rule_filter: None,
            },
        );
        assert_eq!(report.per_rule[0].hits, 3);
        assert_eq!(report.per_rule[0].files.len(), 2);
    }

    #[test]
    fn discover_files_returns_only_matching_extensions() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn x(){}").unwrap();
        std::fs::write(dir.path().join("b.md"), "# hi").unwrap();
        std::fs::write(dir.path().join("c.rs"), "fn y(){}").unwrap();
        let mut got = discover_files(dir.path(), &["rs"]);
        got.sort();
        let names: Vec<_> = got
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["a.rs", "c.rs"]);
    }

    #[test]
    fn discover_files_respects_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        // .gitignore needs the dir to look like a real repo for `ignore` to honor it
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".gitignore"), "ignored.rs\n").unwrap();
        std::fs::write(dir.path().join("keep.rs"), "fn x(){}").unwrap();
        std::fs::write(dir.path().join("ignored.rs"), "fn y(){}").unwrap();
        let got = discover_files(dir.path(), &["rs"]);
        let names: Vec<_> = got
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["keep.rs"]);
    }

    #[test]
    fn discover_files_wildcard_returns_all_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "x").unwrap();
        std::fs::write(dir.path().join("b.md"), "x").unwrap();
        let got = discover_files(dir.path(), &["*"]);
        assert_eq!(got.len(), 2);
    }

    // ── compute_trend tests ───────────────────────────────────────────────────

    use crate::action_log::LogEntry;
    use serde_json::json;

    fn audit_entry(ts: u64, per_rule: serde_json::Value) -> LogEntry {
        let mut e = LogEntry::new("mcp", "audit_codebase")
            .with("files_scanned", 100u64)
            .with("blocked_total", 0u64)
            .with("warned_total", 0u64);
        e.data.insert("per_rule".to_string(), per_rule);
        e.ts = ts;
        e
    }

    #[test]
    fn compute_trend_returns_empty_with_no_snapshots() {
        let trend = compute_trend(&[], &TrendOpts::default());
        assert!(trend.rules.is_empty());
        assert_eq!(trend.snapshots_considered, 0);
    }

    #[test]
    fn compute_trend_computes_net_change_for_single_rule() {
        let snaps = vec![
            audit_entry(
                1_700_000_000,
                json!({"no-unwrap": {"level":"block","hits":18}}),
            ),
            audit_entry(
                1_700_500_000,
                json!({"no-unwrap": {"level":"block","hits":14}}),
            ),
            audit_entry(
                1_701_000_000,
                json!({"no-unwrap": {"level":"block","hits":10}}),
            ),
        ];
        let trend = compute_trend(&snaps, &TrendOpts::default());
        assert_eq!(trend.snapshots_considered, 3);
        assert_eq!(trend.rules.len(), 1);
        let r = &trend.rules[0];
        assert_eq!(r.rule_id, "no-unwrap");
        assert_eq!(r.level, Level::Block);
        assert_eq!(r.first_hits, 18);
        assert_eq!(r.last_hits, 10);
        assert_eq!(r.net_change, -8);
        assert_eq!(r.history.len(), 3);
    }

    #[test]
    fn compute_trend_respects_last_limit() {
        let snaps = vec![
            audit_entry(1, json!({"r": {"level":"block","hits":1}})),
            audit_entry(2, json!({"r": {"level":"block","hits":2}})),
            audit_entry(3, json!({"r": {"level":"block","hits":3}})),
            audit_entry(4, json!({"r": {"level":"block","hits":4}})),
        ];
        let trend = compute_trend(
            &snaps,
            &TrendOpts {
                last: Some(2),
                ..TrendOpts::default()
            },
        );
        assert_eq!(trend.snapshots_considered, 2);
        assert_eq!(trend.rules[0].first_hits, 3);
        assert_eq!(trend.rules[0].last_hits, 4);
    }

    #[test]
    fn compute_trend_respects_rule_filter() {
        let snaps = vec![
            audit_entry(
                1,
                json!({"a":{"level":"block","hits":1},"b":{"level":"block","hits":5}}),
            ),
            audit_entry(
                2,
                json!({"a":{"level":"block","hits":2},"b":{"level":"block","hits":6}}),
            ),
        ];
        let trend = compute_trend(
            &snaps,
            &TrendOpts {
                rule_filter: Some("a".to_string()),
                ..TrendOpts::default()
            },
        );
        assert_eq!(trend.rules.len(), 1);
        assert_eq!(trend.rules[0].rule_id, "a");
    }

    #[test]
    fn compute_trend_handles_rule_appearing_mid_series() {
        let snaps = vec![
            audit_entry(1, json!({"a":{"level":"block","hits":10}})),
            audit_entry(
                2,
                json!({"a":{"level":"block","hits":8},"b":{"level":"warn","hits":3}}),
            ),
        ];
        let trend = compute_trend(&snaps, &TrendOpts::default());
        let b = trend.rules.iter().find(|r| r.rule_id == "b").unwrap();
        // b only appears in the second snapshot — first_hits should reflect that.
        assert_eq!(b.first_hits, 3);
        assert_eq!(b.last_hits, 3);
        assert_eq!(b.net_change, 0);
        assert_eq!(b.history.len(), 2);
        assert_eq!(b.history[0].hits, None);
        assert_eq!(b.history[1].hits, Some(3));
    }

    #[test]
    fn compute_trend_sorts_by_net_change_ascending() {
        let snaps = vec![
            audit_entry(
                1,
                json!({"big-improve":{"level":"block","hits":50},"regress":{"level":"block","hits":5}}),
            ),
            audit_entry(
                2,
                json!({"big-improve":{"level":"block","hits":20},"regress":{"level":"block","hits":8}}),
            ),
        ];
        let trend = compute_trend(&snaps, &TrendOpts::default());
        // big-improve (-30) should come before regress (+3)
        assert_eq!(trend.rules[0].rule_id, "big-improve");
        assert_eq!(trend.rules[1].rule_id, "regress");
    }

    #[test]
    fn compute_trend_only_one_snapshot_yields_zero_net_change() {
        let snaps = vec![audit_entry(1, json!({"r":{"level":"block","hits":5}}))];
        let trend = compute_trend(&snaps, &TrendOpts::default());
        assert_eq!(trend.snapshots_considered, 1);
        assert_eq!(trend.rules[0].net_change, 0);
        assert_eq!(trend.rules[0].first_hits, 5);
        assert_eq!(trend.rules[0].last_hits, 5);
    }

    fn make_report() -> AuditReport {
        AuditReport {
            generated_at: 1_700_000_000,
            scan_duration_ms: 412,
            files_scanned: 312,
            per_rule: vec![
                RuleAudit {
                    rule_id: "no-unwrap-in-src".into(),
                    level: Level::Block,
                    hits: 10,
                    files: vec![
                        FileAudit {
                            path: PathBuf::from("src/engine.rs"),
                            lines: vec![42, 91, 240],
                            details: vec![],
                        },
                        FileAudit {
                            path: PathBuf::from("src/parser.rs"),
                            lines: vec![18],
                            details: vec![],
                        },
                    ],
                },
                RuleAudit {
                    rule_id: "warn-clone-heavy".into(),
                    level: Level::Warn,
                    hits: 5,
                    files: vec![FileAudit {
                        path: PathBuf::from("src/parser.rs"),
                        lines: vec![3, 4, 5, 6, 7],
                        details: vec![],
                    }],
                },
            ],
        }
    }

    #[test]
    fn render_table_summary_sorts_blocks_above_warns() {
        let out = render_table(&make_report(), false);
        let block_idx = out.find("no-unwrap-in-src").expect("block row present");
        let warn_idx = out.find("warn-clone-heavy").expect("warn row present");
        assert!(
            block_idx < warn_idx,
            "block should sort above warn:\n{}",
            out
        );
        assert!(out.contains("Total: 10 blocked, 5 warned"));
        assert!(out.contains("312 files"));
    }

    #[test]
    fn render_table_expand_shows_files_and_lines() {
        let out = render_table(&make_report(), true);
        assert!(out.contains("src/engine.rs"));
        assert!(out.contains("42"));
        assert!(out.contains("91"));
        assert!(out.contains("240"));
    }

    #[test]
    fn render_table_expand_shows_named_ast_details() {
        // AST hits carry a per-function detail (e.g. "add_rule (12 let
        // bindings)") and a meaningless placeholder line of `1` per hit.
        // The expanded table must surface the names and drop the
        // placeholder "lines: 1, 1" — otherwise the user sees
        // `server.rs — lines: 1, 1, 1` and has to grep for the offenders.
        let report = AuditReport {
            generated_at: 0,
            scan_duration_ms: 0,
            files_scanned: 1,
            per_rule: vec![RuleAudit {
                rule_id: "audit-rust-let-binding-count-high".into(),
                level: Level::Warn,
                hits: 2,
                files: vec![FileAudit {
                    path: PathBuf::from("src/server.rs"),
                    lines: vec![1, 1],
                    details: vec![
                        "add_rule (12 let bindings)".to_string(),
                        "run (9 let bindings)".to_string(),
                    ],
                }],
            }],
        };
        let out = render_table(&report, true);
        assert!(
            out.contains("add_rule (12 let bindings)"),
            "expanded table must name the function, got:\n{out}"
        );
        assert!(
            out.contains("run (9 let bindings)"),
            "expanded table must name the second function, got:\n{out}"
        );
        assert!(
            !out.contains("lines: 1, 1"),
            "expanded table must drop placeholder lines when details are present, got:\n{out}"
        );
    }

    #[test]
    fn render_table_handles_empty_report() {
        let empty = AuditReport {
            generated_at: 1_700_000_000,
            scan_duration_ms: 5,
            files_scanned: 100,
            per_rule: vec![],
        };
        let out = render_table(&empty, false);
        assert!(out.contains("no audit violations found"));
    }

    fn make_trend() -> DebtTrend {
        DebtTrend {
            generated_at: 1_701_000_000,
            snapshots_considered: 3,
            first_snapshot_ts: 1_700_000_000,
            last_snapshot_ts: 1_701_000_000,
            rules: vec![
                RuleTrend {
                    rule_id: "no-unwrap".into(),
                    level: Level::Block,
                    history: vec![
                        TrendPoint {
                            ts: 1_700_000_000,
                            hits: Some(18),
                        },
                        TrendPoint {
                            ts: 1_700_500_000,
                            hits: Some(14),
                        },
                        TrendPoint {
                            ts: 1_701_000_000,
                            hits: Some(10),
                        },
                    ],
                    first_hits: 18,
                    last_hits: 10,
                    net_change: -8,
                },
                RuleTrend {
                    rule_id: "warn-clone-heavy".into(),
                    level: Level::Warn,
                    history: vec![
                        TrendPoint {
                            ts: 1_700_000_000,
                            hits: None,
                        },
                        TrendPoint {
                            ts: 1_700_500_000,
                            hits: Some(5),
                        },
                        TrendPoint {
                            ts: 1_701_000_000,
                            hits: Some(7),
                        },
                    ],
                    first_hits: 5,
                    last_hits: 7,
                    net_change: 2,
                },
            ],
        }
    }

    #[test]
    fn render_trend_table_shows_columns_and_deltas() {
        let out = render_trend_table(&make_trend());
        assert!(out.contains("no-unwrap"));
        assert!(out.contains("warn-clone-heavy"));
        assert!(out.contains("-8"));
        assert!(out.contains("+2"));
    }

    #[test]
    fn render_trend_table_shows_dash_for_missing_snapshot() {
        let out = render_trend_table(&make_trend());
        // warn-clone-heavy has no value in the first snapshot → '–' (em-dash)
        assert!(
            out.contains("–"),
            "missing value should render as em-dash:\n{}",
            out
        );
    }

    #[test]
    fn render_trend_table_handles_no_snapshots() {
        let trend = DebtTrend {
            generated_at: 1_701_000_000,
            snapshots_considered: 0,
            first_snapshot_ts: 0,
            last_snapshot_ts: 0,
            rules: Vec::new(),
        };
        let out = render_trend_table(&trend);
        assert!(out.contains("no audit snapshots recorded yet"));
    }

    #[test]
    fn render_trend_table_handles_single_snapshot() {
        let trend = DebtTrend {
            generated_at: 1_701_000_000,
            snapshots_considered: 1,
            first_snapshot_ts: 1_701_000_000,
            last_snapshot_ts: 1_701_000_000,
            rules: vec![RuleTrend {
                rule_id: "r".into(),
                level: Level::Block,
                history: vec![TrendPoint {
                    ts: 1_701_000_000,
                    hits: Some(5),
                }],
                first_hits: 5,
                last_hits: 5,
                net_change: 0,
            }],
        };
        let out = render_trend_table(&trend);
        assert!(out.contains("need at least 2 snapshots to compute trend"));
        assert!(out.contains("r"));
        assert!(out.contains("5"));
    }

    #[test]
    fn render_trend_json_shape() {
        let out = render_trend_json(&make_trend());
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["snapshots_considered"], 3);
        assert_eq!(v["first_snapshot_ts"], 1_700_000_000);
        assert_eq!(v["last_snapshot_ts"], 1_701_000_000);
        let rules = v["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0]["rule_id"], "no-unwrap");
        assert_eq!(rules[0]["level"], "block");
        assert_eq!(rules[0]["first_hits"], 18);
        assert_eq!(rules[0]["last_hits"], 10);
        assert_eq!(rules[0]["net_change"], -8);
        let history = rules[0]["history"].as_array().unwrap();
        assert_eq!(history.len(), 3);
        // Second rule has a null hits value at index 0.
        assert!(rules[1]["history"][0]["hits"].is_null());
    }

    #[test]
    fn render_json_shape() {
        let out = render_json(&make_report());
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["files_scanned"], 312);
        assert_eq!(v["totals"]["blocked"], 10);
        assert_eq!(v["totals"]["warned"], 5);
        assert_eq!(v["totals"]["rules"], 2);
        let rules = v["rules"].as_array().unwrap();
        assert_eq!(rules[0]["rule_id"], "no-unwrap-in-src");
        assert_eq!(rules[0]["level"], "block");
        assert_eq!(rules[0]["hits"], 10);
        let files = rules[0]["files"].as_array().unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0]["path"], "src/engine.rs");
        let lines = files[0]["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn render_json_includes_per_hit_details() {
        // AST hits must serialize their per-function detail so machine
        // consumers (trend tooling, CI dashboards) can name offenders
        // without re-parsing the tree.
        let report = AuditReport {
            generated_at: 0,
            scan_duration_ms: 0,
            files_scanned: 1,
            per_rule: vec![RuleAudit {
                rule_id: "audit-rust-let-binding-count-high".into(),
                level: Level::Warn,
                hits: 1,
                files: vec![FileAudit {
                    path: PathBuf::from("src/a.rs"),
                    lines: vec![1],
                    details: vec!["ladder (9 let bindings)".to_string()],
                }],
            }],
        };
        let out = render_json(&report);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let files = v["rules"][0]["files"].as_array().unwrap();
        let details = files[0]["details"].as_array().unwrap();
        assert_eq!(details.len(), 1);
        assert_eq!(details[0], "ladder (9 let bindings)");
    }

    fn empty_report() -> AuditReport {
        AuditReport {
            generated_at: 0,
            scan_duration_ms: 0,
            files_scanned: 0,
            per_rule: Vec::new(),
        }
    }

    fn report_with_hits() -> AuditReport {
        AuditReport {
            generated_at: 0,
            scan_duration_ms: 0,
            files_scanned: 5,
            per_rule: vec![RuleAudit {
                rule_id: "r".into(),
                level: Level::Warn,
                hits: 1,
                files: vec![FileAudit {
                    path: PathBuf::from("a.rs"),
                    lines: vec![1],
                    details: vec![],
                }],
            }],
        }
    }

    #[test]
    fn empty_diagnostic_returns_none_when_report_has_hits() {
        let r = report_with_hits();
        assert!(empty_result_diagnostic(&r, &opted_in(5), None, Path::new("/proj")).is_none());
    }

    /// A rules file with `n` opted-in rules.
    fn opted_in(n: usize) -> RulesFile {
        RulesFile {
            rules: (0..n)
                .map(|i| rule(&format!("r{i}"), "x", "warning"))
                .collect(),
        }
    }

    #[test]
    fn empty_diagnostic_flags_zero_audit_tagged_rules() {
        let r = empty_report();
        let msg = empty_result_diagnostic(&r, &opted_in(0), None, Path::new("/proj"))
            .expect("zero audit-tagged rules must produce a diagnostic");
        assert!(
            msg.contains("audit: true"),
            "message must name the recoverable cause; got: {msg}"
        );
    }

    #[test]
    fn empty_diagnostic_flags_walker_zero_files_when_rules_opted_in() {
        let r = empty_report();
        let msg = empty_result_diagnostic(&r, &opted_in(5), None, Path::new("/proj/src"))
            .expect("opted-in rules but zero scanned files must produce a diagnostic");
        assert!(
            msg.contains("0 files") && msg.contains("/proj/src"),
            "message must call out walker scope and scan_root; got: {msg}"
        );
        assert!(
            msg.contains(".gitignore") || msg.contains(".phronesisignore"),
            "message should point at ignore files as a likely cause; got: {msg}"
        );
    }

    #[test]
    fn empty_diagnostic_silent_for_legitimately_clean_audit() {
        // Rules opted in, files walked, simply no violations — the honest
        // zero. No diagnostic; the normal renderer's "no violations" line
        // covers it.
        let r = AuditReport {
            files_scanned: 50,
            ..empty_report()
        };
        assert!(empty_result_diagnostic(&r, &opted_in(3), None, Path::new("/proj")).is_none());
    }

    #[test]
    fn run_profiled_matches_run_and_populates_section_times() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.rs"),
            "fn main() { let x = foo.unwrap(); }",
        )
        .unwrap();
        let rules = RulesFile {
            rules: vec![rule("no-unwrap", ".unwrap()", "constraint_violation")],
        };
        let opts = AuditOpts {
            project_root: dir.path().to_path_buf(),
            scan_root: dir.path().to_path_buf(),
            rule_filter: None,
        };
        let report = run(&rules, &opts);
        let (profiled, times) = run_profiled(&rules, &opts);

        // Structural equality between the two paths.
        assert_eq!(report.files_scanned, profiled.files_scanned);
        assert_eq!(report.per_rule.len(), profiled.per_rule.len());
        for (r, p) in report.per_rule.iter().zip(profiled.per_rule.iter()) {
            assert_eq!(r.rule_id, p.rule_id);
            assert_eq!(r.hits, p.hits);
            assert_eq!(r.files.len(), p.files.len());
            // FileAudit does not derive PartialEq; compare fields individually.
            for (a, b) in r.files.iter().zip(p.files.iter()) {
                assert_eq!(a.path, b.path);
                assert_eq!(a.lines, b.lines);
                assert_eq!(a.details, b.details);
            }
        }

        // Timing fields populated.
        assert_eq!(times.files_scanned, profiled.files_scanned);
        assert!(times.audit_rules >= 1, "audit_rules must be counted");
        assert!(
            times.total >= times.match_loop,
            "total {:?} must be >= match_loop {:?}",
            times.total,
            times.match_loop
        );
    }
}

// ── Structural (graph) rules ────────────────────────────────────────────────

/// Fold structural findings into an `AuditReport`.
///
/// Graph rules cannot be evaluated by the file-scanning loop above — their
/// conditions join relations across the whole repository rather than matching
/// text in one file — so they are evaluated separately by
/// `graph::audit::audit_graph_rules` and merged here.
///
/// Findings carry no line number: the graph records that a function is
/// untested, not where it sits. They use the same line-1 placeholder plus
/// detail string that AST hits already use, so renderers need no new case.
/// The repo-relative prefix a scoped audit reports under, or `None` for a
/// whole-tree scan.
///
/// Graph rules are evaluated over the entire graph by design — the test that
/// covers a function may live anywhere — but a caller who scoped the audit to
/// one directory is asking "what is wrong *here*", and answering with another
/// module's debt reads as their own.
pub fn graph_scope_prefix(project_root: &Path, scan_root: &Path) -> Option<String> {
    let rel = scan_root.strip_prefix(project_root).ok()?;
    let s = rel.to_str()?.trim_end_matches('/');
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

/// True when `file` (repo-relative) lies under `scope`.
///
/// Compares whole path segments: `src/journey` must not swallow
/// `src/journeyman.rs`.
fn within_scope(file: &str, scope: &str) -> bool {
    file == scope
        || file
            .strip_prefix(scope)
            .is_some_and(|rest| rest.starts_with('/'))
}

pub fn merge_graph_hits(
    report: &mut AuditReport,
    hits: &[crate::graph::audit::GraphHit],
    rule_filter: Option<&str>,
    scope: Option<&str>,
) {
    let mut accum: BTreeMap<String, (Level, BTreeMap<PathBuf, PerFileHits>)> = BTreeMap::new();
    for hit in hits {
        if let Some(want) = rule_filter
            && !rule_matches_filter(&hit.rule_id, want)
        {
            continue;
        }
        if let Some(scope) = scope
            && !within_scope(&hit.file, scope)
        {
            continue;
        }
        let Some(level) = Level::from_action_type(&hit.action_type) else {
            continue;
        };
        let entry = accum
            .entry(hit.rule_id.clone())
            .or_insert_with(|| (level, BTreeMap::new()));
        entry
            .1
            .entry(PathBuf::from(&hit.file))
            .or_default()
            .push_detail(hit.detail.clone());
    }
    report.per_rule.extend(build_per_rule(accum));
    // `build_per_rule` sorts within its own batch; re-sort the union so the
    // merged report keeps the documented ordering rather than showing
    // structural rules bolted on the end.
    report.per_rule.sort_by(rule_report_order);
}

#[cfg(test)]
mod graph_merge_tests {
    use super::*;
    use crate::graph::audit::GraphHit;

    fn hit(rule: &str, file: &str, action: &str) -> GraphHit {
        GraphHit {
            rule_id: rule.to_string(),
            action_type: action.to_string(),
            file: file.to_string(),
            detail: format!("{rule} on {file}"),
        }
    }

    fn empty_report() -> AuditReport {
        AuditReport {
            generated_at: 0,
            scan_duration_ms: 0,
            files_scanned: 0,
            per_rule: Vec::new(),
        }
    }

    #[test]
    fn structural_hits_become_audit_entries() {
        let mut r = empty_report();
        merge_graph_hits(
            &mut r,
            &[hit("warn-import-cycle", "src/a.rs", "constraint_warning")],
            None,
            None,
        );
        assert_eq!(r.per_rule.len(), 1);
        assert_eq!(r.per_rule[0].hits, 1);
    }

    #[test]
    fn graph_hit_rule_filter_matches_or_expansions() {
        let mut r = empty_report();
        merge_graph_hits(
            &mut r,
            &[
                hit(
                    "warn-untested-risky-call#or0",
                    "src/a.rs",
                    "constraint_warning",
                ),
                hit(
                    "warn-untested-risky-call#or1",
                    "src/b.rs",
                    "constraint_warning",
                ),
                hit("warn-import-cycle", "src/c.rs", "constraint_warning"),
            ],
            Some("warn-untested-risky-call"),
            None,
        );
        let ids: Vec<&str> = r.per_rule.iter().map(|p| p.rule_id.as_str()).collect();
        assert_eq!(ids.len(), 2, "{ids:?}");
        assert!(
            ids.iter()
                .all(|id| id.starts_with("warn-untested-risky-call#or")),
            "{ids:?}"
        );
    }

    #[test]
    fn graph_hits_outside_the_scan_scope_are_dropped() {
        // Graph rules must still *evaluate* over the whole graph — a test that
        // covers this file may live anywhere — but a scoped audit reports
        // findings in scope. Without this, `--path src/journey` returns
        // violations in src/init.rs and reads as debt in the caller's area.
        let mut r = empty_report();
        merge_graph_hits(
            &mut r,
            &[
                hit(
                    "warn-import-cycle",
                    "src/journey/mod.rs",
                    "constraint_warning",
                ),
                hit("warn-import-cycle", "src/init.rs", "constraint_warning"),
            ],
            None,
            Some("src/journey"),
        );
        assert_eq!(r.per_rule.len(), 1, "one rule survives");
        assert_eq!(r.per_rule[0].hits, 1, "only the in-scope hit");
        assert_eq!(
            r.per_rule[0].files[0].path,
            PathBuf::from("src/journey/mod.rs")
        );
    }

    #[test]
    fn a_scope_matches_on_path_boundaries_not_string_prefix() {
        // `src/journey` must not swallow `src/journeyman.rs`.
        let mut r = empty_report();
        merge_graph_hits(
            &mut r,
            &[hit(
                "warn-import-cycle",
                "src/journeyman.rs",
                "constraint_warning",
            )],
            None,
            Some("src/journey"),
        );
        assert!(r.per_rule.is_empty(), "sibling path must not match");
    }

    #[test]
    fn hits_in_the_same_rule_group_by_file() {
        let mut r = empty_report();
        merge_graph_hits(
            &mut r,
            &[
                hit("warn-import-cycle", "src/a.rs", "constraint_warning"),
                hit("warn-import-cycle", "src/b.rs", "constraint_warning"),
            ],
            None,
            None,
        );
        assert_eq!(r.per_rule.len(), 1, "one rule");
        assert_eq!(r.per_rule[0].files.len(), 2, "two files");
        assert_eq!(r.per_rule[0].hits, 2);
    }

    #[test]
    fn the_rule_filter_is_honored() {
        let mut r = empty_report();
        merge_graph_hits(
            &mut r,
            &[
                hit("warn-import-cycle", "src/a.rs", "constraint_warning"),
                hit("warn-untested-risky-call", "src/b.rs", "constraint_warning"),
            ],
            Some("warn-import-cycle"),
            None,
        );
        assert_eq!(r.per_rule.len(), 1);
        assert_eq!(r.per_rule[0].rule_id.to_string(), "warn-import-cycle");
    }

    #[test]
    fn the_message_is_preserved_as_per_hit_detail() {
        let mut r = empty_report();
        merge_graph_hits(
            &mut r,
            &[hit("warn-import-cycle", "src/a.rs", "constraint_warning")],
            None,
            None,
        );
        assert_eq!(
            r.per_rule[0].files[0].details[0],
            "warn-import-cycle on src/a.rs"
        );
    }

    #[test]
    fn blocking_structural_rules_outrank_warnings_in_the_merged_report() {
        let mut r = empty_report();
        merge_graph_hits(
            &mut r,
            &[
                hit("a-warn", "src/a.rs", "constraint_warning"),
                hit("z-block", "src/b.rs", "constraint_violation"),
            ],
            None,
            None,
        );
        assert_eq!(r.per_rule[0].rule_id.to_string(), "z-block");
    }
}

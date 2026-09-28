//! Audit core engine: rule evaluation and scan logic. Split from the
//! original `audit.rs`; functions moved verbatim. Exemption carried from
//! the god-file surface.
//!
//! phronesis-allow: enforce-no-result-string-error (verbatim move from audit.rs)
//! phronesis-allow: audit-file-loc-high (cohesive audit-engine surface)

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::rules_file::{DiskRule, RulesFile};
use crate::syntax;
use phr::Fact;

use super::run::AuditSectionTimes;
use super::types::{FileAudit, Level, PerFileHits, RuleAudit};

// ── Core engine ─────────────────────────────────────────────────────────────

/// Returns `true` if all gate predicates in `rule` pass for `path`, and every
/// condition uses a predicate that audit can evaluate. Returns `false` if any
/// gate predicate fails or if any condition uses an unsupported predicate
/// (one that requires diff context, e.g. `function_added`). Content predicates
/// (`new_content_contains`) and AST predicates emitted by `SyntaxFacts::all_facts`
/// are both considered "supported" here and skipped — they're evaluated in the
/// scan loop.
pub(crate) fn rule_applies_to_file(rule: &DiskRule, path: &Path, line_count: usize) -> bool {
    for cond in &rule.conditions {
        match cond.predicate.as_str() {
            "new_content_contains" => {
                // Content predicate — evaluated separately in the scan loop.
                continue;
            }
            "__script__" => {
                // File-scoped guard — evaluated after the cheap ordinary
                // gates, against fresh audit facts for this file.
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
pub(crate) fn is_ast_predicate(predicate: &str) -> bool {
    crate::syntax::facts::SyntaxFacts::PREDICATES.contains(&predicate)
}

/// True if `rule` has at least one condition whose predicate is an AST
/// predicate evaluated via `SyntaxFacts`. Used to decide whether the audit
/// loop needs to lazily parse the file's syntax.
pub(crate) fn rule_has_ast_predicate(rule: &DiskRule) -> bool {
    rule.conditions
        .iter()
        .any(|c| is_ast_predicate(&c.predicate))
}

/// Build a human-readable per-hit detail string from an AST fact's args.
/// `args[0]` is the file path (already shown by the renderer); `args[1]`
/// is the function/entity name; for count predicates `args[2]` is the
/// threshold count. Returns `None` only for a shapeless fact (none of the
/// current AST predicates are shapeless — guard anyway).
pub(crate) fn ast_hit_detail(predicate: &str, args: &[String]) -> Option<String> {
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
pub(crate) fn is_whole_file_rule(rule: &DiskRule) -> bool {
    rule.conditions
        .iter()
        .all(|c| c.predicate != "new_content_contains")
}

pub(crate) fn normalized_relative_path(project_root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(project_root).unwrap_or(path);
    relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

pub(crate) fn audit_path_facts(project_root: &Path, path: &Path) -> Vec<Fact> {
    let relative = normalized_relative_path(project_root, path);
    let mut facts = vec![Fact {
        id: "audit_file_path".to_string(),
        predicate: "file_path".to_string(),
        args: vec![relative.clone()],
        timestamp: 0,
        source: Some("audit".to_string()),
    }];
    for (index, part) in relative
        .split('/')
        .filter(|part| !part.is_empty())
        .enumerate()
    {
        facts.push(Fact {
            id: format!("audit_file_path_matches_{index}"),
            predicate: "file_path_matches".to_string(),
            args: vec![part.to_string()],
            timestamp: 0,
            source: Some("audit".to_string()),
        });
    }
    if let Some(extension) = path.extension().and_then(|value| value.to_str()) {
        facts.push(Fact {
            id: "audit_file_extension".to_string(),
            predicate: "file_extension_is".to_string(),
            args: vec![extension.to_ascii_lowercase()],
            timestamp: 0,
            source: Some("audit".to_string()),
        });
    }
    facts
}

pub(crate) fn script_body(condition: &crate::rules_file::DiskCondition) -> Result<&str, String> {
    condition
        .script
        .as_deref()
        .filter(|script| !script.trim().is_empty())
        .ok_or_else(|| "missing script body".to_string())
}

pub(crate) fn validate_builtin_script(script: &str) -> Result<(), String> {
    if script.contains('?') {
        return Err("binding-dependent scripts are not supported by audit".to_string());
    }
    phr::BuiltinScriptEvaluator::new()
        .evaluate(script, &[], &HashMap::new())
        .map(|_| ())
        .map_err(|error| format!("unsupported or malformed builtin script: {error}"))
}

/// Diagnose opted-in rules whose script guards cannot be evaluated by the
/// whole-tree audit's builtin, zero-binding script surface.
pub fn script_diagnostics(rules: &RulesFile, rule_filter: Option<&str>) -> Vec<String> {
    filter_audit_rules(rules, rule_filter)
        .into_iter()
        .filter_map(|rule| {
            rule.conditions
                .iter()
                .filter(|condition| condition.predicate == "__script__")
                .find_map(|condition| {
                    script_body(condition)
                        .and_then(validate_builtin_script)
                        .err()
                })
                .map(|reason| {
                    format!(
                        "phronesis: audit rule `{}` was skipped because its `__script__` guard is unsupported: {}.",
                        rule.id, reason
                    )
                })
        })
        .collect()
}

pub(crate) fn script_guards_pass(rule: &DiskRule, facts: &[Fact]) -> bool {
    let evaluator = phr::BuiltinScriptEvaluator::new();
    let bindings = HashMap::new();
    rule.conditions
        .iter()
        .filter(|condition| condition.predicate == "__script__")
        .all(|condition| {
            script_body(condition)
                .and_then(validate_builtin_script)
                .and_then(|_| {
                    evaluator
                        .evaluate(
                            condition.script.as_deref().unwrap_or_default(),
                            facts,
                            &bindings,
                        )
                        .map_err(|error| error.to_string())
                })
                .unwrap_or(false)
        })
}

/// True if the file's top-level `//!` doc-comment carries an exemption
/// marker for `rule_id`. Looks for a line of the form
/// `//! phronesis-allow: <rule-id>[ <free-form reason>]` anywhere in the
/// leading run of `//!` doc-comment lines (allowing blank lines between).
/// Stops scanning at the first non-blank, non-`//!` line.
pub(crate) fn file_exempts_rule(lines: &[&str], rule_id: &str) -> bool {
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
pub(crate) fn line_preceded_by_doc_comment(lines: &[&str], i: usize) -> bool {
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

/// Match a requested source-rule ID against its runtime rule ID.
///
/// [`crate::rules_file::unfold_or`] expands a source rule with `or` clauses
/// into IDs such as `rule#or1` and `rule#or0-or1`. CLI filters keep treating
/// the source ID as the public identity, while still accepting an exact
/// expanded ID.
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
pub(crate) fn filter_audit_rules<'a>(
    rules: &'a RulesFile,
    rule_filter: Option<&str>,
) -> Vec<&'a DiskRule> {
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
pub(crate) fn eval_ast_rule(
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
pub(crate) struct ContentEvalCtx<'a> {
    pub(crate) lines: &'a [&'a str],
    pub(crate) keep_mask: &'a Option<Vec<bool>>,
    pub(crate) doc_excepted: bool,
    pub(crate) times: Option<&'a mut AuditSectionTimes>,
}

pub(crate) fn eval_content_rule(needle: &str, mut ctx: ContentEvalCtx<'_>) -> Vec<u32> {
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
pub(crate) struct EvalCtx<'a> {
    pub(crate) path: &'a Path,
    pub(crate) path_str: &'a str,
    pub(crate) content: &'a str,
    pub(crate) lines: &'a [&'a str],
    pub(crate) keep_mask: &'a Option<Vec<bool>>,
    pub(crate) effective_line_count: usize,
    pub(crate) ast_facts: &'a mut Option<Vec<Fact>>,
    pub(crate) script_facts: &'a [Fact],
    pub(crate) times: Option<&'a mut AuditSectionTimes>,
}

/// Apply one rule's actions against one file's pre-parsed data, writing any
/// hits into `accum`.
pub(crate) fn evaluate_rule_for_file(
    rule: &DiskRule,
    ctx: &mut EvalCtx<'_>,
    accum: &mut BTreeMap<String, (Level, BTreeMap<PathBuf, PerFileHits>)>,
) {
    if !rule_applies_to_file(rule, ctx.path, ctx.effective_line_count) {
        return;
    }
    if !script_guards_pass(rule, ctx.script_facts) {
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
pub(crate) struct ScanFileInput<'a> {
    pub(crate) project_root: &'a Path,
    pub(crate) path: &'a Path,
    pub(crate) content: &'a str,
    pub(crate) rules: &'a [&'a DiskRule],
    pub(crate) accum: &'a mut BTreeMap<String, (Level, BTreeMap<PathBuf, PerFileHits>)>,
    pub(crate) times: Option<&'a mut AuditSectionTimes>,
}

pub(crate) fn scan_file_into_accum(input: ScanFileInput<'_>) {
    let ScanFileInput {
        project_root,
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
    let script_facts = audit_path_facts(project_root, path);
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
            script_facts: &script_facts,
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
pub(crate) fn build_per_rule(
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
pub(crate) fn rule_report_order(a: &RuleAudit, b: &RuleAudit) -> std::cmp::Ordering {
    let lvl = match (a.level, b.level) {
        (Level::Block, Level::Warn) => std::cmp::Ordering::Less,
        (Level::Warn, Level::Block) => std::cmp::Ordering::Greater,
        _ => std::cmp::Ordering::Equal,
    };
    lvl.then_with(|| b.hits.cmp(&a.hits))
        .then_with(|| a.rule_id.cmp(&b.rule_id))
}

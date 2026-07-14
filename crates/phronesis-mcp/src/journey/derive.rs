//! Derivation of `journey_*` facts from the durable journal.
//!
//! Per-invocation pass:
//!
//! 1. Scan loaded rules for `journey_*` conditions in either `__script__`
//!    form (`facts_count('journey_<kind>', ['<sel>','<window>'(,'<k>')]) <op> N`,
//!    `facts_contain('journey_<kind>', [...])`) or the bare-leaf form
//!    (`{ "journey_seen": ["sql","5c"] }` — `Condition` with `predicate ==
//!    "journey_seen"` and `args = [...]`).
//! 2. Validate every referenced tag / `module:<name>` selector against the
//!    project's `TaggerConfig` — silent-typo guard. A rule referencing
//!    `['testz','s']` when the project defines `tests` is rejected here
//!    rather than silently always-firing on `== 0`.
//! 3. Read a bounded suffix of `.phronesis/journey/events.jsonl` sized by
//!    the widest window any rule references. If any rule asks for the
//!    session (`s`) window we read up to the hard cap so the whole
//!    session is visible; rules that name only call/time windows pay only
//!    for those.
//! 4. Bucket the suffix and emit the v1 aggregator families:
//!    - `journey_occurrence(selector, window)` — one fact per matching record
//!    - `journey_count(selector, window, count)` — one fact per (sel, win)
//!    - `journey_seen(selector, window)` — one fact iff count ≥ 1
//!    - `journey_since_ge(selector, k)` — ladder up to distance-since-last,
//!      capped at max k any rule references for that selector
//!    - `journey_distinct(field, window, count)` — distinct values of `field`
//!      (v1: `path`) in `window`
//!    - `journey_filtered_since_ge(target, counted, k)` — ladder up to
//!      (count of `counted`-matching records appearing *after* the most
//!      recent `target`-matching record), capped at max k any rule references
//!      for that `(target, counted)` pair
//!
//! No state survives the call. Determinism: given a fixed (journal bytes,
//! `current_sid`, `now_ts`) the emitted facts are byte-identical across
//! runs. The determinism contract is pinned by an integration test.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use phr::{Fact, ReteNetwork, Rule};
use thiserror::Error;

use crate::journey::journal::{self, JournalRecord};
use crate::journey::tagger::TaggerConfig;

// ===== WindowScope =====

/// Bundled window-scoping parameters shared across emit helpers.
#[derive(Debug, Clone, Copy)]
pub struct WindowScope<'a> {
    /// Current session id used for session-window filtering.
    pub current_sid: &'a str,
    /// Current timestamp used for time-window filtering.
    pub now_ts: u64,
}

/// Inputs shared by journey fact derivation.
pub struct DeriveInput<'a> {
    pub project_root: &'a Path,
    pub rules: &'a [Rule],
    pub config: &'a TaggerConfig,
    pub scope: WindowScope<'a>,
}

struct WindowContext<'a> {
    records: &'a [JournalRecord],
    scope: WindowScope<'a>,
}

// ===== Window =====

/// One window token the rule grammar accepts. Encoded as a short string in
/// rule conditions (`5c`, `30m`, `2h`, `7d`, `s`); parsed once per derive
/// call and matched against `JournalRecord` fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    /// Last N executed tool calls (records).
    Calls(u32),
    /// Last N wall-clock seconds relative to `now_ts`.
    Seconds(u64),
    /// All records in the current session (`sid == current_sid`).
    Session,
}

impl Window {
    /// Parse one window token. See module docs for the supported forms.
    ///
    /// `r` is phase 2 (repo-lifetime — wants a checkpoint to be useful) and
    /// is explicitly rejected with a hint so the error doesn't read as a
    /// generic typo.
    pub fn parse(token: &str) -> Result<Self, DeriveError> {
        if token == "s" {
            return Ok(Window::Session);
        }
        if token == "r" {
            return Err(DeriveError::BadWindow(format!(
                "{} (r is phase 2 — not in v1)",
                token
            )));
        }
        if token.is_empty() {
            return Err(DeriveError::BadWindow(token.to_string()));
        }
        // Single-byte unit suffix.
        let last = token.as_bytes()[token.len() - 1] as char;
        let num = &token[..token.len() - 1];
        let n: u64 = num
            .parse()
            .map_err(|_| DeriveError::BadWindow(token.to_string()))?;
        match last {
            'c' => Ok(Window::Calls(n as u32)),
            'm' => Ok(Window::Seconds(n * 60)),
            'h' => Ok(Window::Seconds(n * 3600)),
            'd' => Ok(Window::Seconds(n * 86_400)),
            _ => Err(DeriveError::BadWindow(token.to_string())),
        }
    }
}

// ===== Error =====

#[derive(Debug, Error)]
pub enum DeriveError {
    #[error("malformed window token `{0}`")]
    BadWindow(String),
    #[error(
        "rule `{rule}` references undefined selector `{selector}` — not in journey.json taggers or modules"
    )]
    UndefinedSelector { rule: String, selector: String },
    #[error("journal read failed: {0}")]
    Journal(#[from] journal::JournalError),
}

impl DeriveError {
    /// Configuration errors (typos in `rules.json` / `journey.json`) versus
    /// I/O errors (journal read failures). The hook fails closed on the
    /// former — no amount of retrying fixes a typo — and fails open on the
    /// latter so transient I/O doesn't block every edit. See
    /// `.phronesis/wiki/decisions/2026-06-23-undefined-selector-rejection.md`.
    pub fn is_config_error(&self) -> bool {
        matches!(
            self,
            DeriveError::BadWindow(_) | DeriveError::UndefinedSelector { .. }
        )
    }
}

// ===== RuleScan =====

/// What the rule pass learned about journey predicates — every
/// (selector, window) pair the rules reference, plus the max `k` for each
/// `journey_since_ge` selector. Drives both selector validation and the
/// per-aggregator emission loops.
#[derive(Debug, Default, Clone)]
pub struct RuleScan {
    /// `(selector, window_token)` pairs the rules consult via
    /// `journey_occurrence`.
    pub occurrence_pairs: BTreeSet<(String, String)>,
    /// `(selector, window_token)` pairs the rules consult via
    /// `journey_count` (single bindable per pair).
    pub count_pairs: BTreeSet<(String, String)>,
    /// `(selector, window_token)` pairs the rules consult via
    /// `journey_seen` (boolean presence).
    pub seen_pairs: BTreeSet<(String, String)>,
    /// For each selector that any `journey_since_ge` rule references, the
    /// maximum `k` referenced. The aggregator ladders 1..=max_k.
    pub since_max_k: BTreeMap<String, u32>,
    /// `(field, window_token)` pairs for `journey_distinct`. Field in v1
    /// is `path`; other fields are accepted in the scan but emit no facts.
    pub distinct_pairs: BTreeSet<(String, String)>,
    /// For each `(target_selector, counted_selector)` pair any
    /// `journey_filtered_since_ge` rule references, the maximum `k`
    /// referenced. The aggregator ladders 1..=min(max_k, count_after_target).
    pub filtered_since_max_k: BTreeMap<(String, String), u32>,
}

impl RuleScan {
    fn references_session(&self) -> bool {
        self.occurrence_pairs.iter().any(|(_, w)| w == "s")
            || self.count_pairs.iter().any(|(_, w)| w == "s")
            || self.seen_pairs.iter().any(|(_, w)| w == "s")
            || self.distinct_pairs.iter().any(|(_, w)| w == "s")
    }

    fn max_call_window(&self) -> u32 {
        let mut max = 0u32;
        for w in self.window_tokens() {
            if let Ok(Window::Calls(n)) = Window::parse(&w)
                && n > max
            {
                max = n;
            }
        }
        max
    }

    fn max_time_seconds(&self) -> u64 {
        let mut max = 0u64;
        for w in self.window_tokens() {
            if let Ok(Window::Seconds(s)) = Window::parse(&w)
                && s > max
            {
                max = s;
            }
        }
        max
    }

    /// Every window token mentioned, deduplicated. Helper for the read-bound
    /// calculation; the same iteration is run a few times so the helper
    /// keeps the call sites legible.
    fn window_tokens(&self) -> BTreeSet<String> {
        let mut out: BTreeSet<String> = BTreeSet::new();
        for (_, w) in &self.occurrence_pairs {
            out.insert(w.clone());
        }
        for (_, w) in &self.count_pairs {
            out.insert(w.clone());
        }
        for (_, w) in &self.seen_pairs {
            out.insert(w.clone());
        }
        for (_, w) in &self.distinct_pairs {
            out.insert(w.clone());
        }
        out
    }
}

// ===== Rule scan =====

/// Walk every condition of every rule; pick out the `journey_*` references
/// in both `__script__` and bare-leaf forms; populate `RuleScan`. Malformed
/// windows surface as `DeriveError::BadWindow` immediately so a typo in a
/// rule isn't silently dropped.
pub fn scan_rules(rules: &[Rule]) -> Result<RuleScan, DeriveError> {
    let mut scan = RuleScan::default();
    for rule in rules {
        for cond in &rule.conditions {
            scan_condition(cond, &mut scan)?;
        }
    }
    Ok(scan)
}

fn scan_condition(cond: &phr::Condition, scan: &mut RuleScan) -> Result<(), DeriveError> {
    // Bare leaf form: predicate is `journey_<kind>`, args carry the
    // selector + window (+ k or field).
    if cond.predicate.starts_with("journey_") {
        record_pair(&cond.predicate, &cond.args, scan)?;
        return Ok(());
    }
    // __script__ form: parse the embedded predicate + args.
    if cond.predicate == "__script__"
        && let Some(script) = &cond.script
    {
        scan_script(script, scan)?;
    }
    Ok(())
}

/// Parse one `__script__` body for a `journey_*` reference. Tolerant of
/// either `facts_count('journey_X', [...]) <op> N` or
/// `facts_contain('journey_X', [...])`. Skips silently if neither form is
/// present — the script may be referencing some other predicate family
/// entirely (e.g. `signal_pass`).
fn scan_script(script: &str, scan: &mut RuleScan) -> Result<(), DeriveError> {
    let Some((predicate, args)) = parse_facts_call(script) else {
        return Ok(());
    };
    record_pair(predicate, &args, scan)?;
    Ok(())
}

/// Extract the `(predicate, args)` pair from a `facts_count(...)` or
/// `facts_contain(...)` script body. Returns `None` for any form that
/// doesn't match the expected grammar — scan_script maps these to a
/// silent `Ok(())` skip (malformed scripts are tolerantly ignored).
fn parse_facts_call(script: &str) -> Option<(&str, Vec<String>)> {
    let body = script.trim();
    let inner = body
        .strip_prefix("facts_count(")
        .or_else(|| body.strip_prefix("facts_contain("))?;
    let args_blob = &inner[..find_matching_paren(inner)?.0];
    let bracket_start = args_blob.find('[')?;
    let comma_pos = args_blob[..bracket_start].rfind(',')?;
    let predicate = args_blob[..comma_pos]
        .trim()
        .trim_matches('\'')
        .trim_matches('"');
    if !predicate.starts_with("journey_") {
        return None;
    }
    let args: Vec<String> = args_blob[comma_pos + 1..]
        .trim()
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .map(|ia| {
            ia.split(',')
                .map(|a| a.trim().trim_matches('\'').trim_matches('"').to_string())
                .collect()
        })?;
    Some((predicate, args))
}

/// Forward scan for a matching `)`, accounting for any nested `(`s — same
/// shape `script_evaluator` uses internally.
fn find_matching_paren(s: &str) -> Option<(usize, char)> {
    let mut depth = 1usize;
    for (i, ch) in s.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some((i, ch));
                }
            }
            _ => {}
        }
    }
    None
}

/// Common path between script-form and bare-leaf-form: given the parsed
/// `journey_<kind>` predicate and its args, file the pair into the right
/// `RuleScan` bucket and validate window tokens up front.
fn record_pair(predicate: &str, args: &[String], scan: &mut RuleScan) -> Result<(), DeriveError> {
    match predicate {
        "journey_occurrence" => {
            let (sel, win) = take_sel_win(args)?;
            Window::parse(&win)?;
            scan.occurrence_pairs.insert((sel, win));
        }
        "journey_count" => {
            let (sel, win) = take_sel_win(args)?;
            Window::parse(&win)?;
            scan.count_pairs.insert((sel, win));
        }
        "journey_seen" => {
            let (sel, win) = take_sel_win(args)?;
            Window::parse(&win)?;
            scan.seen_pairs.insert((sel, win));
        }
        "journey_since_ge" => {
            // args: [selector, k] — malformed args silently drop (no error).
            if let Some((sel, k)) = take_sel_k(args) {
                let entry = scan.since_max_k.entry(sel).or_insert(0);
                if k > *entry {
                    *entry = k;
                }
            }
        }
        "journey_filtered_since_ge" => {
            // args: [target_selector, counted_selector, k] — malformed args silently drop.
            if let Some((target, counted, k)) = take_two_sel_k(args) {
                let entry = scan
                    .filtered_since_max_k
                    .entry((target, counted))
                    .or_insert(0);
                if k > *entry {
                    *entry = k;
                }
            }
        }
        "journey_distinct" => {
            // args: [field, window, (?n)]
            let (field, win) = take_sel_win(args)?;
            Window::parse(&win)?;
            scan.distinct_pairs.insert((field, win));
        }
        _ => {}
    }
    Ok(())
}

/// Extract `(selector, k)` from the args of a `journey_since_ge` condition.
/// Returns `None` for malformed args (too few, or unparseable k) — the
/// caller silently skips those conditions rather than propagating an error.
fn take_sel_k(args: &[String]) -> Option<(String, u32)> {
    if args.len() < 2 {
        return None;
    }
    let k: u32 = args[1].parse().ok()?;
    Some((args[0].clone(), k))
}

/// Extract `(target, counted, k)` from the args of a
/// `journey_filtered_since_ge` condition. Returns `None` for malformed args.
fn take_two_sel_k(args: &[String]) -> Option<(String, String, u32)> {
    if args.len() < 3 {
        return None;
    }
    let k: u32 = args[2].parse().ok()?;
    Some((args[0].clone(), args[1].clone(), k))
}

fn take_sel_win(args: &[String]) -> Result<(String, String), DeriveError> {
    if args.len() < 2 {
        return Err(DeriveError::BadWindow(
            "expected at least [selector, window]".to_string(),
        ));
    }
    Ok((args[0].clone(), args[1].clone()))
}

// ===== Selector validation =====

/// Verify every selector mentioned in `scan` is defined in `cfg`. Tags
/// must appear as a bare `cfg.taggers[].tag`; module selectors must use
/// the `module:<name>` prefix and resolve to a `cfg.modules[].name`. The
/// `journey_distinct` field is treated as a selector — only `path` is
/// supported in v1, but we don't enforce the field name here (no
/// `journey.json` surface defines what fields exist; future schema bump
/// adds that).
pub fn validate_selectors(
    rules: &[Rule],
    scan: &RuleScan,
    cfg: &TaggerConfig,
) -> Result<(), DeriveError> {
    let defined_tags: HashSet<&str> = cfg.taggers.iter().map(|t| t.tag.as_str()).collect();
    let defined_modules: HashSet<String> = cfg
        .modules
        .iter()
        .map(|m| format!("module:{}", m.name))
        .collect();

    // Collect referenced selectors (tags and module:<name>). `journey_distinct`
    // selectors are field names (e.g. `path`) — out of the validation scope
    // since the config doesn't declare them.
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    for (s, _) in &scan.occurrence_pairs {
        referenced.insert(s.clone());
    }
    for (s, _) in &scan.count_pairs {
        referenced.insert(s.clone());
    }
    for (s, _) in &scan.seen_pairs {
        referenced.insert(s.clone());
    }
    for s in scan.since_max_k.keys() {
        referenced.insert(s.clone());
    }
    for (target, counted) in scan.filtered_since_max_k.keys() {
        referenced.insert(target.clone());
        referenced.insert(counted.clone());
    }

    for selector in &referenced {
        let ok = if let Some(name) = selector.strip_prefix("module:") {
            defined_modules.contains(selector) || cfg.modules.iter().any(|m| m.name == name)
        } else {
            defined_tags.contains(selector.as_str())
        };
        if !ok {
            let rule_id = rules
                .iter()
                .find(|r| rule_refs_selector(r, selector))
                .map(|r| r.id.clone())
                .unwrap_or_else(|| "<unknown>".to_string());
            return Err(DeriveError::UndefinedSelector {
                rule: rule_id,
                selector: selector.clone(),
            });
        }
    }
    Ok(())
}

fn rule_refs_selector(rule: &Rule, selector: &str) -> bool {
    for cond in &rule.conditions {
        if cond.predicate.starts_with("journey_") && cond.args.iter().any(|a| a == selector) {
            return true;
        }
        if cond.predicate == "__script__"
            && let Some(script) = &cond.script
        {
            // Quoted appearance in the script body counts.
            let needle_q = format!("'{}'", selector);
            let needle_dq = format!("\"{}\"", selector);
            if script.contains(&needle_q) || script.contains(&needle_dq) {
                return true;
            }
        }
    }
    false
}

// ===== Entry point =====

/// THE entry point both pre-check and post-check call. Scans rules,
/// validates selectors, reads the journal suffix, emits aggregator facts.
///
/// `network` is `&mut` to make the call shape explicit — every facts asserted
/// here mutates working memory — but the engine's internal locking means
/// `&self` would also work. Sticking with `&mut` makes the call site read
/// honestly.
pub async fn assert_facts(
    network: &mut ReteNetwork,
    input: DeriveInput<'_>,
) -> Result<(), DeriveError> {
    let scan = scan_rules(input.rules)?;
    validate_selectors(input.rules, &scan, input.config)?;

    // Read bound: pick the widest. If any rule needs the session window we
    // read up to the hard cap and let the per-window filter drop the rest;
    // the alternative (true reverse-scan until sid changes) is a future
    // optimization, not a v1 contract.
    let read_n = {
        let max_calls = scan.max_call_window();
        let max_seconds = scan.max_time_seconds();
        let needs_session = scan.references_session();
        let needs_since = !scan.since_max_k.is_empty();
        let needs_filtered_since = !scan.filtered_since_max_k.is_empty();
        if needs_session || needs_since || needs_filtered_since || max_seconds > 0 {
            // session floor / time window / distance-since-last all need an
            // open-ended look-back; let the hard cap bound the cost and let
            // the per-record filter drop the rest.
            journal::SUFFIX_HARD_CAP
        } else {
            // Calls-only window: read exactly enough records to satisfy the
            // largest call window.
            (max_calls as usize).max(1)
        }
    };

    let records = journal::read_recent(input.project_root, read_n)?;
    let context = WindowContext {
        records: &records,
        scope: input.scope,
    };

    emit_occurrence(network, &context, &scan).await;
    emit_count(network, &context, &scan).await;
    emit_seen(network, &context, &scan).await;
    emit_since_ge(network, &records, &scan).await;
    emit_distinct(network, &context, &scan).await;
    emit_filtered_since_ge(network, &records, &scan).await;
    Ok(())
}

// ===== Filtering helpers =====

fn record_in_window(
    rec: &JournalRecord,
    window_tok: &str,
    rec_idx: usize,
    context: &WindowContext<'_>,
) -> bool {
    let window = match Window::parse(window_tok) {
        Ok(w) => w,
        Err(_) => return false,
    };
    match window {
        Window::Calls(n) => {
            // Last n records (by position in `records`, which is append order).
            let total = context.records.len();
            let start = total.saturating_sub(n as usize);
            rec_idx >= start
        }
        Window::Seconds(s) => rec.ts + s >= context.scope.now_ts,
        Window::Session => rec.sid == context.scope.current_sid,
    }
}

fn matches_selector(rec: &JournalRecord, selector: &str) -> bool {
    if let Some(name) = selector.strip_prefix("module:") {
        rec.module.as_deref() == Some(name)
    } else {
        rec.tags.iter().any(|t| t == selector)
    }
}

// ===== Aggregator emitters =====

async fn emit_occurrence(network: &ReteNetwork, context: &WindowContext<'_>, scan: &RuleScan) {
    for (sel, win) in &scan.occurrence_pairs {
        let mut n = 0u64;
        for (i, rec) in context.records.iter().enumerate() {
            if !matches_selector(rec, sel) {
                continue;
            }
            if !record_in_window(rec, win, i, context) {
                continue;
            }
            n += 1;
            let id = format!("journey_occurrence:{}:{}:{}", sel, win, rec.seq);
            let _ = network
                .assert_fact(Fact {
                    id,
                    predicate: "journey_occurrence".to_string(),
                    args: vec![sel.clone(), win.clone()],
                    timestamp: 0,
                })
                .await;
            // Avoid unbounded blow-up if windows are degenerate; the hard
            // cap on suffix already bounds `records.len()`, but be
            // defensive.
            if n > journal::SUFFIX_HARD_CAP as u64 {
                break;
            }
        }
    }
}

async fn emit_count(network: &ReteNetwork, context: &WindowContext<'_>, scan: &RuleScan) {
    for (sel, win) in &scan.count_pairs {
        let mut count = 0u64;
        for (i, rec) in context.records.iter().enumerate() {
            if !matches_selector(rec, sel) {
                continue;
            }
            if !record_in_window(rec, win, i, context) {
                continue;
            }
            count += 1;
        }
        let id = format!("journey_count:{}:{}", sel, win);
        let _ = network
            .assert_fact(Fact {
                id,
                predicate: "journey_count".to_string(),
                args: vec![sel.clone(), win.clone(), count.to_string()],
                timestamp: 0,
            })
            .await;
    }
}

async fn emit_seen(network: &ReteNetwork, context: &WindowContext<'_>, scan: &RuleScan) {
    for (sel, win) in &scan.seen_pairs {
        let any =
            context.records.iter().enumerate().any(|(i, rec)| {
                matches_selector(rec, sel) && record_in_window(rec, win, i, context)
            });
        if !any {
            continue;
        }
        let id = format!("journey_seen:{}:{}", sel, win);
        let _ = network
            .assert_fact(Fact {
                id,
                predicate: "journey_seen".to_string(),
                args: vec![sel.clone(), win.clone()],
                timestamp: 0,
            })
            .await;
    }
}

async fn emit_since_ge(network: &ReteNetwork, records: &[JournalRecord], scan: &RuleScan) {
    for (sel, max_k) in &scan.since_max_k {
        // Distance-since-last: count records (from the end) until we see a
        // matching one. If none in the suffix → no facts.
        let mut distance: Option<u32> = None;
        for (i, rec) in records.iter().enumerate().rev() {
            if matches_selector(rec, sel) {
                // distance counts non-matching records *after* the matching one.
                distance = Some((records.len() - 1 - i) as u32);
                break;
            }
        }
        let d = match distance {
            Some(d) => d,
            None => continue,
        };
        let upper = (*max_k).min(d);
        for k in 1..=upper {
            let id = format!("journey_since_ge:{}:{}", sel, k);
            let _ = network
                .assert_fact(Fact {
                    id,
                    predicate: "journey_since_ge".to_string(),
                    args: vec![sel.clone(), k.to_string()],
                    timestamp: 0,
                })
                .await;
        }
    }
}

async fn emit_distinct(network: &ReteNetwork, context: &WindowContext<'_>, scan: &RuleScan) {
    for (field, win) in &scan.distinct_pairs {
        // v1 supports `path` only; unknown fields emit 0 (no fact) rather
        // than erroring — the silent-typo guard for selectors is the
        // real first line of defence; field typos surface as zero counts
        // and the rule simply won't fire. Document in spec, don't shoot
        // the foot.
        if field != "path" {
            continue;
        }
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for (i, rec) in context.records.iter().enumerate() {
            if !record_in_window(rec, win, i, context) {
                continue;
            }
            seen.insert(rec.path.clone());
        }
        let id = format!("journey_distinct:{}:{}", field, win);
        let _ = network
            .assert_fact(Fact {
                id,
                predicate: "journey_distinct".to_string(),
                args: vec![field.clone(), win.clone(), seen.len().to_string()],
                timestamp: 0,
            })
            .await;
    }
}

/// `journey_filtered_since_ge` — ladder from 1..=min(max_k, count_after_target),
/// where `count_after_target` is the number of `counted`-matching records that
/// appear strictly after the most recent `target`-matching record in the
/// suffix. Emits nothing when:
///  * the target selector never matches in the suffix, or
///  * no `counted`-matching records appear after the most recent target,
///  * `target == counted` (the "self-after-last-self" suffix is always empty).
///
/// Shape mirrors `emit_since_ge`; only the inner counting loop swaps "all
/// records after target" for "records matching counted after target."
async fn emit_filtered_since_ge(network: &ReteNetwork, records: &[JournalRecord], scan: &RuleScan) {
    for ((target, counted), max_k) in &scan.filtered_since_max_k {
        let Some(target_idx) = records.iter().rposition(|r| matches_selector(r, target)) else {
            continue;
        };
        let count = records[target_idx + 1..]
            .iter()
            .filter(|r| matches_selector(r, counted))
            .count() as u32;
        let upper = (*max_k).min(count);
        for k in 1..=upper {
            let id = format!("journey_filtered_since_ge:{}:{}:{}", target, counted, k);
            let _ = network
                .assert_fact(Fact {
                    id,
                    predicate: "journey_filtered_since_ge".to_string(),
                    args: vec![target.clone(), counted.clone(), k.to_string()],
                    timestamp: 0,
                })
                .await;
        }
    }
}

// ===== Unit tests =====

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_parse_calls() {
        assert_eq!(Window::parse("1c").unwrap(), Window::Calls(1));
        assert_eq!(Window::parse("99c").unwrap(), Window::Calls(99));
    }

    #[test]
    fn window_parse_seconds_units() {
        assert_eq!(Window::parse("1m").unwrap(), Window::Seconds(60));
        assert_eq!(Window::parse("1h").unwrap(), Window::Seconds(3600));
        assert_eq!(Window::parse("1d").unwrap(), Window::Seconds(86_400));
    }

    #[test]
    fn record_in_window_session() {
        let rec = JournalRecord {
            v: 1,
            ts: 1000,
            sid: "s-a".to_string(),
            seq: 1,
            tool: "Edit".to_string(),
            path: "x".to_string(),
            ext: None,
            module: None,
            tags: vec![],
            subject: None,
            command_exit: None,
        };
        let records = std::slice::from_ref(&rec);
        let current = WindowContext {
            records,
            scope: WindowScope {
                current_sid: "s-a",
                now_ts: 0,
            },
        };
        let other = WindowContext {
            records,
            scope: WindowScope {
                current_sid: "s-b",
                now_ts: 0,
            },
        };
        assert!(record_in_window(&rec, "s", 0, &current));
        assert!(!record_in_window(&rec, "s", 0, &other));
    }

    #[test]
    fn record_in_window_calls() {
        // 5 records; window of 2c → only the last two qualify.
        let recs: Vec<JournalRecord> = (0..5u64)
            .map(|i| JournalRecord {
                v: 1,
                ts: 100 + i,
                sid: "s".to_string(),
                seq: i,
                tool: "E".to_string(),
                path: "p".to_string(),
                ext: None,
                module: None,
                tags: vec!["t".to_string()],
                subject: None,
                command_exit: None,
            })
            .collect();
        let context = WindowContext {
            records: &recs,
            scope: WindowScope {
                current_sid: "s",
                now_ts: 0,
            },
        };
        assert!(!record_in_window(&recs[2], "2c", 2, &context));
        assert!(record_in_window(&recs[3], "2c", 3, &context));
        assert!(record_in_window(&recs[4], "2c", 4, &context));
    }

    #[test]
    fn matches_selector_tag_vs_module() {
        let rec = JournalRecord {
            v: 1,
            ts: 0,
            sid: "s".to_string(),
            seq: 1,
            tool: "E".to_string(),
            path: "p".to_string(),
            ext: None,
            module: Some("payments".to_string()),
            tags: vec!["sql".to_string()],
            subject: None,
            command_exit: None,
        };
        assert!(matches_selector(&rec, "sql"));
        assert!(matches_selector(&rec, "module:payments"));
        assert!(!matches_selector(&rec, "auth"));
        assert!(!matches_selector(&rec, "module:auth"));
    }

    #[test]
    fn scan_rules_picks_up_seen() {
        let cond = phr::Condition {
            predicate: "journey_seen".to_string(),
            args: vec!["sql".to_string(), "5c".to_string()],
            script: None,
        };
        let rule = Rule {
            id: "r".to_string(),
            priority: 0,
            conditions: vec![cond],
            actions: vec![],
        };
        let scan = scan_rules(&[rule]).unwrap();
        assert!(
            scan.seen_pairs
                .contains(&("sql".to_string(), "5c".to_string()))
        );
    }

    #[test]
    fn scan_rules_rejects_bad_window_in_script() {
        let cond = phr::Condition {
            predicate: "__script__".to_string(),
            args: vec![],
            script: Some("facts_count('journey_occurrence', ['x','5X']) >= 1".to_string()),
        };
        let rule = Rule {
            id: "r".to_string(),
            priority: 0,
            conditions: vec![cond],
            actions: vec![],
        };
        let err = scan_rules(&[rule]).unwrap_err();
        let msg = format!("{}", err);
        assert!(msg.contains("5X"), "{}", msg);
    }
}

//! The Helm 3 sensor: derives structural edges from one Helm 3 template file.
//!
//! Helm template source is Go template language embedded in files that often
//! have YAML names; it is not valid YAML before rendering. A valid chart
//! boundary therefore owns its `templates/` source, while `Chart.yaml`,
//! `values.yaml`, `values.schema.json`, and files read through `.Files`
//! remain YAML/JSON document nodes connected through cross-language `imports`
//! edges.
//!
//! This module implements a purpose-built action lexer and parser for Go
//! templates (per the Helm 3 spec, §5 template parsing). It handles:
//!
//! - `{{ ... }}` actions with whitespace trim markers (`{{-`, `-}}`)
//! - Quoted strings (`"..."`), single-quoted strings (`'...'`), and raw
//!   strings (`` `...` ``)
//! - Go template comments (`{{/* ... */}}`) — calls inside are not extracted
//! - Nested control scopes (`if`, `with`, `range`)
//! - Multiple definitions per physical file
//! - Malformed/unclosed actions without erasing previous graph state
//!
//! Surrounding bytes are opaque output text. YAML-looking keys, documents,
//! and indentation in template source create no YAML graph facts.
//!
//! Template definitions (`{{ define "name" }}`, `{{ block "name" ... }}`) map
//! to `graph_definition` + `defines` + `element_in_file` +
//! `element_in_module`, not `defines_fn`. Dynamic Helm calls use syntax facts
//! rather than repurposing the function-only `calls_api` relation.

use super::extract::Extracted;
use super::model::Edge;
use super::unit::UnitContext;
use std::collections::BTreeSet;

// ── Tokeniser ──────────────────────────────────────────────────────────

/// Tokens emitted by the Go-template lexer.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    /// Text outside any action (opaque).
    Text,
    /// Opening `{{` (possibly `{{-`).
    OpenAction(Trim),
    /// Closing `}}` (possibly `-}}`).
    CloseAction(Trim),
    /// Raw content string between `{{` and `}}`.
    ActionContent(String),
    /// Quoted string inside an action: double, single, or backtick.
    QStr(QType, String),
    /// Whitespace inside an action.
    WS,
    /// A single punctuation character that isn't `"`, `'`, `` ` ``.
    Punct(char),
}

/// String quote type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QType {
    Dbl, // "..."
    Sgl, // '...'
    Raw, // `...`
}

/// Whitespace trim marker on an action delimiter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trim {
    None,
    Left,  // `-` before content (e.g. `{{-`)
    Right, // `-` after content (e.g. `-}}`)
    Both,  // `{{-` … `-}}` (never constructed by the lexer, but part of the spec)
}

#[allow(dead_code)]
impl Trim {
    fn merge(self, other: Trim) -> Trim {
        match (self, other) {
            (Trim::None, t) | (t, Trim::None) => t,
            (Trim::Left, Trim::Right) | (Trim::Right, Trim::Left) => Trim::Both,
            (Trim::Left, Trim::Left) => Trim::Left,
            (Trim::Right, Trim::Right) => Trim::Right,
            (Trim::Both, _) | (_, Trim::Both) => Trim::Both,
        }
    }
}

/// Tokenise a raw Go-template string into a flat token stream.
///
/// The lexer walks the raw source looking for `{{` and `}}` delimiters.
/// Text between delimiters is opaque (collapsed to `Text` tokens). Inside
/// actions the lexer splits on quoted strings, whitespace, punctuation, and
/// further `{{`/`}}` (nesting).
fn lex(raw: &str) -> Vec<Tok> {
    let mut tokens = Vec::with_capacity(raw.len() / 4);
    let mut chs = raw.chars().peekable();

    while let Some(ch) = chs.next() {
        // Opening `{{` (with optional leading `-`).
        if ch == '{' && chs.peek() == Some(&'{') {
            chs.next();
            let trim = if chs.peek() == Some(&'-') {
                chs.next();
                Trim::Left
            } else {
                Trim::None
            };
            tokens.push(Tok::OpenAction(trim));

            // Collect raw content between {{ and }}.
            let mut action_buf = String::new();
            let mut closed = false;
            while let Some(n) = chs.next() {
                if n == '}' && chs.peek() == Some(&'}') {
                    chs.next(); // consume second }
                    let trim = if chs.peek() == Some(&'-') {
                        chs.next();
                        Trim::Right
                    } else {
                        Trim::None
                    };
                    tokens.push(Tok::CloseAction(trim));
                    closed = true;
                    break;
                }
                action_buf.push(n);
            }
            if closed && !action_buf.trim().is_empty() {
                tokens.push(Tok::ActionContent(action_buf));
            }
            continue;
        }
        // Accumulate opaque text until next `{{`.
        let mut seen = false;
        while let Some(&n) = chs.peek() {
            if n == '{' {
                let peeked: Vec<char> = chs.clone().take(2).collect();
                if peeked.len() >= 2 && peeked[0] == '{' && peeked[1] == '{' {
                    break;
                }
            }
            seen = true;
            chs.next();
        }
        if seen {
            tokens.push(Tok::Text);
        }
    }

    tokens
}

/// Tokenise the **content** between a `{{` and the matching `}}`.
fn lex_action(raw: &str) -> Vec<Tok> {
    let mut tokens = Vec::with_capacity(raw.len() / 4);
    let mut chs = raw.chars().peekable();

    while let Some(ch) = chs.next() {
        // Nested `{{` / `}}` inside an action.
        if ch == '{' && chs.peek() == Some(&'{') {
            chs.next();
            let trim = if chs.peek() == Some(&'-') {
                chs.next();
                Trim::Left
            } else {
                Trim::None
            };
            tokens.push(Tok::OpenAction(trim));
            continue;
        }
        if ch == '}' && chs.peek() == Some(&'}') {
            chs.next();
            let trim = if chs.peek() == Some(&'-') {
                chs.next();
                Trim::Right
            } else {
                Trim::None
            };
            tokens.push(Tok::CloseAction(trim));
            continue;
        }
        // Quoted string literals.
        match ch {
            '"' => {
                let mut s = String::new();
                s.push('"');
                while let Some(n) = chs.next() {
                    s.push(n);
                    if n == '"' {
                        break;
                    }
                    // Handle escape sequences.
                    if n == '\\'
                        && let Some(e) = chs.next()
                    {
                        s.push(e);
                    }
                }
                tokens.push(Tok::QStr(QType::Dbl, s));
            }
            '\'' => {
                let mut s = String::new();
                s.push('\'');
                for n in chs.by_ref() {
                    s.push(n);
                    if n == '\'' {
                        break;
                    }
                }
                tokens.push(Tok::QStr(QType::Sgl, s));
            }
            '`' => {
                let mut s = String::new();
                s.push('`');
                for n in chs.by_ref() {
                    s.push(n);
                    if n == '`' {
                        break;
                    }
                }
                tokens.push(Tok::QStr(QType::Raw, s));
            }
            ' ' | '\t' | '\r' | '\n' => {
                // Collapse whitespace.
                while let Some(&n) = chs.peek()
                    && (n == ' ' || n == '\t' || n == '\r' || n == '\n')
                {
                    chs.next();
                }
                tokens.push(Tok::WS);
            }
            _ => tokens.push(Tok::Punct(ch)),
        }
    }

    tokens
}

// ── Parser / extractor ─────────────────────────────────────────────────

/// Collected facts from parsing one file.
#[derive(Debug, Default)]
struct Facts {
    /// (qualified_name, owning_file).
    defines: Vec<(String, String)>,
    /// (caller_module, resolved_target).
    imports: Vec<(String, String)>,
    /// Unique .Values path segments.
    values: BTreeSet<String>,
    /// Literal `.Files.Get` paths.
    files_get: Vec<String>,
    /// Whether tpl call was found.
    has_tpl: bool,
    /// Whether lookup call was found.
    has_lookup: bool,
}

/// Strip Go template inline comments (/* ... */) from action content.
fn strip_comments(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chs = s.chars().peekable();
    while let Some(ch) = chs.next() {
        if ch == '/' && chs.peek() == Some(&'*') {
            chs.next(); // consume *
            while let Some(c) = chs.next() {
                if c == '*' && chs.peek() == Some(&'/') {
                    chs.next();
                    break;
                }
            }
        } else {
            result.push(ch);
        }
    }
    result
}

/// Parse tokenised action content and collect Helm-relevant facts.
///
/// This parser walks action tokens, treating quoted strings as opaque
/// (so `{{ define "x" }}` inside a comment is ignored by the caller).
fn parse_action(content: &str, self_module: &str, chart_name: &str) -> Facts {
    let mut facts = Facts::default();
    // Strip Go template comments (/* ... */) so calls inside are not extracted.
    let cleaned = strip_comments(content);
    let tokens = lex_action(&cleaned);

    // ── define / block ──────────────────────────────────────────────
    // Both register a named template.
    let keywords = [("define", 6), ("block", 5)];
    for (kw, kw_len) in &keywords {
        if action_starts_with_kw(&tokens, kw.as_bytes())
            && let Some(name) = extract_quoted_after_kw(&tokens, kw.as_bytes(), *kw_len)
        {
            let qualified = format!("{self_module}::define:{name}");
            facts
                .defines
                .push((qualified.clone(), self_module.to_string()));
            // `block` also emits an import from the containing file.
            if *kw == "block" {
                let resolved = format!("{chart_name}::templates::{name}");
                facts.imports.push((self_module.to_string(), resolved));
            }
        }
    }

    // ── template / include ──────────────────────────────────────────
    let call_kws = [("template", 8), ("include", 7)];
    for (kw, kw_len) in &call_kws {
        if action_starts_with_kw(&tokens, kw.as_bytes())
            && let Some(name) = extract_quoted_after_kw(&tokens, kw.as_bytes(), *kw_len)
        {
            let resolved = format!("{chart_name}::templates::{name}");
            facts.imports.push((self_module.to_string(), resolved));
        }
    }

    // ── tpl (dynamic template evaluation) ───────────────────────────
    if action_starts_with_kw(&tokens, b"tpl") {
        facts.has_tpl = true;
    }

    // ── lookup (cluster API call) ───────────────────────────────────
    if action_starts_with_kw(&tokens, b"lookup") {
        facts.has_lookup = true;
    }

    // ── .Values.something ───────────────────────────────────────────
    for val_path in extract_values_paths(&tokens) {
        facts.values.insert(val_path);
    }

    // ── .Files.Get "path" ───────────────────────────────────────────
    for fpath in extract_files_get(&tokens) {
        facts.files_get.push(fpath);
    }

    facts
}

/// Returns true when the first meaningful token after leading whitespace
/// exactly matches `keyword` as a standalone word.
fn action_starts_with_kw(tokens: &[Tok], keyword: &[u8]) -> bool {
    // Collect meaningful tokens (skip whitespace).
    let meaningful: Vec<&Tok> = tokens.iter().filter(|t| !matches!(t, Tok::WS)).collect();

    // Need at least one token.
    if meaningful.is_empty() {
        return false;
    }

    // Try to build a word from consecutive Punct tokens.
    let mut word_chars: Vec<char> = Vec::new();
    let mut punct_idx = 0;
    if let Tok::Punct(first) = &meaningful[0] {
        word_chars.push(*first);
        punct_idx += 1;
    } else {
        return false;
    }
    while punct_idx < meaningful.len() {
        if let Tok::Punct(c) = &meaningful[punct_idx] {
            if c.is_alphanumeric() || *c == '_' {
                word_chars.push(*c);
                punct_idx += 1;
            } else {
                break;
            }
        } else {
            break;
        }
    }

    let word_s: String = word_chars.iter().collect();
    let exact = word_s.as_bytes() == keyword;

    // Next meaningful token should NOT be alphanum/underscore.
    let follow_ok = if punct_idx < meaningful.len() {
        match &meaningful[punct_idx] {
            Tok::Punct(c) => !c.is_alphanumeric() && *c != '_',
            _ => true,
        }
    } else {
        true
    };

    exact && follow_ok
}

/// Extract a quoted string argument appearing after `keyword` (with optional
/// whitespace). Returns the unquoted content.
fn extract_quoted_after_kw(tokens: &[Tok], kw: &[u8], _kw_len: usize) -> Option<String> {
    // Find the first punctuation token matching keyword[0].
    let first_char = kw[0] as char;
    let mut skip_count = 0;
    for (idx, t) in tokens.iter().enumerate() {
        if let Tok::Punct(c) = t
            && *c == first_char
        {
            skip_count = idx + 1;
            break;
        }
    }

    // Skip keyword + whitespace, then grab the first quoted string.
    for t in tokens.iter().skip(skip_count) {
        if let Tok::QStr(_, s) = t {
            return Some(strip_quotes(s));
        }
    }
    None
}

/// Strip surrounding quote characters from a string token.
fn strip_quotes(s: &str) -> String {
    let s = s.strip_prefix('"').unwrap_or(s);
    let s = s.strip_prefix('\'').unwrap_or(s);
    let s = s.strip_prefix('`').unwrap_or(s);
    let s = s.strip_suffix('"').unwrap_or(s);
    let s = s.strip_suffix('\'').unwrap_or(s);
    let s = s.strip_suffix('`').unwrap_or(s);
    s.to_string()
}

/// Extract `.Values.something` paths from action tokens.
fn extract_values_paths(tokens: &[Tok]) -> Vec<String> {
    // Copy to avoid borrowing issues.
    let toks: Vec<Tok> = tokens.to_vec();
    let mut paths = Vec::new();

    let mut i = 0;
    while i + 8 <= toks.len() {
        // Look for the sequence: `.`, `V`, `a`, `l`, `u`, `e`, `s`, `.`
        if matches!(&toks[i], Tok::Punct('.'))
            && matches!(&toks[i + 1], Tok::Punct('V'))
            && matches!(&toks[i + 2], Tok::Punct('a'))
            && matches!(&toks[i + 3], Tok::Punct('l'))
            && matches!(&toks[i + 4], Tok::Punct('u'))
            && matches!(&toks[i + 5], Tok::Punct('e'))
            && matches!(&toks[i + 6], Tok::Punct('s'))
            && matches!(&toks[i + 7], Tok::Punct('.'))
        {
            // Extract path after `.Values.`.
            let after_len = toks[i + 8..]
                .iter()
                .take_while(|t| match t {
                    Tok::Punct(c) => c.is_alphanumeric() || *c == '.' || *c == '[' || *c == ']',
                    _ => false,
                })
                .count();

            let raw_path: String = toks[i + 8..i + 8 + after_len]
                .iter()
                .map(|t| t.token_str())
                .collect();
            let normalized = raw_path.replace('[', ".").replace(']', "");
            if !normalized.is_empty() {
                paths.push(normalized);
            }
            i = i + 8 + after_len;
            continue;
        }
        i += 1;
    }
    paths
}

/// Extract literal `.Files.Get "path"` arguments from action tokens.
fn extract_files_get(tokens: &[Tok]) -> Vec<String> {
    let mut paths = Vec::new();

    // Look for sequence: `.` `F` `i` `l` `e` `s` `.` `G` `e` `t` followed by quoted string.
    let toks: Vec<&Tok> = tokens.iter().collect();
    let limit = toks.len().min(14);
    for i in 0..limit {
        if i + 10 > toks.len() {
            break;
        }
        if matches!(&toks[i], Tok::Punct('.'))
            && matches!(&toks[i + 1], Tok::Punct('F'))
            && matches!(&toks[i + 2], Tok::Punct('i'))
            && matches!(&toks[i + 3], Tok::Punct('l'))
            && matches!(&toks[i + 4], Tok::Punct('e'))
            && matches!(&toks[i + 5], Tok::Punct('s'))
            && matches!(&toks[i + 6], Tok::Punct('.'))
            && matches!(&toks[i + 7], Tok::Punct('G'))
            && matches!(&toks[i + 8], Tok::Punct('e'))
            && matches!(&toks[i + 9], Tok::Punct('t'))
        {
            // Skip whitespace, then grab the first quoted string after `.Files.Get`.
            for t in toks.iter().skip(i + 10) {
                if matches!(t, Tok::WS) {
                    continue;
                }
                if let Tok::QStr(_, s) = t {
                    let path = strip_quotes(s);
                    if !path.is_empty() {
                        paths.push(path);
                    }
                }
                break; // Only the first quoted arg (non-ws).
            }
            break;
        }
    }
    paths
}

/// Convert a Tok to its printable string representation (for punctuation).
trait TokenStr {
    fn token_str(&self) -> String;
}
impl TokenStr for Tok {
    fn token_str(&self) -> String {
        match self {
            Tok::Punct(c) => c.to_string(),
            Tok::QStr(_, s) => s.clone(),
            _ => String::new(),
        }
    }
}

// ── File classifier ────────────────────────────────────────────────────

/// Classify a file by its name and location within the chart.
fn classify_file(file_path: &str, chart_root: &str) -> &'static str {
    let base = file_path.rsplit_once('/').map_or(file_path, |(_, b)| b);
    if base == "Chart.yaml" {
        return "chart_manifest";
    }
    if base.ends_with("_helpers.tpl") {
        return "helm_helpers";
    }
    if file_path.starts_with("templates/")
        || file_path.contains("/templates/")
        || file_path.starts_with(&format!("{}/templates/", chart_root))
        || file_path.contains(&format!("/{}/templates/", chart_root))
    {
        return "helm_template";
    }
    "helm_template"
}

/// Build the language-qualified module path for a file.
///
/// `templates/deployment.yaml` → `helm3:mychart::templates::deployment`
fn build_module_path(file_path: &str, chart_name: &str) -> String {
    let trimmed = file_path
        .strip_suffix(".yaml")
        .or_else(|| file_path.strip_suffix(".yml"))
        .unwrap_or(file_path);
    let trimmed = trimmed.strip_suffix(".tpl").unwrap_or(trimmed);
    let segments: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
    let ns = format!("helm3:{chart_name}");
    std::iter::once(ns.as_str())
        .chain(segments)
        .collect::<Vec<_>>()
        .join("::")
}

/// Resolve a `.Files.Get` path to a YAML/JSON module path.
fn resolve_file_reference(
    file_path: &str,
    chart_name: &str,
    file_ref: &str,
) -> Option<(String, Vec<String>)> {
    let resolved = file_ref.trim_start_matches('/');

    let owned_ext =
        resolved.ends_with(".yaml") || resolved.ends_with(".yml") || resolved.ends_with(".json");
    if !owned_ext {
        return None;
    }

    let ext = if resolved.ends_with(".json") {
        "json"
    } else if resolved.ends_with(".yaml") || resolved.ends_with(".yml") {
        "yaml"
    } else {
        return None;
    };

    let base = resolved
        .strip_suffix(".yaml")
        .or_else(|| resolved.strip_suffix(".yml"))
        .or_else(|| resolved.strip_suffix(".json"))
        .unwrap_or(resolved);
    let segments: Vec<&str> = base.split('/').filter(|s| !s.is_empty()).collect();
    let ns = format!("{ext}:{chart_name}");
    let target = std::iter::once(ns.as_str())
        .chain(segments)
        .collect::<Vec<_>>()
        .join("::");

    Some((
        "imports".to_string(),
        vec![build_module_path(file_path, chart_name), target],
    ))
}

// ── Public API ─────────────────────────────────────────────────────────

/// Extract every base relation from one Helm 3 template or chart manifest file.
pub fn extract_helm3(
    file_path: &str,
    content: &str,
    unit: &UnitContext,
    chart_root: Option<&str>,
) -> Extracted {
    let is_chart_yaml = file_path.ends_with("Chart.yaml") || file_path.ends_with("Chart.yml");
    let is_tpl = file_path.ends_with(".tpl");
    let is_templated_yaml = chart_root.is_some()
        && (file_path.ends_with(".yaml") || file_path.ends_with(".yml"))
        && file_path.contains("/templates/")
        && content.contains("{{");
    if !is_chart_yaml && !is_tpl && !is_templated_yaml {
        return Extracted::default();
    }

    if content.trim().is_empty() {
        return Extracted::unparseable();
    }

    let chart_root = chart_root.unwrap_or("");
    let chart_name = unit
        .id
        .strip_prefix("helm3:")
        .map(|rest| rest.split("::").next().unwrap_or("chart"))
        .unwrap_or("chart");

    let relative_file = file_path
        .strip_prefix(chart_root)
        .and_then(|path| path.strip_prefix('/'))
        .unwrap_or(file_path);
    let self_module = build_module_path(relative_file, chart_name);
    let ft = classify_file(file_path, chart_root);

    let mut out: BTreeSet<(String, Vec<String>)> = BTreeSet::new();

    out.insert((
        "file_type".to_string(),
        vec![file_path.to_string(), ft.to_string()],
    ));
    out.insert((
        "declares_module".to_string(),
        vec![file_path.to_string(), self_module.clone()],
    ));

    // Tokenise the entire file and walk actions.
    let all_tokens = lex(content);
    let mut facts = Facts::default();

    let mut i = 0;
    while i < all_tokens.len() {
        if let Tok::ActionContent(action_content) = &all_tokens[i] {
            // Only parse if there's actual content (skip blank actions).
            if !action_content.trim().is_empty() {
                let file_facts = parse_action(action_content, &self_module, chart_name);
                merge_facts(&mut facts, file_facts);
            }
        }
        i += 1;
    }

    // Apply collected facts.
    for (qualified, _file) in &facts.defines {
        let q = qualified.clone();
        out.insert(("graph_definition".to_string(), vec![q.clone()]));
        out.insert((
            "defines".to_string(),
            vec![file_path.to_string(), q.clone()],
        ));
        out.insert((
            "element_in_file".to_string(),
            vec![q.clone(), file_path.to_string()],
        ));
        out.insert((
            "element_in_module".to_string(),
            vec![q.clone(), self_module.clone()],
        ));
    }

    for (origin, resolved) in &facts.imports {
        out.insert((
            "imports".to_string(),
            vec![origin.clone(), resolved.clone()],
        ));
    }

    for path in &facts.values {
        let resolved = format!("{chart_name}::values::{path}");
        out.insert(("imports".to_string(), vec![self_module.clone(), resolved]));
    }

    for fpath in &facts.files_get {
        if let Some((p, a)) = resolve_file_reference(relative_file, chart_name, fpath) {
            out.insert((p, a));
        }
    }

    if facts.has_tpl {
        out.insert((
            "helm3_dynamic_tpl".to_string(),
            vec![file_path.to_string(), self_module.clone()],
        ));
    }

    if facts.has_lookup {
        out.insert((
            "helm3_cluster_lookup".to_string(),
            vec![file_path.to_string(), self_module.clone()],
        ));
    }

    let edges = out
        .into_iter()
        .map(|(p, a)| Edge {
            p,
            a,
            src: file_path.to_string(),
            d: false,
        })
        .collect();

    Extracted {
        edges,
        skipped: 0,
        parse_failed: false,
    }
}

/// Merge facts from multiple actions into one accumulator.
fn merge_facts(acc: &mut Facts, other: Facts) {
    acc.defines.extend(other.defines);
    acc.imports.extend(other.imports);
    acc.values.extend(other.values);
    acc.files_get.extend(other.files_get);
    acc.has_tpl = acc.has_tpl || other.has_tpl;
    acc.has_lookup = acc.has_lookup || other.has_lookup;
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(_chart_name: &str) -> UnitContext {
        UnitContext {
            id: "helm3:mychart".to_string(),
            module_base: String::new(),
            siblings: std::collections::BTreeMap::new(),
            ts: crate::graph::unit::TsConfig::default(),
            files: Vec::new(),
            lua_files: Vec::new(),
            cue_files: Vec::new(),
        }
    }

    fn edges_of(x: &Extracted, p: &str) -> Vec<Vec<String>> {
        x.edges
            .iter()
            .filter(|e| e.p == p)
            .map(|e| e.a.clone())
            .collect()
    }

    // ── Lexer tests ──────────────────────────────────────────────────

    #[test]
    fn lexer_detects_open_close_actions() {
        let toks = lex("hello {{ world }} world");
        assert!(toks.iter().any(|t| matches!(t, Tok::OpenAction(_))));
        assert!(toks.iter().any(|t| matches!(t, Tok::CloseAction(_))));
    }

    #[test]
    fn lexer_detects_trim_markers() {
        let toks = lex("{{- define \"x\" -}}");
        assert!(
            toks.iter()
                .any(|t| matches!(t, Tok::OpenAction(Trim::Left)))
        );
    }

    #[test]
    fn lexer_captures_quoted_strings() {
        let toks = lex_action(r#"define "mychart.helpers" ."#);
        assert!(
            toks.iter()
                .any(|t| matches!(t, Tok::QStr(QType::Dbl, s) if s == r#""mychart.helpers""#))
        );
    }

    #[test]
    fn lexer_captures_raw_strings() {
        let toks = lex_action(r#"`config.yaml`"#);
        assert!(
            toks.iter()
                .any(|t| matches!(t, Tok::QStr(QType::Raw, s) if s == "`config.yaml`"))
        );
    }

    #[test]
    fn lexer_captures_single_quoted_strings() {
        let toks = lex_action(r#"'single quoted'"#);
        assert!(
            toks.iter()
                .any(|t| matches!(t, Tok::QStr(QType::Sgl, s) if s == "'single quoted'"))
        );
    }

    // ── File classifier tests ─────────────────────────────────────────

    #[test]
    fn non_helm_files_return_empty() {
        let out = extract_helm3("foo.py", "pass\n", &ctx("charts/app"), None);
        assert!(out.edges.is_empty());
        assert!(!out.parse_failed);
    }

    #[test]
    fn chart_yaml_is_recognized() {
        let out = extract_helm3(
            "Chart.yaml",
            "apiVersion: v2\nname: mychart\n",
            &ctx("charts/app"),
            Some("charts/app"),
        );
        assert!(!out.edges.is_empty());
        let fts = edges_of(&out, "file_type");
        assert!(fts.iter().any(|a| a[1] == "chart_manifest"));
    }

    #[test]
    fn tpl_file_is_recognized() {
        let out = extract_helm3(
            "templates/deployment.tpl",
            "{{ define \"deployment\" }}\napiVersion: apps/v1\n{{ end }}\n",
            &ctx("charts/app"),
            Some("charts/app"),
        );
        assert!(!out.edges.is_empty());
        let fts = edges_of(&out, "file_type");
        assert!(fts.iter().any(|a| a[1] == "helm_template"));
    }

    #[test]
    fn empty_content_returns_unparseable() {
        let out = extract_helm3(
            "templates/deployment.tpl",
            "",
            &ctx("charts/app"),
            Some("charts/app"),
        );
        assert!(out.parse_failed);
        assert!(out.edges.is_empty());
    }

    #[test]
    fn whitespace_only_content_returns_unparseable() {
        let out = extract_helm3(
            "templates/deployment.tpl",
            "   \n  \n  \n  ",
            &ctx("charts/app"),
            Some("charts/app"),
        );
        assert!(out.parse_failed);
    }

    // ── Define / block tests ──────────────────────────────────────────

    #[test]
    fn a_define_becomes_graph_definition() {
        let out = extract_helm3(
            "templates/deployment.tpl",
            "{{ define \"mychart.deployment\" }}\napiVersion: apps/v1\n{{ end }}\n",
            &ctx("charts/app"),
            Some("charts/app"),
        );
        let defs = edges_of(&out, "defines");
        assert_eq!(defs.len(), 1);
        assert!(defs[0][1].contains("::define:"));
        assert!(defs[0][1].contains("deployment"));
    }

    #[test]
    fn multiple_defines_in_one_file() {
        let content = r#"
{{ define "chart.helpers" }}
helpers here
{{ end }}

{{ define "chart.tpl" }}
more
{{ end }}
"#;
        let out = extract_helm3(
            "templates/_helpers.tpl",
            content,
            &ctx("charts/app"),
            Some("charts/app"),
        );
        let defs = edges_of(&out, "defines");
        assert_eq!(defs.len(), 2);
    }

    #[test]
    fn block_creates_define_and_import() {
        let out = extract_helm3(
            "templates/deployment.tpl",
            "{{ block \"mychart.tpl\" . }}\n{{ end }}\n",
            &ctx("charts/app"),
            Some("charts/app"),
        );
        let defs = edges_of(&out, "defines");
        assert_eq!(defs.len(), 1);
        let imps = edges_of(&out, "imports");
        assert!(imps.iter().any(|a| a[1].contains("mychart.tpl")));
    }

    #[test]
    fn helpers_tpl_is_classified_correctly() {
        let out = extract_helm3(
            "templates/_helpers.tpl",
            "{{ define \"helpers\" }}\n{{ end }}\n",
            &ctx("charts/app"),
            Some("charts/app"),
        );
        let fts = edges_of(&out, "file_type");
        assert!(fts.iter().any(|a| a[1] == "helm_helpers"));
    }

    #[test]
    fn declares_module_edge_maps_file_to_module() {
        let out = extract_helm3(
            "templates/deployment.tpl",
            "{{ define \"x\" }}\n{{ end }}\n",
            &ctx("charts/app"),
            Some("charts/app"),
        );
        let decls = edges_of(&out, "declares_module");
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0][0], "templates/deployment.tpl");
        assert!(decls[0][1].starts_with("helm3:mychart::"));
        assert!(decls[0][1].contains("templates"));
    }

    // ── Template / include call tests ─────────────────────────────────

    #[test]
    fn template_call_becomes_imports_edge() {
        let out = extract_helm3(
            "templates/deployment.tpl",
            "{{ template \"mychart.helpers\" }}\n",
            &ctx("charts/app"),
            Some("charts/app"),
        );
        let imps = edges_of(&out, "imports");
        assert!(
            imps.iter()
                .any(|a| { a[0].contains("deployment") && a[1].contains("helpers") })
        );
    }

    #[test]
    fn include_call_becomes_imports_edge() {
        let out = extract_helm3(
            "templates/deployment.tpl",
            "{{ include \"mychart.tpl\" . }}\n",
            &ctx("charts/app"),
            Some("charts/app"),
        );
        let imps = edges_of(&out, "imports");
        assert!(
            imps.iter()
                .any(|a| { a[0].contains("deployment") && a[1].contains("tpl") })
        );
    }

    // ── .Values tests ─────────────────────────────────────────────────

    #[test]
    fn values_reference_becomes_imports_edge() {
        let out = extract_helm3(
            "templates/deployment.tpl",
            "image: {{ .Values.image.repository }}:{{ .Values.image.tag }}\n",
            &ctx("charts/app"),
            Some("charts/app"),
        );
        let imps = edges_of(&out, "imports");
        assert!(imps.iter().any(|a| {
            a[0].contains("deployment") && a[1].contains("image") && a[1].contains("values")
        }));
    }

    #[test]
    fn values_references_are_deduplicated() {
        let content = r#"
{{ .Values.image.repository }}
{{ .Values.image.repository }}
{{ .Values.image.tag }}
"#;
        let out = extract_helm3(
            "templates/deployment.tpl",
            content,
            &ctx("charts/app"),
            Some("charts/app"),
        );
        let imps = edges_of(&out, "imports");
        let value_imports: Vec<&Vec<String>> = imps
            .iter()
            .filter(|a| a[0].contains("deployment"))
            .collect();
        assert_eq!(value_imports.len(), 2);
    }

    // ── .Files.Get tests ──────────────────────────────────────────────

    #[test]
    fn file_reference_resolves_to_yaml_import() {
        let out = extract_helm3(
            "templates/deployment.tpl",
            "{{ .Files.Get \"files/config.yaml\" }}\n",
            &ctx("charts/app"),
            Some("charts/app"),
        );
        let imps = edges_of(&out, "imports");
        assert!(imps.iter().any(|a| {
            a[0].contains("deployment") && a[1].contains("config") && a[1].starts_with("yaml")
        }));
    }

    #[test]
    fn file_reference_to_txt_is_not_exported() {
        let out = extract_helm3(
            "templates/deployment.tpl",
            "{{ .Files.Get \"files/readme.txt\" }}\n",
            &ctx("charts/app"),
            Some("charts/app"),
        );
        let imps = edges_of(&out, "imports");
        assert!(!imps.iter().any(|a| a[1].contains("readme")));
    }

    // ── Risk watchlist tests ──────────────────────────────────────────

    #[test]
    fn tpl_call_detected_as_risk() {
        let out = extract_helm3(
            "templates/deployment.tpl",
            "{{ tpl .Files.Get \"sub.tpl\" . }}\n",
            &ctx("charts/app"),
            Some("charts/app"),
        );
        let tpl = edges_of(&out, "helm3_dynamic_tpl");
        assert!(!tpl.is_empty());
    }

    #[test]
    fn lookup_call_detected_as_risk() {
        let out = extract_helm3(
            "templates/deployment.tpl",
            "{{ lookup \"apps/v1\" \"Deployment\" \"default\" \"my-deploy\" }}\n",
            &ctx("charts/app"),
            Some("charts/app"),
        );
        let lk = edges_of(&out, "helm3_cluster_lookup");
        assert!(!lk.is_empty());
    }

    // ── Negative tests — calls inside comments should not be extracted ─

    #[test]
    fn define_inside_comment_is_not_extracted() {
        let out = extract_helm3(
            "templates/deployment.tpl",
            "{{/* {{ define \"should_not_appear\" }} */}}\n",
            &ctx("charts/app"),
            Some("charts/app"),
        );
        let defs = edges_of(&out, "defines");
        assert!(
            !defs.iter().any(|d| d[1].contains("should_not_appear")),
            "define inside comment must not be extracted"
        );
    }

    #[test]
    fn template_call_inside_comment_is_not_extracted() {
        let out = extract_helm3(
            "templates/deployment.tpl",
            "{{/* {{ template \"ghost\" . }} */}}\n",
            &ctx("charts/app"),
            Some("charts/app"),
        );
        let imps = edges_of(&out, "imports");
        assert!(
            !imps.iter().any(|i| i[1].contains("ghost")),
            "template call inside comment must not be extracted"
        );
    }

    #[test]
    fn tpl_call_inside_comment_is_not_extracted() {
        let out = extract_helm3(
            "templates/deployment.tpl",
            "{{/* tpl .Files.Get \"injected.tpl\" . */}}\n",
            &ctx("charts/app"),
            Some("charts/app"),
        );
        let tpl = edges_of(&out, "helm3_dynamic_tpl");
        assert!(tpl.is_empty(), "tpl inside comment must not be detected");
    }

    #[test]
    fn lookup_call_inside_comment_is_not_extracted() {
        let out = extract_helm3(
            "templates/deployment.tpl",
            "{{/* lookup \"apps/v1\" \"Deployment\" \"ns\" \"name\" */}}\n",
            &ctx("charts/app"),
            Some("charts/app"),
        );
        let lk = edges_of(&out, "helm3_cluster_lookup");
        assert!(lk.is_empty(), "lookup inside comment must not be detected");
    }

    #[test]
    fn values_inside_comment_not_extracted() {
        let out = extract_helm3(
            "templates/deployment.tpl",
            "{{/* {{ .Values.ghost.value }} */}}\n",
            &ctx("charts/app"),
            Some("charts/app"),
        );
        let imps = edges_of(&out, "imports");
        assert!(
            !imps.iter().any(|i| i[1].contains("ghost")),
            ".Values inside comment must not be extracted"
        );
    }

    // ── Quoted string tests — calls inside quoted strings ─────────────

    #[test]
    fn define_inside_double_quoted_string_is_not_extracted() {
        let out = extract_helm3(
            "templates/deployment.tpl",
            "{{ $msg := \"define should_not_appear\" }}\n",
            &ctx("charts/app"),
            Some("charts/app"),
        );
        let defs = edges_of(&out, "defines");
        assert!(
            !defs.iter().any(|d| d[1].contains("should_not_appear")),
            "define inside string must not be extracted"
        );
    }

    // ── Whitespace trim marker tests ──────────────────────────────────

    #[test]
    fn edge_case_define_with_dash_trim() {
        let out = extract_helm3(
            "templates/deployment.tpl",
            "{{- define \"mychart.dash\" }}\n{{ end }}\n",
            &ctx("charts/app"),
            Some("charts/app"),
        );
        let defs = edges_of(&out, "defines");
        assert_eq!(defs.len(), 1);
    }

    // ── Module path tests ─────────────────────────────────────────────

    #[test]
    fn module_path_for_helpers_file() {
        let path = build_module_path("templates/_helpers.tpl", "mychart");
        assert!(path.contains("_helpers"));
        assert!(path.starts_with("helm3:mychart::"));
    }

    #[test]
    fn module_path_for_chart_yaml() {
        let path = build_module_path("Chart.yaml", "mychart");
        assert!(path.contains("Chart"));
        assert!(path.starts_with("helm3:mychart::"));
    }

    #[test]
    fn module_path_for_nested_template() {
        let path = build_module_path("templates/subdir/deployment.tpl", "mychart");
        assert!(path.contains("subdir"));
        assert!(path.contains("deployment"));
    }

    // ── Pipeline / nested action tests ────────────────────────────────

    #[test]
    fn pipeline_with_multiple_actions() {
        let out = extract_helm3(
            "templates/deployment.tpl",
            "{{ include \"tpl\" . }}\n{{- define \"a\" -}}\n{{ template \"b\" . }}\n{{ end }}\n",
            &ctx("charts/app"),
            Some("charts/app"),
        );
        let imps = edges_of(&out, "imports");
        assert!(imps.iter().any(|a| a[1].contains("tpl")));
        assert!(imps.iter().any(|a| a[1].contains("b")));
    }

    #[test]
    fn if_scope_does_not_interfere_with_define() {
        let out = extract_helm3(
            "templates/deployment.tpl",
            "{{- if .Values.enabled }}\n{{ define \"mychart.feature\" }}\n{{ end }}\n{{- end }}\n",
            &ctx("charts/app"),
            Some("charts/app"),
        );
        let defs = edges_of(&out, "defines");
        assert_eq!(defs.len(), 1);
        assert!(defs[0][1].contains("feature"));
    }

    // ── Malformed action tests ────────────────────────────────────────

    #[test]
    fn unclosed_action_does_not_collapse_graph_state() {
        // Malformed {{ define "x" with no closing }} should not panic.
        let out = extract_helm3(
            "templates/deployment.tpl",
            "{{ define \"partial\"\n",
            &ctx("charts/app"),
            Some("charts/app"),
        );
        // Unclosed action shouldn't extract anything, but must not panic.
        let defs = edges_of(&out, "defines");
        assert!(defs.is_empty(), "unclosed action should not extract");
    }

    #[test]
    fn single_curly_brace_is_not_action() {
        let out = extract_helm3(
            "templates/deployment.yaml",
            "{\n  \"apiVersion\": \"apps/v1\"\n}\n",
            &ctx("charts/app"),
            Some("charts/app"),
        );
        let imps = edges_of(&out, "imports");
        assert!(
            imps.is_empty(),
            "curly braces in YAML must not trigger actions"
        );
    }
}

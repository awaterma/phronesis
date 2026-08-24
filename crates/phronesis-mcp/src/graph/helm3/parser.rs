//! Action parser / fact extractor for the Helm 3 sensor.

use super::lexer::{Tok, lex_action};
use std::collections::BTreeSet;

/// Collected facts from parsing one file.
#[derive(Debug, Default)]
pub(super) struct Facts {
    /// (qualified_name, owning_file).
    pub(super) defines: Vec<(String, String)>,
    /// (caller_module, resolved_target).
    pub(super) imports: Vec<(String, String)>,
    /// Unique .Values path segments.
    pub(super) values: BTreeSet<String>,
    /// Literal `.Files.Get` paths.
    pub(super) files_get: Vec<String>,
    /// Whether tpl call was found.
    pub(super) has_tpl: bool,
    /// Whether lookup call was found.
    pub(super) has_lookup: bool,
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
/// This is deliberately a small evidence extractor, not a complete Go
/// template interpreter. It recognizes only the closed set of Helm constructs
/// represented by graph facts, treats quoted strings as opaque, and does not
/// attempt pipelines, variable evaluation, whitespace-control semantics, or
/// template execution. Keep additions paired with lexer/parser corpus tests;
/// use Helm itself when semantic rendering evidence is required.
pub(super) fn parse_action(content: &str, self_module: &str, chart_name: &str) -> Facts {
    let mut facts = Facts::default();
    // Strip Go template comments (/* ... */) so calls inside are not extracted.
    let tokens = {
        let cleaned = strip_comments(content);
        lex_action(&cleaned)
    };

    // ── define / block ──────────────────────────────────────────────
    // Both register a named template.
    let keywords = [("define", 6), ("block", 5)];
    for (kw, kw_len) in &keywords {
        if action_starts_with_kw(&tokens, kw.as_bytes())
            && let Some(name) = extract_quoted_after_kw(&tokens, kw.as_bytes(), *kw_len)
        {
            facts.defines.push((
                format!("{self_module}::define:{name}"),
                self_module.to_string(),
            ));
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

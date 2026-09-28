//! Rendered-body validation (SPEC-verification-artifact-generation.md §S5):
//! the second validator layer between the render entry and the file write.
//!
//! Layer 1 (the field-class contract) lives in `properties/store.rs` at
//! ingest. This layer re-parses the rendered body and asserts:
//! (a) every *inert* interpolated property value (free text: condition,
//!     guarantee, the property id, `depends_on` region strings) appears only
//!     inside string literals or comments — hostile payloads render inert;
//! (a') every *identifier* value (the property's `subject`, and fn/type
//!     names parsed from its `fn:`/`branch:` `depends_on` entries) appears in
//!     live code, if at all, only as a complete identifier or path token —
//!     never as a substring of a longer identifier, a macro invocation name,
//!     or part of a longer path — everywhere else it is held to the same
//!     inert-only rule as (a) (see `is_identifier_value`, `mark_sanctioned`);
//! (b) the (language, verifier) deny-list constructs are absent from the
//!     body outside sanctioned positions.
//!
//! For rust both checks run over the token stream of a real Rust lexer
//! (`proc-macro2`) and the body must parse as a file (`syn`) — an unlexable
//! or unparseable body is refused. Matching is on tokens, never on text, so
//! whitespace, comments, char literals (`'"'`), raw strings and grouped
//! `use` trees cannot hide a denied construct or a live interpolation.
//!
//! The identifier/inert split is a rendering-side decision (`render.rs`
//! classifies each property value into one list or the other); this module
//! does not trust that classification blindly — a value that does not
//! parse as identifier-shaped falls back to the ordinary inert-only rule
//! even when passed in `identifier_values`.

use proc_macro2::{Delimiter, TokenStream, TokenTree};

/// Per-language dangerous-construct deny-list — part of the
/// (language, verifier) instantiation, not the pipeline. A hit rejects
/// generation. For rust these are the reported construct names; matching is
/// token-based (see `rust_denied_construct`), for other languages it is a
/// plain substring scan.
pub fn deny_list(language: &str) -> &'static [&'static str] {
    match language {
        "rust" => &[
            "include!",
            "include_str!",
            "include_bytes!",
            "env!",
            "option_env!",
            "#[path",
            "extern crate",
            "unsafe",
            "macro_rules!",
            "$",
            "std::process",
            "std::fs",
            "std::net",
            "std::os",
            "Command::new",
            "env::var",
            "env::var_os",
            "env::vars",
            "env::vars_os",
        ],
        "python" => &["eval(", "exec(", "os.system", "subprocess", "__import__"],
        _ => &[],
    }
}

/// Denied macros (rust): denied when invoked (`name !`) or named in a path
/// (`use std::include_str as inc;`), so renaming cannot launder them.
const RUST_DENIED_MACROS: &[&str] = &[
    "include",
    "include_str",
    "include_bytes",
    "env",
    "option_env",
];

/// Denied `std` modules (rust): denied wherever they occur below `std`
/// (`std::fs`, `std::os::unix::fs`, `std::os::unix::net`), since the
/// platform extension modules re-home the same capabilities. `std::os` is
/// denied outright: it is nothing but those extensions (raw fds, `CommandExt`,
/// symlinks, Unix sockets) and a harness has no use for it.
const RUST_DENIED_STD_MODULES: &[(&str, &str)] = &[
    ("process", "std::process"),
    ("fs", "std::fs"),
    ("net", "std::net"),
    ("os", "std::os"),
];

/// Maximum bracket nesting in a rendered rust body. The parser and the
/// validator's own walks recurse per level, so unbounded nesting would
/// overflow the stack and abort the process; generated harnesses nest a
/// handful of levels, and 64 leaves headroom on a 2 MiB thread in debug.
const RUST_MAX_NESTING: usize = 64;

/// Denied adjacent path segments (rust), matched across `::` with any
/// whitespace/comments and through expanded `use` trees. A glob import of, or
/// an `as` rename of, the first segment is denied too (it would reach the
/// second segment under another name).
const RUST_DENIED_PATHS: &[(&str, &str, &str)] = &[
    ("std", "process", "std::process"),
    ("Command", "new", "Command::new"),
    ("env", "var", "env::var"),
    ("env", "var_os", "env::var_os"),
    ("env", "vars", "env::vars"),
    ("env", "vars_os", "env::vars_os"),
];

/// Rust strict and reserved keywords (the Reference's keyword list, minus
/// the 2015-edition-only `async`/`await`/`dyn`/`try` distinction — all four
/// are keywords in every edition this validator needs to reason about).
/// Weak/contextual keywords (`union`, `macro_rules`, `raw`, `dyn` — already
/// listed, `yeet`) are deliberately not banned: they are legal plain
/// identifiers outside their special contexts, so excluding them would only
/// over-refuse. A value naming one of these can never be the genuine subject
/// or path segment the render pipeline computed it from, so it is refused
/// identifier treatment defensively.
const RUST_KEYWORDS: &[&str] = &[
    // strict keywords
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for",
    "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
    "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where",
    "while", "async", "await", "dyn", // reserved keywords
    "abstract", "become", "box", "do", "final", "macro", "override", "priv", "try", "typeof",
    "unsized", "virtual", "yield",
];

/// A single `::`-delimited segment of an identifier-value candidate: a plain
/// Rust identifier — `[A-Za-z_][A-Za-z0-9_]*`, ASCII only — that is not a
/// keyword. The charset already excludes `#`, so a raw identifier (`r#fn`)
/// can never pass this check.
fn is_plain_identifier_segment(seg: &str) -> bool {
    let mut chars = seg.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    if !chars.all(|c| c == '_' || c.is_ascii_alphanumeric()) {
        return false;
    }
    !RUST_KEYWORDS.contains(&seg)
}

/// Whether `value` is identifier-shaped: it matches
/// `^[A-Za-z_][A-Za-z0-9_]*(::[A-Za-z_][A-Za-z0-9_]*)*$` with no segment a
/// Rust keyword. Only such a value is ever eligible for identifier-position
/// sanctioning in live code (`mark_sanctioned`); the render pipeline uses
/// this to decide whether a candidate goes in `identifier_values` at all,
/// and `validate_body` re-checks it for every value it is handed, rather
/// than trusting the caller's classification.
pub fn is_identifier_value(value: &str) -> bool {
    !value.is_empty() && value.split("::").all(is_plain_identifier_segment)
}

/// Escape a free-text property field for embedding as a Rust string literal:
/// the HOST escapes before the value enters Rhai scope (S5) — templates never
/// do their own escaping.
pub fn escape_rust_string_literal(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

#[derive(Debug, thiserror::Error)]
pub enum BodyValidationError {
    #[error("rendered body fails to parse as {language}: {message}")]
    Unparseable { language: String, message: String },
    #[error("denied construct `{construct}` in rendered body (language {language})")]
    DeniedConstruct { language: String, construct: String },
    #[error("interpolated value {interpolated:?} appears outside a string literal (S5)")]
    InterpolationOutsideString { interpolated: String },
}

/// Validate the rendered body: deny-list constructs must not appear, every
/// *inert* value must sit inside a string literal (or comment), and every
/// *identifier* value may additionally appear in live code as a complete
/// identifier or path token (module doc comment above).
///
/// `language` keys the deny-list. `inert_values` are free-text property
/// values (condition/guarantee, the property id, `depends_on` region
/// strings). `identifier_values` are the property's subject and the fn/type
/// names parsed from its `depends_on` entries — each still re-checked here
/// for being identifier-shaped (`is_identifier_value`) before it is granted
/// any live-code allowance; one that is not falls back to the `inert_values`
/// rule. This distinction is Rust-specific: for every other language
/// `identifier_values` is folded into the same inert-only check as
/// `inert_values` (unaffected — no language but rust has a structural
/// identifier notion here).
///
/// Rust: `Ok` ⇒ the body lexes and parses as a Rust file, no deny-listed
/// path/macro/attribute/keyword occurs in any token, no occurrence of an
/// inert value overlaps a code token (anything but a string/char/byte
/// literal or a comment), and every occurrence of an identifier value either
/// lies wholly inside such inert text or is itself a sanctioned identifier
/// or path token.
pub fn validate_body(
    language: &str,
    body: &str,
    inert_values: &[&str],
    identifier_values: &[&str],
) -> Result<(), BodyValidationError> {
    if language == "rust" {
        return validate_rust_body(body, inert_values, identifier_values);
    }
    for construct in deny_list(language) {
        if body.contains(construct) {
            return Err(denied(language, construct));
        }
    }
    // Every occurrence must lie wholly inside inert text: a value that
    // straddles a literal boundary (`a' + x + 'b`) touches live code.
    let live: Vec<bool> = non_rust_inert_mask(language, body)
        .into_iter()
        .map(|inert| !inert)
        .collect();
    check_interpolations(body, inert_values, &live)?;
    check_interpolations(body, identifier_values, &live)
}

/// Reject if any occurrence (overlapping ones included) of any non-empty
/// interpolated value overlaps a live byte of `body`.
fn check_interpolations(
    body: &str,
    interpolated: &[&str],
    live: &[bool],
) -> Result<(), BodyValidationError> {
    for value in interpolated {
        if value.is_empty() {
            continue;
        }
        let mut from = 0;
        while let Some(pos) = body[from..].find(value) {
            let start = from + pos;
            if live[start..start + value.len()].iter().any(|b| *b) {
                return Err(BodyValidationError::InterpolationOutsideString {
                    interpolated: (*value).to_string(),
                });
            }
            from = start + body[start..].chars().next().map_or(1, char::len_utf8);
        }
    }
    Ok(())
}

fn denied(language: &str, construct: &str) -> BodyValidationError {
    BodyValidationError::DeniedConstruct {
        language: language.to_string(),
        construct: construct.to_string(),
    }
}

fn validate_rust_body(
    body: &str,
    inert_values: &[&str],
    identifier_values: &[&str],
) -> Result<(), BodyValidationError> {
    let result = check_rust_body(body, inert_values, identifier_values);
    // The fallback lexer records every parsed source in a thread-local span
    // map; every span from this call is dropped by now, so release it rather
    // than grow it for the life of a long-running server.
    proc_macro2::extra::invalidate_current_thread_spans();
    result
}

fn check_rust_body(
    body: &str,
    inert_values: &[&str],
    identifier_values: &[&str],
) -> Result<(), BodyValidationError> {
    let unparseable = |message: String| BodyValidationError::Unparseable {
        language: "rust".to_string(),
        message,
    };
    let tokens: TokenStream = body
        .parse()
        .map_err(|e: proc_macro2::LexError| unparseable(e.to_string()))?;
    // Before anything recursive (syn, the walks below) sees the stream.
    let depth = max_nesting(&tokens);
    if depth > RUST_MAX_NESTING {
        return Err(unparseable(format!(
            "bracket nesting depth {depth} exceeds {RUST_MAX_NESTING}"
        )));
    }
    syn::parse2::<syn::File>(tokens.clone()).map_err(|e| unparseable(e.to_string()))?;

    if let Some(construct) = rust_denied_construct(&tokens) {
        return Err(denied("rust", construct));
    }

    // Mark every byte that belongs to a code token. Literal string/char/byte
    // tokens, comments, and whitespace stay unmarked.
    let mut live = vec![false; body.len()];
    mark_live(body, &tokens, &mut live);
    check_interpolations(body, inert_values, &live)?;
    check_identifier_interpolations(body, identifier_values, &live, &tokens)
}

/// Identifier-value occurrences (module doc comment (a')): a value that is
/// not identifier-shaped (`is_identifier_value`) is held to the ordinary
/// inert-only rule; one that is may additionally occur at a position
/// `mark_sanctioned` finds — a complete identifier or path token, never a
/// substring of a longer identifier, a macro invocation name, or part of a
/// longer path.
fn check_identifier_interpolations(
    body: &str,
    identifier_values: &[&str],
    live: &[bool],
    tokens: &TokenStream,
) -> Result<(), BodyValidationError> {
    for value in identifier_values {
        if value.is_empty() {
            continue;
        }
        if !is_identifier_value(value) {
            check_interpolations(body, std::slice::from_ref(value), live)?;
            continue;
        }
        let segments: Vec<&str> = value.split("::").collect();
        let mut ranges = Vec::new();
        mark_sanctioned(tokens, &segments, &mut ranges);
        let mut sanctioned = vec![false; body.len()];
        for range in ranges {
            if let Some(slice) = sanctioned.get_mut(range) {
                slice.iter_mut().for_each(|b| *b = true);
            }
        }
        let mut from = 0;
        while let Some(pos) = body[from..].find(value) {
            let start = from + pos;
            let end = start + value.len();
            let occurrence_is_live = live[start..end].iter().any(|b| *b);
            let occurrence_is_sanctioned = sanctioned
                .get(start..end)
                .is_some_and(|s| s.iter().all(|b| *b));
            if occurrence_is_live && !occurrence_is_sanctioned {
                return Err(BodyValidationError::InterpolationOutsideString {
                    interpolated: (*value).to_string(),
                });
            }
            from = start + body[start..].chars().next().map_or(1, char::len_utf8);
        }
    }
    Ok(())
}

/// Append the byte range of every place `segments` occurs in `tokens` as a
/// *complete* identifier or path: `segments.len()` consecutive `Ident`
/// tokens joined by exact `::` pairs (whitespace/comments between tokens are
/// fine — spans are token spans, not text spans), with:
/// - no `::` immediately before the first segment or after the last (either
///   would make this a sub-path of a longer, different path: `x::divide`
///   sanctions neither `divide` nor `x::divide` for a `divide` value read
///   from a shorter path — conservative, since a leading `::`-qualified
///   version of the very same absolute path is refused too);
/// - no `!` immediately after (a macro invocation — identifier values are
///   never sanctioned as macro names, however trusted the plain call is).
///
/// Recurses into groups the same way `mark_live`/`rust_denied_construct` do.
fn mark_sanctioned(tokens: &TokenStream, segments: &[&str], out: &mut Vec<std::ops::Range<usize>>) {
    let tts: Vec<TokenTree> = tokens.clone().into_iter().collect();
    for i in 0..tts.len() {
        if let TokenTree::Group(g) = &tts[i] {
            mark_sanctioned(&g.stream(), segments, out);
        }
        let Some(end) = path_match_end(&tts, i, segments) else {
            continue;
        };
        let preceded_by_path_sep = i >= 2 && is_path_sep(&tts, i - 2);
        let followed_by_path_sep = is_path_sep(&tts, end);
        let followed_by_bang = is_punct(tts.get(end), '!');
        if preceded_by_path_sep || followed_by_path_sep || followed_by_bang {
            continue;
        }
        let start = tts[i].span().byte_range().start;
        let stop = tts[end - 1].span().byte_range().end;
        out.push(start..stop);
    }
}

/// If `segments` matches as a run of `Ident`-`::`-`Ident`... tokens starting
/// at `tts[start]`, the index just past it; `None` otherwise.
fn path_match_end(tts: &[TokenTree], start: usize, segments: &[&str]) -> Option<usize> {
    let mut j = start;
    for (k, seg) in segments.iter().enumerate() {
        if ident_name(tts.get(j)?).as_deref() != Some(*seg) {
            return None;
        }
        j += 1;
        if k + 1 < segments.len() {
            if !is_path_sep(tts, j) {
                return None;
            }
            j += 2;
        }
    }
    Some(j)
}

/// Deepest group nesting in `tokens`, computed without recursion (the lexer
/// and the token stream's drop are iterative too).
fn max_nesting(tokens: &TokenStream) -> usize {
    let mut max = 0;
    let mut stack = vec![tokens.clone().into_iter()];
    while let Some(top) = stack.last_mut() {
        match top.next() {
            Some(TokenTree::Group(g)) => {
                stack.push(g.stream().into_iter());
                max = max.max(stack.len() - 1);
            }
            Some(_) => {}
            None => {
                stack.pop();
            }
        }
    }
    max
}

/// Set `live[i]` for every byte of a code token. A doc comment lexes as a
/// synthesized `#[doc = "..."]` (or `#![doc = ...]`) whose token spans cover
/// the comment text — and whose bracket spans are fragments of it — so the
/// whole synthesized attribute is skipped: it is a comment, not code.
fn mark_live(body: &str, tokens: &TokenStream, live: &mut [bool]) {
    let tts: Vec<TokenTree> = tokens.clone().into_iter().collect();
    let mut i = 0;
    while i < tts.len() {
        let tt = &tts[i];
        i += 1;
        match tt {
            TokenTree::Punct(p) if p.as_char() == '#' && is_comment(body, tt) => {
                if is_punct(tts.get(i), '!') {
                    i += 1;
                }
                if matches!(tts.get(i), Some(TokenTree::Group(_))) {
                    i += 1;
                }
            }
            TokenTree::Group(g) => {
                mark_range(g.span_open().byte_range(), live);
                mark_range(g.span_close().byte_range(), live);
                mark_live(body, &g.stream(), live);
            }
            TokenTree::Literal(lit) if is_text_literal(&lit.to_string()) => {}
            _ => mark_range(tt.span().byte_range(), live),
        }
    }
}

fn is_comment(body: &str, tt: &TokenTree) -> bool {
    body.get(tt.span().byte_range())
        .is_some_and(|text| text.starts_with("//") || text.starts_with("/*"))
}

fn mark_range(range: std::ops::Range<usize>, live: &mut [bool]) {
    if let Some(bytes) = live.get_mut(range) {
        bytes.iter_mut().for_each(|b| *b = true);
    }
}

/// String, raw string, byte string, C string, char and byte-char literals —
/// the positions an interpolated value may occupy. Numeric literals are code.
fn is_text_literal(repr: &str) -> bool {
    let rest = repr.trim_start_matches(|c: char| c.is_ascii_alphabetic());
    rest.starts_with('"') || rest.starts_with('\'') || rest.starts_with('#')
}

fn ident_name(tt: &TokenTree) -> Option<String> {
    match tt {
        TokenTree::Ident(i) => {
            let s = i.to_string();
            Some(s.strip_prefix("r#").map(str::to_string).unwrap_or(s))
        }
        _ => None,
    }
}

fn is_punct(tt: Option<&TokenTree>, ch: char) -> bool {
    matches!(tt, Some(TokenTree::Punct(p)) if p.as_char() == ch)
}

/// `::` — two `:` tokens. The first is normally `Joint`; a spaced `: :` is
/// not a path separator to rustc, but treating it as one only over-rejects.
fn is_path_sep(tokens: &[TokenTree], i: usize) -> bool {
    is_punct(tokens.get(i), ':') && is_punct(tokens.get(i + 1), ':')
}

/// One path found in the token stream, with `use`-tree braces expanded.
struct PathHit {
    segments: Vec<String>,
    glob: bool,
    renamed: bool,
    /// Followed by `!`: a macro invocation through this path.
    bang: bool,
}

/// The first deny-listed construct in `tokens` (recursing into groups).
fn rust_denied_construct(tokens: &TokenStream) -> Option<&'static str> {
    let tts: Vec<TokenTree> = tokens.clone().into_iter().collect();
    let mut i = 0;
    while i < tts.len() {
        // `#[...]` / `#![...]`: `path` anywhere inside (covers `cfg_attr`).
        if is_punct(tts.get(i), '#') {
            let j = if is_punct(tts.get(i + 1), '!') {
                i + 2
            } else {
                i + 1
            };
            if let Some(TokenTree::Group(g)) = tts.get(j)
                && g.delimiter() == Delimiter::Bracket
                && contains_ident(&g.stream(), "path")
            {
                return Some("#[path");
            }
        }
        // `macro_rules!` / `macro` definitions and `$` metavariables can
        // assemble a denied path or macro name from pieces no single token
        // shows, so a rendered body may not define macros at all.
        if is_punct(tts.get(i), '$') {
            return Some("$");
        }
        if let Some(name) = ident_name(&tts[i]) {
            if name == "unsafe" {
                return Some("unsafe");
            }
            if name == "macro_rules"
                || (name == "macro" && tts.get(i + 1).and_then(ident_name).is_some())
            {
                return Some("macro_rules!");
            }
            if name == "extern" && tts.get(i + 1).and_then(ident_name).as_deref() == Some("crate") {
                return Some("extern crate");
            }
            if is_punct(tts.get(i + 1), '!')
                && let Some(construct) = denied_macro(&name)
            {
                return Some(construct);
            }
            let mut hits = Vec::new();
            let end = parse_path(&tts, i, Vec::new(), &mut hits);
            for hit in &hits {
                if let Some(construct) = denied_path(hit) {
                    return Some(construct);
                }
            }
            // Recurse into anything the path walk stepped over (brace
            // groups it expanded), then resume after the path.
            for tt in &tts[i + 1..end] {
                if let TokenTree::Group(g) = tt
                    && let Some(c) = rust_denied_construct(&g.stream())
                {
                    return Some(c);
                }
            }
            i = end.max(i + 1);
            continue;
        }
        if let TokenTree::Group(g) = &tts[i]
            && let Some(c) = rust_denied_construct(&g.stream())
        {
            return Some(c);
        }
        i += 1;
    }
    None
}

fn contains_ident(tokens: &TokenStream, wanted: &str) -> bool {
    tokens.clone().into_iter().any(|tt| match &tt {
        TokenTree::Group(g) => contains_ident(&g.stream(), wanted),
        _ => ident_name(&tt).as_deref() == Some(wanted),
    })
}

fn denied_macro(name: &str) -> Option<&'static str> {
    let idx = RUST_DENIED_MACROS.iter().position(|m| *m == name)?;
    // deny_list() carries the same names with a `!` suffix.
    deny_list("rust")
        .iter()
        .copied()
        .find(|c| c.strip_suffix('!') == Some(RUST_DENIED_MACROS[idx]))
}

fn denied_path(hit: &PathHit) -> Option<&'static str> {
    let segs: Vec<&str> = hit
        .segments
        .iter()
        .map(String::as_str)
        .filter(|s| *s != "self")
        .collect();
    if hit.bang
        && let Some(construct) = segs.last().and_then(|last| denied_macro(last))
    {
        return Some(construct);
    }
    if let Some(std_at) = segs.iter().position(|s| *s == "std") {
        let below = &segs[std_at + 1..];
        for (module, construct) in RUST_DENIED_STD_MODULES {
            if below.contains(module) {
                return Some(construct);
            }
        }
    }
    for (a, b, construct) in RUST_DENIED_PATHS {
        let adjacent = segs.windows(2).any(|w| w[0] == *a && w[1] == *b);
        let reaches = (hit.glob || hit.renamed) && segs.last() == Some(a);
        if adjacent || reaches {
            return Some(construct);
        }
    }
    if segs.len() > 1 {
        for seg in &segs {
            if *seg != "env"
                && let Some(construct) = denied_macro(seg)
            {
                return Some(construct);
            }
        }
    }
    None
}

/// Walk the path starting at `tts[start]` (an ident), appending every
/// complete path — `use` brace groups expanded against `prefix` — to `hits`.
/// Returns the index just past the path.
fn parse_path(
    tts: &[TokenTree],
    start: usize,
    mut prefix: Vec<String>,
    hits: &mut Vec<PathHit>,
) -> usize {
    let mut i = start;
    while let Some(name) = tts.get(i).and_then(ident_name) {
        prefix.push(name);
        i += 1;
        if !is_path_sep(tts, i) {
            break;
        }
        i += 2;
        match tts.get(i) {
            Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Brace => {
                expand_use_group(&g.stream(), &prefix, hits);
                return i + 1;
            }
            Some(TokenTree::Punct(p)) if p.as_char() == '*' => {
                hits.push(PathHit {
                    segments: prefix,
                    glob: true,
                    renamed: false,
                    bang: false,
                });
                return i + 1;
            }
            _ => {}
        }
    }
    let renamed = tts.get(i).and_then(ident_name).as_deref() == Some("as");
    hits.push(PathHit {
        segments: prefix,
        glob: false,
        renamed,
        bang: is_punct(tts.get(i), '!'),
    });
    i
}

/// Expand one `use` brace group: comma-separated subtrees, each a path
/// (itself possibly braced, globbed, or renamed) under `prefix`.
fn expand_use_group(group: &TokenStream, prefix: &[String], hits: &mut Vec<PathHit>) {
    let tts: Vec<TokenTree> = group.clone().into_iter().collect();
    let mut i = 0;
    while i < tts.len() {
        match &tts[i] {
            TokenTree::Punct(p) if p.as_char() == ',' => i += 1,
            TokenTree::Punct(p) if p.as_char() == '*' => {
                hits.push(PathHit {
                    segments: prefix.to_vec(),
                    glob: true,
                    renamed: false,
                    bang: false,
                });
                i += 1;
            }
            TokenTree::Punct(p) if p.as_char() == ':' => i += 1,
            TokenTree::Group(g) if g.delimiter() == Delimiter::Brace => {
                expand_use_group(&g.stream(), prefix, hits);
                i += 1;
            }
            TokenTree::Ident(_) => {
                let end = parse_path(&tts, i, prefix.to_vec(), hits);
                i = end.max(i + 1);
            }
            _ => i += 1,
        }
    }
}

/// Non-rust fallback: which bytes of `body` are inert — inside a string
/// literal or a comment — so an interpolated value may sit there. Every other
/// byte (code, whitespace between tokens) is live. Language-aware: python
/// gets its own lexer (see `python_inert_chars`); every other non-rust
/// language keeps the original `"..."` / `//` handling.
///
/// Fails safe: text the lexer cannot place with certainty (an unterminated
/// literal and everything after it) stays live — over-rejecting is safe,
/// under-rejecting is a bypass.
fn non_rust_inert_mask(language: &str, body: &str) -> Vec<bool> {
    let chars: Vec<char> = body.chars().collect();
    let inert_chars = if language == "python" {
        python_inert_chars(&chars)
    } else {
        generic_inert_chars(&chars)
    };
    let mut inert = vec![false; body.len()];
    for ((byte, c), is_inert) in body.char_indices().zip(inert_chars) {
        if is_inert {
            inert[byte..byte + c.len_utf8()].fill(true);
        }
    }
    inert
}

/// `"..."` string literals (backslash escapes honored) and `//` line
/// comments. An unterminated string leaves it and the rest of the body live.
fn generic_inert_chars(chars: &[char]) -> Vec<bool> {
    let mut inert = vec![false; chars.len()];
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '"' => {
                let mut j = i + 1;
                let mut end = None;
                while j < chars.len() {
                    match chars[j] {
                        '\\' => j += 2,
                        '"' => {
                            end = Some(j + 1);
                            break;
                        }
                        _ => j += 1,
                    }
                }
                let Some(end) = end else { break };
                inert[i..end].fill(true);
                i = end;
            }
            '/' if chars.get(i + 1) == Some(&'/') => {
                let start = i;
                // Ending at a CR too is conservative: text after it is live.
                while i < chars.len() && !is_python_line_end(chars[i]) {
                    i += 1;
                }
                inert[start..i].fill(true);
            }
            _ => i += 1,
        }
    }
    inert
}

/// Nesting limit for Python f-/t-string replacement fields and the strings
/// nested inside them. Deeper nesting fails safe (treated as unterminated),
/// which also bounds the lexer's recursion.
const PYTHON_MAX_NESTING: usize = 32;

/// A Python identifier character, for the prefix boundary check. Any
/// non-ASCII character counts: outside a string or comment CPython accepts
/// non-ASCII only in identifiers, so `éf'{x}'` is the name `éf` followed by
/// a plain string, never an f-string.
fn is_python_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || !c.is_ascii()
}

/// A Python string literal starting at `chars[i]`: a bare quote, or one or
/// two prefix letters (`r`/`b`/`u`/`f`/`t`, any case) at a token boundary
/// immediately followed by a quote. Returns `(prefix_len, interpolates)`,
/// where `interpolates` marks an f-string or t-string (PEP 750), whose
/// `{...}` replacement fields are code.
///
/// Any one- or two-letter combination is accepted, including ones CPython
/// refuses (`ur`, `bf`, `ff`): CPython rejects such a program outright, so
/// how it is lexed here cannot hide live code, and treating it as a string
/// that interpolates is the conservative reading.
fn python_string_start(chars: &[char], i: usize) -> Option<(usize, bool)> {
    let c = chars[i];
    if c == '\'' || c == '"' {
        return Some((0, false));
    }
    if i > 0 && is_python_ident_char(chars[i - 1]) {
        return None;
    }
    let is_prefix = |c: &char| "rRbBuUfFtT".contains(*c);
    for len in [1usize, 2] {
        let slice = chars.get(i..i + len)?;
        if !slice.iter().all(is_prefix) {
            return None;
        }
        if matches!(chars.get(i + len), Some('\'' | '"')) {
            let interpolates = slice.iter().any(|c| "fFtT".contains(*c));
            return Some((len, interpolates));
        }
    }
    None
}

/// Python-aware fallback lexer: marks `'...'`/`"..."`/`'''...'''`/`"""..."""`
/// string literals (with any prefix) and `#` line comments inert; everything
/// else is code.
///
/// Termination follows CPython's tokenizer, not the literal's value: a
/// backslash always takes the next character with it — in raw strings too,
/// where the backslash stays in the value but `r'x\'` still does not close
/// at the escaped quote — and a single-quoted literal ends at an unescaped
/// newline (CPython rejects it). In an f-/t-string the one exception is `\{`
/// / `\}`: the backslash is literal and the brace still opens a replacement
/// field (or is a doubled brace), as CPython does.
///
/// f-/t-strings are only partly inert: a `{expr}` replacement field is code
/// and stays live (so an interpolated value there is still caught), while
/// `{{`/`}}` are literal braces and the rest of the literal text is inert.
/// A field is scanned with the PEP 701 (Python 3.12+) grammar — nested
/// strings of any quote, prefix, and escapes, `#` comments, brackets, and a
/// `:` format spec with nested fields — so a `}` or quote inside any of those
/// cannot end the field or the string early. Everything inside a field,
/// nested strings included, stays live.
///
/// Fails safe: a literal that never finds its closing quote(s) (or nests
/// deeper than `PYTHON_MAX_NESTING`) leaves it and the rest of the body live.
fn python_inert_chars(chars: &[char]) -> Vec<bool> {
    let mut inert = vec![false; chars.len()];
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '#' {
            let start = i;
            while i < chars.len() && !is_python_line_end(chars[i]) {
                i += 1;
            }
            inert[start..i].fill(true);
            continue;
        }
        if let Some(prefix) = python_string_start(chars, i) {
            let mut segments = Vec::new();
            match scan_python_string(chars, i, prefix, 0, Some(&mut segments)) {
                Some(end) => {
                    for segment in segments {
                        inert[segment].fill(true);
                    }
                    i = end;
                    continue;
                }
                None => break,
            }
        }
        i += 1;
    }
    inert
}

/// Scan the string literal whose prefix starts at `chars[start]`; return the
/// index just past its closing quote(s), or `None` if it is unterminated.
/// With `segments`, record the literal (non-field) character ranges.
fn scan_python_string(
    chars: &[char],
    start: usize,
    (prefix_len, interpolates): (usize, bool),
    depth: usize,
    mut segments: Option<&mut Vec<std::ops::Range<usize>>>,
) -> Option<usize> {
    if depth > PYTHON_MAX_NESTING {
        return None;
    }
    let q = start + prefix_len;
    let qc = chars[q];
    let triple = chars.get(q + 1) == Some(&qc) && chars.get(q + 2) == Some(&qc);
    let quote_len = if triple { 3 } else { 1 };
    let mut literal_start = start;
    let mut j = q + quote_len;
    loop {
        let d = *chars.get(j)?;
        if closes_python_string(chars, j, qc, triple) {
            let end = j + quote_len;
            if let Some(segments) = segments.as_deref_mut() {
                segments.push(literal_start..end);
            }
            return Some(end);
        }
        match d {
            '\n' | '\r' if !triple => return None,
            '\\' => {
                let next = *chars.get(j + 1)?;
                j += if interpolates && (next == '{' || next == '}') {
                    1
                } else {
                    python_escape_len(chars, j)
                };
            }
            '{' if interpolates => {
                if chars.get(j + 1) == Some(&'{') {
                    j += 2;
                    continue;
                }
                if let Some(segments) = segments.as_deref_mut() {
                    segments.push(literal_start..j);
                }
                j = scan_python_field(chars, j + 1, qc, triple, depth + 1)?;
                literal_start = j;
            }
            _ => j += 1,
        }
    }
}

fn closes_python_string(chars: &[char], j: usize, qc: char, triple: bool) -> bool {
    chars[j] == qc && (!triple || (chars.get(j + 1) == Some(&qc) && chars.get(j + 2) == Some(&qc)))
}

/// A character that ends a physical line. CPython reads source with
/// universal newlines, so a bare CR (and CRLF, whose LF then follows) ends a
/// line as LF does. Other Unicode line breaks — form feed, `\v`,
/// `\x1c`–`\x1e`, NEL, U+2028/U+2029 — do not end a line in CPython's
/// tokenizer, so they never end a comment or a single-quoted literal here.
fn is_python_line_end(c: char) -> bool {
    c == '\n' || c == '\r'
}

/// Characters consumed by the backslash at `chars[j]` and what it escapes:
/// three for a backslash-CRLF line continuation, otherwise two.
fn python_escape_len(chars: &[char], j: usize) -> usize {
    if chars.get(j + 1) == Some(&'\r') && chars.get(j + 2) == Some(&'\n') {
        3
    } else {
        2
    }
}

/// Scan an f-/t-string replacement field whose `{` sits just before
/// `chars[j]`; return the index just past its closing `}`. `qc`/`triple`
/// describe the enclosing literal.
fn scan_python_field(
    chars: &[char],
    mut j: usize,
    qc: char,
    triple: bool,
    depth: usize,
) -> Option<usize> {
    if depth > PYTHON_MAX_NESTING {
        return None;
    }
    let mut brackets = 0usize;
    loop {
        let c = *chars.get(j)?;
        if let Some(prefix) = python_string_start(chars, j) {
            j = scan_python_string(chars, j, prefix, depth + 1, None)?;
            continue;
        }
        match c {
            '#' => {
                while chars.get(j).is_some_and(|c| !is_python_line_end(*c)) {
                    j += 1;
                }
            }
            '\\' => j += python_escape_len(chars, j),
            '(' | '[' | '{' => {
                brackets += 1;
                j += 1;
            }
            ')' | ']' => {
                brackets = brackets.saturating_sub(1);
                j += 1;
            }
            '}' if brackets == 0 => return Some(j + 1),
            '}' => {
                brackets -= 1;
                j += 1;
            }
            ':' if brackets == 0 => {
                return scan_python_format_spec(chars, j + 1, qc, triple, depth);
            }
            _ => j += 1,
        }
    }
}

/// Scan a replacement field's format spec (after its top-level `:`); return
/// the index just past the field's closing `}`. The spec is literal text in
/// which `{` opens a nested field and a backslash escapes as in the literal
/// part; the enclosing literal's closing quote here is a CPython error.
fn scan_python_format_spec(
    chars: &[char],
    mut j: usize,
    qc: char,
    triple: bool,
    depth: usize,
) -> Option<usize> {
    loop {
        let c = *chars.get(j)?;
        if closes_python_string(chars, j, qc, triple) {
            return None;
        }
        match c {
            '\n' | '\r' if !triple => return None,
            '\\' => {
                let next = *chars.get(j + 1)?;
                j += if next == '{' || next == '}' {
                    1
                } else {
                    python_escape_len(chars, j)
                };
            }
            '{' => j = scan_python_field(chars, j + 1, qc, triple, depth + 1)?,
            '}' => return Some(j + 1),
            _ => j += 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rejects(body: &str, interpolated: &[&str]) -> bool {
        validate_body("rust", body, interpolated, &[]).is_err()
    }

    fn rejects_identifiers(body: &str, identifiers: &[&str]) -> bool {
        validate_body("rust", body, &[], identifiers).is_err()
    }

    #[test]
    fn non_rust_validation_ignores_interpolation_inside_strings_and_comments() {
        assert!(
            validate_body(
                "python",
                "message = 'unsafe_value' # unsafe_value",
                &["unsafe_value"],
                &[]
            )
            .is_ok()
        );
        assert!(matches!(
            validate_body("python", "value = unsafe_value", &["unsafe_value"], &[]),
            Err(BodyValidationError::InterpolationOutsideString { .. })
        ));
    }

    #[test]
    fn python_single_quotes_and_triple_quotes_spanning_lines_are_inert() {
        assert!(
            validate_body("python", "message = 'unsafe_value'", &["unsafe_value"], &[]).is_ok()
        );
        let body = "message = \"\"\"line one\nunsafe_value\nline three\"\"\"";
        assert!(validate_body("python", body, &["unsafe_value"], &[]).is_ok());
        let body = "message = '''line one\nunsafe_value\nline three'''";
        assert!(validate_body("python", body, &["unsafe_value"], &[]).is_ok());
    }

    #[test]
    fn python_escaped_quote_inside_string_does_not_end_it() {
        // The escaped quote must not terminate the literal early and expose
        // the rest of the line as live code.
        let body = r"message = 'a\'unsafe_value'";
        assert!(validate_body("python", body, &["unsafe_value"], &[]).is_ok());
    }

    fn python_live(body: &str) -> bool {
        matches!(
            validate_body("python", body, &["unsafe_value"], &[]),
            Err(BodyValidationError::InterpolationOutsideString { .. })
        )
    }

    #[test]
    fn python_raw_string_backslash_still_protects_the_next_character() {
        // CPython: in `r'x\' y '` the backslash keeps the `'` from closing
        // the literal (it stays in the value), so the string is `x\' y ` and
        // `unsafe_value` is live code between two strings.
        assert!(python_live(r"s = r'x\' y ' + unsafe_value + ' z'"));
        // Unterminated under CPython rules: fails safe, stays live.
        assert!(python_live(r"value = r'a\' unsafe_value"));
        // A raw string containing the value with no embedded escapes is inert.
        assert!(!python_live(r"value = r'unsafe_value'"));
        // ...and one whose backslash-quote keeps the value inside is inert.
        assert!(!python_live(r"value = r'a\' unsafe_value'"));
    }

    #[test]
    fn python_raw_prefix_variants_do_not_misalign() {
        for prefix in ["r", "R", "rb", "Rb", "bR", "BR", "rf", "Rf", "fR", "FR"] {
            let body = format!(r"s = {prefix}'x\' y ' + unsafe_value + ' z'");
            assert!(python_live(&body), "{body}");
            let body = format!(r#"s = {prefix}"x\" y " + unsafe_value + " z""#);
            assert!(python_live(&body), "{body}");
            let body = format!(r"s = {prefix}'''x\''' y ''' + unsafe_value + ''' z'''");
            assert!(python_live(&body), "{body}");
            let body = format!(r#"s = {prefix}"""x\""" y """ + unsafe_value + """ z""""#);
            assert!(python_live(&body), "{body}");
        }
    }

    #[test]
    fn python_backslash_quote_inside_raw_triple_string_does_not_end_it() {
        // `\'''` inside a raw triple string: the backslash takes the first
        // quote, so the literal ends at the second `'''`.
        assert!(!python_live(r"s = r'''a\''' unsafe_value '''"));
        assert!(python_live(r"s = rb'''a\'''b''' + unsafe_value"));
        // A trailing backslash can never close the literal: fail safe.
        assert!(python_live("s = r'unsafe_value\\"));
    }

    #[test]
    fn python_fstring_field_nested_string_honors_escapes() {
        // PEP 701: the nested `"\"}"` is one string, so its `}` does not end
        // the field and `+ unsafe_value` is live inside it.
        assert!(python_live(r#"s = f'{"\"}" + unsafe_value}'"#));
        assert!(python_live(r#"s = f'{r"\"}" + unsafe_value}'"#));
        assert!(python_live(r#"s = f'{f"{'}'}" + unsafe_value}'"#));
    }

    #[test]
    fn python_fstring_field_comment_and_format_spec_do_not_end_it_early() {
        // A `}` inside a field's comment does not close the field.
        assert!(python_live("s = f'''{x # }\n + unsafe_value}'''"));
        // A quote inside a format spec is literal text, not a nested string.
        assert!(python_live(r#"s = f'{x:"}' + "}" + unsafe_value + "'""#));
        // Nested fields in a format spec are live.
        assert!(python_live("s = f'{x:{unsafe_value}}'"));
    }

    #[test]
    fn python_backslash_brace_in_fstring_still_opens_a_field() {
        // CPython: `\{` in an f-string is a literal backslash, then a field.
        assert!(python_live(r"s = f'\{unsafe_value}'"));
        assert!(python_live(r"s = rf'\{unsafe_value}'"));
        assert!(!python_live(r"s = '\{unsafe_value}'"));
    }

    #[test]
    fn python_tstring_replacement_field_is_live() {
        for prefix in ["t", "T", "tr", "rt", "Rt", "TR"] {
            let body = format!("s = {prefix}'{{unsafe_value}}'");
            assert!(python_live(&body), "{body}");
        }
        assert!(!python_live("s = t'unsafe_value'"));
    }

    #[test]
    fn python_prefix_letters_after_identifier_are_not_a_prefix() {
        // `xf'...'` is the name `xf` then a plain string (CPython rejects the
        // program; either way the braces are not a field).
        assert!(!python_live("s = xf'{unsafe_value}'"));
        // Non-ASCII identifier characters count too.
        assert!(!python_live("s = éf'{unsafe_value}'"));
        // `if` then a plain string, as CPython tokenizes `1if'...'`.
        assert!(!python_live("s = 1if'{unsafe_value}'else'y'"));
    }

    #[test]
    fn python_single_quoted_string_ends_at_unescaped_newline() {
        // CPython refuses the newline; the lexer fails safe instead of
        // reading on to the next line's quote.
        assert!(python_live("s = 'a\nunsafe_value'"));
        // An escaped newline is a line continuation inside the literal.
        assert!(!python_live("s = 'a\\\nunsafe_value'"));
    }

    #[test]
    fn python_bare_cr_and_crlf_end_a_comment() {
        // CPython normalizes universal newlines: a bare CR ends the line, so
        // the assignment after it is live code.
        assert!(validate_body("python", "# comment\rVAL=1", &["VAL"], &[]).is_err());
        assert!(python_live("# comment\runsafe_value = 1"));
        assert!(python_live("# comment\r\nunsafe_value = 1"));
        assert!(python_live("x = 1  # comment\runsafe_value()"));
        // The value inside the comment itself stays inert.
        assert!(!python_live("# unsafe_value\r\nx = 1"));
    }

    #[test]
    fn python_cr_inside_single_quoted_string_fails_safe() {
        // CPython reads the CR as a newline: the literal is unterminated.
        assert!(python_live("s = 'a\runsafe_value'"));
        assert!(python_live("s = 'a\r\nunsafe_value'"));
        assert!(python_live("s = f'{x:a\runsafe_value}'"));
        // A triple-quoted literal may span CR line ends.
        assert!(!python_live("s = '''a\runsafe_value\r\n'''"));
    }

    #[test]
    fn python_backslash_crlf_continues_a_string() {
        // `\` + CRLF is one line continuation, as `\` + LF is.
        assert!(!python_live("s = 'a\\\r\nunsafe_value'"));
        assert!(!python_live("s = 'a\\\runsafe_value'"));
        // ...in a format spec too: the literal still closes, and the plain
        // string after it is inert.
        assert!(!python_live("s = f'{x:a\\\r\n}' + 'unsafe_value'"));
    }

    #[test]
    fn python_cr_ends_a_comment_inside_an_fstring_field() {
        assert!(python_live("s = f'''{x # c\r+ unsafe_value}'''"));
        assert!(python_live("s = f'''{x # c\r\n+ unsafe_value}'''"));
        // CPython closes the field and the literal on the CR line, so the
        // value is a live statement; an LF-only comment end would run the
        // field on to the dict's `}` and read the value as literal text.
        assert!(python_live(
            "f'''{x #\r}'''; d = {1: 2 #\n}; unsafe_value; ''''''"
        ));
    }

    #[test]
    fn python_non_newline_line_breaks_do_not_end_a_comment() {
        // CPython's tokenizer ends lines only at LF, CR, and CRLF; these stay
        // inside the comment, and the value in it is inert.
        for sep in [
            '\x0c', '\x0b', '\x1c', '\x1d', '\x1e', '\u{85}', '\u{2028}', '\u{2029}',
        ] {
            let body = format!("# c{sep}unsafe_value = 1");
            assert!(!python_live(&body), "{body:?}");
        }
    }

    #[test]
    fn value_straddling_a_python_literal_boundary_is_rejected() {
        let body = "s = 'a' + unsafe + 'b'";
        assert!(validate_body("python", body, &["a' + unsafe + 'b"], &[]).is_err());
        assert!(validate_body("python", body, &["a' "], &[]).is_err());
        assert!(validate_body("python", body, &["b"], &[]).is_ok());
    }

    #[test]
    fn generic_unterminated_string_fails_safe() {
        assert!(validate_body("lean", "x := \"unsafe_value", &["unsafe_value"], &[]).is_err());
        assert!(
            validate_body(
                "lean",
                "x := \"unsafe_value\" // unsafe_value",
                &["unsafe_value"],
                &[]
            )
            .is_ok()
        );
    }

    #[test]
    fn python_hash_inside_string_is_not_a_comment() {
        let body = "message = 'unsafe_value # not a comment'\nlive_code()";
        assert!(validate_body("python", body, &["unsafe_value"], &[]).is_ok());
        assert!(matches!(
            validate_body("python", body, &["live_code"], &[]),
            Err(BodyValidationError::InterpolationOutsideString { .. })
        ));
    }

    #[test]
    fn python_fstring_literal_part_is_inert_but_replacement_field_is_live() {
        // The value sitting in the literal text of an f-string is inert.
        assert!(
            validate_body(
                "python",
                "message = f'safe: unsafe_value'",
                &["unsafe_value"],
                &[]
            )
            .is_ok()
        );
        // The value sitting inside a `{}` replacement field is live code.
        assert!(matches!(
            validate_body(
                "python",
                "message = f'{unsafe_value}'",
                &["unsafe_value"],
                &[]
            ),
            Err(BodyValidationError::InterpolationOutsideString { .. })
        ));
    }

    #[test]
    fn python_fstring_escaped_braces_are_inert() {
        assert!(
            validate_body(
                "python",
                "message = f'{{unsafe_value}}'",
                &["unsafe_value"],
                &[]
            )
            .is_ok()
        );
    }

    #[test]
    fn python_unterminated_string_fails_safe_never_accepts() {
        // No closing quote: the dangling literal (and whatever text follows
        // it on the line) must be treated as live code, not silently inert.
        let body = "message = 'unsafe_value";
        assert!(matches!(
            validate_body("python", body, &["unsafe_value"], &[]),
            Err(BodyValidationError::InterpolationOutsideString { .. })
        ));
        // Unterminated triple-quoted string.
        let body = "message = '''unsafe_value";
        assert!(matches!(
            validate_body("python", body, &["unsafe_value"], &[]),
            Err(BodyValidationError::InterpolationOutsideString { .. })
        ));
    }

    #[test]
    fn char_literal_quote_does_not_hide_live_interpolation() {
        // `'"'` is a char literal; a hand lexer that treats `"` as opening a
        // string hides everything after it.
        let body = "fn h() { let q = '\"'; EVIL_VALUE(); }";
        assert!(rejects(body, &["EVIL_VALUE()"]), "{body}");
    }

    #[test]
    fn char_literal_quote_does_not_hide_denied_path() {
        let body = "fn h() { let q = '\"'; std :: fs :: remove_dir_all(\"/\"); }";
        assert!(rejects(body, &[]), "{body}");
    }

    #[test]
    fn whitespace_inside_macro_invocation_is_denied() {
        assert!(rejects(
            "fn h() { let _ = include_str ! (\"/etc/passwd\"); }",
            &[]
        ));
        assert!(rejects(
            "fn h() { let _ = include_bytes!(\"/etc/passwd\"); }",
            &[]
        ));
        assert!(rejects("fn h() { let _ = include !(\"/etc/x.rs\"); }", &[]));
        assert!(rejects("fn h() { let _ = env ! (\"HOME\"); }", &[]));
        assert!(rejects("fn h() { let _ = option_env!(\"HOME\"); }", &[]));
    }

    #[test]
    fn whitespace_inside_path_is_denied() {
        assert!(rejects(
            "fn h() { let _ = std :: fs :: read(\"/etc/passwd\"); }",
            &[]
        ));
        assert!(rejects(
            "fn h() { let _ = ::std::\n  net::TcpStream::connect(\"x:1\"); }",
            &[]
        ));
        assert!(rejects(
            "fn h() { let _ = std::env :: var(\"HOME\"); }",
            &[]
        ));
        assert!(rejects(
            "fn h() { let _ = std::env::var_os(\"HOME\"); }",
            &[]
        ));
        assert!(rejects(
            "fn h() { let _: Vec<_> = env :: vars().collect(); }",
            &[]
        ));
        assert!(rejects("use std::env::{vars_os as v};\nfn h() {}", &[]));
    }

    #[test]
    fn grouped_use_trees_are_denied() {
        assert!(rejects("use std::{fs, process};\nfn h() {}", &[]));
        assert!(rejects("use std::{io::{self}, fs as f};\nfn h() {}", &[]));
        assert!(rejects(
            "use std::{io, {process::Command}};\nfn h() {}",
            &[]
        ));
    }

    #[test]
    fn std_root_rebinding_and_globs_are_denied() {
        assert!(rejects(
            "use std as s;\nfn h() { s::fs::read(\"x\"); }",
            &[]
        ));
        assert!(rejects("use std::{self as s};\nfn h() {}", &[]));
        assert!(rejects("use std::*;\nfn h() { fs::read(\"x\"); }", &[]));
        assert!(rejects("use std::env::*;\nfn h() { var(\"x\"); }", &[]));
        assert!(rejects("use std::include_str as inc;\nfn h() {}", &[]));
    }

    #[test]
    fn attribute_path_and_unsafe_are_denied() {
        assert!(rejects("#[path = \"/etc/x.rs\"]\nmod m;", &[]));
        assert!(rejects("#[ path = \"/etc/x.rs\" ]\nmod m;", &[]));
        assert!(rejects(
            "#[cfg_attr(all(), path = \"/etc/x.rs\")]\nmod m;",
            &[]
        ));
        assert!(rejects("fn h() { unsafe { } }", &[]));
        assert!(rejects("extern  crate  std as s;", &[]));
    }

    #[test]
    fn raw_string_containing_quote_keeps_following_code_live() {
        let body = "fn h() { let s = r#\"a\"b\"#; EVIL_VALUE(); }";
        assert!(rejects(body, &["EVIL_VALUE()"]), "{body}");
        // ...while a value wholly inside a raw string is inert.
        let body = "fn h() { let s = r#\"EVIL_VALUE() \" \"#; }";
        assert!(!rejects(body, &["EVIL_VALUE()"]), "{body}");
    }

    #[test]
    fn values_inside_literals_and_comments_are_inert() {
        let body = "//! p.id\n// Property: p.id\n/* p.id */\n/// p.id\n/** p.id */\nfn h() { let s = \"p.id\"; let c = 'x'; }";
        assert!(validate_body("rust", body, &["p.id"], &[]).is_ok());
        // A doc comment does not make the code after it inert.
        let body = "/// p.id\nfn p_id() {}";
        assert!(rejects(body, &["p_id"]));
    }

    #[test]
    fn value_straddling_a_literal_boundary_is_rejected() {
        // The value's occurrence touches the live `,` between two literals.
        let body = "fn h() { f(\"a\", \"b\"); }";
        assert!(rejects(body, &["a\", \"b"]));
    }

    #[test]
    fn denied_path_text_inside_strings_and_comments_is_inert() {
        let body =
            "// std::fs::read\nfn h() { let s = \"std::process::Command::new include_str!\"; }";
        assert!(validate_body("rust", body, &[], &[]).is_ok());
    }

    #[test]
    fn unlexable_or_unparseable_body_fails_closed() {
        assert!(matches!(
            validate_body("rust", "fn h() { let s = \"unterminated; }", &[], &[]),
            Err(BodyValidationError::Unparseable { .. })
        ));
        assert!(matches!(
            validate_body("rust", "fn h() { ( }", &[], &[]),
            Err(BodyValidationError::Unparseable { .. })
        ));
        assert!(matches!(
            validate_body("rust", "fn fn fn", &[], &[]),
            Err(BodyValidationError::Unparseable { .. })
        ));
    }

    #[test]
    fn benign_verus_harness_validates() {
        let body = "// GENERATED - DO NOT EDIT\n// Property: safe_divide.zero (verus-native)\nuse vstd::prelude::*;\n\nverus! {\npub enum DivResult { Ok(i32), Err(i32) }\npub open spec fn zero(res: DivResult) -> bool { matches!(res, DivResult::Err(_)) }\npub fn divide_zero() -> (res: DivResult)\n    ensures zero(res),\n{\n    DivResult::Err(0)\n}\nfn main() {}\n} // verus!\n";
        validate_body("rust", body, &["safe_divide.zero", "safe_divide"], &[]).expect("benign");
    }

    #[test]
    fn path_qualified_denied_macro_is_denied() {
        assert!(rejects("fn h() { let _ = std::env!(\"HOME\"); }", &[]));
        assert!(rejects("fn h() { let _ = ::core::env!(\"HOME\"); }", &[]));
        assert!(rejects(
            "fn h() { let _ = core :: include_str ! (\"x\"); }",
            &[]
        ));
        // A module path through `env` is still fine.
        assert!(!rejects("fn h() { let _ = std::env::args(); }", &[]));
    }

    #[test]
    fn macro_rules_cannot_rebuild_denied_names() {
        assert!(rejects(
            "macro_rules! p{($x:ident)=>{std::$x::read_to_string(\"/etc/passwd\")}}\nfn h() { p!(fs); }",
            &[]
        ));
        assert!(rejects(
            "macro_rules! m{($n:ident)=>{$n!(\"/etc/passwd\")}}\nfn h() { m!(include_str); }",
            &[]
        ));
    }

    #[test]
    fn std_fs_net_process_anywhere_under_std_are_denied() {
        assert!(rejects(
            "fn h() { std::os::unix::fs::symlink(\"a\", \"b\"); }",
            &[]
        ));
        assert!(rejects(
            "fn h() { std::os::unix::net::UnixStream::connect(\"/s\"); }",
            &[]
        ));
        assert!(rejects("use std::os::unix::net as n;\nfn h() {}", &[]));
        assert!(rejects(
            "use std::os::unix as u;\nfn h() { u::fs::symlink(\"a\", \"b\"); }",
            &[]
        ));
        assert!(rejects(
            "use std::{os::{unix::{process::CommandExt}}};\nfn h() {}",
            &[]
        ));
    }

    #[test]
    fn deep_bracket_nesting_is_refused_without_overflowing_the_stack() {
        let depth = 5000;
        let body = format!(
            "fn h() {{ let _ = {}1{}; }}",
            "(".repeat(depth),
            ")".repeat(depth)
        );
        // A small stack: recursion over the nesting would overflow and abort.
        let handle = std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(move || validate_body("rust", &body, &[], &[]).map_err(|e| e.to_string()))
            .expect("spawn");
        let result = handle
            .join()
            .expect("validator must not overflow the stack");
        let err = result.expect_err("deep nesting must be refused");
        assert!(err.contains("nesting"), "{err}");
        // Nesting right at the cap validates on a default-sized (2 MiB)
        // thread in a debug build: the cap leaves headroom for syn.
        let at_cap = RUST_MAX_NESTING - 1;
        let body = format!(
            "fn h() {{ let _ = {}1{}; }}",
            "(".repeat(at_cap),
            ")".repeat(at_cap)
        );
        let handle = std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(move || validate_body("rust", &body, &[], &[]).is_ok())
            .expect("spawn");
        assert!(handle.join().expect("no overflow at the cap"));
        // Ordinary nesting still validates.
        let body = format!(
            "fn h() {{ let _ = {}1{}; }}",
            "(".repeat(20),
            ")".repeat(20)
        );
        assert!(validate_body("rust", &body, &[], &[]).is_ok());
    }

    /// Differential check: random benign bodies with a deny item spliced in
    /// through random whitespace / grouping / renaming must all be rejected,
    /// and the benign bodies alone must all validate.
    #[test]
    fn spliced_deny_items_are_always_rejected() {
        use rand::{Rng, SeedableRng};
        let mut rng = rand::rngs::StdRng::seed_from_u64(0x05ee_dc15);
        let ws = [" ", "", "\n", "\t", "  ", " /* c */ ", "\n// c\n"];
        let benign_stmts = [
            "let a = 1u32;",
            "let q = '\"';",
            "let s = r#\"x\"y\"#;",
            "let t = \"std::fs\";",
            "assert!(a + 1 > 0);",
            "let v: Vec<u8> = Vec::new();",
        ];
        for _ in 0..500 {
            let picks: Vec<&str> = (0..4).map(|_| ws[rng.gen_range(0..ws.len())]).collect();
            let mut next = picks.iter().cycle();
            let mut w = || next.next().copied().unwrap_or(" ");
            let (item, top_level): (String, bool) = match rng.gen_range(0..12) {
                0 => (
                    format!("std{}::{}fs{}::{}read(\"x\");", w(), w(), w(), w()),
                    false,
                ),
                1 => (
                    format!("include_str{}!{}(\"/etc/passwd\");", w(), w()),
                    false,
                ),
                2 => (format!("use std::{{io{}, {}process}};", w(), w()), true),
                3 => (format!("use std::{{io::{{self}},{}net as n}};", w()), true),
                4 => (format!("use{}std{}as s;", " ", w()), true),
                5 => (format!("unsafe{}{{ }}", w()), false),
                6 => (
                    format!("#{}[{}path{}= \"x.rs\"] mod m;", w(), w(), w()),
                    true,
                ),
                7 => (format!("env{}::{}var(\"HOME\");", w(), w()), false),
                9 => (
                    format!(
                        "std{}::{}os::unix{}::fs::symlink(\"a\", \"b\");",
                        w(),
                        w(),
                        w()
                    ),
                    false,
                ),
                10 => (format!("std{}::{}env{}!(\"HOME\");", w(), w(), w()), false),
                11 => (format!("macro_rules!{}m{{ () => {{}} }}", w()), true),
                _ => (format!("Command{}::{}new(\"sh\");", w(), w()), false),
            };
            let mut stmts: Vec<&str> = (0..rng.gen_range(0..4))
                .map(|_| benign_stmts[rng.gen_range(0..benign_stmts.len())])
                .collect();
            let benign = format!("fn h() {{ {} }}", stmts.join(" "));
            assert!(validate_body("rust", &benign, &[], &[]).is_ok(), "{benign}");
            let body = if top_level {
                format!("{item}\n{benign}")
            } else {
                let at = rng.gen_range(0..=stmts.len());
                stmts.insert(at, &item);
                format!("fn h() {{ {} }}", stmts.join(" "))
            };
            assert!(validate_body("rust", &body, &[], &[]).is_err(), "{body}");
        }
    }

    // ---- identifier-field values (S5 identifier-field rule) ----

    #[test]
    fn is_identifier_value_matches_the_documented_shape() {
        assert!(is_identifier_value("divide"));
        assert!(is_identifier_value("safe_divide"));
        assert!(is_identifier_value("m::divide"));
        assert!(is_identifier_value("a::b::c"));
        assert!(!is_identifier_value(""));
        assert!(!is_identifier_value("self")); // keyword
        assert!(!is_identifier_value("fn")); // keyword
        assert!(!is_identifier_value("m::fn")); // keyword segment
        assert!(!is_identifier_value("r#fn")); // raw identifier (charset excludes `#`)
        assert!(!is_identifier_value("foo(bar)"));
        assert!(!is_identifier_value("::divide")); // empty leading segment
        assert!(!is_identifier_value("a.b")); // not `::`-delimited
    }

    #[test]
    fn identifier_value_called_directly_in_live_code_is_accepted() {
        let body = "fn divide() -> i32 { 0 } fn main() { divide(); }";
        assert!(
            validate_body("rust", body, &[], &["divide"]).is_ok(),
            "{body}"
        );
    }

    #[test]
    fn identifier_value_as_a_substring_of_a_longer_identifier_is_refused() {
        // "divide" must never be granted a pass merely because it is a
        // substring of the distinct identifier "divide_evil".
        let body = "fn divide_evil() -> i32 { 0 } fn main() { divide_evil(); }";
        assert!(rejects_identifiers(body, &["divide"]), "{body}");
    }

    #[test]
    fn non_identifier_shaped_value_in_identifier_values_still_requires_inertness() {
        // Not identifier-shaped (contains `(`/`)`): validate_body must fall
        // back to the ordinary inert-only rule for it, not silently drop the
        // check because it arrived via `identifier_values`.
        let value = "foo(bar)";
        let body = format!("fn main() {{ {value}; }}");
        assert!(
            matches!(
                validate_body("rust", &body, &[], &[value]),
                Err(BodyValidationError::InterpolationOutsideString { .. })
            ),
            "{body}"
        );
    }

    #[test]
    fn non_identifier_shaped_subject_value_in_live_code_is_refused() {
        // The task's own example of a hostile, non-identifier-shaped
        // "subject": it also happens to trip the `std::process` deny-list,
        // but even a value that did not would still be refused (previous
        // test) — this one pins the literal scenario a hostile property
        // subject could look like.
        let value = "foo(); std::process::exit(0)";
        let body = format!("fn main() {{ {value}; }}");
        assert!(rejects_identifiers(&body, &[value]), "{body}");
    }

    #[test]
    fn identifier_value_used_as_a_macro_invocation_is_refused() {
        let body = "fn main() { subject_name!(); }";
        assert!(rejects_identifiers(body, &["subject_name"]), "{body}");
    }

    #[test]
    fn free_text_value_in_live_code_is_still_refused_alongside_identifier_values() {
        // The split into two lists must not weaken the inert-only rule for
        // free text just because an identifier value is also being checked.
        let body = "fn divide() {} fn main() { let x = condition_text; divide(); }";
        assert!(
            matches!(
                validate_body("rust", body, &["condition_text"], &["divide"]),
                Err(BodyValidationError::InterpolationOutsideString { .. })
            ),
            "{body}"
        );
    }

    #[test]
    fn identifier_value_full_path_used_as_a_qualified_call_is_accepted() {
        let body = "mod m { pub fn divide() {} } fn main() { m::divide(); }";
        assert!(
            validate_body("rust", body, &[], &["m::divide"]).is_ok(),
            "{body}"
        );
    }

    #[test]
    fn identifier_value_as_a_shorter_suffix_of_a_longer_path_is_refused() {
        // "m::divide" must not be granted a pass by a call to the different
        // path "x::m::divide".
        let body = "mod x { pub mod m { pub fn divide() {} } } fn main() { x::m::divide(); }";
        assert!(rejects_identifiers(body, &["m::divide"]), "{body}");
    }

    #[test]
    fn identifier_value_inert_in_a_comment_or_string_is_still_fine() {
        let body = "// calls divide() below\nfn main() { let s = \"divide\"; }";
        assert!(
            validate_body("rust", body, &[], &["divide"]).is_ok(),
            "{body}"
        );
    }
}

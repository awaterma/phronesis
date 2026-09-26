//! Rendered-body validation (SPEC-verification-artifact-generation.md §S5):
//! the second validator layer between the render entry and the file write.
//!
//! Layer 1 (the field-class contract) lives in `properties/store.rs` at
//! ingest. This layer re-parses the rendered body and asserts:
//! (a) every interpolated property value appears only inside string
//!     literals or comments — hostile payloads render inert;
//! (b) the (language, verifier) deny-list constructs are absent from the
//!     body outside sanctioned positions.
//!
//! For rust both checks run over the token stream of a real Rust lexer
//! (`proc-macro2`) and the body must parse as a file (`syn`) — an unlexable
//! or unparseable body is refused. Matching is on tokens, never on text, so
//! whitespace, comments, char literals (`'"'`), raw strings and grouped
//! `use` trees cannot hide a denied construct or a live interpolation.

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

/// Validate the rendered body: deny-list constructs must not appear, and
/// every interpolated value must sit inside a string literal (or comment).
/// `language` keys the deny-list; `interpolated` are the property values the
/// render embedded (id, subject, condition/guarantee after escaping).
///
/// Rust: `Ok` ⇒ the body lexes and parses as a Rust file, no deny-listed
/// path/macro/attribute/keyword occurs in any token, and no occurrence of an
/// interpolated value overlaps a code token (anything but a string/char/byte
/// literal or a comment).
pub fn validate_body(
    language: &str,
    body: &str,
    interpolated: &[&str],
) -> Result<(), BodyValidationError> {
    if language == "rust" {
        return validate_rust_body(body, interpolated);
    }
    for construct in deny_list(language) {
        if body.contains(construct) {
            return Err(denied(language, construct));
        }
    }
    let outside_strings = strip_string_literals(body);
    for value in interpolated {
        if !value.is_empty() && outside_strings.contains(value) {
            return Err(BodyValidationError::InterpolationOutsideString {
                interpolated: (*value).to_string(),
            });
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

fn validate_rust_body(body: &str, interpolated: &[&str]) -> Result<(), BodyValidationError> {
    let result = check_rust_body(body, interpolated);
    // The fallback lexer records every parsed source in a thread-local span
    // map; every span from this call is dropped by now, so release it rather
    // than grow it for the life of a long-running server.
    proc_macro2::extra::invalidate_current_thread_spans();
    result
}

fn check_rust_body(body: &str, interpolated: &[&str]) -> Result<(), BodyValidationError> {
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
    for value in interpolated {
        if value.is_empty() {
            continue;
        }
        // Every occurrence, overlapping ones included.
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

/// Non-rust fallback: remove `"..."` literals and `//` line comments so what
/// remains is code structure — the region where interpolated values are
/// forbidden.
fn strip_string_literals(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                out.push(' ');
                // consume the literal, honoring backslash escapes
                let mut escaped = false;
                for d in chars.by_ref() {
                    if escaped {
                        escaped = false;
                    } else if d == '\\' {
                        escaped = true;
                    } else if d == '"' {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'/') => {
                // line comment: skip to end of line
                out.push(' ');
                for d in chars.by_ref() {
                    if d == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rejects(body: &str, interpolated: &[&str]) -> bool {
        validate_body("rust", body, interpolated).is_err()
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
        assert!(validate_body("rust", body, &["p.id"]).is_ok());
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
        assert!(validate_body("rust", body, &[]).is_ok());
    }

    #[test]
    fn unlexable_or_unparseable_body_fails_closed() {
        assert!(matches!(
            validate_body("rust", "fn h() { let s = \"unterminated; }", &[]),
            Err(BodyValidationError::Unparseable { .. })
        ));
        assert!(matches!(
            validate_body("rust", "fn h() { ( }", &[]),
            Err(BodyValidationError::Unparseable { .. })
        ));
        assert!(matches!(
            validate_body("rust", "fn fn fn", &[]),
            Err(BodyValidationError::Unparseable { .. })
        ));
    }

    #[test]
    fn benign_verus_harness_validates() {
        let body = "// GENERATED - DO NOT EDIT\n// Property: safe_divide.zero (verus-native)\nuse vstd::prelude::*;\n\nverus! {\npub enum DivResult { Ok(i32), Err(i32) }\npub open spec fn zero(res: DivResult) -> bool { matches!(res, DivResult::Err(_)) }\npub fn divide_zero() -> (res: DivResult)\n    ensures zero(res),\n{\n    DivResult::Err(0)\n}\nfn main() {}\n} // verus!\n";
        validate_body("rust", body, &["safe_divide.zero", "safe_divide"]).expect("benign");
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
            .spawn(move || validate_body("rust", &body, &[]).map_err(|e| e.to_string()))
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
            .spawn(move || validate_body("rust", &body, &[]).is_ok())
            .expect("spawn");
        assert!(handle.join().expect("no overflow at the cap"));
        // Ordinary nesting still validates.
        let body = format!(
            "fn h() {{ let _ = {}1{}; }}",
            "(".repeat(20),
            ")".repeat(20)
        );
        assert!(validate_body("rust", &body, &[]).is_ok());
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
            assert!(validate_body("rust", &benign, &[]).is_ok(), "{benign}");
            let body = if top_level {
                format!("{item}\n{benign}")
            } else {
                let at = rng.gen_range(0..=stmts.len());
                stmts.insert(at, &item);
                format!("fn h() {{ {} }}", stmts.join(" "))
            };
            assert!(validate_body("rust", &body, &[]).is_err(), "{body}");
        }
    }
}

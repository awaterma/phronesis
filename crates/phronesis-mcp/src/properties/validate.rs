//! Rendered-body validation (SPEC-verification-artifact-generation.md §S5):
//! the second validator layer between the render entry and the file write.
//!
//! Layer 1 (the field-class contract) lives in `properties/store.rs` at
//! ingest. This layer re-parses the rendered body and asserts:
//! (a) every interpolated property value appears only inside string
//!     literals — hostile payloads render inert;
//! (b) the (language, verifier) deny-list constructs are absent from the
//!     body outside sanctioned positions.

/// Per-language dangerous-construct deny-list — part of the
/// (language, verifier) instantiation, not the pipeline. Rendered bodies are
/// plain-token-scanned for these; a hit rejects generation.
pub fn deny_list(language: &str) -> &'static [&'static str] {
    match language {
        "rust" => &[
            "include!",
            "include_str!",
            "include_bytes!",
            "#[path",
            "extern crate",
            "unsafe",
            "std::process",
            "std::fs",
            "std::net",
            "Command::new",
            "env::var",
        ],
        "python" => &["eval(", "exec(", "os.system", "subprocess", "__import__"],
        _ => &[],
    }
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

/// Validate the rendered body: deny-list constructs must not appear, and
/// every interpolated value must sit inside a string literal. `language`
/// keys the deny-list; `interpolated` are the property values the render
/// embedded (id, subject, condition/guarantee after escaping).
pub fn validate_body(
    language: &str,
    body: &str,
    interpolated: &[&str],
) -> Result<(), BodyValidationError> {
    for construct in deny_list(language) {
        if body.contains(construct) {
            return Err(BodyValidationError::DeniedConstruct {
                language: language.to_string(),
                construct: construct.to_string(),
            });
        }
    }
    // Interpolated values must appear only inside string literals: strip all
    // string-literal spans, then assert none of the interpolated values
    // survive outside them.
    let outside_strings = strip_string_literals(body, language);
    for value in interpolated {
        if value.is_empty() {
            continue;
        }
        if outside_strings.contains(value) {
            return Err(BodyValidationError::InterpolationOutsideString {
                interpolated: (*value).to_string(),
            });
        }
    }
    Ok(())
}

/// Remove string-literal spans from `body` (rust: `"..."` with escapes) so
/// what remains is code structure — the region where interpolated values are
/// forbidden.
fn strip_string_literals(body: &str, language: &str) -> String {
    let _ = language; // per-language string syntax; rust handled now
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

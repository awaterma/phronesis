//! Query-only record templates: render a manifest `RecordTemplate` against
//! one `QueryMatch`'s captures, with no script
//! (`docs/superpowers/specs/2026-10-17-go-and-language-packs-design.md`
//! §"Manifest and assets" — the `QuerySpec.emit` paragraph).
//!
//! A template is a JSON object matching the record schema. A whole-string
//! value of exactly `${name}` substitutes that capture's text; `${file}`
//! substitutes the file being analyzed; `${name.start_byte}` substitutes
//! that capture's start byte (for numeric fields such as
//! `owner_start_byte` — not in the frozen shared contract text, but
//! required to construct a `Reference` record without a script, since
//! `span_capture` only ever supplies the record's own `span`). Every other
//! value is copied literally. A `span_capture` key names the capture whose
//! span becomes the record's `span`; every record kind requires one.

use std::collections::HashMap;

use serde_json::{Map, Value};

use super::manifest::RecordTemplate;
use super::records::{Capture, PackDiagnostic, PackRecord, QueryMatch, RecordError};

/// Validate the static template shape against the public record enum. Capture
/// placeholders are replaced with representative values of their runtime type;
/// their existence and uniqueness remain per-match diagnostics during render.
pub fn validate_template(template: &RecordTemplate) -> Result<(), RecordError> {
    let mut value = Value::Object(template.0.clone());
    let Value::Object(object) = &mut value else {
        unreachable!()
    };
    object.remove("span_capture");
    object.remove("span");
    object.insert(
        "span".into(),
        serde_json::json!({"start_byte": 0, "end_byte": 0, "start_line": 1, "end_line": 1}),
    );
    replace_placeholders(&mut value);
    serde_json::from_value::<PackRecord>(value)
        .map(|_| ())
        .map_err(|e| RecordError(format!("template does not match the record schema: {e}")))
}

fn replace_placeholders(value: &mut Value) {
    match value {
        Value::String(text) => {
            if let Some(name) = whole_placeholder(text) {
                *value = if name.ends_with(".start_byte") {
                    Value::from(0usize)
                } else {
                    Value::String(String::new())
                };
            }
        }
        Value::Array(values) => values.iter_mut().for_each(replace_placeholders),
        Value::Object(values) => values.values_mut().for_each(replace_placeholders),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

/// Render `template` against `match_`'s captures into a JSON record value.
/// Returns a [`PackDiagnostic`] — not a hard error — when a referenced
/// capture is missing, matched more than once, or `span_capture` is absent:
/// that one emission is skipped, not the whole file's extraction.
pub fn render_template(
    pack: &str,
    file_rel: &str,
    template: &RecordTemplate,
    match_: &QueryMatch,
) -> Result<Value, PackDiagnostic> {
    render(template, match_, file_rel).map_err(|e| PackDiagnostic {
        pack: Some(pack.to_string()),
        path: Some(file_rel.to_string()),
        stage: "template".to_string(),
        message: e.0,
    })
}

fn render(
    template: &RecordTemplate,
    match_: &QueryMatch,
    file_rel: &str,
) -> Result<Value, RecordError> {
    let mut by_name: HashMap<&str, Vec<&Capture>> = HashMap::new();
    for capture in &match_.captures {
        by_name
            .entry(capture.name.as_str())
            .or_default()
            .push(capture);
    }

    let mut out = Map::with_capacity(template.0.len());
    for (key, value) in &template.0 {
        if key == "span_capture" {
            continue;
        }
        let rendered = render_value(value, file_rel, &by_name)?;
        out.insert(key.clone(), rendered);
    }

    match template.0.get("span_capture") {
        Some(Value::String(name)) => {
            let capture = unique_capture(&by_name, name)?;
            let span = serde_json::to_value(capture.span).expect("SourceSpan serializes to JSON");
            out.insert("span".to_string(), span);
        }
        _ => return Err(RecordError("template has no `span_capture`".to_string())),
    }

    Ok(Value::Object(out))
}

/// Render whole-string placeholders at any JSON depth. Embedded placeholders
/// remain literal, including inside nested argument arrays.
fn render_value(
    value: &Value,
    file_rel: &str,
    by_name: &HashMap<&str, Vec<&Capture>>,
) -> Result<Value, RecordError> {
    match value {
        Value::String(s) => substitute(s, file_rel, by_name),
        Value::Array(values) => values
            .iter()
            .map(|value| render_value(value, file_rel, by_name))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Value::Object(values) => values
            .iter()
            .map(|(key, value)| {
                render_value(value, file_rel, by_name).map(|rendered| (key.clone(), rendered))
            })
            .collect::<Result<Map<_, _>, _>>()
            .map(Value::Object),
        other => Ok(other.clone()),
    }
}

/// The one capture named `name`, or an error naming whether it was absent
/// or ambiguous.
fn unique_capture<'a>(
    by_name: &HashMap<&str, Vec<&'a Capture>>,
    name: &str,
) -> Result<&'a Capture, RecordError> {
    match by_name.get(name).map(Vec::as_slice) {
        Some([one]) => Ok(*one),
        Some([]) | None => Err(RecordError(format!("capture `{name}` was not matched"))),
        Some(_) => Err(RecordError(format!(
            "capture `{name}` matched more than once"
        ))),
    }
}

/// Substitute `s` if it is exactly one `${...}` placeholder; otherwise
/// return it as a literal string.
fn substitute(
    s: &str,
    file_rel: &str,
    by_name: &HashMap<&str, Vec<&Capture>>,
) -> Result<Value, RecordError> {
    let Some(name) = whole_placeholder(s) else {
        return Ok(Value::String(s.to_string()));
    };
    if name == "file" {
        return Ok(Value::String(file_rel.to_string()));
    }
    if let Some(field) = name.strip_suffix(".start_byte") {
        let capture = unique_capture(by_name, field)?;
        return Ok(Value::from(capture.span.start_byte));
    }
    let capture = unique_capture(by_name, name)?;
    Ok(Value::String(capture.text.clone()))
}

/// `s` is exactly `${...}` with no other characters — `None` (and thus a
/// literal, not a placeholder) otherwise. A placeholder embedded in a
/// longer string (`"prefix-${name}"`) is deliberately not interpolated: the
/// shared contract specifies whole-string substitution only.
fn whole_placeholder(s: &str) -> Option<&str> {
    s.strip_prefix("${").and_then(|rest| rest.strip_suffix('}'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language_packs::records::SourceSpan;
    use serde_json::json;

    fn capture(name: &str, text: &str, start: usize, end: usize) -> Capture {
        Capture {
            name: name.to_string(),
            kind: "identifier".to_string(),
            text: text.to_string(),
            span: SourceSpan {
                start_byte: start,
                end_byte: end,
                start_line: 1,
                end_line: 1,
            },
        }
    }

    fn template(value: Value) -> RecordTemplate {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn substitutes_whole_string_capture_and_file_placeholders() {
        let m = QueryMatch {
            query: "decls".into(),
            captures: vec![capture("def", "foo", 0, 3), capture("name", "foo", 3, 6)],
        };
        let t = template(json!({
            "kind": "declaration",
            "name": "${name}",
            "scope": "",
            "span_capture": "def"
        }));
        let rendered = render_template("demo", "a.demo", &t, &m).unwrap();
        assert_eq!(
            rendered,
            json!({"kind": "declaration", "name": "foo", "scope": "", "span": {
                "start_byte": 0, "end_byte": 3, "start_line": 1, "end_line": 1
            }})
        );
    }

    #[test]
    fn substitutes_whole_placeholders_in_nested_fact_args_and_test_command_args() {
        let m = QueryMatch {
            query: "q".into(),
            captures: vec![capture("name", "resolved", 0, 8)],
        };
        let fact = template(json!({
            "kind": "fact", "name": "demo", "args": ["${name}", "${file}", "prefix-${name}"],
            "span_capture": "name"
        }));
        let rendered = render_template("demo", "src/a.go", &fact, &m).unwrap();
        assert_eq!(
            rendered["args"],
            json!(["resolved", "src/a.go", "prefix-${name}"])
        );

        let declaration = template(json!({
            "kind": "declaration", "name": "${name}", "scope": "",
            "test": true,
            "command": {"program": "go", "args": ["test", "${file}", "${name}"], "cwd": "."},
            "span_capture": "name"
        }));
        let rendered = render_template("demo", "src/a.go", &declaration, &m).unwrap();
        assert_eq!(
            rendered["command"]["args"],
            json!(["test", "src/a.go", "resolved"])
        );
    }

    #[test]
    fn embedded_placeholder_is_literal_not_interpolated() {
        let m = QueryMatch {
            query: "decls".into(),
            captures: vec![capture("def", "foo", 0, 3)],
        };
        let t = template(json!({
            "kind": "fact", "name": "n", "args": ["prefix-${def}"], "span_capture": "def"
        }));
        let rendered = render_template("demo", "a.demo", &t, &m).unwrap();
        assert_eq!(rendered["args"], json!(["prefix-${def}"]));
    }

    #[test]
    fn missing_capture_fails_that_emission() {
        let m = QueryMatch {
            query: "decls".into(),
            captures: vec![capture("def", "foo", 0, 3)],
        };
        let t = template(json!({
            "kind": "declaration", "name": "${name}", "scope": "", "span_capture": "def"
        }));
        let err = render_template("demo", "a.demo", &t, &m).unwrap_err();
        assert_eq!(err.pack.as_deref(), Some("demo"));
        assert!(err.message.contains("name"), "{}", err.message);
    }

    #[test]
    fn repeated_capture_fails_that_emission() {
        let m = QueryMatch {
            query: "decls".into(),
            captures: vec![
                capture("def", "foo", 0, 3),
                capture("name", "a", 0, 1),
                capture("name", "b", 1, 2),
            ],
        };
        let t = template(json!({
            "kind": "declaration", "name": "${name}", "scope": "", "span_capture": "def"
        }));
        let err = render_template("demo", "a.demo", &t, &m).unwrap_err();
        assert!(err.message.contains("more than once"), "{}", err.message);
    }

    #[test]
    fn missing_span_capture_fails_that_emission() {
        let m = QueryMatch {
            query: "decls".into(),
            captures: vec![capture("def", "foo", 0, 3)],
        };
        let t = template(json!({"kind": "declaration", "name": "x", "scope": ""}));
        let err = render_template("demo", "a.demo", &t, &m).unwrap_err();
        assert!(err.message.contains("span_capture"), "{}", err.message);
    }

    #[test]
    fn start_byte_placeholder_fills_a_numeric_field() {
        let m = QueryMatch {
            query: "refs".into(),
            captures: vec![
                capture("owner", "foo", 5, 8),
                capture("call", "foo", 10, 13),
            ],
        };
        let t = template(json!({
            "kind": "reference",
            "owner_start_byte": "${owner.start_byte}",
            "target": "foo",
            "scope": "",
            "resolution": "same_scope",
            "span_capture": "call"
        }));
        let rendered = render_template("demo", "a.demo", &t, &m).unwrap();
        assert_eq!(rendered["owner_start_byte"], json!(5));
    }
}

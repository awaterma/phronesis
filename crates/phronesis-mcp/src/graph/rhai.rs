//! Conservative structural extraction for application-owned Rhai scripts.

use super::extract::Extracted;
use super::model::Edge;
use super::unit::UnitContext;
use std::collections::BTreeSet;
use std::sync::LazyLock;

static CALL_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\b([_A-Za-z][_A-Za-z0-9]*)\s*\(").expect("static Rhai call regex")
});
static FN_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\bfn\s+([_A-Za-z][_A-Za-z0-9]*)\s*\(").expect("static Rhai function regex")
});
static EMIT_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"\bemit_fact\s*\(\s*["']([_A-Za-z][_A-Za-z0-9]*)["']"#)
        .expect("static Rhai emitted-predicate regex")
});

const NON_HOST_CALLS: &[&str] = &[
    "if",
    "for",
    "while",
    "switch",
    "fn",
    "print",
    "debug",
    "type_of",
    "is_def_var",
    "emit_fact",
];

fn module_path(file_path: &str, unit: &UnitContext) -> String {
    let trimmed = file_path.strip_suffix(".rhai").unwrap_or(file_path);
    std::iter::once(unit.id.as_str())
        .chain(trimmed.split('/').filter(|segment| !segment.is_empty()))
        .collect::<Vec<_>>()
        .join("::")
}

fn code_shape(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut quote = None;
    let mut escaped = false;
    let mut block_comment = false;
    let mut characters = content.chars().peekable();
    while let Some(character) = characters.next() {
        if block_comment {
            if character == '*' && characters.peek() == Some(&'/') {
                out.push_str("  ");
                characters.next();
                block_comment = false;
            } else if character == '\n' {
                out.push('\n');
            } else {
                out.push(' ');
            }
            continue;
        }
        if quote.is_none() && character == '/' && characters.peek() == Some(&'/') {
            out.push_str("  ");
            characters.next();
            for comment in characters.by_ref() {
                if comment == '\n' {
                    out.push('\n');
                    break;
                }
                out.push(' ');
            }
            continue;
        }
        if quote.is_none() && character == '/' && characters.peek() == Some(&'*') {
            out.push_str("  ");
            characters.next();
            block_comment = true;
            continue;
        }
        {
            if let Some(active) = quote {
                if escaped {
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else if character == active {
                    quote = None;
                }
                out.push(' ');
            } else if character == '\'' || character == '"' || character == '`' {
                quote = Some(character);
                out.push(' ');
            } else {
                out.push(character);
            }
        }
    }
    out
}

pub fn extract_rhai(file_path: &str, content: &str, unit: &UnitContext) -> Extracted {
    if !file_path.ends_with(".rhai") {
        return Extracted::default();
    }
    if content.trim().is_empty() {
        return Extracted::unparseable();
    }
    let script = module_path(file_path, unit);
    let shaped = code_shape(content);
    let local_functions: BTreeSet<_> = FN_RE
        .captures_iter(&shaped)
        .filter_map(|captures| captures.get(1).map(|name| name.as_str()))
        .collect();
    let mut out = BTreeSet::from([
        (
            "file_type".to_string(),
            vec![file_path.to_string(), "production".to_string()],
        ),
        (
            "declares_module".to_string(),
            vec![file_path.to_string(), script.clone()],
        ),
    ]);
    for captures in CALL_RE.captures_iter(&shaped) {
        let Some(name) = captures.get(1).map(|value| value.as_str()) else {
            continue;
        };
        if NON_HOST_CALLS.contains(&name) || local_functions.contains(name) {
            continue;
        }
        out.insert((
            "calls".to_string(),
            vec![script.clone(), format!("rhai:callable::{name}")],
        ));
    }
    for captures in EMIT_RE.captures_iter(content) {
        if let Some(predicate) = captures.get(1).map(|value| value.as_str()) {
            out.insert((
                "rhai_emits_predicate".to_string(),
                vec![script.clone(), predicate.to_string()],
            ));
        }
    }
    Extracted {
        edges: out
            .into_iter()
            .map(|(predicate, args)| {
                Edge::base(
                    &predicate,
                    &args.iter().map(String::as_str).collect::<Vec<_>>(),
                    file_path,
                )
            })
            .collect(),
        skipped: 0,
        parse_failed: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexes_literal_host_calls_but_not_strings_or_local_definitions() {
        let source = r#"
            fn helper(x) { x }
            helper(1);
            state_attempt_stunning_strike(actor, target);
            let prose = "fake_call()";
            // Climb/jump/swim -> Athletics(actor). Hide -> Stealth(actor).
            /* Playtest bug(actor) and DC(actor). */
            emit_fact("combat_action_requested", [actor]);
        "#;
        let out = extract_rhai(
            "scripts/combat.rhai",
            source,
            &UnitContext::unnamed_for(super::super::unit::LANG_RHAI),
        );
        let calls: Vec<_> = out.edges.iter().filter(|edge| edge.p == "calls").collect();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].a[1],
            "rhai:callable::state_attempt_stunning_strike"
        );
        assert!(out.edges.iter().any(|edge| {
            edge.p == "rhai_emits_predicate" && edge.a[1] == "combat_action_requested"
        }));
    }
}

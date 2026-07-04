//! Generator for the rule-catalogue page (`docs/catalogue.html`).
//!
//! Renders every rule in the content-bearing default packs as the
//! catalogue's `<article class="rule">` entry markup and splices the
//! result between the GENERATED RULES markers in the hand-authored
//! page. The frame outside the markers is never touched.

use crate::init::Pack;

pub const BEGIN_MARKER: &str = "<!-- BEGIN GENERATED RULES -->";
pub const END_MARKER: &str = "<!-- END GENERATED RULES -->";

/// Content-bearing default packs, in catalogue order. `Journey` and
/// `None` ship no starter rules and are omitted.
pub const CATALOGUE_PACKS: &[Pack] = &[
    Pack::Llm,
    Pack::Rust,
    Pack::Rhai,
    Pack::Python,
    Pack::TypeScript,
    Pack::Swift,
    Pack::Confidence,
];

/// Render the generated region: a version stamp followed by one
/// `<section>` per pack, one `<article class="rule">` per rule.
pub fn render_rules_html() -> String {
    let mut out = format!(
        "<p class=\"catalogue-stamp\">documents the default packs as of v{}</p>\n",
        env!("CARGO_PKG_VERSION")
    );
    for pack in CATALOGUE_PACKS {
        let rules = pack.rules();
        let entries = rules["rules"].as_array().cloned().unwrap_or_default();
        if entries.is_empty() {
            continue;
        }
        out.push_str(&format!(
            "<section class=\"pack-section\" id=\"pack-{label}\">\n<h2 class=\"pack-name\">{label} pack</h2>\n",
            label = pack.label()
        ));
        for rule in &entries {
            out.push_str(&render_entry(rule, pack.label()));
        }
        out.push_str("</section>\n");
    }
    out
}

/// Replace everything between the BEGIN/END markers with `generated`.
/// The page outside the markers is preserved byte-for-byte.
pub fn splice(page: &str, generated: &str) -> Result<String, &'static str> {
    let begin = page
        .find(BEGIN_MARKER)
        .ok_or("missing BEGIN GENERATED RULES marker")?;
    let end = page
        .find(END_MARKER)
        .ok_or("missing END GENERATED RULES marker")?;
    if end < begin {
        return Err("END marker precedes BEGIN marker");
    }
    Ok(format!(
        "{}{}\n{}\n{}",
        &page[..begin],
        BEGIN_MARKER,
        generated.trim_end(),
        &page[end..]
    ))
}

fn render_entry(rule: &serde_json::Value, pack_label: &str) -> String {
    let id = rule["id"].as_str().unwrap_or("unknown-rule");
    let phase = rule["phase"].as_str().unwrap_or("pre");
    let (verb, message) = action_of(rule);
    format!(
        "<article class=\"rule\" data-level=\"{verb}\" id=\"{id}\">\n  <div class=\"rule-mark\" aria-hidden=\"true\">!</div>\n  <div class=\"rule-content\">\n    <div class=\"rule-tags\"><span class=\"tag tag-{verb}\">{verb}</span><span class=\"tag\">{phase}</span><span class=\"tag\">{pack_label}</span></div>\n    <h3 class=\"rule-id\">{id}</h3>\n    <p class=\"rule-summary\">{msg}</p>\n    <div class=\"rule-body\"><code>{preds}</code></div>\n  </div>\n</article>\n",
        msg = escape(&message),
        preds = escape(&predicate_summary(rule)),
    )
}

/// The rule's action verb and message, from the single-key v2 `then`
/// object (`block` / `warn` / `log`, or any forward-compatible verb).
fn action_of(rule: &serde_json::Value) -> (String, String) {
    if let Some(then) = rule["then"].as_object()
        && let Some((verb, msg)) = then.iter().next()
    {
        return (verb.clone(), msg.as_str().unwrap_or("").to_string());
    }
    ("warn".to_string(), String::new())
}

/// Human-scannable condition summary: leaf predicate names joined with
/// " + "; `or` clauses render their alternatives joined with " | ".
fn predicate_summary(rule: &serde_json::Value) -> String {
    let clauses = rule["when"].as_array().cloned().unwrap_or_default();
    let parts: Vec<String> = clauses.iter().map(clause_name).collect();
    parts.join(" + ")
}

fn clause_name(clause: &serde_json::Value) -> String {
    let Some(obj) = clause.as_object() else {
        return "?".to_string();
    };
    let Some((key, val)) = obj.iter().next() else {
        return "?".to_string();
    };
    if key == "or" {
        let alts: Vec<String> = val
            .as_array()
            .map(|a| a.iter().map(clause_name).collect())
            .unwrap_or_default();
        return format!("({})", alts.join(" | "));
    }
    key.clone()
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_one_entry_per_rule() {
        let n: usize = CATALOGUE_PACKS
            .iter()
            .map(|p| p.rules()["rules"].as_array().unwrap().len())
            .sum();
        let html = render_rules_html();
        assert_eq!(html.matches("<article class=\"rule\"").count(), n);
    }

    #[test]
    fn anchor_ids_are_unique_and_match_rule_ids() {
        let html = render_rules_html();
        let mut seen = std::collections::HashSet::new();
        for pack in CATALOGUE_PACKS {
            for rule in pack.rules()["rules"].as_array().unwrap() {
                let id = rule["id"].as_str().unwrap().to_string();
                assert!(
                    html.contains(&format!("id=\"{id}\"")),
                    "missing anchor {id}"
                );
                assert!(seen.insert(id.clone()), "duplicate anchor {id}");
            }
        }
    }

    #[test]
    fn output_carries_version_stamp() {
        let html = render_rules_html();
        assert!(html.contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn splice_replaces_only_between_markers() {
        let page = format!(
            "<header>keep</header>\n{BEGIN_MARKER}\nOLD\n{END_MARKER}\n<footer>keep</footer>"
        );
        let out = splice(&page, "NEW").unwrap();
        assert!(out.contains("<header>keep</header>"));
        assert!(out.contains("<footer>keep</footer>"));
        assert!(out.contains("NEW"));
        assert!(!out.contains("OLD"));
    }

    #[test]
    fn splice_is_idempotent() {
        let page = format!("A\n{BEGIN_MARKER}\nx\n{END_MARKER}\nB");
        let generated = render_rules_html();
        let once = splice(&page, &generated).unwrap();
        let twice = splice(&once, &generated).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn splice_errors_without_markers() {
        assert!(splice("no markers here", "x").is_err());
        let end_only = format!("text {END_MARKER} tail");
        assert!(splice(&end_only, "x").is_err());
        let reversed = format!("{END_MARKER} mid {BEGIN_MARKER}");
        assert!(splice(&reversed, "x").is_err());
    }

    #[test]
    fn internal_hrefs_resolve_to_generated_anchors() {
        let html = render_rules_html();
        for chunk in html.split("href=\"#").skip(1) {
            let target = chunk.split('"').next().unwrap();
            assert!(
                html.contains(&format!("id=\"{target}\"")),
                "dangling #{target}"
            );
        }
    }

    #[test]
    fn messages_are_html_escaped() {
        // The rust pack contains messages with `<`/`&` characters
        // (generic types, `&String`); none may leak raw into the HTML.
        let html = render_rules_html();
        for bad in ["<String>", "&Vec<", "&String,"] {
            assert!(
                !html.contains(bad),
                "unescaped fragment {bad:?} leaked into output"
            );
        }
    }
}

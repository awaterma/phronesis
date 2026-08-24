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

mod lexer;
mod parser;
#[cfg(test)]
mod tests;

use lexer::{Tok, lex};
use parser::{Facts, parse_action};

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
    if !is_helm_file(file_path, content, chart_root) {
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

    let mut out: BTreeSet<(String, Vec<String>)> = BTreeSet::new();

    out.insert((
        "file_type".to_string(),
        vec![
            file_path.to_string(),
            classify_file(file_path, chart_root).to_string(),
        ],
    ));
    out.insert((
        "declares_module".to_string(),
        vec![file_path.to_string(), self_module.clone()],
    ));

    // Tokenise the entire file and walk actions.
    let facts = collect_facts(content, &self_module, chart_name);

    // Apply collected facts.
    for (q, _file) in &facts.defines {
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
        out.insert((
            "imports".to_string(),
            vec![self_module.clone(), format!("{chart_name}::values::{path}")],
        ));
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

    Extracted {
        edges: out
            .into_iter()
            .map(|(p, a)| Edge {
                p,
                a,
                src: file_path.to_string(),
                d: false,
            })
            .collect(),
        skipped: 0,
        parse_failed: false,
    }
}

/// Does this file belong to the Helm 3 sensor at all?
fn is_helm_file(file_path: &str, content: &str, chart_root: Option<&str>) -> bool {
    let is_chart_yaml = file_path.ends_with("Chart.yaml") || file_path.ends_with("Chart.yml");
    let is_tpl = file_path.ends_with(".tpl");
    let is_templated_yaml = chart_root.is_some()
        && (file_path.ends_with(".yaml") || file_path.ends_with(".yml"))
        && file_path.contains("/templates/")
        && content.contains("{{");
    is_chart_yaml || is_tpl || is_templated_yaml
}

/// Lex the whole file and fold every non-blank action's facts together.
fn collect_facts(content: &str, self_module: &str, chart_name: &str) -> Facts {
    let all_tokens = lex(content);
    let mut facts = Facts::default();

    let mut i = 0;
    while i < all_tokens.len() {
        if let Tok::ActionContent(action_content) = &all_tokens[i] {
            // Only parse if there's actual content (skip blank actions).
            if !action_content.trim().is_empty() {
                let file_facts = parse_action(action_content, self_module, chart_name);
                merge_facts(&mut facts, file_facts);
            }
        }
        i += 1;
    }
    facts
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

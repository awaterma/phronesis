//! The YAML sensor: derives structural edges from one YAML source file.
//!
//! Uses `serde_norway` for parsing (multi-document support), regex extraction
//! for Helm template exclusion and anchor/alias pattern matching.
//!
//! Anchor/alias references are intra-document — they do not create cross-file
//! import edges. Alias references are detected and validated against anchor
//! definitions; missing anchors are counted, not guessed. (spec §4.1, shared
//! graph contract: "An alias references an anchor in the same YAML document.
//! Validate and count it, but do not emit imports(module, module).")
//!
//! JSON Schema keywords trigger schema-aware extraction shared with the JSON dialect: $defs /
//! definitions map to graph_definition + defines + element_in_file +
//! element_in_module, and $anchor maps to the same four relations. Object
//! properties are not promoted to stable graph definitions.

use super::extract::Extracted;
use super::model::Edge;
use super::unit::UnitContext;
use std::collections::BTreeSet;
use std::sync::LazyLock;

/// Regex matching Helm3 template delimiters. If a file contains this pattern
/// it is Helm template source — owned by the Helm3 extractor.
static HELM_TEMPLATE_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\{\{").expect("static regex compiles"));

/// Regex matching a YAML anchor definition: `&name` where name is `[a-zA-Z_][a-zA-Z0-9_-]*`
/// followed by any non-name character. Group 1 = full match, Group 2 = anchor name.
/// Matches `&name:` (block key), `&name,` or `&name]` or `&name}` (flow),
/// or `&name<ws>` (inline value like `&a value`), or `&name\n` (end of line).
static ANCHOR_DEF_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(&([_a-zA-Z][_a-zA-Z0-9-]*)(?:\s*:|[,\]\}\r\n ]))")
        .expect("static regex compiles")
});

/// Regex matching a YAML alias reference (`*name`).
static ANCHOR_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\*([_a-zA-Z][_a-zA-Z0-9-]*)").expect("static regex compiles")
});

/// Preserve byte offsets while hiding quoted scalar content and comments.
/// YAML aliases and anchors are syntax only outside quoted scalars.
fn mask_quoted_yaml(content: &str) -> String {
    let bytes = content.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut quote = None;
    let mut i = 0;
    while i < bytes.len() {
        let byte = bytes[i];
        if byte == b'\n' {
            quote = None;
            out.push(byte);
            i += 1;
            continue;
        }
        if let Some(active) = quote {
            if active == b'\'' && byte == b'\'' && bytes.get(i + 1) == Some(&b'\'') {
                out.extend_from_slice(b"  ");
                i += 2;
                continue;
            }
            if byte == active {
                quote = None;
            }
            out.push(b' ');
            i += 1;
            continue;
        }
        if byte == b'#' {
            while i < bytes.len() && bytes[i] != b'\n' {
                out.push(b' ');
                i += 1;
            }
            continue;
        }
        if byte == b'\'' || byte == b'"' {
            quote = Some(byte);
            out.push(b' ');
        } else {
            out.push(byte);
        }
        i += 1;
    }
    String::from_utf8(out).expect("mask preserves UTF-8 byte boundaries")
}

/// Regex matching JSON Schema `$anchor` values.
static SCHEMA_ANCHOR_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"\$anchor\s*:\s*['"]?([_a-zA-Z][_a-zA-Z0-9-]*)['"]?"#)
        .expect("static regex compiles")
});

/// Unsafe YAML tags that may cause unexpected code execution.
/// These are the YAML risk watchlist (starter pack rules).
const UNSAFE_TAGS: &[&str] = &[
    "!!javascript",
    "!ruby/object",
    "!!perl",
    "!!python",
    "!!hcl",
];

/// Classify a `.yaml` / `.yml` file as `test`, `example`, `build`, or
/// `production`.
///
/// Mirrors the Rust extractor's approach: test/fixture paths, examples,
/// recognized workflow/build manifests, and others.
fn classify_file(file_path: &str) -> &'static str {
    if file_path.starts_with("test/")
        || file_path.starts_with("tests/")
        || file_path.starts_with("fixture/")
        || file_path.starts_with("fixtures/")
        || file_path.starts_with("spec/")
        || file_path.starts_with("specs/")
        || file_path.contains("/test/")
        || file_path.contains("/tests/")
        || file_path.contains("/fixture/")
        || file_path.contains("/fixtures/")
        || file_path.contains("/spec/")
        || file_path.contains("/specs/")
    {
        return "test";
    }
    if file_path.starts_with("examples/") || file_path.contains("/examples/") {
        return "example";
    }
    // Recognized workflow/build manifests.
    if file_path.ends_with("Chart.yaml")
        || file_path.ends_with("Makefile.yaml")
        || file_path.ends_with("workflow.yaml")
        || file_path.ends_with("workflow.yml")
        || file_path.ends_with(".github/workflows/")
    {
        return "build";
    }
    "production"
}

/// Build the language-qualified module path for a `.yaml` / `.yml` file.
///
/// `src/config.yaml` with unit ID `yaml:project` →
/// `yaml:project::src::config`
fn compute_module_path(file_path: &str, unit: &UnitContext) -> String {
    // Strip the extension.
    let trimmed = file_path
        .strip_suffix(".yaml")
        .or_else(|| file_path.strip_suffix(".yml"))
        .unwrap_or(file_path);
    // Replace path separators with `::` and chain with the unit prefix.
    let segments: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
    std::iter::once(unit.id.as_str())
        .chain(segments)
        .collect::<Vec<_>>()
        .join("::")
}

/// Extract anchor definition names from the YAML content.
///
/// Returns a deduplicated set of anchor names.
fn extract_anchor_defs(content: &str) -> BTreeSet<String> {
    ANCHOR_DEF_RE
        .captures_iter(content)
        .filter_map(|cap| cap.get(2).map(|m| m.as_str().to_string()))
        .collect()
}

/// Count duplicate explicit mapping keys in the YAML content.
///
/// Uses serde_norway to parse each document. When serde_norway encounters
/// duplicate mapping keys it returns an error containing "duplicate".
/// Returns the total number of documents that contained at least one
/// duplicate key.
fn count_duplicate_keys(content: &str) -> usize {
    let docs: Vec<&str> = split_yaml_documents(content);
    let mut total = 0;
    for doc in &docs {
        let trimmed = doc.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_norway::from_str::<serde_norway::Value>(trimmed) {
            Err(e) => {
                if e.to_string().contains("duplicate") {
                    total += 1;
                }
            }
            Ok(serde_norway::Value::Mapping(_m)) => {
                if has_duplicate_keys_in_mapping(trimmed) {
                    total += 1;
                }
            }
            Ok(_) => {}
        }
    }
    total
}

/// Detect duplicate mapping keys by scanning the raw YAML text.
///
/// YAML mappings use `key: value` syntax at a given indentation level.
/// This function walks the content line by line, tracking keys at each
/// indent level using a stack. A key repeated at the same level counts
/// as a duplicate.
fn has_duplicate_keys_in_mapping(content: &str) -> bool {
    // Stack of sets: each level gets its own key set.
    // Top level (indent 0) is always present.
    let mut stack: Vec<std::collections::HashSet<String>> = Vec::new();
    let mut indents: Vec<usize> = Vec::new();
    stack.push(std::collections::HashSet::new());
    indents.push(0);
    let mut block_scalar_indent: Option<usize> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if let Some(parent_indent) = block_scalar_indent {
            if indent > parent_indent {
                continue;
            }
            block_scalar_indent = None;
        }
        if trimmed.starts_with('#') || trimmed.starts_with("---") || trimmed.starts_with("...") {
            continue;
        }

        // A block-sequence item starts a fresh node. Mapping keys in sibling
        // items belong to different mappings even though they share the same
        // indentation (`- id: ...`). Reset scopes owned by the prior item,
        // then treat an inline key after `-` as belonging one level deeper.
        let (mapping_indent, mapping_text) = if let Some(rest) = trimmed
            .strip_prefix('-')
            .filter(|rest| rest.chars().next().is_none_or(char::is_whitespace))
        {
            // Drop the prior item's deeper scopes, but preserve the enclosing
            // mapping at the sequence indicator's own indentation. This is
            // required for YAML's valid indentless-sequence form:
            // `rules:\n- id: ...`.
            while indents.last().is_some_and(|top| *top > indent) {
                indents.pop();
                stack.pop();
            }
            // Block sequence indicators occupy `- `, so an inline mapping key
            // shares the scope of following keys conventionally indented two
            // columns beneath the dash.
            let item_indent = indent.saturating_add(2);
            indents.push(item_indent);
            stack.push(std::collections::HashSet::new());
            (item_indent, rest.trim_start())
        } else {
            (indent, trimmed)
        };

        // Pop levels that are strictly above current indent.
        // We keep levels at the current indent (they may contain more keys).
        while let Some(&top) = indents.last() {
            if top <= mapping_indent {
                break;
            }
            indents.pop();
            stack.pop();
        }

        // If this line is indented deeper than the top of stack, push a new level.
        if let Some(&top) = indents.last()
            && mapping_indent > top
        {
            indents.push(mapping_indent);
            stack.push(std::collections::HashSet::new());
        }

        // Check if this line is a mapping key.
        if let Some(colon_pos) = find_mapping_key_colon(mapping_text) {
            let key_str = mapping_text[..colon_pos].trim().to_string();
            if key_str.is_empty() || key_str == "{" || key_str == "}" {
                continue;
            }
            // Malformed or future scanner states must not crash a whole graph
            // rebuild. The top-level scope should always exist, but treat a
            // violated invariant as unclassifiable rather than panicking.
            let Some(keys) = stack.last_mut() else {
                continue;
            };
            if keys.contains(&key_str) {
                return true; // Duplicate found!
            }
            keys.insert(key_str);
            let value = mapping_text[colon_pos + 1..].trim_start();
            if value.starts_with('|') || value.starts_with('>') {
                block_scalar_indent = Some(mapping_indent);
            }
        }
    }
    false
}

/// Find the position of the colon that separates a YAML mapping key from its value.
///
/// Returns the index of `:` in the trimmed line, or None if it's not a mapping key
/// (e.g., it's inside a quoted string or it's a value separator like `key: |`).
fn find_mapping_key_colon(line: &str) -> Option<usize> {
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut escape = false;

    for (i, ch) in line.char_indices() {
        if escape {
            escape = false;
            continue;
        }
        if ch == '\\' && in_double_quote {
            escape = true;
            continue;
        }
        if ch == '"' && !in_single_quote {
            in_double_quote = !in_double_quote;
            continue;
        }
        if ch == '\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
            continue;
        }
        if !in_single_quote && !in_double_quote && ch == ':' {
            // Check that what follows is a space, end of line, or flow punctuation.
            let rest = &line[i + 1..];
            if rest.is_empty()
                || rest.starts_with(' ')
                || rest.starts_with('\t')
                || rest.starts_with(',')
                || rest.starts_with('}')
                || rest.starts_with(']')
            {
                return Some(i);
            }
        }
    }
    None
}

/// Split YAML content into individual document strings at `---` boundaries.
///
/// Handles:
/// - Leading `---` (document start marker)
/// - `---` between documents
/// - Trailing `---` (document end marker)
fn split_yaml_documents(content: &str) -> Vec<&str> {
    let mut docs = Vec::new();
    let mut start = 0;
    let mut offset = 0;
    for segment in content.split_inclusive('\n') {
        let line = segment.trim_end_matches(['\r', '\n']);
        if line.trim() == "---" {
            docs.push(&content[start..offset]);
            start = offset + segment.len();
        }
        offset += segment.len();
    }
    docs.push(&content[start..]);
    docs
}

/// Check whether any document in the content contains JSON Schema keywords.
///
/// Returns true if the content contains `$schema`, `$ref`, `$defs`,
/// `definitions`, `properties`, or `items` keys.
fn has_schema_keywords(content: &str) -> bool {
    // Check for key JSON Schema indicators.
    let keywords = [
        "$schema",
        "$ref",
        "$defs",
        "definitions",
        "properties",
        "items",
    ];
    keywords
        .iter()
        .any(|kw| content.contains(&format!("{kw}:")) || content.contains(&format!("{kw} :")))
}

/// Extract schema entity names from YAML content using regex.
///
/// Collect stable JSON Schema resources hosted in YAML.
/// Deserialize every document in a multi-document stream, one result per
/// document. Returns `None` when the parser panics instead of erroring:
/// serde_norway 0.9 panics with "unexpected end of mapping" when an undefined
/// alias is the value of a mapping's final key, and a hook must degrade that
/// to "unparseable" rather than abort.
///
/// Do not collect `serde_norway::Deserializer` directly. For some malformed
/// inputs its document iterator repeatedly yields the same error without
/// advancing, so `collect()` never returns. Splitting the stream ourselves and
/// invoking the single-document parser gives each input a finite amount of
/// parser work and preserves the same per-document result contract.
fn parse_documents(content: &str) -> Option<Vec<Result<serde_norway::Value, String>>> {
    std::panic::catch_unwind(|| {
        split_yaml_documents(content)
            .into_iter()
            .filter(|document| !document.trim().is_empty())
            .map(|document| {
                serde_norway::from_str::<serde_norway::Value>(document)
                    .map_err(|error| error.to_string())
            })
            .collect()
    })
    .ok()
}

fn extract_schema_entities(content: &str) -> BTreeSet<String> {
    let mut entities = BTreeSet::new();

    fn collect(value: &serde_norway::Value, entities: &mut BTreeSet<String>) {
        match value {
            serde_norway::Value::Mapping(mapping) => {
                for (key, value) in mapping {
                    if matches!(key.as_str(), Some("$defs" | "definitions"))
                        && let serde_norway::Value::Mapping(definitions) = value
                    {
                        for name in definitions.keys().filter_map(serde_norway::Value::as_str) {
                            entities.insert(name.to_string());
                        }
                    }
                    collect(value, entities);
                }
            }
            serde_norway::Value::Sequence(sequence) => {
                for value in sequence {
                    collect(value, entities);
                }
            }
            _ => {}
        }
    }

    for value in parse_documents(content).into_iter().flatten().flatten() {
        collect(&value, &mut entities);
    }

    // $anchor values
    for cap in SCHEMA_ANCHOR_RE.captures_iter(content) {
        if let Some(name) = cap.get(1) {
            entities.insert(name.as_str().to_string());
        }
    }

    entities
}

/// Extract every base relation from one YAML file.
///
/// Handles multi-document streams (YAML 1.2 supports `---` separators),
/// Helm template exclusion, duplicate key detection, and JSON Schema
/// keyword detection.
pub fn extract_yaml(file_path: &str, content: &str, unit: &UnitContext) -> Extracted {
    // Only process .yaml / .yml files.
    if !file_path.ends_with(".yaml") && !file_path.ends_with(".yml") {
        return Extracted::default();
    }

    // Empty / whitespace-only content contributes no edges.
    if content.trim().is_empty() {
        return Extracted::unparseable();
    }

    // Helm template exclusion: files under a `templates/` directory that
    // contain Go template syntax (`{{`) are Helm3 template source, not
    // pure YAML. Return empty (not unparseable — owned by Helm3 extractor).
    if is_helm_template(file_path, content) {
        return Extracted {
            edges: Vec::new(),
            skipped: 0,
            parse_failed: false,
        };
    }

    let mut edges = std::collections::BTreeMap::new();
    let mut skipped = 0;
    let mut parse_failed = false;
    for (ordinal, document) in split_yaml_documents(content).into_iter().enumerate() {
        if document.trim().is_empty() {
            continue;
        }
        let extracted = extract_yaml_document(file_path, document, unit, ordinal);
        skipped += extracted.skipped;
        parse_failed |= extracted.parse_failed;
        for edge in extracted.edges {
            edges.insert((edge.p.clone(), edge.a.clone()), edge);
        }
    }
    Extracted {
        edges: edges.into_values().collect(),
        skipped,
        parse_failed,
    }
}

fn extract_yaml_document(
    file_path: &str,
    content: &str,
    unit: &UnitContext,
    document_ordinal: usize,
) -> Extracted {
    // Classify the file type.
    let ft = classify_file(file_path);

    // Build the language-qualified module path.
    let module_path = format!(
        "{}::doc:{document_ordinal}",
        compute_module_path(file_path, unit)
    );

    // Extract anchor names (definitions and references).
    let syntax = mask_quoted_yaml(content);
    let anchor_defs = extract_anchor_defs(&syntax);
    let undefined_aliases: BTreeSet<String> = ANCHOR_RE
        .captures_iter(&syntax)
        .filter_map(|alias| {
            let name = alias.get(1)?.as_str();
            let alias_offset = alias.get(0)?.start();
            let remainder = &syntax[alias.get(0)?.end()..];
            let scalar_tail = remainder
                .split(['\n', ',', ']', '}', '#'])
                .next()
                .unwrap_or("");
            if scalar_tail.contains('*') {
                return None;
            }
            let defined_earlier =
                ANCHOR_DEF_RE
                    .captures_iter(&syntax[..alias_offset])
                    .any(|anchor| {
                        anchor
                            .get(2)
                            .is_some_and(|candidate| candidate.as_str() == name)
                    });
            (!defined_earlier).then(|| name.to_string())
        })
        .collect();

    // Count duplicate keys.
    let dup_count = count_duplicate_keys(content);

    // Reject malformed YAML before asserting structural evidence. Duplicate
    // mappings are handled below so they can retain their dedicated
    // diagnostic fact rather than collapsing into a generic parse failure.
    // Likewise, an undefined alias already has its dedicated diagnostic
    // (and trips the serde_norway panic when it is a trailing value), so the
    // validation pass is skipped when the alias scan found one.
    if dup_count == 0 && undefined_aliases.is_empty() {
        let Some(documents) = parse_documents(content) else {
            return Extracted::unparseable();
        };
        for error in documents.into_iter().filter_map(Result::err) {
            // An unknown/forward anchor has a dedicated diagnostic below;
            // retain that evidence instead of collapsing it into a generic
            // parse failure.
            if !error.to_ascii_lowercase().contains("anchor") {
                return Extracted::unparseable();
            }
        }
    }

    // Schema detection.
    let _has_schema = has_schema_keywords(content);
    let schema_entities = extract_schema_entities(content);

    // Build deduplicated edge set.
    let mut out: BTreeSet<(String, Vec<String>)> = BTreeSet::new();

    // Always emit file_type and declares_module.
    out.insert((
        "file_type".to_string(),
        vec![file_path.to_string(), ft.to_string()],
    ));
    out.insert((
        "declares_module".to_string(),
        vec![file_path.to_string(), module_path.clone()],
    ));

    // Anchor definitions → graph_definition + defines + element_in_file + element_in_module.
    for anchor in &anchor_defs {
        let qualified = format!("{module_path}.{anchor}");
        out.insert(("graph_definition".to_string(), vec![qualified.clone()]));
        out.insert((
            "defines".to_string(),
            vec![file_path.to_string(), qualified.clone()],
        ));
        out.insert((
            "element_in_file".to_string(),
            vec![qualified.clone(), file_path.to_string()],
        ));
        out.insert((
            "element_in_module".to_string(),
            vec![qualified, module_path.clone()],
        ));
    }

    // Schema entities → graph_definition + defines + element_in_file + element_in_module.
    for entity in &schema_entities {
        let qualified = format!("{module_path}.{entity}");
        out.insert(("graph_definition".to_string(), vec![qualified.clone()]));
        out.insert((
            "defines".to_string(),
            vec![file_path.to_string(), qualified.clone()],
        ));
        out.insert((
            "element_in_file".to_string(),
            vec![qualified.clone(), file_path.to_string()],
        ));
        out.insert((
            "element_in_module".to_string(),
            vec![qualified, module_path.clone()],
        ));
    }

    // Alias references → validate against anchor definitions.
    // Undefined aliases become yaml_undefined_alias diagnostic edges.
    for alias in &undefined_aliases {
        out.insert((
            "yaml_undefined_alias".to_string(),
            vec![
                file_path.to_string(),
                module_path.clone(),
                alias.to_string(),
            ],
        ));
    }

    // Detect YAML merge keys (<<) → yaml_merge_key diagnostic.
    let merge_re = regex::Regex::new(r"(?:^|\s)<<(?::\s|\s|\s*\{)").expect("static regex compiles");
    for cap in merge_re.captures_iter(content) {
        let line_num = cap.get(0).map_or(0, |m| {
            let before = &content[..m.start()];
            before.lines().count()
        });
        out.insert((
            "yaml_merge_key".to_string(),
            vec![file_path.to_string(), line_num.to_string()],
        ));
    }

    // Unsafe YAML tags → calls_api edges (risk watchlist).
    let tag_re = regex::Regex::new(r"(?:^|\s)(!!\w+|!\w+/object|!ruby/\w+|!!hcl)\b")
        .expect("static regex compiles");
    let mut seen_tags = BTreeSet::new();
    for cap in tag_re.captures_iter(content) {
        if let Some(tag_match) = cap.get(1) {
            let tag = tag_match.as_str().trim();
            if UNSAFE_TAGS.iter().any(|safe| tag.starts_with(safe))
                && seen_tags.insert(tag.to_string())
            {
                out.insert((
                    "calls_api".to_string(),
                    vec![module_path.clone(), tag.to_string()],
                ));
            }
        }
    }

    // Duplicate keys make mapping structure ambiguous, so retain only the
    // dedicated diagnostic evidence. This is a successful classification,
    // not a generic parse failure: persisting it lets audit and hooks report
    // the exact defect while discarding stale structural claims.
    if dup_count > 0 {
        // Emit one edge per document that had duplicates.
        let docs: Vec<&str> = split_yaml_documents(content);
        for (i, doc) in docs.iter().enumerate() {
            let trimmed = doc.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_norway::from_str::<serde_norway::Value>(trimmed) {
                Err(e) if e.to_string().contains("duplicate") => {
                    // Find the duplicate key name from the error.
                    let err_str = e.to_string();
                    let key_name = err_str.split('"').nth(1).unwrap_or("key");
                    out.insert((
                        "yaml_duplicate_key".to_string(),
                        vec![file_path.to_string(), key_name.to_string(), i.to_string()],
                    ));
                }
                Ok(serde_norway::Value::Mapping(_m)) if has_duplicate_keys_in_mapping(trimmed) => {
                    out.insert((
                        "yaml_duplicate_key".to_string(),
                        vec![file_path.to_string(), "unknown".to_string(), i.to_string()],
                    ));
                }
                _ => {}
            }
        }
        out.retain(|(predicate, _)| predicate == "yaml_duplicate_key");
        return Extracted {
            edges: out
                .into_iter()
                .map(|(p, a)| Edge {
                    p,
                    a,
                    src: file_path.to_string(),
                    d: false,
                })
                .collect(),
            skipped: dup_count,
            parse_failed: false,
        };
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

/// True when `file_path` is under a `templates/` directory AND `content`
/// contains Go template syntax (`{{`).
///
/// Such files are Helm3 template source — owned by the Helm3 extractor —
/// and should not be parsed as plain YAML.
fn is_helm_template(file_path: &str, content: &str) -> bool {
    // Check if the file is under a `templates/` directory.
    let under_templates = file_path.split('/').any(|seg| seg == "templates");
    // Check if the file contains Go template syntax.
    let has_go_template = HELM_TEMPLATE_RE.is_match(content);
    under_templates && has_go_template
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::unit::{LANG_YAML, UnitContext};

    fn ctx() -> UnitContext {
        UnitContext::unnamed_for(LANG_YAML)
    }

    fn edges_of(x: &Extracted, p: &str) -> Vec<Vec<String>> {
        x.edges
            .iter()
            .filter(|e| e.p == p)
            .map(|e| e.a.clone())
            .collect()
    }

    // ─── file filtering ─────────────────────────────────────────────

    #[test]
    fn non_yaml_files_return_empty() {
        let out = extract_yaml("foo.py", "pass\n", &ctx());
        assert!(out.edges.is_empty());
    }

    #[test]
    fn empty_content_returns_unparseable() {
        let out = extract_yaml("foo.yaml", "", &ctx());
        assert!(out.parse_failed);
        assert!(out.edges.is_empty());
    }

    #[test]
    fn whitespace_only_content_returns_unparseable() {
        let out = extract_yaml("foo.yaml", "   \n  \n  ", &ctx());
        assert!(out.parse_failed);
    }

    #[test]
    fn malformed_yaml_returns_unparseable_without_graph_evidence() {
        let out = extract_yaml("config.yaml", "items: [one, two\n", &ctx());
        assert!(out.parse_failed);
        assert!(out.edges.is_empty());
    }

    // ─── module path ────────────────────────────────────────────────

    #[test]
    fn module_path_strips_yaml_extension() {
        let out = extract_yaml("src/config.yaml", "key: value\n", &ctx());
        let decls = edges_of(&out, "declares_module");
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0][0], "src/config.yaml");
        assert!(decls[0][1].ends_with("config::doc:0"));
    }

    #[test]
    fn module_path_strips_yml_extension() {
        let out = extract_yaml("src/config.yml", "key: value\n", &ctx());
        let decls = edges_of(&out, "declares_module");
        assert_eq!(decls.len(), 1);
        assert!(decls[0][1].ends_with("config::doc:0"));
    }

    #[test]
    fn a_stream_emits_one_module_per_document_and_scopes_anchors() {
        let content = "first: &shared one\n---\nsecond: &shared two\n";
        let out = extract_yaml("config.yaml", content, &ctx());
        let modules = edges_of(&out, "declares_module");
        assert_eq!(modules.len(), 2);
        assert!(modules.iter().any(|args| args[1].ends_with("::doc:0")));
        assert!(modules.iter().any(|args| args[1].ends_with("::doc:1")));
        let definitions = edges_of(&out, "graph_definition");
        assert!(definitions.iter().any(|args| args[0].contains("doc:0")));
        assert!(definitions.iter().any(|args| args[0].contains("doc:1")));
    }

    #[test]
    fn module_path_replaces_slashes() {
        let out = extract_yaml("src/utils/helpers.yaml", "key: value\n", &ctx());
        let decls = edges_of(&out, "declares_module");
        assert_eq!(decls.len(), 1);
        assert!(decls[0][1].contains("::"));
        assert!(decls[0][1].contains("helpers"));
    }

    // ─── file_type classification ───────────────────────────────────

    #[test]
    fn test_path_is_classified_as_test() {
        let out = extract_yaml("tests/fixtures/data.yaml", "key: value\n", &ctx());
        assert!(
            out.edges
                .iter()
                .any(|e| e.p == "file_type" && e.a[1] == "test")
        );
    }

    #[test]
    fn production_is_default() {
        let out = extract_yaml("src/config.yaml", "key: value\n", &ctx());
        assert!(
            out.edges
                .iter()
                .any(|e| e.p == "file_type" && e.a[1] == "production")
        );
    }

    #[test]
    fn examples_are_classified_as_example() {
        let out = extract_yaml("examples/demo.yaml", "key: value\n", &ctx());
        assert!(
            out.edges
                .iter()
                .any(|e| e.p == "file_type" && e.a[1] == "example")
        );
    }

    // ─── anchor extraction ──────────────────────────────────────────

    #[test]
    fn anchor_definitions_become_graph_definition() {
        let content = "default: &defaults\n  timeout: 30\n";
        let out = extract_yaml("src/config.yaml", content, &ctx());
        let defs = edges_of(&out, "defines");
        assert!(defs.iter().any(|a| a[1].contains("defaults")));
    }

    #[test]
    fn alias_references_are_intra_document_not_imports() {
        let content = "
default: &defaults
  timeout: 30
override: *defaults
";
        let out = extract_yaml("src/config.yaml", content, &ctx());
        assert!(edges_of(&out, "imports").is_empty());
    }

    #[test]
    fn an_alias_cannot_reference_an_anchor_declared_later() {
        let content = "early: *later\nlate: &later value\n";
        let out = extract_yaml("src/config.yaml", content, &ctx());
        let invalid = edges_of(&out, "yaml_undefined_alias");
        assert_eq!(invalid.len(), 1);
        assert_eq!(invalid[0][2], "later");
    }

    #[test]
    fn markdown_emphasis_inside_quoted_scalars_is_not_a_yaml_alias() {
        let content = "categories:\n  - - '*Ale*'\n    - \"*Ammunition*\"\n";
        let out = extract_yaml("config/manifest.yaml", content, &ctx());
        assert!(edges_of(&out, "yaml_undefined_alias").is_empty());
    }

    #[test]
    fn markdown_emphasis_inside_plain_scalars_is_not_a_yaml_alias() {
        let content = "items:\n  - *Ioun stone*\n  - *beads of force*\n";
        let out = extract_yaml("config/manifest.yaml", content, &ctx());
        assert!(edges_of(&out, "yaml_undefined_alias").is_empty());
    }

    #[test]
    fn a_real_unquoted_alias_is_still_reported() {
        let out = extract_yaml(
            "config/manifest.yaml",
            "value: *later\nlater: &later 1\n",
            &ctx(),
        );
        assert_eq!(
            edges_of(&out, "yaml_undefined_alias"),
            vec![vec![
                "config/manifest.yaml".to_string(),
                "yaml:project::config::manifest::doc:0".to_string(),
                "later".to_string()
            ]]
        );
    }

    // ─── Helm template exclusion ────────────────────────────────────

    #[test]
    fn helm_templates_under_templates_dir_are_excluded() {
        let content = "apiVersion: apps/v1\n{{- range .Values.pods }}\n{{ end }}\n";
        let out = extract_yaml("templates/deployment.yaml", content, &ctx());
        assert!(out.edges.is_empty());
        assert!(!out.parse_failed);
    }

    #[test]
    fn yaml_under_templates_but_no_go_syntax_is_not_excluded() {
        let content = "key: value\n";
        let out = extract_yaml("templates/config.yaml", content, &ctx());
        // Should NOT be excluded — no Go template syntax.
        assert!(out.edges.iter().any(|e| e.p == "file_type"));
    }

    #[test]
    fn go_template_outside_templates_dir_is_not_excluded() {
        let content = "key: {{ .Values.foo }}\n";
        let out = extract_yaml("src/template.yaml", content, &ctx());
        // Should NOT be excluded — not under templates/ directory.
        assert!(out.edges.iter().any(|e| e.p == "file_type"));
    }

    // ─── schema detection ───────────────────────────────────────────

    #[test]
    fn json_schema_defines_fn_for_defs() {
        let content =
            "$schema: http://json-schema.org/draft-07/schema#\n$defs:\n  Foo:\n    type: object\n";
        let out = extract_yaml("src/schema.yaml", content, &ctx());
        // $defs block is detected; anchor names inside would be extracted
        // via parsing. At minimum, the file should parse successfully.
        assert!(!out.parse_failed);
    }

    #[test]
    fn json_schema_defines_fn_for_anchor() {
        let content = "$schema: http://json-schema.org/draft-07/schema#\n$anchor: MyAnchor\n";
        let out = extract_yaml("src/schema.yaml", content, &ctx());
        let defs = edges_of(&out, "defines");
        assert!(defs.iter().any(|a| a[1].contains("MyAnchor")));
    }

    // ─── duplicate keys ─────────────────────────────────────────────

    #[test]
    fn duplicate_keys_counted() {
        let content = "
key: value1
key: value2
";
        let out = extract_yaml("src/config.yaml", content, &ctx());
        assert!(out.skipped > 0);
        assert!(!out.parse_failed);
        assert_eq!(edges_of(&out, "yaml_duplicate_key").len(), 1);
    }

    #[test]
    fn no_duplicate_keys_parses_cleanly() {
        let content = "
key1: value1
key2: value2
";
        let out = extract_yaml("src/config.yaml", content, &ctx());
        assert!(!out.parse_failed);
    }

    #[test]
    fn uniform_record_list_does_not_report_duplicate_keys_or_discard_structure() {
        let content = r#"
rules:
  - id: first_rule
    when: alpha
  - id: second_rule
    when: beta
"#;
        assert!(!has_duplicate_keys_in_mapping(content));
        let out = extract_yaml("config/rules.yaml", content, &ctx());
        assert_eq!(out.skipped, 0);
        assert!(edges_of(&out, "yaml_duplicate_key").is_empty());
        assert!(
            !out.edges.is_empty(),
            "valid record lists must retain graph edges"
        );
    }

    #[test]
    fn duplicate_key_within_one_sequence_item_is_reported() {
        let content = r#"
rules:
  - id: first_rule
    id: shadowed_rule
  - id: second_rule
"#;
        assert!(has_duplicate_keys_in_mapping(content));
        let out = extract_yaml("config/rules.yaml", content, &ctx());
        assert!(out.skipped > 0);
        assert_eq!(edges_of(&out, "yaml_duplicate_key").len(), 1);
    }

    #[test]
    fn nested_record_lists_have_independent_item_scopes() {
        let content = r#"
groups:
  - name: first
    rules:
      - id: one
        when: alpha
      - id: two
        when: beta
  - name: second
    rules:
      - id: three
        when: gamma
"#;
        assert!(!has_duplicate_keys_in_mapping(content));
        let out = extract_yaml("config/nested.yaml", content, &ctx());
        assert_eq!(out.skipped, 0);
        assert!(edges_of(&out, "yaml_duplicate_key").is_empty());
    }

    #[test]
    fn cue_style_indentless_record_list_preserves_enclosing_scope() {
        let content = r#"
rules:
- id: first_rule
  when: alpha
- id: second_rule
  when: beta
metadata:
  version: 1
"#;
        assert!(!has_duplicate_keys_in_mapping(content));
        let out = extract_yaml("config/srd_rete_rules.yaml", content, &ctx());
        assert!(!out.parse_failed);
        assert_eq!(out.skipped, 0);
        assert!(edges_of(&out, "yaml_duplicate_key").is_empty());
        assert!(!out.edges.is_empty());
    }

    #[test]
    fn root_sequence_of_records_does_not_empty_the_scope_stack() {
        let content = r#"
- id: first_rule
  when: alpha
- id: second_rule
  when: beta
"#;
        assert!(!has_duplicate_keys_in_mapping(content));
        let out = extract_yaml("config/rules.yaml", content, &ctx());
        assert!(!out.parse_failed);
        assert_eq!(out.skipped, 0);
        assert!(edges_of(&out, "yaml_duplicate_key").is_empty());
    }

    #[test]
    fn nested_indentless_actions_sequence_keeps_parent_item_scope() {
        let content = r#"
rules:
- id: first_rule
  when: alpha
  actions:
  - type: warn
    message: first
  - type: log
    message: second
- id: second_rule
  when: beta
  actions:
  - type: block
    message: third
"#;
        assert!(!has_duplicate_keys_in_mapping(content));
        let out = extract_yaml("config/srd_rete_rules.yaml", content, &ctx());
        assert!(!out.parse_failed);
        assert_eq!(out.skipped, 0);
        assert!(edges_of(&out, "yaml_duplicate_key").is_empty());
        assert!(!out.edges.is_empty());
    }

    #[test]
    fn block_scalar_prompt_content_is_not_scanned_as_mapping_keys() {
        let content = r#"
prompt: |
  {"action_type": "add_combatant", "target": "first"}
  {"action_type": "send_notification", "target": "second"}
  [RESOLVED: yes]
  [RESOLVED: no]
name: Waterman's Camp
folded: >-
  key: first
  key: repeated prose is still scalar text
version: 1
"#;
        assert!(!has_duplicate_keys_in_mapping(content));
        let out = extract_yaml("config/watermans_camp.yaml", content, &ctx());
        assert_eq!(out.skipped, 0);
        assert!(edges_of(&out, "yaml_duplicate_key").is_empty());
        assert!(!out.edges.is_empty());
    }

    // ─── unsafe tags ────────────────────────────────────────────────

    #[test]
    fn unsafe_tags_detected() {
        let content = "
js: !!javascript |
  function hello() { return 'hi' }
";
        let out = extract_yaml("src/config.yaml", content, &ctx());
        let apis = edges_of(&out, "calls_api");
        assert!(apis.iter().any(|a| a[1].contains("javascript")));
    }

    // ─── edge deduplication ─────────────────────────────────────────

    #[test]
    fn duplicate_edges_are_deduplicated() {
        let content = "
key1: &a val1
key2: &a val2
use: *a
";
        let out = extract_yaml("src/config.yaml", content, &ctx());
        // Both definitions of &a should produce one defines edge (BTreeSet dedup).
        let defs = edges_of(&out, "defines");
        let a_defs: Vec<_> = defs.iter().filter(|a| a[1].contains("a")).collect();
        assert_eq!(a_defs.len(), 1);
    }
}

#[cfg(test)]
mod yaml_document_tests {
    use super::*;
    use crate::graph::unit::{LANG_YAML, UnitContext};

    fn ctx() -> UnitContext {
        UnitContext::unnamed_for(LANG_YAML)
    }

    fn preds(x: &Extracted, p: &str) -> Vec<Vec<String>> {
        x.edges
            .iter()
            .filter(|e| e.p == p)
            .map(|e| e.a.clone())
            .collect()
    }

    // ── mask_quoted_yaml ──────────────────────────────────────────────

    #[test]
    fn mask_quoted_yaml_preserves_length_and_plain_syntax() {
        let input = "a: &anchor 1\nb: *anchor\n";
        let out = mask_quoted_yaml(input);
        assert_eq!(out.len(), input.len());
        assert_eq!(out, input);
    }

    #[test]
    fn mask_quoted_yaml_blanks_double_and_single_quoted_scalars() {
        let out = mask_quoted_yaml("a: \"*not-alias\"\nb: '&not-anchor'\n");
        assert!(!out.contains('*'));
        assert!(!out.contains('&'));
        assert!(!out.contains('"'));
        assert!(!out.contains('\''));
        assert_eq!(out.len(), "a: \"*not-alias\"\nb: '&not-anchor'\n".len());
        assert!(out.starts_with("a: "));
    }

    #[test]
    fn mask_quoted_yaml_handles_escaped_single_quote_inside_single_quotes() {
        // 'it''s *x' — the doubled quote does not terminate the scalar.
        let input = "k: 'it''s *x' *after\n";
        let out = mask_quoted_yaml(input);
        assert_eq!(out.len(), input.len());
        assert!(!out.contains("*x"), "inner alias should be masked");
        assert!(out.contains("*after"), "alias after the scalar survives");
    }

    #[test]
    fn mask_quoted_yaml_blanks_comments_to_end_of_line() {
        let input = "a: 1 # *alias &anchor\nb: *real\n";
        let out = mask_quoted_yaml(input);
        assert_eq!(out.len(), input.len());
        assert!(!out.contains('#'));
        assert!(!out.contains("&anchor"));
        assert!(out.contains("*real"));
    }

    #[test]
    fn mask_quoted_yaml_unterminated_quote_resets_at_newline() {
        let input = "a: \"open\nb: *alias\n";
        let out = mask_quoted_yaml(input);
        assert_eq!(out.len(), input.len());
        assert!(out.contains("*alias"));
    }

    #[test]
    fn mask_quoted_yaml_keeps_multibyte_utf8_intact() {
        let input = "name: \"héllo ☃\"\nx: *é\n";
        let out = mask_quoted_yaml(input);
        assert_eq!(out.len(), input.len());
        assert!(out.contains("x: *é"));
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
    }

    #[test]
    fn mask_quoted_yaml_empty_input() {
        assert_eq!(mask_quoted_yaml(""), "");
    }

    // ── extract_yaml_document ─────────────────────────────────────────

    #[test]
    fn extract_yaml_document_emits_file_type_and_ordinal_module() {
        let out = extract_yaml_document("config/app.yaml", "a: 1\n", &ctx(), 2);
        assert!(!out.parse_failed);
        let modules = preds(&out, "declares_module");
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0][0], "config/app.yaml");
        assert!(
            modules[0][1].ends_with("::config::app::doc:2"),
            "{:?}",
            modules[0]
        );
        assert!(!preds(&out, "file_type").is_empty());
    }

    #[test]
    fn extract_yaml_document_defines_anchors_and_flags_undefined_aliases() {
        let content = "base: &base\n  x: 1\nuse: *base\nbad: *missing\n";
        let out = extract_yaml_document("a.yaml", content, &ctx(), 0);
        let defs = preds(&out, "graph_definition");
        assert!(defs.iter().any(|d| d[0].ends_with(".base")), "{defs:?}");
        let undefined = preds(&out, "yaml_undefined_alias");
        assert_eq!(undefined.len(), 1, "{undefined:?}");
        assert!(undefined[0].iter().any(|v| v == "missing"));
    }

    #[test]
    fn extract_yaml_document_ignores_aliases_inside_quotes_and_comments() {
        let content = "a: \"*quoted\"\nb: 1 # *commented\n";
        let out = extract_yaml_document("a.yaml", content, &ctx(), 0);
        assert!(!out.parse_failed);
        assert!(preds(&out, "yaml_undefined_alias").is_empty());
    }

    #[test]
    fn extract_yaml_document_malformed_is_unparseable() {
        let out = extract_yaml_document("a.yaml", "a: [1, 2\nb: }\n", &ctx(), 0);
        assert!(out.parse_failed);
        assert!(out.edges.is_empty());
    }

    #[test]
    fn extract_yaml_document_forward_alias_keeps_diagnostic_instead_of_parse_failure() {
        // serde parses this as an unknown-anchor error; we retain the
        // undefined-alias diagnostic rather than collapsing to unparseable.
        let content = "use: *later\nlater: &later 1\n";
        let out = extract_yaml_document("a.yaml", content, &ctx(), 0);
        assert!(!out.parse_failed);
        let undefined = preds(&out, "yaml_undefined_alias");
        assert!(
            undefined.iter().any(|u| u.iter().any(|v| v == "later")),
            "{undefined:?}"
        );
    }

    #[test]
    fn extract_yaml_document_duplicate_keys_keep_only_their_diagnostic() {
        let content = "a: 1\na: 2\n";
        let out = extract_yaml_document("a.yaml", content, &ctx(), 0);
        assert!(
            !out.parse_failed,
            "dup keys keep their dedicated diagnostic"
        );
        assert_eq!(out.skipped, 1);
        let dups = preds(&out, "yaml_duplicate_key");
        assert_eq!(dups.len(), 1, "{dups:?}");
        assert_eq!(dups[0][1], "a");
        assert!(out.edges.iter().all(|e| e.p == "yaml_duplicate_key"));
    }

    /// Regression: serde_norway 0.9 panics when an undefined alias is the
    /// value of a mapping's last key. The extractor must not propagate that.
    #[test]
    fn extract_yaml_document_trailing_undefined_alias_does_not_panic() {
        for content in [
            "bad: *missing\n",
            "x: 1\nbad: *missing\n",
            "base: &base 1\nbad: *missing\n",
        ] {
            let out = extract_yaml_document("a.yaml", content, &ctx(), 0);
            assert!(!out.parse_failed, "{content:?}");
            let undefined = preds(&out, "yaml_undefined_alias");
            assert!(
                undefined.iter().any(|u| u.iter().any(|v| v == "missing")),
                "{content:?}: {undefined:?}"
            );
        }
    }
}

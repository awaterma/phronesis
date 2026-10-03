//! Every JSON rule example in the docs must pass the load-time validator.
//!
//! Rule examples get copied into real `rules.json` files, and the loader
//! fails closed (decision D1): an example with an unknown key or verb would
//! make every hook in the copying project block. This scans fenced ```json
//! blocks in `docs/**/*.md`, the crate guides, README and AGENTS, and loads
//! each block that looks like a rules file or a single rule (a block may hold
//! several JSON values). A rule-shaped block that is not valid JSON fails the
//! test. A block whose preceding line is `<!-- rule-example: proposed -->` is
//! skipped: it shows a proposed field, or a deliberate sketch, the loader
//! does not accept.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn markdown_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            markdown_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
}

fn is_rule(v: &Value) -> bool {
    v.get("id").is_some() && (v.get("when").is_some() || v.get("conditions").is_some())
}

/// A rules-file-shaped value for a fenced block, or `None` when the block
/// is not a rule example.
fn as_rules_file(v: Value) -> Option<Value> {
    if let Some(items) = v.get("rules").and_then(Value::as_array) {
        // Other payloads embed a top-level `"rules"` array too: `phr-mcp
        // audit --json` reports `{totals, rules: [{rule_id, level, hits,
        // files}]}`, and plan docs pin that real shape. Only a value with
        // at least one rule-shaped entry is a rules-file example; the
        // loader still rejects malformed sibling entries inside it.
        if !items.iter().any(is_rule) {
            return None;
        }
        return Some(v);
    }
    if is_rule(&v) {
        return Some(json!({"rules": [v]}));
    }
    if let Value::Array(items) = &v
        && !items.is_empty()
        && items.iter().all(is_rule)
    {
        return Some(json!({"rules": v}));
    }
    None
}

/// Marker a spec puts on the line before a fence whose rule example uses a
/// field that is proposed but not implemented yet. Such an example cannot
/// load until the field joins the loader's allowed keys.
const PROPOSED: &str = "<!-- rule-example: proposed -->";

fn json_blocks(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut previous = "";
    let mut lines = text.lines().enumerate();
    while let Some((n, line)) = lines.next() {
        let fence = line.trim_start();
        let proposed = previous.trim() == PROPOSED;
        previous = line;
        if (fence == "```json" || fence == "```jsonc") && !proposed {
            let mut body = String::new();
            for (_, l) in lines.by_ref() {
                if l.trim_start().starts_with("```") {
                    break;
                }
                body.push_str(l);
                body.push('\n');
            }
            out.push((n + 1, body));
        }
    }
    out
}

/// Whether an unparseable block still looks like a rule example (it names an
/// id plus conditions). Such a block must parse: silently skipping it is how
/// broken examples went unnoticed.
fn looks_rule_shaped(block: &str) -> bool {
    block.contains("\"id\"") && (block.contains("\"when\"") || block.contains("\"conditions\""))
}

/// Parse a block as a stream of JSON values (several objects in one fence
/// are common in the docs).
fn parse_stream(block: &str) -> Option<Vec<Value>> {
    serde_json::Deserializer::from_str(block)
        .into_iter::<Value>()
        .collect::<Result<Vec<_>, _>>()
        .ok()
}

#[test]
fn every_documented_rule_example_loads() {
    let root = workspace_root();
    let mut files = Vec::new();
    markdown_files(&root.join("docs"), &mut files);
    markdown_files(&root.join("crates/phronesis-mcp/docs"), &mut files);
    for extra in [
        "README.md",
        "AGENTS.md",
        "crates/phronesis-mcp/CLAUDE.md",
        "crates/phronesis/README.md",
    ] {
        files.push(root.join(extra));
    }
    let tmp = tempfile::tempdir().expect("tmp");
    let path = tmp.path().join("rules.json");
    let mut checked = 0;
    let mut failures = Vec::new();
    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        for (line, block) in json_blocks(&text) {
            let Some(values) = parse_stream(&block) else {
                if looks_rule_shaped(&block) {
                    failures.push(format!(
                        "{}:{line}: rule example is not valid JSON (mark a deliberate sketch `{PROPOSED}`)",
                        file.display()
                    ));
                }
                continue;
            };
            for value in values {
                let Some(rules) = as_rules_file(value) else {
                    continue;
                };
                std::fs::write(&path, rules.to_string()).expect("write");
                checked += 1;
                if let Err(e) = phronesis_mcp::rules_file::read(&path) {
                    failures.push(format!("{}:{line}: {e}", file.display()));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "documented rule examples fail the loader:\n{}",
        failures.join("\n")
    );
    // Guard against the scan silently matching nothing.
    assert!(checked >= 15, "only {checked} rule examples found");
}

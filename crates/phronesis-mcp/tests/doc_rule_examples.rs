//! Every JSON rule example in the docs must pass the load-time validator.
//!
//! Rule examples get copied into real `rules.json` files, and the loader
//! fails closed (decision D1): an example with an unknown key or verb would
//! make every hook in the copying project block. This scans fenced ```json
//! blocks in `docs/**/*.md`, the crate guides, README and AGENTS, and loads
//! each block that looks like a rules file or a single rule. A block whose
//! preceding line is `<!-- rule-example: proposed -->` is skipped: it shows a
//! proposed field the loader does not accept yet.

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
    if v.get("rules").is_some_and(Value::is_array) {
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

#[test]
fn every_documented_rule_example_loads() {
    let root = workspace_root();
    let mut files = Vec::new();
    markdown_files(&root.join("docs"), &mut files);
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
            let Ok(value) = serde_json::from_str::<Value>(&block) else {
                continue; // prose-y pseudo-JSON with `…` placeholders
            };
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
    assert!(
        failures.is_empty(),
        "documented rule examples fail the loader:\n{}",
        failures.join("\n")
    );
    // Guard against the scan silently matching nothing.
    assert!(checked >= 15, "only {checked} rule examples found");
}

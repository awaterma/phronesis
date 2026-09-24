use anyhow::{Context, Result};
use std::collections::HashSet;
use tree_sitter::{Node, Parser};

pub fn function_region_id(f: &str) -> String {
    format!("fn:{f}")
}

pub fn branch_region_id(f: &str, anchor: &str) -> String {
    format!("branch:{f}:{anchor}")
}

pub struct BranchSite {
    pub function: String,
    pub anchor: String,
    pub start_line: u64,
    pub end_line: u64,
}

/// FNV-1a 64-bit over the condition source text, rendered as 12 lowercase
/// hex chars of the 16-hex digest. Identity is the condition text; the span
/// (below) is the whole `if_expression` so body edits flag the branch.
fn fnv1a_64(data: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn compute_anchor(condition: &str) -> String {
    let hash = fnv1a_64(condition.as_bytes());
    format!("{hash:016x}")[..12].to_string()
}

/// Depth-first collection of every node — `root.children()` alone only
/// visits top-level items, and `if_expression`s live inside function bodies.
fn all_nodes<'t>(root: Node<'t>) -> Vec<Node<'t>> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        out.push(node);
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }
    out
}

fn find_function_name(node: Node, source: &str) -> String {
    let mut current = node;
    while let Some(parent) = current.parent() {
        if parent.kind() == "function_item" {
            if let Some(name_node) = parent.child_by_field_name("name") {
                if let Ok(name) = name_node.utf8_text(source.as_bytes()) {
                    return name.to_string();
                }
            }
        }
        current = parent;
    }
    String::new()
}

fn parse(source: &str) -> Result<tree_sitter::Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .context("failed to set Rust language")?;
    parser.parse(source, None).context("failed to parse source")
}

/// Every `if_expression`, with the anchor hashed from its condition text and
/// the span covering the whole expression (so edits to the branch body count
/// as changes to the branch).
pub fn extract_branch_sites(source: &str) -> Result<Vec<BranchSite>> {
    let tree = parse(source)?;
    let mut sites = Vec::new();
    for node in all_nodes(tree.root_node()) {
        if node.kind() != "if_expression" {
            continue;
        }
        let Some(condition) = node.child_by_field_name("condition") else {
            continue;
        };
        let condition_text = condition.utf8_text(source.as_bytes()).unwrap_or_default();
        sites.push(BranchSite {
            anchor: compute_anchor(condition_text),
            function: find_function_name(node, source),
            start_line: node.start_position().row as u64 + 1,
            end_line: node.end_position().row as u64 + 1,
        });
    }
    Ok(sites)
}

fn extract_functions(source: &str) -> Result<Vec<(String, u64, u64)>> {
    let tree = parse(source)?;
    let mut functions = Vec::new();
    for node in all_nodes(tree.root_node()) {
        if node.kind() != "function_item" {
            continue;
        }
        if let Some(name_node) = node.child_by_field_name("name") {
            if let Ok(name) = name_node.utf8_text(source.as_bytes()) {
                functions.push((
                    name.to_string(),
                    node.start_position().row as u64 + 1,
                    node.end_position().row as u64 + 1,
                ));
            }
        }
    }
    Ok(functions)
}

/// LCS over lines; returns the 1-based line numbers absent from the LCS on
/// each side.
fn changed_lines(old: &str, new: &str) -> (HashSet<u64>, HashSet<u64>) {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let m = old_lines.len();
    let n = new_lines.len();

    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for i in 1..=m {
        for j in 1..=n {
            dp[i][j] = if old_lines[i - 1] == new_lines[j - 1] {
                dp[i - 1][j - 1] + 1
            } else {
                dp[i - 1][j].max(dp[i][j - 1])
            };
        }
    }

    let mut changed_old = HashSet::new();
    let mut changed_new = HashSet::new();
    let mut i = m;
    let mut j = n;
    while i > 0 && j > 0 {
        if old_lines[i - 1] == new_lines[j - 1] {
            i -= 1;
            j -= 1;
        } else if dp[i - 1][j] >= dp[i][j - 1] {
            changed_old.insert(i as u64);
            i -= 1;
        } else {
            changed_new.insert(j as u64);
            j -= 1;
        }
    }
    while i > 0 {
        changed_old.insert(i as u64);
        i -= 1;
    }
    while j > 0 {
        changed_new.insert(j as u64);
        j -= 1;
    }

    (changed_old, changed_new)
}

pub struct ChangedRegions {
    pub functions: Vec<String>,
    pub branches: Vec<String>,
}

/// Overlap policy: any changed line inside a function's or branch site's
/// span flags that region (SPEC-coverage-evidence §"region identity").
/// Branch mapping is skipped for inputs over 100_000 lines (documented cap).
pub fn changed_regions(old: &str, new: &str) -> Result<ChangedRegions> {
    let (changed_old, changed_new) = changed_lines(old, new);

    let mut functions = HashSet::new();
    for (name, start, end) in extract_functions(old)? {
        if (start..=end).any(|line| changed_old.contains(&line)) {
            functions.insert(function_region_id(&name));
        }
    }
    for (name, start, end) in extract_functions(new)? {
        if (start..=end).any(|line| changed_new.contains(&line)) {
            functions.insert(function_region_id(&name));
        }
    }

    let mut branches = HashSet::new();
    let too_big = old.lines().count() > 100_000 || new.lines().count() > 100_000;
    if !too_big {
        for site in extract_branch_sites(old)? {
            if (site.start_line..=site.end_line).any(|line| changed_old.contains(&line)) {
                branches.insert(branch_region_id(&site.function, &site.anchor));
            }
        }
        for site in extract_branch_sites(new)? {
            if (site.start_line..=site.end_line).any(|line| changed_new.contains(&line)) {
                branches.insert(branch_region_id(&site.function, &site.anchor));
            }
        }
    }

    Ok(ChangedRegions {
        functions: functions.into_iter().collect(),
        branches: branches.into_iter().collect(),
    })
}
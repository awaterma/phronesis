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
    // Whitespace-normalized before hashing: rustfmt reflowing a condition
    // (default-on in Rust workflows) must not orphan the anchor — the
    // semantic-preserving transformation survives, per the cross-revision
    // persistence requirement (review finding #5; full token-stream
    // normalization is the follow-up, operand reorder still re-anchors).
    let normalized: String = condition.split_whitespace().collect::<Vec<_>>().join(" ");
    let hash = fnv1a_64(normalized.as_bytes());
    format!("{hash:016x}")[..12].to_string()
}

/// Depth-first collection of every node — `root.children()` alone only
/// visits top-level items, and `if_expression`s live inside function bodies.
fn all_nodes<'t>(root: Node<'t>) -> Vec<Node<'t>> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        out.push(node);
        stack.extend(node.children(&mut node.walk()));
    }
    out
}

fn find_function_name(node: Node, source: &str) -> String {
    let mut current = node;
    while let Some(parent) = current.parent() {
        if parent.kind() == "function_item"
            && let Some(name_node) = parent.child_by_field_name("name")
            && let Ok(name) = name_node.utf8_text(source.as_bytes())
        {
            return name.to_string();
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
        if let Some(name_node) = node.child_by_field_name("name")
            && let Ok(name) = name_node.utf8_text(source.as_bytes())
        {
            functions.push((
                name.to_string(),
                node.start_position().row as u64 + 1,
                node.end_position().row as u64 + 1,
            ));
        }
    }
    Ok(functions)
}

/// One step of the LCS walk: a matched pair (old line, new line), a
/// deletion from old, or an insertion into new. Line numbers are 1-based;
/// only `Match` carries consumed line numbers — changed lines are derived
/// from the complement of the matched set.
enum WalkStep {
    Match(usize, usize),
    Delete,
    Insert,
}

struct LcsWalk<'a> {
    dp: &'a [Vec<usize>],
    old: &'a [&'a str],
    new: &'a [&'a str],
    i: usize,
    j: usize,
}

impl Iterator for LcsWalk<'_> {
    type Item = WalkStep;
    fn next(&mut self) -> Option<WalkStep> {
        if self.i > 0 && self.j > 0 {
            if self.old[self.i - 1] == self.new[self.j - 1] {
                let step = WalkStep::Match(self.i, self.j);
                self.i -= 1;
                self.j -= 1;
                return Some(step);
            }
            if self.dp[self.i - 1][self.j] >= self.dp[self.i][self.j - 1] {
                self.i -= 1;
                return Some(WalkStep::Delete);
            }
            self.j -= 1;
            return Some(WalkStep::Insert);
        }
        if self.i > 0 {
            self.i -= 1;
            return Some(WalkStep::Delete);
        }
        if self.j > 0 {
            self.j -= 1;
            return Some(WalkStep::Insert);
        }
        None
    }
}

fn lcs_table(old: &[&str], new: &[&str]) -> Vec<Vec<usize>> {
    let mut dp = vec![vec![0usize; new.len() + 1]; old.len() + 1];
    for (i, a) in old.iter().enumerate() {
        for (j, b) in new.iter().enumerate() {
            dp[i + 1][j + 1] = if a == b {
                dp[i][j] + 1
            } else {
                dp[i][j + 1].max(dp[i + 1][j])
            };
        }
    }
    dp
}

/// LCS over lines; returns the 1-based line numbers absent from the LCS on
/// each side (matched pairs from the walk are the complement of the change
/// set on each side).
fn changed_lines(old: &str, new: &str) -> (HashSet<u64>, HashSet<u64>) {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let steps: Vec<WalkStep> = LcsWalk {
        dp: &lcs_table(&old_lines, &new_lines),
        old: &old_lines,
        new: &new_lines,
        i: old_lines.len(),
        j: new_lines.len(),
    }
    .collect();
    let matched = steps
        .iter()
        .fold((HashSet::new(), HashSet::new()), |mut acc, step| {
            if let WalkStep::Match(a, b) = step {
                acc.0.insert(*a);
                acc.1.insert(*b);
            }
            acc
        });
    let changed_old = (1..=old_lines.len())
        .filter(|i| !matched.0.contains(i))
        .map(|i| i as u64)
        .collect();
    let changed_new = (1..=new_lines.len())
        .filter(|j| !matched.1.contains(j))
        .map(|j| j as u64)
        .collect();
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

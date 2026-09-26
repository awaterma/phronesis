//! Region identity (SPEC-coverage-evidence §3.2): one id per code site
//! within a revision, shared by every producer (collector, importer) and
//! consumer (hydration, selection, property staleness).
//!
//! Grammar, within the importer's identifier charset `[A-Za-z0-9_:./-]`:
//!
//! ```text
//! fn-id      = "fn:" file "::" item-path
//! branch-id  = "branch:" file "::" item-path ":" anchor [ "." ordinal ]
//! file       = repo-relative path; any char outside [A-Za-z0-9_./-] becomes
//!              "_" and the segment gains ".h" + 12 hex of FNV-1a(path)
//! item-path  = segment *( "::" segment )     ; outermost scope first
//! segment    = name                           ; mod, trait, or fn
//!            | type [ ".as." trait ]          ; impl block
//!            | "_"                            ; branch outside any fn
//! ```
//!
//! `name`, `type`, and `trait` are the source text with whitespace removed,
//! `::` rewritten to `.`, and every other char outside `[A-Za-z0-9_]`
//! (generic brackets, `&`, `'`, `,`, non-ASCII) rewritten to `-` — so
//! `impl From<A> for X` and `impl From<B> for X` stay distinct
//! (`X.as.From-A-` / `X.as.From-B-`). That encoding is not injective, so a
//! function whose item path repeats earlier in the same file (cfg variants,
//! encoding collisions) gets `.2`, `.3`, ... on its last segment, in source
//! order. The branch `ordinal` does the same for identical conditions in
//! one function: the first site has none, later ones `.2`, `.3`, ... —
//! whitespace-only reflow moves neither the anchor nor the order.
//!
//! An id longer than 256 bytes keeps its first 242 bytes and appends
//! `.h` + 12 hex of FNV-1a over the full id.
//!
//! Ids from before this grammar (`fn:<leaf>`, `branch:<leaf>:<anchor>`)
//! carry no `::`; [`is_qualified_region_id`] tells them apart so hydration
//! can report such a store as stale instead of joining on leaf names.

use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use tree_sitter::{Node, Parser};

/// The importer's identifier cap (`validate_identifier_field`).
pub const MAX_REGION_ID_BYTES: usize = 256;

/// Bytes kept from an over-long id before the `.h<12 hex>` suffix.
const CAPPED_PREFIX_BYTES: usize = MAX_REGION_ID_BYTES - 14;

pub fn function_region_id(file: &str, item_path: &str) -> String {
    cap(format!("fn:{}::{item_path}", file_segment(file)))
}

pub fn branch_region_id(file: &str, item_path: &str, anchor: &str, ordinal: u32) -> String {
    let suffix = if ordinal > 1 {
        format!(".{ordinal}")
    } else {
        String::new()
    };
    cap(format!(
        "branch:{}::{item_path}:{anchor}{suffix}",
        file_segment(file)
    ))
}

/// The repo-relative spelling of an edited path — the `file` every region id
/// is qualified with. Hooks pass the host's `file_path` through unmodified,
/// and Claude Code sends it absolute, so an absolute path is made relative
/// to `root`: lexically first, then with both sides canonicalized (symlinked
/// roots, `/var` vs `/private/var`); a file that does not exist yet
/// canonicalizes through its parent. A relative path is already
/// root-relative. `None` when the path lies outside the root (or climbs out
/// with `..`): such an edit names no region of this project.
pub fn repo_relative_path(root: &Path, path: &str) -> Option<String> {
    let p = Path::new(path);
    let rel = if p.is_absolute() {
        match p.strip_prefix(root) {
            Ok(rel) => rel.to_path_buf(),
            Err(_) => {
                let canonical_root = root.canonicalize().ok()?;
                canonicalize_lenient(p)?
                    .strip_prefix(&canonical_root)
                    .ok()?
                    .to_path_buf()
            }
        }
    } else {
        p.to_path_buf()
    };
    let mut parts: Vec<String> = Vec::new();
    for component in rel.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_str()?.to_string()),
            Component::CurDir => {}
            _ => return None,
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

fn canonicalize_lenient(path: &Path) -> Option<PathBuf> {
    if let Ok(canonical) = path.canonicalize() {
        return Some(canonical);
    }
    let parent = path.parent()?.canonicalize().ok()?;
    Some(parent.join(path.file_name()?))
}

/// The `file` production of the grammar: the path itself when it is already
/// in the charset, otherwise a sanitized path plus a hash of the original so
/// two paths that sanitize alike stay distinct.
pub fn file_segment(file: &str) -> String {
    let clean: String = file
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if clean == file {
        clean
    } else {
        format!("{clean}.h{}", hash12(file))
    }
}

fn cap(id: String) -> String {
    if id.len() <= MAX_REGION_ID_BYTES {
        return id;
    }
    // Every production above is ASCII, so any byte index is a char boundary.
    format!("{}.h{}", &id[..CAPPED_PREFIX_BYTES], hash12(&id))
}

/// True for ids in the current grammar; false for the leaf-name ids older
/// stores carry (`fn:new`, `branch:safe_divide:cd6054b02dde`), which cannot
/// be joined against per-site ids without conflating sites.
pub fn is_qualified_region_id(id: &str) -> bool {
    id.strip_prefix("fn:")
        .or_else(|| id.strip_prefix("branch:"))
        .is_some_and(|rest| rest.contains("::"))
}

/// Whether the region reference `reference` (a property's `depends_on`
/// entry) names the changed region `changed`. A qualified reference must
/// match exactly. A legacy leaf-name reference cannot say which site it
/// meant, so it matches every changed site with that leaf name (and, for a
/// branch, that anchor) — over-approximating keeps a stale property store
/// raising obligations rather than silently never matching.
pub fn reference_matches(reference: &str, changed: &str) -> bool {
    if reference == changed {
        return true;
    }
    if is_qualified_region_id(reference) {
        return false;
    }
    match (legacy_parts(reference), qualified_parts(changed)) {
        (Some(r), Some(c)) => r == c,
        _ => false,
    }
}

/// `(is_branch, leaf fn name, anchor)` of a legacy id.
fn legacy_parts(id: &str) -> Option<(bool, &str, Option<&str>)> {
    if let Some(leaf) = id.strip_prefix("fn:") {
        return Some((false, leaf, None));
    }
    let (leaf, anchor) = id.strip_prefix("branch:")?.split_once(':')?;
    Some((true, leaf, Some(anchor)))
}

/// Last item-path segment without its ordinal: the function's own name.
fn leaf_of(item_path: &str) -> Option<&str> {
    item_path.rsplit("::").next()?.split('.').next()
}

/// `(is_branch, leaf fn name, anchor)` of a qualified id: the leaf is the
/// last item-path segment without its ordinal, the anchor drops its ordinal.
fn qualified_parts(id: &str) -> Option<(bool, &str, Option<&str>)> {
    if let Some(rest) = id.strip_prefix("fn:") {
        let (_, item_path) = rest.split_once("::")?;
        return Some((false, leaf_of(item_path)?, None));
    }
    let (_, rest) = id.strip_prefix("branch:")?.split_once("::")?;
    let (item_path, anchor) = rest.rsplit_once(':')?;
    Some((true, leaf_of(item_path)?, anchor.split('.').next()))
}

pub struct FunctionSite {
    /// Qualified item path (grammar above), ordinal included.
    pub item_path: String,
    pub start_line: u64,
    pub end_line: u64,
}

impl FunctionSite {
    pub fn region_id(&self, file: &str) -> String {
        function_region_id(file, &self.item_path)
    }

    /// The function's own name: the last item-path segment, ordinal dropped.
    pub fn name(&self) -> &str {
        leaf_of(&self.item_path).unwrap_or(&self.item_path)
    }
}

pub struct BranchSite {
    /// Item path of the innermost enclosing function (`_` when none).
    pub function: String,
    pub anchor: String,
    /// 1-based position among sites with the same `function` and `anchor`,
    /// in source order.
    pub ordinal: u32,
    pub start_line: u64,
    pub end_line: u64,
}

impl BranchSite {
    pub fn region_id(&self, file: &str) -> String {
        branch_region_id(file, &self.function, &self.anchor, self.ordinal)
    }
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

fn hash12(text: &str) -> String {
    format!("{:016x}", fnv1a_64(text.as_bytes()))[..12].to_string()
}

fn compute_anchor(condition: &str) -> String {
    // Whitespace-normalized before hashing: rustfmt reflowing a condition
    // (default-on in Rust workflows) must not orphan the anchor — the
    // semantic-preserving transformation survives, per the cross-revision
    // persistence requirement (review finding #5; full token-stream
    // normalization is the follow-up, operand reorder still re-anchors).
    let normalized: String = condition.split_whitespace().collect::<Vec<_>>().join(" ");
    hash12(&normalized)
}

/// Depth-first collection of every node — `root.children()` alone only
/// visits top-level items, and `if_expression`s live inside function bodies.
/// Returned in source order (by start byte), which the ordinals rely on.
fn all_nodes<'t>(root: Node<'t>) -> Vec<Node<'t>> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        out.push(node);
        stack.extend(node.children(&mut node.walk()));
    }
    out.sort_by_key(|n| (n.start_byte(), std::cmp::Reverse(n.end_byte())));
    out
}

/// `name`/`type`/`trait` production: whitespace dropped, `::` -> `.`,
/// everything else outside `[A-Za-z0-9_]` -> `-`.
fn encode_segment(text: &str) -> String {
    let compact: String = text.split_whitespace().collect();
    compact
        .replace("::", ".")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// The item-path segment a scope node contributes, if it is a scope.
fn scope_segment(node: Node, source: &str) -> Option<String> {
    let field = |name: &str| {
        node.child_by_field_name(name)
            .and_then(|n| n.utf8_text(source.as_bytes()).ok())
    };
    match node.kind() {
        "function_item" | "mod_item" | "trait_item" => field("name").map(encode_segment),
        "impl_item" => {
            let ty = encode_segment(field("type")?);
            Some(match field("trait") {
                Some(tr) => format!("{ty}.as.{}", encode_segment(tr)),
                None => ty,
            })
        }
        _ => None,
    }
}

/// Item path of `node` itself plus every enclosing scope, outermost first.
fn scope_path(node: Node, source: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = Some(node);
    while let Some(n) = current {
        if let Some(segment) = scope_segment(n, source) {
            segments.push(segment);
        }
        current = n.parent();
    }
    segments.reverse();
    segments
}

fn parse(source: &str) -> Result<tree_sitter::Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .context("failed to set Rust language")?;
    parser.parse(source, None).context("failed to parse source")
}

/// Function sites keyed by tree-sitter node id, so branch sites can name
/// their enclosing function by the same (ordinal-disambiguated) item path.
fn function_sites_by_node(tree: &tree_sitter::Tree, source: &str) -> Vec<(usize, FunctionSite)> {
    let mut seen: HashMap<String, u32> = HashMap::new();
    let mut sites = Vec::new();
    for node in all_nodes(tree.root_node()) {
        if node.kind() != "function_item" || node.child_by_field_name("name").is_none() {
            continue;
        }
        let base = scope_path(node, source).join("::");
        let count = seen.entry(base.clone()).or_insert(0);
        *count += 1;
        let item_path = if *count > 1 {
            format!("{base}.{count}")
        } else {
            base
        };
        sites.push((
            node.id(),
            FunctionSite {
                item_path,
                start_line: node.start_position().row as u64 + 1,
                end_line: node.end_position().row as u64 + 1,
            },
        ));
    }
    sites
}

/// Every named function (free fns, methods, trait default methods, nested
/// fns), in source order, with its qualified item path.
pub fn extract_function_sites(source: &str) -> Result<Vec<FunctionSite>> {
    let tree = parse(source)?;
    Ok(function_sites_by_node(&tree, source)
        .into_iter()
        .map(|(_, site)| site)
        .collect())
}

/// Every `if_expression`, with the anchor hashed from its condition text and
/// the span covering the whole expression (so edits to the branch body count
/// as changes to the branch).
pub fn extract_branch_sites(source: &str) -> Result<Vec<BranchSite>> {
    let tree = parse(source)?;
    let functions: HashMap<usize, String> = function_sites_by_node(&tree, source)
        .into_iter()
        .map(|(id, site)| (id, site.item_path))
        .collect();
    let mut ordinals: HashMap<(String, String), u32> = HashMap::new();
    let mut sites = Vec::new();
    for node in all_nodes(tree.root_node()) {
        if node.kind() != "if_expression" {
            continue;
        }
        let Some(condition) = node.child_by_field_name("condition") else {
            continue;
        };
        let condition_text = condition.utf8_text(source.as_bytes()).unwrap_or_default();
        let anchor = compute_anchor(condition_text);
        let function = enclosing_function(node, &functions).unwrap_or_else(|| "_".to_string());
        let ordinal = ordinals
            .entry((function.clone(), anchor.clone()))
            .or_insert(0);
        *ordinal += 1;
        sites.push(BranchSite {
            function,
            anchor,
            ordinal: *ordinal,
            start_line: node.start_position().row as u64 + 1,
            end_line: node.end_position().row as u64 + 1,
        });
    }
    Ok(sites)
}

fn enclosing_function(node: Node, functions: &HashMap<usize, String>) -> Option<String> {
    let mut current = node.parent();
    while let Some(n) = current {
        if let Some(path) = functions.get(&n.id()) {
            return Some(path.clone());
        }
        current = n.parent();
    }
    None
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
/// `file` is the repo-relative path the ids are qualified with.
/// Branch mapping is skipped for inputs over 100_000 lines (documented cap).
pub fn changed_regions(file: &str, old: &str, new: &str) -> Result<ChangedRegions> {
    let (changed_old, changed_new) = changed_lines(old, new);

    let mut functions = HashSet::new();
    for (source, changed) in [(old, &changed_old), (new, &changed_new)] {
        for site in extract_function_sites(source)? {
            if (site.start_line..=site.end_line).any(|line| changed.contains(&line)) {
                functions.insert(site.region_id(file));
            }
        }
    }

    let mut branches = HashSet::new();
    let too_big = old.lines().count() > 100_000 || new.lines().count() > 100_000;
    if !too_big {
        for (source, changed) in [(old, &changed_old), (new, &changed_new)] {
            for site in extract_branch_sites(source)? {
                if (site.start_line..=site.end_line).any(|line| changed.contains(&line)) {
                    branches.insert(site.region_id(file));
                }
            }
        }
    }

    Ok(ChangedRegions {
        functions: functions.into_iter().collect(),
        branches: branches.into_iter().collect(),
    })
}

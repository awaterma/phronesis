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
//!              (also when capped, below)
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
//! Each segment is capped on its own so an id never exceeds 256 bytes and
//! never loses its `::`: a `file` over 120 bytes keeps its first 106 and
//! appends `.h` + 12 hex of FNV-1a(path); an `item-path` over 100 bytes
//! becomes `_h` + 12 hex of FNV-1a(item path) + `::` + its last segment
//! (dropped too when that alone would not fit).
//!
//! A file too large to map per region (over [`REGION_MAP_MAX_BYTES`]) or
//! unreadable is one coarse region, `file:` file, standing for the whole
//! file: it joins no hit, so every gap rule fires on it.
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

/// Byte budget of the `file` segment. With the item-path budget below, the
/// longest id — `branch:` (7) + file (120) + `::` (2) + item path (100) +
/// `:` (1) + anchor (12) + `.` and a u32 ordinal (11) — is 253 bytes.
const MAX_FILE_SEGMENT_BYTES: usize = 120;
/// Byte budget of the `item-path` segment.
const MAX_ITEM_PATH_BYTES: usize = 100;
/// `.h` + 12 hex.
const HASH_SUFFIX_BYTES: usize = 14;

/// Largest input (per side, in bytes) mapped per function and branch site.
/// Region mapping runs on the synchronous pre-check path; past this budget
/// the file is one coarse [`file_region_id`] region instead — gap rules
/// still fire (over-report), and the hook's latency stays bounded.
pub const REGION_MAP_MAX_BYTES: usize = 1024 * 1024;

/// Most cells the line-diff table may hold after the common prefix and
/// suffix are trimmed. Past it, every line in the untrimmed middle counts
/// as changed (over-report, never under-report).
const LCS_MAX_CELLS: usize = 4_000_000;

/// The coarse region standing for a whole file that was not mapped per site.
pub fn file_region_id(file: &str) -> String {
    format!("file:{}", file_segment(file))
}

pub fn function_region_id(file: &str, item_path: &str) -> String {
    format!("fn:{}::{}", file_segment(file), cap_item_path(item_path))
}

pub fn branch_region_id(file: &str, item_path: &str, anchor: &str, ordinal: u32) -> String {
    let suffix = if ordinal > 1 {
        format!(".{ordinal}")
    } else {
        String::new()
    };
    format!(
        "branch:{}::{}:{anchor}{suffix}",
        file_segment(file),
        cap_item_path(item_path)
    )
}

/// An over-budget item path becomes `_h<12 hex of the full path>::<leaf>`:
/// the hash keeps distinct paths distinct, and the leaf segment (the fn
/// name with its ordinal) survives so legacy references still find it. A
/// leaf too long to keep is dropped; the hash alone then names the site.
fn cap_item_path(item_path: &str) -> String {
    if item_path.len() <= MAX_ITEM_PATH_BYTES {
        return item_path.to_string();
    }
    let hashed = format!("_h{}", hash12(item_path));
    match item_path.rsplit("::").next() {
        Some(leaf) if hashed.len() + 2 + leaf.len() <= MAX_ITEM_PATH_BYTES => {
            format!("{hashed}::{leaf}")
        }
        _ => hashed,
    }
}

/// The repo-relative spelling of an edited path — the `file` every region id
/// is qualified with. Hooks pass the host's `file_path` through unmodified,
/// and Claude Code sends it absolute. A relative path is root-relative (the
/// hook's own `resolve_safe_path` contract, which reads the file the same
/// way). The path is joined to `root` and normalized lexically (`.` and `..`
/// resolved) before the root is stripped; failing that, both sides are
/// canonicalized (symlinked roots, `/var` vs `/private/var`), a path that
/// does not exist yet canonicalizing through its deepest existing ancestor.
/// `None` when the path lies outside the root (or climbs out with `..`):
/// such an edit names no region of this project.
pub fn repo_relative_path(root: &Path, path: &str) -> Option<String> {
    let p = Path::new(path);
    let joined = normalize_lexically(&root.join(p))?;
    let rel = match joined.strip_prefix(normalize_lexically(root)?) {
        Ok(rel) => rel.to_path_buf(),
        Err(_) => {
            let canonical_root = root.canonicalize().ok()?;
            canonicalize_lenient(&joined)?
                .strip_prefix(&canonical_root)
                .ok()?
                .to_path_buf()
        }
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

/// Resolve `.` and `..` without touching the filesystem. `None` for a `..`
/// that would climb above the filesystem root.
fn normalize_lexically(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    Some(out)
}

/// Canonicalize the deepest existing ancestor and re-append the rest, so a
/// file in a directory the edit is about to create still maps through a
/// symlinked spelling of the root. `path` must already be lexically
/// normalized (no `..` left to resolve against a symlink).
fn canonicalize_lenient(path: &Path) -> Option<PathBuf> {
    let mut existing = path;
    let mut rest: Vec<&std::ffi::OsStr> = Vec::new();
    loop {
        if let Ok(canonical) = existing.canonicalize() {
            let mut out = canonical;
            for part in rest.iter().rev() {
                out.push(part);
            }
            return Some(out);
        }
        rest.push(existing.file_name()?);
        existing = existing.parent()?;
    }
}

/// The `file` production of the grammar: the path itself when it is already
/// in the charset and within budget, otherwise a sanitized (and, past 120
/// bytes, truncated) path plus a hash of the original, so two paths that
/// sanitize or truncate alike stay distinct.
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
    if clean == file && clean.len() <= MAX_FILE_SEGMENT_BYTES {
        return clean;
    }
    // Sanitized or over budget: keep a readable prefix and add a hash of the
    // original path. `clean` is ASCII, so any byte index is a char boundary.
    let keep = clean.len().min(MAX_FILE_SEGMENT_BYTES - HASH_SUFFIX_BYTES);
    format!("{}.h{}", &clean[..keep], hash12(file))
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
///
/// A coarse whole-file region matches every reference into that file, and
/// every legacy reference (which names no file): the file changed somewhere.
pub fn reference_matches(reference: &str, changed: &str) -> bool {
    if reference == changed {
        return true;
    }
    if let Some(file) = changed.strip_prefix("file:") {
        return match reference
            .strip_prefix("fn:")
            .or_else(|| reference.strip_prefix("branch:"))
            .and_then(|rest| rest.split_once("::"))
        {
            Some((ref_file, _)) => ref_file == file,
            None => true,
        };
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

/// One node of the tree with the ancestry the site extractors need,
/// captured top-down: tree-sitter's `Node::parent()` rescans the parent's
/// children, so walking up from each of N sibling items is O(N²).
struct Visit<'t> {
    node: Node<'t>,
    /// Item path of the node itself plus every enclosing scope, outermost
    /// first.
    scopes: std::rc::Rc<Vec<String>>,
    /// Id of the innermost enclosing named `function_item`, excluding the
    /// node itself.
    enclosing_fn: Option<usize>,
}

/// Depth-first collection of every node — `root.children()` alone only
/// visits top-level items, and `if_expression`s live inside function bodies.
/// Returned in source order (by start byte), which the ordinals rely on.
fn all_nodes<'t>(root: Node<'t>, source: &str) -> Vec<Visit<'t>> {
    let mut out = Vec::new();
    let mut stack = vec![(root, std::rc::Rc::new(Vec::new()), None)];
    while let Some((node, parent_scopes, enclosing_fn)) = stack.pop() {
        let scopes = match scope_segment(node, source) {
            Some(segment) => {
                let mut v = Vec::clone(&parent_scopes);
                v.push(segment);
                std::rc::Rc::new(v)
            }
            None => parent_scopes,
        };
        let child_fn = if is_named_function(node) {
            Some(node.id())
        } else {
            enclosing_fn
        };
        for child in node.children(&mut node.walk()) {
            stack.push((child, std::rc::Rc::clone(&scopes), child_fn));
        }
        out.push(Visit {
            node,
            scopes,
            enclosing_fn,
        });
    }
    out.sort_by_key(|v| (v.node.start_byte(), std::cmp::Reverse(v.node.end_byte())));
    out
}

fn is_named_function(node: Node) -> bool {
    node.kind() == "function_item" && node.child_by_field_name("name").is_some()
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

fn parse(source: &str) -> Result<tree_sitter::Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .context("failed to set Rust language")?;
    parser.parse(source, None).context("failed to parse source")
}

/// Function sites keyed by tree-sitter node id, so branch sites can name
/// their enclosing function by the same (ordinal-disambiguated) item path.
fn function_sites_by_node(visits: &[Visit]) -> Vec<(usize, FunctionSite)> {
    let mut seen: HashMap<String, u32> = HashMap::new();
    let mut sites = Vec::new();
    for Visit { node, scopes, .. } in visits {
        if !is_named_function(*node) {
            continue;
        }
        let base = scopes.join("::");
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
    Ok(function_sites_by_node(&all_nodes(tree.root_node(), source))
        .into_iter()
        .map(|(_, site)| site)
        .collect())
}

/// Every `if_expression`, with the anchor hashed from its condition text and
/// the span covering the whole expression (so edits to the branch body count
/// as changes to the branch).
pub fn extract_branch_sites(source: &str) -> Result<Vec<BranchSite>> {
    extract_sites(source).map(|(_, branches)| branches)
}

/// Function and branch sites from one parse and one tree walk.
fn extract_sites(source: &str) -> Result<(Vec<FunctionSite>, Vec<BranchSite>)> {
    let tree = parse(source)?;
    let visits = all_nodes(tree.root_node(), source);
    let function_sites = function_sites_by_node(&visits);
    let functions: HashMap<usize, &str> = function_sites
        .iter()
        .map(|(id, site)| (*id, site.item_path.as_str()))
        .collect();
    let mut ordinals: HashMap<(String, String), u32> = HashMap::new();
    let mut sites = Vec::new();
    for &Visit {
        node, enclosing_fn, ..
    } in &visits
    {
        if node.kind() != "if_expression" {
            continue;
        }
        let Some(condition) = node.child_by_field_name("condition") else {
            continue;
        };
        let condition_text = condition.utf8_text(source.as_bytes()).unwrap_or_default();
        let anchor = compute_anchor(condition_text);
        let function = enclosing_fn
            .and_then(|id| functions.get(&id))
            .map_or_else(|| "_".to_string(), |path| path.to_string());
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
    let function_sites = function_sites.into_iter().map(|(_, site)| site).collect();
    Ok((function_sites, sites))
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
    // An edit touches a small window of a large file: trim the common
    // prefix and suffix, then diff only the middle. Lines are 1-based.
    let prefix = old_lines
        .iter()
        .zip(&new_lines)
        .take_while(|(a, b)| a == b)
        .count();
    let suffix = old_lines[prefix..]
        .iter()
        .rev()
        .zip(new_lines[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let old_mid = &old_lines[prefix..old_lines.len() - suffix];
    let new_mid = &new_lines[prefix..new_lines.len() - suffix];
    let line_no = |i: usize| (prefix + i) as u64;

    if old_mid.len().saturating_mul(new_mid.len()) > LCS_MAX_CELLS {
        // Too scattered to diff within budget: the whole middle changed.
        return (
            (1..=old_mid.len()).map(line_no).collect(),
            (1..=new_mid.len()).map(line_no).collect(),
        );
    }
    let steps: Vec<WalkStep> = LcsWalk {
        dp: &lcs_table(old_mid, new_mid),
        old: old_mid,
        new: new_mid,
        i: old_mid.len(),
        j: new_mid.len(),
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
    let changed_old = (1..=old_mid.len())
        .filter(|i| !matched.0.contains(i))
        .map(line_no)
        .collect();
    let changed_new = (1..=new_mid.len())
        .filter(|j| !matched.1.contains(j))
        .map(line_no)
        .collect();
    (changed_old, changed_new)
}

pub struct ChangedRegions {
    pub functions: Vec<String>,
    pub branches: Vec<String>,
    /// Coarse [`file_region_id`] regions: files changed but not mapped per
    /// site (over budget, or unreadable). Each stands for the whole file.
    pub files: Vec<String>,
}

impl ChangedRegions {
    /// The whole of `file` changed, unmapped.
    pub fn whole_file(file: &str) -> Self {
        Self {
            functions: Vec::new(),
            branches: Vec::new(),
            files: vec![file_region_id(file)],
        }
    }

    /// Every changed region id: functions, branch sites, coarse files.
    pub fn all(&self) -> impl Iterator<Item = &String> {
        self.functions
            .iter()
            .chain(&self.branches)
            .chain(&self.files)
    }
}

/// Overlap policy: any changed line inside a function's or branch site's
/// span flags that region (SPEC-coverage-evidence §"region identity").
/// `file` is the repo-relative path the ids are qualified with.
/// Either side over [`REGION_MAP_MAX_BYTES`] maps to one coarse whole-file
/// region instead of per-site regions.
pub fn changed_regions(file: &str, old: &str, new: &str) -> Result<ChangedRegions> {
    if old.len() > REGION_MAP_MAX_BYTES || new.len() > REGION_MAP_MAX_BYTES {
        return Ok(ChangedRegions::whole_file(file));
    }
    let (changed_old, changed_new) = changed_lines(old, new);

    let mut functions = HashSet::new();
    let mut branches = HashSet::new();
    for (source, changed) in [(old, &changed_old), (new, &changed_new)] {
        let touched = |start: u64, end: u64| (start..=end).any(|line| changed.contains(&line));
        let (function_sites, branch_sites) = extract_sites(source)?;
        for site in function_sites {
            if touched(site.start_line, site.end_line) {
                functions.insert(site.region_id(file));
            }
        }
        for site in branch_sites {
            if touched(site.start_line, site.end_line) {
                branches.insert(site.region_id(file));
            }
        }
    }

    Ok(ChangedRegions {
        functions: functions.into_iter().collect(),
        branches: branches.into_iter().collect(),
        files: Vec::new(),
    })
}

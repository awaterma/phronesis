//! Coverage collector (SPEC-coverage-evidence §2/§8): normalize cargo-llvm-cov
//! JSON exports into per-test `HitRecord`s, validated against the region map.
//!
//! Design constraint — one source of truth for region identity: function
//! idents and branch anchors come from `region_map` itself (tree-sitter over
//! the covered source), never from a reimplementation. A record is emitted
//! only when the region map independently produces that identity, so the
//! collector cannot drift from what hydration will join against. An LLVM
//! branch region covers an individual sub-condition (`x` in `if x && y`)
//! while the region map anchors the whole condition expression, so branch
//! hits are attributed to the enclosing `if_expression` site by span
//! containment (tightest match wins) and dropped when no site contains them.
//!
//! Input: one llvm-cov JSON export per test (per-test isolation — each test
//! ran alone under coverage, so every executed region in that run is
//! attributable to that test). `test` labels come from the caller.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use crate::coverage::region_map::{extract_branch_sites, extract_functions};
use crate::coverage::store::HitRecord;

/// One llvm-cov JSON export: `{"data": [{files, functions, totals}]}`.
#[derive(Debug, Deserialize)]
pub struct LlvmCovDocument {
    data: Vec<LlvmCovData>,
}

#[derive(Debug, Deserialize)]
struct LlvmCovData {
    #[serde(default)]
    functions: Vec<LlvmCovFunction>,
}

#[derive(Debug, Deserialize)]
struct LlvmCovFunction {
    name: String,
    #[serde(default)]
    count: u64,
    /// `[start_line, start_col, end_line, end_col, count, ...]` (1-based
    /// lines, 0-based cols).
    #[serde(default)]
    regions: Vec<Vec<u64>>,
    /// `[start_line, start_col, end_line, end_col, count]`.
    #[serde(default)]
    branches: Vec<Vec<u64>>,
    #[serde(default)]
    filenames: Vec<String>,
}

/// Leaf identifier from a demangled path, filtered to what tree-sitter would
/// name a function: plain identifiers only — closures (`{closure#0}`),
/// generic wrappers (`<impl ...>`), and anonymous items are dropped.
/// Public because the leaf-ident contract is part of the collector's
/// testable surface (impl methods pass; closures do not).
pub fn leaf_ident(demangled: &str) -> Option<String> {
    let leaf = demangled.rsplit("::").next()?.trim();
    let mut chars = leaf.chars();
    if !chars
        .next()
        .map(|c| c.is_ascii_alphabetic() || c == '_')
        .unwrap_or(false)
        || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return None;
    }
    Some(leaf.to_string())
}

/// llvm-cov path -> repo-root-relative source path. Anchored on the last
/// `/crates/` component, which holds for the main checkout and for
/// `phronesis-wt/<name>/` worktrees alike. Relative paths pass through.
pub fn repo_rel(path: &str) -> Option<String> {
    if let Some(i) = path.rfind("/crates/") {
        return Some(path[i + 1..].to_string());
    }
    let rel = path.trim_start_matches("./");
    if rel.starts_with("crates/") {
        return Some(rel.to_string());
    }
    None
}

/// Which source files a record may target. Coverage of test binaries,
/// benches, and build scripts is not production evidence.
fn is_wanted_source(rel: &str) -> bool {
    rel.ends_with(".rs")
        && rel.contains("/src/")
        && !rel.contains("/src/bin/")
        && !rel.ends_with("build.rs")
}

/// Convert already-collected llvm-cov JSON exports into per-test hit
/// records. `docs` pairs a test label with the parsed export; `root` resolves
/// the repo-relative source paths. Self-validating: fn idents must be
/// functions tree-sitter names in that exact file, and branch hits must fall
/// inside an `extract_branch_sites` span for the same function.
pub fn collect_from_documents(
    root: &Path,
    revision: &str,
    docs: &[(String, LlvmCovDocument)],
) -> Result<Vec<HitRecord>> {
    let mut out: Vec<HitRecord> = Vec::new();
    let mut seen = BTreeSet::new();
    let mut fn_maps: HashMap<String, BTreeSet<String>> = HashMap::new();
    let mut branch_maps: HashMap<String, Vec<BranchSiteFull>> = HashMap::new();

    for (test, doc) in docs {
        let Some(data) = doc.data.first() else {
            bail!("llvm-cov export has no data[0]");
        };
        for f in &data.functions {
            if f.count == 0 || f.filenames.is_empty() {
                continue;
            }
            let Some(rel) = repo_rel(&f.filenames[0]) else { continue };
            if !is_wanted_source(&rel) {
                continue;
            }
            let dem = rustc_demangle::demangle(&f.name).to_string();
            let Some(ident) = leaf_ident(&dem) else { continue };
            let fns = fn_map(root, &mut fn_maps, &rel)?;
            if !fns.contains(&ident) {
                continue;
            }
            if f.regions.is_empty() {
                continue;
            }
            let start = f.regions[0].first().copied().unwrap_or(1);
            let end = f
                .regions
                .last()
                .and_then(|s| s.get(2))
                .copied()
                .unwrap_or(start);
            let rec = HitRecord {
                v: crate::coverage::store::COVERAGE_FORMAT,
                kind: "hit".into(),
                test: test.clone(),
                region: format!("fn:{ident}"),
                file: rel.clone(),
                start_line: start,
                end_line: end,
                hit_kind: "region".into(),
                revision: revision.to_string(),
                tool: "cargo-llvm-cov".into(),
            };
            if seen.insert((test.clone(), rec.region.clone(), rec.file.clone())) {
                out.push(rec);
            }
            let sites = branch_map(root, &mut branch_maps, &rel)?;
            for br in &f.branches {
                if br.len() < 5 || br[4] == 0 {
                    continue;
                }
                let (line, _col, eline, _ecol, _count) = (br[0], br[1], br[2], br[3], br[4]);
                let best = sites
                    .iter()
                    .filter(|s| s.function == ident)
                    .filter(|s| line >= s.start_line && line <= s.end_line)
                    .min_by_key(|s| (s.end_line.saturating_sub(s.start_line), s.start_line));
                let Some(site) = best else { continue };
                let rec = HitRecord {
                    v: crate::coverage::store::COVERAGE_FORMAT,
                    kind: "hit".into(),
                    test: test.clone(),
                    region: format!("branch:{}:{}", ident, site.anchor),
                    file: rel.clone(),
                    start_line: line,
                    end_line: eline,
                    hit_kind: "branch".into(),
                    revision: revision.to_string(),
                    tool: "cargo-llvm-cov".into(),
                };
                if seen.insert((test.clone(), rec.region.clone(), rec.file.clone())) {
                    out.push(rec);
                }
            }
        }
    }
    Ok(out)
}

fn fn_map<'m>(
    root: &Path,
    cache: &'m mut HashMap<String, BTreeSet<String>>,
    rel: &str,
) -> Result<&'m BTreeSet<String>> {
    if !cache.contains_key(rel) {
        let src = std::fs::read_to_string(root.join(rel))
            .with_context(|| format!("reading covered source: {rel}"))?;
        let map = extract_functions(&src)?
            .into_iter()
            .map(|(name, _, _)| name)
            .collect();
        cache.insert(rel.to_string(), map);
    }
    Ok(cache.get(rel).expect("just inserted"))
}

struct BranchSiteFull {
    function: String,
    anchor: String,
    start_line: u64,
    end_line: u64,
}

fn branch_map<'m>(
    root: &Path,
    cache: &'m mut HashMap<String, Vec<BranchSiteFull>>,
    rel: &str,
) -> Result<&'m Vec<BranchSiteFull>> {
    if !cache.contains_key(rel) {
        let src = std::fs::read_to_string(root.join(rel))
            .with_context(|| format!("reading covered source: {rel}"))?;
        let sites = extract_branch_sites(&src)?
            .into_iter()
            .map(|s| BranchSiteFull {
                function: s.function,
                anchor: s.anchor,
                start_line: s.start_line,
                end_line: s.end_line,
            })
            .collect();
        cache.insert(rel.to_string(), sites);
    }
    Ok(cache.get(rel).expect("just inserted"))
}

/// Read one llvm-cov JSON export written by `cargo llvm-cov --output-path`.
pub fn read_document(path: &Path) -> Result<LlvmCovDocument> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading llvm-cov export: {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing llvm-cov export: {}", path.display()))
}

/// Serialize records to the normalized export JSONL format.
pub fn write_export(records: &[HitRecord], out: &Path) -> Result<()> {
    use std::io::Write as _;
    let mut fh = std::fs::File::create(out)?;
    for r in records {
        serde_json::to_writer(&mut fh, r)?;
        fh.write_all(b"\n")?;
    }
    Ok(())
}

/// Discover test names in a phronesis-mcp test binary via `cargo test --list`.
pub fn list_tests(bin: &str) -> Result<Vec<String>> {
    let out = std::process::Command::new("cargo")
        .args(["test", "-p", "phronesis-mcp", "--test", bin, "--", "--list"])
        .output()
        .context("running cargo test --list")?;
    anyhow::ensure!(
        out.status.success(),
        "cargo test --list failed for {bin}"
    );
    let mut names = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(name) = line
            .strip_suffix(": test")
            .filter(|name| !name.is_empty())
        {
            names.push(name.to_string());
        }
    }
    Ok(names)
}

/// Run one test in isolation under coverage (SPEC §2: per-test attribution by
/// isolation) and write its llvm-cov JSON export to `out`.
pub fn run_isolated(bin: &str, test: &str, out: &Path) -> Result<()> {
    let status = std::process::Command::new("cargo")
        .args([
            "+nightly",
            "llvm-cov",
            "--json",
            "--branch",
            "-p",
            "phronesis-mcp",
            "--test",
            bin,
            "--output-path",
        ])
        .arg(out)
        .args(["--", "--exact", test, "--test-threads=1"])
        .status()
        .context("spawning cargo llvm-cov")?;
    anyhow::ensure!(status.success(), "llvm-cov run failed for {bin}::{test}");
    Ok(())
}
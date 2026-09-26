//! Relevant-test selection — a host query, not a rule head (SPEC §7).
//!
//! Union of:
//! - tests hitting changed regions (dynamic, from the coverage store)
//! - tests statically reaching changed functions (`tested_by` / `test_reaches`
//!   edges from the graph store, when fresh)
//!
//! Each entry is labeled by evidence kind (`coverage_observation` vs
//! `static_reach`), deduplicated, and carries its justifying regions. A test
//! with both kinds of evidence yields one entry per kind, so a statically
//! reached region is never reported as observed. Hits from a store imported
//! at a revision other than HEAD are labeled `coverage_observation_stale`;
//! a corrupt store contributes nothing and says so in `coverage_note`.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

use crate::coverage::region_map::{
    ChangedRegions, changed_regions, file_segment, is_qualified_region_id,
};
use crate::coverage::store::{RECOLLECT_HINT, StoreState, is_stale, load_store};
use crate::graph::model::Edge;
use crate::graph::store as graph_store;
use crate::graph::sync::{self, Freshness};

/// Evidence label for a region the coverage store observed a test hitting.
const COVERAGE_OBSERVATION: &str = "coverage_observation";
/// Evidence label for a hit recorded at a revision other than HEAD: the
/// test once executed the region, but nothing says it still does.
const COVERAGE_OBSERVATION_STALE: &str = "coverage_observation_stale";
/// Evidence label for a region a test reaches only via a graph edge.
const STATIC_REACH: &str = "static_reach";

/// One selected test with its evidence kind and justifying regions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedTest {
    pub test: String,
    /// `coverage_observation` (dynamic hit), `coverage_observation_stale`
    /// (dynamic hit imported at a revision other than HEAD), or
    /// `static_reach` (graph edge).
    pub evidence: String,
    /// Region IDs that justify this test's selection.
    pub regions: Vec<String>,
}

/// The full selection result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    /// Change id used for this selection (`head:<short-sha>` or `--change` value).
    pub change: String,
    /// Changed function region IDs (e.g. `fn:src/lib.rs::safe_divide`).
    pub changed_functions: Vec<String>,
    /// Changed branch region IDs (e.g. `branch:src/lib.rs::safe_divide:cd6054b02dde`).
    pub changed_branches: Vec<String>,
    /// Selected tests, sorted by name then evidence kind. A test may appear
    /// twice: once per evidence kind.
    pub tests: Vec<SelectedTest>,
    /// Whether the static (graph) half was available.
    pub static_reach_available: bool,
    /// Human-readable note when the static half was skipped.
    pub static_note: Option<String>,
    /// Human-readable note when the dynamic half cannot be trusted: the
    /// store is stale (imported at another revision), carries region ids
    /// from before per-site qualification (SPEC-coverage-evidence §3.2,
    /// matching no changed region), or is corrupt. `None` for a fresh or
    /// absent store.
    pub coverage_note: Option<String>,
}

/// A file diff entry: repo-relative path, old content, new content.
#[derive(Debug, Clone)]
pub struct FileDiff {
    pub path: String,
    pub old: Option<String>,
    pub new: String,
}

/// Run `git diff` to get the unstaged working-tree changes for tracked files.
/// Returns `(path, old_content, new_content)` triples. Files with no tracked
/// version use `None` for old content (treated as a new file).
pub fn working_tree_diffs(root: &Path) -> Result<Vec<FileDiff>> {
    let tracked = git_diff_names(root)?;
    let mut diffs = Vec::new();
    for path in &tracked {
        let old = git_show_head(root, path).ok();
        let new = std::fs::read_to_string(root.join(path))
            .context(format!("reading working tree file: {path}"))?;
        if old.as_deref() != Some(new.as_str()) {
            diffs.push(FileDiff {
                path: path.clone(),
                old,
                new,
            });
        }
    }
    Ok(diffs)
}

fn git_diff_names(root: &Path) -> Result<Vec<String>> {
    let output = Command::new("git")
        .args(["diff", "--name-only"])
        .current_dir(root)
        .output()
        .context("running git diff --name-only")?;
    if !output.status.success() {
        return Ok(Vec::new());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

fn git_show_head(root: &Path, path: &str) -> Result<String> {
    let output = Command::new("git")
        .args(["show", &format!("HEAD:{path}")])
        .current_dir(root)
        .output()
        .context(format!("running git show HEAD:{path}"))?;
    if !output.status.success() {
        return Err(anyhow::anyhow!("git show failed for {path}"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Compute changed regions from a set of file diffs.
pub fn changed_regions_from_diffs(diffs: &[FileDiff]) -> Result<ChangedRegions> {
    let mut functions = std::collections::BTreeSet::new();
    let mut branches = std::collections::BTreeSet::new();
    for diff in diffs {
        let regions = changed_regions(&diff.path, diff.old.as_deref().unwrap_or(""), &diff.new)?;
        for f in regions.functions {
            functions.insert(f);
        }
        for b in regions.branches {
            branches.insert(b);
        }
    }
    Ok(ChangedRegions {
        functions: functions.into_iter().collect(),
        branches: branches.into_iter().collect(),
    })
}

/// Run selection against the coverage store, the graph store, and the
/// working-tree diff.
///
/// `change_override` is the `--change <id>` value; when `None`, the change id
/// is `head:<short-sha>` from `git rev-parse HEAD`.
pub fn select(root: &Path, change_override: Option<&str>) -> Result<Selection> {
    let head = crate::lifecycle::outcome::git_head(root);
    let change = match change_override {
        Some(id) => id.to_string(),
        None => {
            let sha = head.as_deref().unwrap_or_default();
            let short = &sha[..sha.len().min(12)];
            format!("head:{short}")
        }
    };

    let diffs = working_tree_diffs(root).unwrap_or_default();
    let regions = changed_regions_from_diffs(&diffs)?;

    // --- Dynamic half: coverage store ---
    let (hits, observation, coverage_note) = match load_store(root) {
        StoreState::Loaded { index, hits } => {
            if hits.iter().any(|h| !is_qualified_region_id(&h.region)) {
                let note = format!(
                    "coverage store predates per-site region ids and matches no changed region; {RECOLLECT_HINT}"
                );
                (hits, COVERAGE_OBSERVATION_STALE, Some(note))
            } else if is_stale(&index, head.as_deref()) {
                let short = &index.revision[..index.revision.len().min(12)];
                let note = format!(
                    "coverage evidence is stale (imported at {short}, HEAD has moved); {RECOLLECT_HINT}"
                );
                (hits, COVERAGE_OBSERVATION_STALE, Some(note))
            } else {
                (hits, COVERAGE_OBSERVATION, None)
            }
        }
        StoreState::Missing => (Vec::new(), COVERAGE_OBSERVATION, None),
        StoreState::Corrupt(c) => (
            Vec::new(),
            COVERAGE_OBSERVATION,
            Some(format!("{c}; dynamic evidence ignored; {RECOLLECT_HINT}")),
        ),
    };
    let changed_region_set: std::collections::BTreeSet<&str> = regions
        .functions
        .iter()
        .chain(regions.branches.iter())
        .map(|s| s.as_str())
        .collect();

    // (test, evidence) -> entry. Keyed by evidence kind too, so a test with
    // both a store hit and a static edge gets one entry per kind: a region
    // is only ever labeled `coverage_observation` when the store holds a hit
    // for that (test, region) pair.
    let mut by_test: BTreeMap<(String, &'static str), SelectedTest> = BTreeMap::new();

    for hit in &hits {
        if changed_region_set.contains(hit.region.as_str()) {
            add_entry(&mut by_test, &hit.test, observation, &hit.region);
        }
    }

    // --- Static half: graph store ---
    let graph_path = graph_store::graph_path(root);
    let edges = graph_store::load(&graph_path).unwrap_or_default();
    let (static_available, static_note) = graph_freshness_status(root, &edges);

    if static_available {
        // A graph function reaches a changed region only when the graph
        // defines it in the region's own file under the same name. Leaf
        // name alone would pair `other.rs::gamma` with a test of
        // `unrelated.rs::gamma`; a function the graph never resolved to a
        // definition (no `defines_fn`) pairs with nothing.
        let defined_in: BTreeMap<&str, &str> = edges
            .iter()
            .filter(|e| e.p == "defines_fn" && e.a.len() == 2)
            .map(|e| (e.a[1].as_str(), e.a[0].as_str()))
            .collect();
        let reached = |func: &str| -> Vec<&String> {
            let Some(file) = defined_in.get(func) else {
                return Vec::new();
            };
            let leaf = func.rsplit("::").next().unwrap_or(func);
            regions
                .functions
                .iter()
                .filter(|region| static_region_matches(region, file, leaf))
                .collect()
        };

        for edge in &edges {
            let (test, func) = match edge.p.as_str() {
                // tested_by: [function, test]
                "tested_by" if edge.a.len() == 2 => (&edge.a[1], &edge.a[0]),
                // test_reaches: [test, function]
                "test_reaches" if edge.a.len() == 2 => (&edge.a[0], &edge.a[1]),
                _ => continue,
            };
            for region in reached(func) {
                add_entry(&mut by_test, test, STATIC_REACH, region);
            }
        }
    }

    // BTreeMap order is (test, evidence kind): deterministic, no re-sort.
    let tests: Vec<SelectedTest> = by_test.into_values().collect();

    Ok(Selection {
        change,
        changed_functions: regions.functions,
        changed_branches: regions.branches,
        tests,
        static_reach_available: static_available,
        static_note,
        coverage_note,
    })
}

/// Whether the changed function region `region` is the function named
/// `leaf` defined in `file`: the region's file segment must be `file`'s and
/// its last item-path segment (ordinal dropped) must be `leaf`.
fn static_region_matches(region: &str, file: &str, leaf: &str) -> bool {
    let Some(item_path) = region
        .strip_prefix("fn:")
        .and_then(|rest| rest.strip_prefix(file_segment(file).as_str()))
        .and_then(|rest| rest.strip_prefix("::"))
    else {
        return false;
    };
    let last = item_path.rsplit("::").next().unwrap_or(item_path);
    last.split('.').next() == Some(leaf)
}

/// Add or merge a region into the entry for `(test, evidence)`.
fn add_entry(
    by_test: &mut BTreeMap<(String, &'static str), SelectedTest>,
    test: &str,
    evidence: &'static str,
    region: &str,
) {
    let entry = by_test
        .entry((test.to_string(), evidence))
        .or_insert_with(|| SelectedTest {
            test: test.to_string(),
            evidence: evidence.to_string(),
            regions: Vec::new(),
        });
    if !entry.regions.contains(&region.to_string()) {
        entry.regions.push(region.to_string());
    }
}

fn graph_freshness_status(root: &Path, edges: &[Edge]) -> (bool, Option<String>) {
    if edges.is_empty() {
        return (
            false,
            Some("no graph found; run `phr-mcp graph rebuild`".to_string()),
        );
    }
    let index = match sync::load_index(&sync::index_path(root)) {
        Ok(idx) => idx,
        Err(_) => {
            return (
                false,
                Some("graph index unreadable; run `phr-mcp graph rebuild`".to_string()),
            );
        }
    };
    match sync::check_freshness(root, &index) {
        Freshness::Fresh => (true, None),
        Freshness::Outdated { .. } => (
            false,
            Some(
                "graph outdated (identity format mismatch); run `phr-mcp graph rebuild`"
                    .to_string(),
            ),
        ),
        Freshness::Stale(files) => {
            let note = format!(
                "graph stale ({} file(s) drifted); static reach omitted. Run `phr-mcp graph rebuild`",
                files.len()
            );
            (false, Some(note))
        }
    }
}

/// Render the selection as a human-readable table.
pub fn render_table(sel: &Selection) -> String {
    let mut out = String::new();

    if sel.tests.is_empty() {
        match &sel.coverage_note {
            Some(note) => out.push_str(&format!("No tests selected: {note}\n")),
            None => out.push_str(
                "No tests selected: the coverage store is empty or no hits match changed regions.\n",
            ),
        }
        if let Some(note) = &sel.static_note {
            out.push_str(&format!("  static reach: {note}\n"));
        }
        return out;
    }

    out.push_str(&format!("Change: {}\n", sel.change));
    if !sel.changed_functions.is_empty() {
        out.push_str(&format!(
            "Changed functions: {}\n",
            sel.changed_functions.join(", ")
        ));
    }
    if !sel.changed_branches.is_empty() {
        out.push_str(&format!(
            "Changed branches: {}\n",
            sel.changed_branches.join(", ")
        ));
    }
    out.push('\n');

    // Split by evidence kind for the spec's "listed separately" requirement.
    let coverage_entries: Vec<&SelectedTest> = sel
        .tests
        .iter()
        .filter(|t| t.evidence == COVERAGE_OBSERVATION)
        .collect();
    let stale_entries: Vec<&SelectedTest> = sel
        .tests
        .iter()
        .filter(|t| t.evidence == COVERAGE_OBSERVATION_STALE)
        .collect();
    let static_entries: Vec<&SelectedTest> = sel
        .tests
        .iter()
        .filter(|t| t.evidence == STATIC_REACH)
        .collect();

    if !coverage_entries.is_empty() {
        out.push_str("coverage_observation (dynamic):\n");
        out.push_str(&format!("{:<40} {}\n", "TEST", "REGIONS"));
        for entry in &coverage_entries {
            out.push_str(&format!(
                "{:<40} {}\n",
                entry.test,
                entry.regions.join(", ")
            ));
        }
        out.push('\n');
    }

    if !stale_entries.is_empty() {
        out.push_str("coverage_observation_stale (dynamic, recorded at an older revision):\n");
        out.push_str(&format!("{:<40} {}\n", "TEST", "REGIONS"));
        for entry in &stale_entries {
            out.push_str(&format!(
                "{:<40} {}\n",
                entry.test,
                entry.regions.join(", ")
            ));
        }
        out.push('\n');
    }

    if !static_entries.is_empty() {
        out.push_str("static_reach (graph edges):\n");
        out.push_str(&format!("{:<40} {}\n", "TEST", "REGIONS"));
        for entry in &static_entries {
            out.push_str(&format!(
                "{:<40} {}\n",
                entry.test,
                entry.regions.join(", ")
            ));
        }
        out.push('\n');
    }

    if let Some(note) = &sel.coverage_note {
        out.push_str(&format!("coverage: {note}\n"));
    }
    if let Some(note) = &sel.static_note {
        out.push_str(&format!("static reach: {note}\n"));
    }

    // Count distinct tests: one with both evidence kinds has two entries.
    let distinct: std::collections::BTreeSet<&str> =
        sel.tests.iter().map(|t| t.test.as_str()).collect();
    out.push_str(&format!("\n{} test(s) selected.\n", distinct.len()));

    out
}

/// Render the selection as JSON. `coverage_note` is `null` for a fresh or
/// absent store.
pub fn render_json(sel: &Selection) -> String {
    serde_json::json!({
        "change": sel.change,
        "changed_functions": sel.changed_functions,
        "changed_branches": sel.changed_branches,
        "static_reach_available": sel.static_reach_available,
        "static_note": sel.static_note,
        "coverage_note": sel.coverage_note,
        "tests": sel.tests.iter().map(|t| serde_json::json!({
            "test": t.test,
            "evidence": t.evidence,
            "regions": t.regions,
        })).collect::<Vec<_>>(),
    })
    .to_string()
}

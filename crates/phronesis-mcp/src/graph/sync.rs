//! The per-save pipeline and the staleness index.
//!
//! Two tiers, deliberately separated (spec §4.5): **parse** touches only the
//! edited file, while **derive** runs over the whole edge set. Derived facts
//! are therefore correct after every save without reparsing the repository.
//!
//! The index exists because edits can bypass the hook entirely — `git
//! checkout`, `git mv`, a rebase, a plain shell edit. A graph that has
//! silently drifted must not be allowed to block work, so drift is detected
//! and downgrades enforcement to warn.

use super::derive::derive_all;
use super::extract::{DEFAULT_WATCHLIST, extract_rust};
use super::model::Edge;
use super::store;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Location of the staleness index, relative to project root.
pub const INDEX_REL_PATH: &str = ".phronesis/graph.index";

/// Content hashes of every file the graph was built from.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Index {
    pub entries: BTreeMap<String, u64>,
}

/// Whether the graph still reflects what is on disk.
#[derive(Debug, PartialEq, Eq)]
pub enum Freshness {
    Fresh,
    /// Files whose content no longer matches the index, sorted.
    Stale(Vec<String>),
}

/// Outcome of a single-file save.
#[derive(Debug, PartialEq, Eq)]
pub struct SaveOutcome {
    /// Base edges for the whole project after compaction.
    pub base: usize,
    /// Derived edges regenerated this pass.
    pub derived: usize,
    /// Items the extractor declined to name.
    pub skipped: usize,
}

/// Deterministic content hash (FNV-1a, 64-bit).
///
/// Not `DefaultHasher`: that is explicitly not stable across Rust releases,
/// and a hash that changes under the reader would mark every file stale after
/// a toolchain upgrade.
pub fn hash_content(content: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in content.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

pub fn index_path(root: &Path) -> PathBuf {
    root.join(INDEX_REL_PATH)
}

/// A missing or unreadable index means "nothing known yet", which callers
/// treat as stale. Failing closed here would take enforcement offline for a
/// recoverable, self-healing condition.
pub fn load_index(path: &Path) -> std::io::Result<Index> {
    let body = match std::fs::read_to_string(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Index::default()),
        Err(e) => return Err(e),
    };
    let mut entries = BTreeMap::new();
    for line in body.lines() {
        if let Some((hash, rel)) = line.split_once(' ')
            && let Ok(h) = hash.parse::<u64>()
        {
            entries.insert(rel.to_string(), h);
        }
    }
    Ok(Index { entries })
}

pub fn save_index(path: &Path, index: &Index) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut body = String::new();
    for (rel, hash) in &index.entries {
        body.push_str(&format!("{hash} {rel}\n"));
    }
    let tmp = path.with_extension("index.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)
}

/// Every Rust file under `root` that the graph should track, as paths
/// relative to `root`. Honours `.gitignore` so build output and vendored
/// trees never enter the graph.
fn tracked_files(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for entry in ignore::WalkBuilder::new(root)
        .hidden(true)
        .build()
        .flatten()
    {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        if let Ok(rel) = path.strip_prefix(root)
            && let Some(rel) = rel.to_str()
        {
            out.push(rel.replace('\\', "/"));
        }
    }
    out.sort();
    out
}

/// Compare the index against what is on disk under `root`.
pub fn check_freshness(root: &Path, index: &Index) -> Freshness {
    let mut drifted = Vec::new();
    let on_disk = tracked_files(root);

    for rel in &on_disk {
        let current = std::fs::read_to_string(root.join(rel))
            .ok()
            .map(|c| hash_content(&c));
        match (current, index.entries.get(rel)) {
            (Some(now), Some(&then)) if now == then => {}
            _ => drifted.push(rel.clone()),
        }
    }
    // Indexed files that have vanished are drift too — their edges are still
    // in the graph and would keep matching rules.
    for rel in index.entries.keys() {
        if !on_disk.contains(rel) {
            drifted.push(rel.clone());
        }
    }

    if drifted.is_empty() {
        Freshness::Fresh
    } else {
        drifted.sort();
        drifted.dedup();
        Freshness::Stale(drifted)
    }
}

/// Recompute derived edges over `base` and persist both sets.
fn persist(root: &Path, base: Vec<Edge>) -> std::io::Result<(usize, usize)> {
    let derived = derive_all(&base);
    let (n_base, n_derived) = (base.len(), derived.len());
    let mut all = base;
    all.extend(derived);
    store::write_atomic(&store::graph_path(root), &all)?;
    Ok((n_base, n_derived))
}

/// Apply one save: parse the edited file, compact by provenance, re-derive
/// over the whole graph, and write atomically.
///
/// Only the edited file is parsed; derivation runs over the full edge set
/// already on disk. That is what makes whole-repo facts affordable per save.
pub fn on_save(root: &Path, file_path: &str, content: &str) -> std::io::Result<SaveOutcome> {
    if !file_path.ends_with(".rs") {
        return Ok(SaveOutcome {
            base: 0,
            derived: 0,
            skipped: 0,
        });
    }
    let extracted = extract_rust(file_path, content, DEFAULT_WATCHLIST);
    let existing = store::load(&store::graph_path(root))?;
    let base = store::compact(existing, file_path, extracted.edges);
    let (n_base, n_derived) = persist(root, base)?;

    let ipath = index_path(root);
    let mut index = load_index(&ipath)?;
    index
        .entries
        .insert(file_path.to_string(), hash_content(content));
    save_index(&ipath, &index)?;

    Ok(SaveOutcome {
        base: n_base,
        derived: n_derived,
        skipped: extracted.skipped,
    })
}

/// Hook entry point: record the file's current on-disk content.
///
/// Best-effort and infallible by design. The sensor runs in `PostToolUse`,
/// after the edit has already happened; a graph write that fails must not
/// turn into a hook error that interrupts the user's work. Failures leave the
/// file's hash stale, which the freshness check will catch and report.
pub fn record_from_disk(root: &Path, file_path: &str) {
    // Never follow a path out of the project — the sensor reads whatever it
    // is handed, and a traversal would pull unrelated files into the graph.
    if file_path.contains("..") || Path::new(file_path).is_absolute() {
        return;
    }
    let Ok(content) = std::fs::read_to_string(root.join(file_path)) else {
        return;
    };
    if let Err(e) = on_save(root, file_path, &content) {
        tracing::debug!("graph sensor skipped {file_path}: {e}");
    }
}

/// Full rescan of every tracked Rust file. The recovery path after the graph
/// has drifted, and the only way edges for deleted files are cleared.
pub fn rebuild(root: &Path) -> std::io::Result<SaveOutcome> {
    let mut base = Vec::new();
    let mut index = Index::default();
    let mut skipped = 0;

    for rel in tracked_files(root) {
        let Ok(content) = std::fs::read_to_string(root.join(&rel)) else {
            continue;
        };
        let extracted = extract_rust(&rel, &content, DEFAULT_WATCHLIST);
        skipped += extracted.skipped;
        base.extend(extracted.edges);
        index.entries.insert(rel, hash_content(&content));
    }

    let (n_base, n_derived) = persist(root, base)?;
    save_index(&index_path(root), &index)?;
    Ok(SaveOutcome {
        base: n_base,
        derived: n_derived,
        skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn project() -> TempDir {
        let d = TempDir::new().expect("tempdir");
        std::fs::create_dir_all(d.path().join("src")).expect("mkdir src");
        d
    }

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(p, body).expect("write");
    }

    fn edges(root: &Path) -> Vec<Edge> {
        store::load(&store::graph_path(root)).expect("load")
    }

    fn has(root: &Path, p: &str) -> bool {
        edges(root).iter().any(|e| e.p == p)
    }

    // ─── hashing ────────────────────────────────────────────────────

    #[test]
    fn identical_content_hashes_identically() {
        assert_eq!(hash_content("fn f() {}"), hash_content("fn f() {}"));
    }

    #[test]
    fn different_content_hashes_differently() {
        assert_ne!(hash_content("fn f() {}"), hash_content("fn g() {}"));
    }

    // ─── index round trip ───────────────────────────────────────────

    #[test]
    fn a_missing_index_loads_as_empty() {
        let d = project();
        assert_eq!(
            load_index(&index_path(d.path())).expect("load"),
            Index::default()
        );
    }

    #[test]
    fn the_index_survives_a_save_load_round_trip() {
        let d = project();
        let mut idx = Index::default();
        idx.entries.insert("src/a.rs".into(), 42);
        let p = index_path(d.path());
        save_index(&p, &idx).expect("save");
        assert_eq!(load_index(&p).expect("load"), idx);
    }

    // ─── the per-save pipeline ──────────────────────────────────────

    #[test]
    fn saving_a_file_writes_its_base_edges() {
        let d = project();
        write(d.path(), "src/a.rs", "fn f() {}");
        on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
        assert!(has(d.path(), "defines_fn"));
    }

    #[test]
    fn saving_a_file_derives_untested_in_the_same_pass() {
        let d = project();
        write(d.path(), "src/a.rs", "fn f() {}");
        on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
        assert!(
            has(d.path(), "untested"),
            "derived facts must be current after every save, not only after rebuild"
        );
    }

    #[test]
    fn a_test_added_later_clears_untested_without_a_rebuild() {
        let d = project();
        write(d.path(), "src/a.rs", "fn fire() {}");
        on_save(d.path(), "src/a.rs", "fn fire() {}").expect("save");
        assert!(has(d.path(), "untested"));

        let test_src = "#[test]\nfn t() { fire(); }";
        write(d.path(), "tests/a.rs", test_src);
        on_save(d.path(), "tests/a.rs", test_src).expect("save");
        assert!(
            !has(d.path(), "untested"),
            "coverage is whole-repo: a test elsewhere must clear it"
        );
    }

    #[test]
    fn re_saving_a_file_replaces_its_edges_rather_than_duplicating_them() {
        let d = project();
        for _ in 0..3 {
            on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
        }
        let defines: Vec<_> = edges(d.path())
            .into_iter()
            .filter(|e| e.p == "defines_fn")
            .collect();
        assert_eq!(defines.len(), 1);
    }

    #[test]
    fn removing_a_function_removes_its_edges() {
        let d = project();
        on_save(d.path(), "src/a.rs", "fn f() {}\nfn g() {}").expect("save");
        on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
        let names: Vec<String> = edges(d.path())
            .into_iter()
            .filter(|e| e.p == "defines_fn")
            .filter_map(|e| e.a.get(1).cloned())
            .collect();
        assert_eq!(names, vec!["crate::a::f".to_string()]);
    }

    #[test]
    fn derived_edges_do_not_accumulate_across_saves() {
        let d = project();
        on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
        on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
        let untested: Vec<_> = edges(d.path())
            .into_iter()
            .filter(|e| e.p == "untested")
            .collect();
        assert_eq!(untested.len(), 1);
    }

    #[test]
    fn saving_records_the_files_hash_in_the_index() {
        let d = project();
        on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
        let idx = load_index(&index_path(d.path())).expect("load");
        assert_eq!(
            idx.entries.get("src/a.rs"),
            Some(&hash_content("fn f() {}"))
        );
    }

    #[test]
    fn a_non_rust_file_is_ignored() {
        let d = project();
        on_save(d.path(), "README.md", "# hi").expect("save");
        assert!(edges(d.path()).is_empty());
    }

    // ─── the hook entry point ───────────────────────────────────────

    #[test]
    fn recording_from_disk_reads_the_current_file_content() {
        let d = project();
        write(d.path(), "src/a.rs", "fn f() {}");
        record_from_disk(d.path(), "src/a.rs");
        assert!(has(d.path(), "defines_fn"));
    }

    #[test]
    fn recording_a_file_outside_the_project_is_a_no_op() {
        // Path traversal must not let the sensor read arbitrary files.
        let d = project();
        record_from_disk(d.path(), "../../etc/hosts.rs");
        assert!(edges(d.path()).is_empty());
    }

    #[test]
    fn recording_a_missing_file_is_a_no_op_rather_than_an_error() {
        let d = project();
        record_from_disk(d.path(), "src/gone.rs");
        assert!(edges(d.path()).is_empty());
    }

    // ─── staleness ──────────────────────────────────────────────────

    #[test]
    fn an_untouched_project_is_fresh() {
        let d = project();
        write(d.path(), "src/a.rs", "fn f() {}");
        on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
        let idx = load_index(&index_path(d.path())).expect("load");
        assert_eq!(check_freshness(d.path(), &idx), Freshness::Fresh);
    }

    #[test]
    fn an_edit_outside_the_hook_path_is_detected_as_stale() {
        let d = project();
        write(d.path(), "src/a.rs", "fn f() {}");
        on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
        // Simulates git checkout / shell edit: content changes, hook never runs.
        write(d.path(), "src/a.rs", "fn f() {}\nfn sneaky() {}");
        let idx = load_index(&index_path(d.path())).expect("load");
        assert_eq!(
            check_freshness(d.path(), &idx),
            Freshness::Stale(vec!["src/a.rs".to_string()])
        );
    }

    #[test]
    fn a_deleted_file_is_detected_as_stale() {
        let d = project();
        write(d.path(), "src/a.rs", "fn f() {}");
        on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
        std::fs::remove_file(d.path().join("src/a.rs")).expect("rm");
        let idx = load_index(&index_path(d.path())).expect("load");
        assert!(matches!(
            check_freshness(d.path(), &idx),
            Freshness::Stale(_)
        ));
    }

    #[test]
    fn an_untracked_new_file_is_detected_as_stale() {
        let d = project();
        write(d.path(), "src/a.rs", "fn f() {}");
        on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
        write(d.path(), "src/b.rs", "fn g() {}");
        let idx = load_index(&index_path(d.path())).expect("load");
        assert!(matches!(
            check_freshness(d.path(), &idx),
            Freshness::Stale(_)
        ));
    }

    // ─── rebuild ────────────────────────────────────────────────────

    #[test]
    fn rebuild_indexes_every_rust_file_in_the_project() {
        let d = project();
        write(d.path(), "src/a.rs", "fn f() {}");
        write(d.path(), "src/b.rs", "fn g() {}");
        rebuild(d.path()).expect("rebuild");
        let names: Vec<String> = edges(d.path())
            .into_iter()
            .filter(|e| e.p == "defines_fn")
            .filter_map(|e| e.a.get(1).cloned())
            .collect();
        assert_eq!(names.len(), 2, "both files must be scanned");
    }

    #[test]
    fn rebuild_restores_freshness_after_drift() {
        let d = project();
        write(d.path(), "src/a.rs", "fn f() {}");
        on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
        write(d.path(), "src/a.rs", "fn f() {}\nfn sneaky() {}");
        rebuild(d.path()).expect("rebuild");
        let idx = load_index(&index_path(d.path())).expect("load");
        assert_eq!(check_freshness(d.path(), &idx), Freshness::Fresh);
    }

    #[test]
    fn rebuild_drops_edges_for_files_that_no_longer_exist() {
        let d = project();
        write(d.path(), "src/a.rs", "fn f() {}");
        on_save(d.path(), "src/a.rs", "fn f() {}").expect("save");
        std::fs::remove_file(d.path().join("src/a.rs")).expect("rm");
        rebuild(d.path()).expect("rebuild");
        assert!(edges(d.path()).is_empty());
    }
}

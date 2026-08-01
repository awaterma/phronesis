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
use super::unit::UnitMap;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Location of the staleness index, relative to project root.
pub const INDEX_REL_PATH: &str = ".phronesis/graph.index";

/// Identity scheme the extractor writes. Bumped whenever entity naming
/// changes, because such a change invalidates every edge already on disk
/// while leaving file contents — and therefore content hashes — untouched.
/// Without it, an upgrade silently yields a graph half in the old naming and
/// half in the new, whose `imports` never join to its `declares_module`.
///
/// 4 — `<lang>:<package>[#<target>]::<module path>` (introduced in spec
/// rev 4; unchanged by rev 5, which added Python under the same scheme).
/// Anything earlier is recorded as 0: pre-versioning, bare `crate::…`.
pub const GRAPH_FORMAT: u32 = 4;

/// Header line stamping the format into the index file.
const FORMAT_KEY: &str = "# format";

/// Content hashes of every file the graph was built from.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Index {
    /// Identity scheme the graph was built under; 0 for a pre-versioning or
    /// absent index. Only meaningful on load — writes always stamp the
    /// current format, because what we write is by definition current.
    pub format: u32,
    pub entries: BTreeMap<String, u64>,
}

/// Whether the graph still reflects what is on disk.
#[derive(Debug, PartialEq, Eq)]
pub enum Freshness {
    Fresh,
    /// Files whose content no longer matches the index, sorted.
    Stale(Vec<String>),
    /// The graph was built under a different identity scheme. Content hashes
    /// prove nothing here: the files are untouched and every edge is still
    /// wrong. Only a rebuild resolves it.
    Outdated {
        found: u32,
        expected: u32,
    },
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
    let mut format = 0;
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix(FORMAT_KEY) {
            format = rest.trim().parse::<u32>().unwrap_or(0);
            continue;
        }
        if let Some((hash, rel)) = line.split_once(' ')
            && let Ok(h) = hash.parse::<u64>()
        {
            entries.insert(rel.to_string(), h);
        }
    }
    Ok(Index { format, entries })
}

pub fn save_index(path: &Path, index: &Index) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut body = format!("{FORMAT_KEY} {GRAPH_FORMAT}\n");
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
        .filter_entry(|e| e.file_name() != "node_modules")
        .build()
        .flatten()
    {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        if !matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("rs") | Some("py") | Some("ts") | Some("tsx") | Some("mts") | Some("cts")
        ) {
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
    // An empty index is "nothing built yet", which the per-file loop below
    // already reports as wholly stale. Only an index that actually describes
    // a graph can be outdated.
    if !index.entries.is_empty() && index.format != GRAPH_FORMAT {
        return Freshness::Outdated {
            found: index.format,
            expected: GRAPH_FORMAT,
        };
    }
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

/// Every file extension the graph has an extractor for. A file outside this
/// set is not tracked, not indexed, and not counted as drift.
pub const TRACKED_EXTENSIONS: &[&str] = &[".rs", ".py", ".ts", ".tsx", ".mts", ".cts"];

fn is_tracked(file_path: &str) -> bool {
    TRACKED_EXTENSIONS.iter().any(|e| file_path.ends_with(e))
}

/// Route one file to the extractor for its language.
fn extract_one(rel: &str, content: &str, units: &UnitMap) -> super::extract::Extracted {
    let unit = units.context_for(rel);
    match super::unit::lang_of_path(rel) {
        Some(super::unit::LANG_PYTHON) => super::python::extract_python(rel, content, &unit),
        Some(super::unit::LANG_TYPESCRIPT) => {
            super::typescript::extract_typescript(rel, content, &unit)
        }
        _ => extract_rust(rel, content, DEFAULT_WATCHLIST, &unit),
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
    if !is_tracked(file_path) {
        return Ok(SaveOutcome {
            base: 0,
            derived: 0,
            skipped: 0,
        });
    }
    let ipath = index_path(root);
    let mut index = load_index(&ipath)?;
    // A graph written under an older identity scheme cannot be patched one
    // file at a time: compaction replaces only the edited file's edges, so
    // every other file would keep its old names and the two halves would
    // never join. Rebuild once, then record this edit on top of the result.
    if !index.entries.is_empty() && index.format != GRAPH_FORMAT {
        rebuild(root)?;
        index = load_index(&ipath)?;
    }

    let units = UnitMap::discover(root);
    let extracted = extract_one(file_path, content, &units);
    let existing = store::load(&store::graph_path(root))?;
    if extracted.parse_failed {
        // Leave the graph and the index exactly as they were. Compacting the
        // empty edge set would erase the file's evidence, and recording the
        // unparseable content's hash would report the result fresh — the
        // harness would then keep enforcing on facts it had just deleted.
        // Leaving the hash stale makes freshness report the file as drifted,
        // which demotes structural rules to warnings: the honest state.
        let base = existing.iter().filter(|e| !e.d).count();
        return Ok(SaveOutcome {
            base,
            derived: existing.len() - base,
            skipped: extracted.skipped,
        });
    }
    let base = store::compact(existing, file_path, extracted.edges);
    let (n_base, n_derived) = persist(root, base)?;

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
    // Only maintain a graph the project actually opted into. The sensor runs
    // before rules are loaded — it has to, since the structural pack ships
    // `phase: "pre"` rules exclusively — so it cannot ask whether a graph
    // rule exists. The graph's own presence is the opt-in signal: `init
    // --packs structural` builds it, and `graph rebuild` restores it. Without
    // this, every phronesis project would start building and rewriting a code
    // graph on every save whether or not it has a single structural rule.
    if !store::graph_path(root).exists() && !index_path(root).exists() {
        return;
    }
    // Hosts send absolute paths while the graph is keyed repo-relative, so
    // relativize first. Rejecting an absolute path outright — as this guard
    // once did — turned the sensor off for every real host, and because the
    // sensor is best-effort it failed silently: the graph simply never
    // updated and the structural pack demoted itself to warnings forever.
    let Some(rel) = super::hydrate::repo_relative(root, file_path) else {
        // Outside the project: `repo_relative` is the containment check.
        return;
    };
    // Never follow a path out of the project — the sensor reads whatever it
    // is handed, and a traversal would pull unrelated files into the graph.
    if rel.contains("..") || Path::new(&rel).is_absolute() {
        return;
    }
    let file_path = rel.as_str();
    let content = match std::fs::read_to_string(root.join(file_path)) {
        Ok(content) => content,
        // The file is gone — a `Delete File` patch block, or a delete routed
        // through the hook. Leaving its edges and its index entry behind
        // makes the very next freshness check report drift, which demotes
        // every structural rule to a warning until someone rebuilds by hand.
        // An empty extraction lets provenance-keyed compaction drop them.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Err(e) = on_delete(root, file_path) {
                tracing::debug!("graph sensor could not record deletion of {file_path}: {e}");
            }
            return;
        }
        Err(_) => return,
    };
    if let Err(e) = on_save(root, file_path, &content) {
        tracing::debug!("graph sensor skipped {file_path}: {e}");
    }
}

/// Drop a vanished file from both the graph and the staleness index.
///
/// Compaction is keyed on provenance, so replacing the file's edge set with
/// nothing removes exactly its contribution. The index entry must go too: a
/// hash recorded against a path that no longer exists is stale evidence that
/// `check_freshness` reports as drift forever.
fn on_delete(root: &Path, file_path: &str) -> std::io::Result<()> {
    let existing = store::load(&store::graph_path(root))?;
    let base = store::compact(existing, file_path, Vec::new());
    persist(root, base)?;

    let ipath = index_path(root);
    let mut index = load_index(&ipath)?;
    if index.entries.remove(file_path).is_some() {
        save_index(&ipath, &index)?;
    }
    Ok(())
}

/// Full rescan of every tracked Rust file. The recovery path after the graph
/// has drifted, and the only way edges for deleted files are cleared.
pub fn rebuild(root: &Path) -> std::io::Result<SaveOutcome> {
    let mut base = Vec::new();
    let mut index = Index::default();
    let mut skipped = 0;
    let units = UnitMap::discover(root);

    for rel in tracked_files(root) {
        let Ok(content) = std::fs::read_to_string(root.join(&rel)) else {
            continue;
        };
        let extracted = extract_one(&rel, &content, &units);
        skipped += extracted.skipped;
        if extracted.parse_failed {
            // Not indexed: a rebuild cannot invent evidence for a file it
            // cannot read, and claiming to have indexed it would report the
            // graph fresh while the file contributes nothing.
            continue;
        }
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
        assert_eq!(load_index(&p).expect("load").entries, idx.entries);
    }

    #[test]
    fn a_written_index_is_stamped_with_the_current_format() {
        let d = project();
        let p = index_path(d.path());
        save_index(&p, &Index::default()).expect("save");
        assert_eq!(load_index(&p).expect("load").format, GRAPH_FORMAT);
    }

    #[test]
    fn an_index_without_a_format_header_reads_as_format_zero() {
        // The shape written before identity versioning existed.
        let d = project();
        let p = index_path(d.path());
        write(d.path(), INDEX_REL_PATH, "42 src/a.rs\n");
        let idx = load_index(&p).expect("load");
        assert_eq!(idx.format, 0);
        assert_eq!(idx.entries.get("src/a.rs"), Some(&42));
    }

    // ─── identity-format migration ──────────────────────────────────

    #[test]
    fn a_graph_built_under_an_older_identity_format_is_not_fresh() {
        // Content hashes match exactly — nothing on disk changed. Only the
        // format header betrays that every edge carries the old naming.
        let d = project();
        write(d.path(), "src/a.rs", "fn f() {}");
        let index = Index {
            format: 0,
            entries: BTreeMap::from([("src/a.rs".to_string(), hash_content("fn f() {}"))]),
        };
        assert_eq!(
            check_freshness(d.path(), &index),
            Freshness::Outdated {
                found: 0,
                expected: GRAPH_FORMAT,
            }
        );
    }

    #[test]
    fn an_index_that_describes_nothing_is_never_reported_as_outdated() {
        // A project that has never built a graph has format 0 too; calling
        // that a migration would demand a rebuild of an empty graph.
        let d = project();
        assert_ne!(
            check_freshness(d.path(), &Index::default()),
            Freshness::Outdated {
                found: 0,
                expected: GRAPH_FORMAT,
            }
        );
    }

    #[test]
    fn saving_into_an_older_format_graph_rebuilds_every_file() {
        let d = project();
        write(d.path(), "src/a.rs", "fn alpha() {}");
        write(d.path(), "src/b.rs", "fn beta() {}");
        // A graph in the pre-versioning naming, with an index that says it is
        // current for both files.
        store::write_atomic(
            &store::graph_path(d.path()),
            &[
                Edge::base("defines_fn", &["src/a.rs", "crate::a::alpha"], "src/a.rs"),
                Edge::base("defines_fn", &["src/b.rs", "crate::b::beta"], "src/b.rs"),
            ],
        )
        .expect("seed graph");
        save_index(
            &index_path(d.path()),
            &Index {
                format: 0,
                entries: BTreeMap::from([
                    ("src/a.rs".to_string(), hash_content("fn alpha() {}")),
                    ("src/b.rs".to_string(), hash_content("fn beta() {}")),
                ]),
            },
        )
        .expect("seed index");
        // Written by `save_index`, which always stamps the current format;
        // force the legacy shape back onto disk.
        std::fs::write(index_path(d.path()), "0 src/a.rs\n0 src/b.rs\n").expect("legacy index");

        on_save(d.path(), "src/a.rs", "fn alpha() {}").expect("save");

        let names: Vec<String> = edges(d.path())
            .into_iter()
            .filter(|e| e.p == "defines_fn")
            .filter_map(|e| e.a.get(1).cloned())
            .collect();
        assert!(
            names.iter().all(|n| n.starts_with("rust:")),
            "the unedited file must be migrated too, not left in the old naming: {names:?}"
        );
        assert!(names.iter().any(|n| n.ends_with("::beta")), "{names:?}");
        assert_eq!(
            load_index(&index_path(d.path())).expect("load").format,
            GRAPH_FORMAT,
            "a migrated graph must stop reporting itself outdated"
        );
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
        assert_eq!(names, vec!["rust:crate::a::f".to_string()]);
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

    #[test]
    fn saving_a_typescript_file_writes_its_base_edges() {
        let d = project();
        write(d.path(), "package.json", r#"{"name": "myapp"}"#);
        write(
            d.path(),
            "src/billing.ts",
            "export function charge() { return 1 }\n",
        );
        rebuild(d.path()).expect("opt the project into the graph");
        let names: Vec<String> = edges(d.path())
            .into_iter()
            .filter(|e| e.p == "defines_fn")
            .filter_map(|e| e.a.get(1).cloned())
            .collect();
        // No tsconfig.json exists, so there is no baseUrl: `src/` stays part
        // of the module path (see graph/resolve.rs's `strip_module_base` and
        // `graph/unit.rs`'s `context_for`, which only strips a unit's
        // `baseUrl` from the front of a file's path).
        assert!(
            names.contains(&"typescript:myapp::src::billing::charge".to_string()),
            "{names:?}"
        );
    }

    // ─── the hook entry point ───────────────────────────────────────

    #[test]
    fn a_project_that_never_opted_into_the_graph_gets_no_graph() {
        // The sensor runs before rules load, so it cannot key off rule
        // phases. Without an explicit gate it builds a graph in every
        // phronesis project on the first edit — imposing a per-save tree walk
        // and an unasked-for file on users who only wanted the `llm` pack.
        let d = project();
        write(d.path(), "src/a.rs", "fn f() {}");
        record_from_disk(d.path(), "src/a.rs");
        assert!(
            !store::graph_path(d.path()).exists(),
            "no graph existed, so none should be created"
        );
        assert!(!index_path(d.path()).exists());
    }

    #[test]
    fn an_existing_graph_is_still_kept_current() {
        // `init --packs structural` builds the graph; its presence is the
        // opt-in signal the sensor keys off.
        let d = project();
        write(d.path(), "src/a.rs", "fn f() {}");
        rebuild(d.path()).expect("rebuild opts the project in");
        write(d.path(), "src/a.rs", "fn f() {}\nfn added() {}");
        record_from_disk(d.path(), "src/a.rs");
        let names: Vec<String> = edges(d.path())
            .into_iter()
            .filter(|e| e.p == "defines_fn")
            .filter_map(|e| e.a.get(1).cloned())
            .collect();
        assert!(names.iter().any(|n| n.ends_with("::added")), "{names:?}");
    }

    #[test]
    fn recording_from_disk_accepts_the_absolute_path_a_host_sends() {
        // Every real host sends an absolute path. A guard that rejected them
        // silently disabled the sensor everywhere, and because the sensor is
        // best-effort by design nothing reported it.
        let d = project();
        write(d.path(), "src/a.rs", "fn f() {}");
        rebuild(d.path()).expect("opt the project into the graph");
        write(d.path(), "src/a.rs", "fn f() {}\nfn added() {}");
        let absolute = d.path().join("src/a.rs");
        record_from_disk(d.path(), absolute.to_str().expect("utf8"));
        let names: Vec<String> = edges(d.path())
            .into_iter()
            .filter(|e| e.p == "defines_fn")
            .filter_map(|e| e.a.get(1).cloned())
            .collect();
        assert!(
            names.iter().any(|n| n.ends_with("::added")),
            "sensor must record the edit: {names:?}"
        );
    }

    #[test]
    fn a_parse_failure_preserves_the_files_existing_edges() {
        // A malformed mid-edit save must not be read as "this file now
        // defines nothing". Compacting an empty extraction erases every
        // function, risky call and import the file had, and recording the
        // malformed content's hash makes the graph report itself fresh — so
        // the harness keeps enforcing on evidence it silently destroyed.
        let d = project();
        write(d.path(), "src/a.rs", "fn important() {}");
        on_save(d.path(), "src/a.rs", "fn important() {}").expect("save");
        assert!(has(d.path(), "defines_fn"), "precondition");

        let broken = "fn important( { ((( ";
        write(d.path(), "src/a.rs", broken);
        on_save(d.path(), "src/a.rs", broken).expect("save");

        let names: Vec<String> = edges(d.path())
            .into_iter()
            .filter(|e| e.p == "defines_fn")
            .filter_map(|e| e.a.get(1).cloned())
            .collect();
        assert!(
            names.iter().any(|n| n.ends_with("::important")),
            "existing evidence must survive a parse failure: {names:?}"
        );
    }

    #[test]
    fn a_parse_failure_leaves_the_file_reported_as_stale() {
        // Staleness is the honest state: the graph no longer reflects what is
        // on disk, and structural rules must demote to warnings until it does.
        let d = project();
        write(d.path(), "src/a.rs", "fn important() {}");
        on_save(d.path(), "src/a.rs", "fn important() {}").expect("save");

        let broken = "fn important( { ((( ";
        write(d.path(), "src/a.rs", broken);
        on_save(d.path(), "src/a.rs", broken).expect("save");

        let index = load_index(&index_path(d.path())).expect("load index");
        assert_eq!(
            check_freshness(d.path(), &index),
            Freshness::Stale(vec!["src/a.rs".to_string()]),
            "an unparseable file must not be recorded as successfully indexed"
        );
    }

    #[test]
    fn deleting_a_file_removes_its_edges_and_its_index_entry() {
        // A `Delete File` patch block routes through the sensor. Leaving the
        // edges and hash behind makes every later freshness check report
        // drift, demoting structural rules to warnings until a manual
        // rebuild — the exact failure the sensor exists to prevent.
        let d = project();
        write(d.path(), "src/a.rs", "fn alpha() {}");
        write(d.path(), "src/b.rs", "fn beta() {}");
        on_save(d.path(), "src/a.rs", "fn alpha() {}").expect("save a");
        on_save(d.path(), "src/b.rs", "fn beta() {}").expect("save b");

        std::fs::remove_file(d.path().join("src/a.rs")).expect("delete");
        record_from_disk(d.path(), "src/a.rs");

        let names: Vec<String> = edges(d.path())
            .into_iter()
            .filter(|e| e.p == "defines_fn")
            .filter_map(|e| e.a.get(1).cloned())
            .collect();
        assert!(
            names.iter().all(|n| !n.ends_with("::alpha")),
            "the deleted file's edges must go: {names:?}"
        );
        assert!(names.iter().any(|n| n.ends_with("::beta")), "{names:?}");
        assert!(
            !load_index(&index_path(d.path()))
                .expect("load index")
                .entries
                .contains_key("src/a.rs"),
            "a hash for a path that no longer exists is permanent drift"
        );
        assert_eq!(
            check_freshness(d.path(), &load_index(&index_path(d.path())).expect("index")),
            Freshness::Fresh,
            "a recorded deletion must leave the graph fresh"
        );
    }

    #[test]
    fn recording_from_disk_ignores_a_path_outside_the_project() {
        let d = project();
        let outside = TempDir::new().expect("tempdir");
        write(outside.path(), "evil.rs", "fn f() {}");
        let absolute = outside.path().join("evil.rs");
        record_from_disk(d.path(), absolute.to_str().expect("utf8"));
        assert!(
            edges(d.path()).is_empty(),
            "a file outside the project has no graph identity"
        );
    }

    #[test]
    fn recording_from_disk_reads_the_current_file_content() {
        let d = project();
        write(d.path(), "src/a.rs", "fn f() {}");
        rebuild(d.path()).expect("opt the project into the graph");
        write(d.path(), "src/a.rs", "fn f() {}\nfn later() {}");
        record_from_disk(d.path(), "src/a.rs");
        let names: Vec<String> = edges(d.path())
            .into_iter()
            .filter(|e| e.p == "defines_fn")
            .filter_map(|e| e.a.get(1).cloned())
            .collect();
        assert!(names.iter().any(|n| n.ends_with("::later")), "{names:?}");
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
    fn rebuild_excludes_node_modules_from_typescript_tracking() {
        // `tracked_files` drives both `rebuild` and the freshness check. If
        // `node_modules` is not pruned there too — discovery already prunes
        // it, but that is a separate walk — `rebuild` would extract from
        // every dependency's TypeScript, and every one of those files would
        // then show as drift on every subsequent freshness check.
        let d = project();
        write(d.path(), "package.json", r#"{"name": "myapp"}"#);
        write(
            d.path(),
            "src/billing.ts",
            "export function charge() { return 1 }\n",
        );
        write(
            d.path(),
            "node_modules/dep/index.ts",
            "export function vendored() { return 1 }\n",
        );
        rebuild(d.path()).expect("rebuild");

        let names: Vec<String> = edges(d.path())
            .into_iter()
            .filter(|e| e.p == "defines_fn")
            .filter_map(|e| e.a.get(1).cloned())
            .collect();
        assert!(
            names.iter().all(|n| !n.contains("vendored")),
            "node_modules must not be extracted: {names:?}"
        );

        let idx = load_index(&index_path(d.path())).expect("load index");
        assert!(
            !idx.entries.keys().any(|k| k.contains("node_modules")),
            "node_modules must not be indexed: {:?}",
            idx.entries.keys().collect::<Vec<_>>()
        );
        assert_eq!(
            check_freshness(d.path(), &idx),
            Freshness::Fresh,
            "an untracked node_modules file must not be reported as drift"
        );
    }

    #[test]
    fn a_typescript_project_is_fresh_immediately_after_rebuild() {
        // Adding four new tracked extensions means files that were
        // previously invisible to `tracked_files` are now tracked. A
        // TypeScript project must report Fresh right after `rebuild`, not
        // immediately show drift.
        let d = project();
        write(d.path(), "package.json", r#"{"name": "myapp"}"#);
        write(
            d.path(),
            "src/billing.ts",
            "export function charge() { return 1 }\n",
        );
        rebuild(d.path()).expect("rebuild");
        let idx = load_index(&index_path(d.path())).expect("load index");
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

//! The staleness index: its on-disk format, the walk that decides which
//! files it covers, and the freshness comparison.

use super::{FORMAT_KEY, Freshness, GENERATION_KEY, GRAPH_FORMAT, Index, hash_content};
use std::collections::BTreeMap;
use std::path::Path;

/// A missing or unreadable index means "nothing known yet", which callers
/// treat as stale. Failing closed here would take enforcement offline for a
/// recoverable, self-healing condition.
pub fn load_index(path: &Path) -> std::io::Result<Index> {
    let body = match std::fs::read_to_string(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Index::default()),
        result => result?,
    };
    let (format, generation, entries) = {
        let mut entries = BTreeMap::new();
        let mut format = 0;
        let mut generation = 0;
        for line in body.lines() {
            if let Some(rest) = line.strip_prefix(FORMAT_KEY) {
                format = rest.trim().parse::<u32>().unwrap_or(0);
                continue;
            }
            if let Some(rest) = line.strip_prefix(GENERATION_KEY) {
                generation = rest.trim().parse::<u64>().unwrap_or(0);
                continue;
            }
            if let Some((hash, rel)) = line.split_once(' ')
                && let Ok(h) = hash.parse::<u64>()
            {
                entries.insert(rel.to_string(), h);
            }
        }
        (format, generation, entries)
    };
    Ok(Index {
        format,
        generation,
        entries,
    })
}

pub fn save_index(path: &Path, index: &Index) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut body = format!(
        "{FORMAT_KEY} {GRAPH_FORMAT}\n{GENERATION_KEY} {}\n",
        index.generation
    );
    for (rel, hash) in &index.entries {
        body.push_str(&format!("{hash} {rel}\n"));
    }
    let tmp = path.with_extension("index.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)
}

/// Every file under `root`, in a tracked language (Rust, Python,
/// TypeScript, Swift, Lua, CUE, JSON, YAML, Helm), that the graph should track, as
/// paths relative to `root`.  Honours `.gitignore` so build output and
/// vendored trees never enter the graph, and prunes `node_modules`
/// unconditionally — `.gitignore` alone cannot be relied on to exclude it,
/// and `is_tracked` must agree with this walk or a sensor-recorded file
/// becomes permanent drift.
pub(super) fn tracked_files(root: &Path) -> Vec<String> {
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
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.trim_start_matches('.'))
            .unwrap_or("");
        if !matches!(
            ext,
            "rs" | "py"
                | "ts"
                | "tsx"
                | "mts"
                | "cts"
                | "swift"
                | "lua"
                | "rhai"
                | "cue"
                | "json"
                | "yaml"
                | "yml"
                | "tpl"
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

pub(super) fn decision_input_files(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if root.join(".phronesis/rules.json").is_file() {
        out.push(".phronesis/rules.json".to_string());
    }
    let dir = root.join(".phronesis/wiki/decisions");
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("md")
                || path.file_name().and_then(|name| name.to_str()) == Some("README.md")
            {
                continue;
            }
            if let Ok(rel) = path.strip_prefix(root)
                && let Some(rel) = rel.to_str()
            {
                out.push(rel.replace('\\', "/"));
            }
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
    let mut on_disk = tracked_files(root);
    if root.join(".phronesis/graph.toml").is_file() {
        on_disk.push(".phronesis/graph.toml".to_string());
    }
    on_disk.extend(decision_input_files(root));

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
///
/// Covers Rust, Python, TypeScript (and siblings), Swift, Lua, CUE, JSON, YAML,
/// and Helm3 template files.
pub const TRACKED_EXTENSIONS: &[&str] = &[
    ".rs", ".py", ".ts", ".tsx", ".mts", ".cts", ".swift", ".lua", ".rhai", ".cue", ".json",
    ".yaml", ".yml", ".tpl",
];

/// Whether `on_save`/`record_from_disk` should index this file.
///
/// Must agree with `tracked_files`'s walk, which prunes `node_modules`.
/// Without the same exclusion here, the sensor can record an index entry for
/// a path `tracked_files` will never enumerate — a hash `check_freshness`
/// can then never match, so the file reports as permanent drift until a
/// manual `rebuild`. Matched by path *component*, not substring, so a
/// legitimate directory like `my_node_modules_helper` is not excluded.
pub(super) fn is_tracked(file_path: &str) -> bool {
    if Path::new(file_path)
        .components()
        .any(|c| c.as_os_str() == "node_modules")
    {
        return false;
    }
    TRACKED_EXTENSIONS.iter().any(|e| file_path.ends_with(e))
}

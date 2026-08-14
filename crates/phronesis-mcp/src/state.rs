//! Inventory and conservative cleanup for `.phronesis` project state.

use serde::Serialize;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StateEntry {
    pub path: String,
    pub class: &'static str,
    pub bytes: u64,
    pub tracked: bool,
    pub sensitive: bool,
}

const ENTRIES: &[(&str, &str, bool)] = &[
    ("rules.json", "authored", false),
    ("wiki", "authored", false),
    ("predicates", "authored", false),
    ("kernel.md", "authored", false),
    ("durable.md", "authored", false),
    ("context.json", "authored", false),
    ("journey.json", "authored", false),
    ("confidence.json", "authored", false),
    ("bugs.json", "authored", false),
    ("toolchains.json", "authored", false),
    ("nudges", "authored", false),
    ("graph.jsonl", "cache", false),
    ("graph.index", "cache", false),
    ("bindings.json", "cache", false),
    ("log.jsonl", "history", false),
    ("log.jsonl.1", "history", false),
    ("journey", "history", false),
    ("outcomes", "runtime", false),
    ("rules.json.bak", "backup", false),
    ("captures", "sensitive", true),
];

pub fn inspect(root: &Path) -> Vec<StateEntry> {
    let dir = root.join(".phronesis");
    let mut entries = ENTRIES
        .iter()
        .filter_map(|(name, class, sensitive)| {
            let path = dir.join(name);
            path.exists().then(|| StateEntry {
                path: format!(".phronesis/{name}"),
                class,
                bytes: size(&path),
                tracked: is_tracked(root, &path),
                sensitive: *sensitive,
            })
        })
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    entries
}

/// Remove only graph state that a rebuild recreates. The targets are fixed;
/// callers cannot broaden cleanup with user-provided paths or globs.
pub fn clean_cache(root: &Path) -> std::io::Result<Vec<String>> {
    let dir = root.join(".phronesis");
    let mut removed = Vec::new();
    for name in ["graph.jsonl", "graph.index", "bindings.json"] {
        let path = dir.join(name);
        if path.is_file() {
            fs::remove_file(&path)?;
            removed.push(format!(".phronesis/{name}"));
        }
    }
    Ok(removed)
}

fn size(path: &Path) -> u64 {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return 0;
    };
    if metadata.is_file() {
        return metadata.len();
    }
    fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| size(&entry.path()))
        .sum()
}

fn is_tracked(root: &Path, path: &Path) -> bool {
    std::process::Command::new("git")
        .args(["ls-files", "--error-unmatch", "--"])
        .arg(path.strip_prefix(root).unwrap_or(path))
        .current_dir(root)
        .output()
        .is_ok_and(|output| output.status.success())
}

pub fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_removes_only_rebuildable_graph_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let phr = dir.path().join(".phronesis");
        fs::create_dir(&phr).expect("mkdir");
        for name in [
            "graph.jsonl",
            "graph.index",
            "bindings.json",
            "rules.json",
            "log.jsonl",
        ] {
            fs::write(phr.join(name), name).expect("fixture");
        }
        assert_eq!(clean_cache(dir.path()).expect("clean").len(), 3);
        assert!(phr.join("rules.json").exists());
        assert!(phr.join("log.jsonl").exists());
    }

    #[test]
    fn capture_directory_is_reported_as_sensitive() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join(".phronesis/captures")).expect("mkdir");
        fs::write(dir.path().join(".phronesis/captures/payloads.jsonl"), "raw").expect("fixture");
        let entries = inspect(dir.path());
        assert!(
            entries
                .iter()
                .any(|entry| entry.sensitive && entry.bytes == 3)
        );
    }
}

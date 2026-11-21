//! Local agent state policy.
//!
//! The whole root `.phronesis/` directory is local-only (anchored
//! `/.phronesis/` ignore). Reviewed policy that used to be tracked there
//! travels in presets under `config/phronesis/`, and [`setup`] provisions
//! the local state from those presets with dry-run support, idempotency,
//! conflict reporting, and preservation of customized files.
//!
//! Explicit opt-in via `phr-mcp provision`; ordinary initialization preserves
//! each project's existing state policy.

use std::io::Write;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// Root-local agent state directory.
pub const ROOT_STATE_DIR: &str = ".phronesis";

/// Tracked presets directory: reviewed developer policy copied from the
/// previously tracked `.phronesis/` files.
pub const PRESETS_DIR: &str = "config/phronesis";

/// The anchored whole-directory ignore entry adopted by provisioning.
pub const ROOT_IGNORE_ENTRY: &str = "/.phronesis/";

const IGNORE_COMMENT: &str =
    "# Local agent state (whole directory; presets live in config/phronesis/)";

/// `.gitignore` lines previous phronesis versions managed. Under the
/// whole-directory policy they are subsumed or wrong (the `!` carveouts
/// would re-expose local state), so init and setup remove them instead of
/// extending them. The `**/.phronesis/...` runtime lines for nested
/// projects are not managed here and stay untouched.
const LEGACY_MANAGED_LINES: &[&str] = &[
    ".phronesis/log.jsonl",
    ".phronesis/log.jsonl.1",
    ".phronesis/rules.json.bak",
    ".phronesis/*",
];

fn is_legacy_managed(line: &str) -> bool {
    LEGACY_MANAGED_LINES.contains(&line) || line.starts_with("!.phronesis/")
}

#[derive(Debug, Clone, Default)]
pub struct IgnoreOutcome {
    /// `/.phronesis/` was missing and would be (or was) appended.
    pub added_entry: bool,
    /// Bare `.phronesis/` lines rewritten to the anchored form.
    pub migrated_bare_lines: usize,
    /// Legacy managed lines (broad ignore + carveouts) removed.
    pub removed_legacy_lines: usize,
    /// Duplicate anchored entries collapsed to the first occurrence.
    pub deduped_anchored_lines: usize,
    pub dry_run: bool,
}

impl IgnoreOutcome {
    pub fn changed(&self) -> bool {
        self.added_entry
            || self.migrated_bare_lines > 0
            || self.removed_legacy_lines > 0
            || self.deduped_anchored_lines > 0
    }
}

#[derive(Debug, Error)]
pub enum SetupError {
    #[error("refusing to provision through symbolic link {path}; customized paths are preserved")]
    SymbolicLink { path: String },
    #[error("{action} {path}: {source}")]
    Io {
        action: &'static str,
        path: String,
        #[source]
        source: std::io::Error,
    },
}

/// Provisioning owns regular files below the project, not linked destinations.
fn reject_symlinks(root: &Path, relative: &Path) -> Result<(), SetupError> {
    let mut path = root.to_path_buf();
    for part in relative.components() {
        path.push(part);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(SetupError::SymbolicLink {
                    path: path.display().to_string(),
                });
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(source) => {
                return Err(SetupError::Io {
                    action: "inspect",
                    path: path.display().to_string(),
                    source,
                });
            }
        }
    }
    Ok(())
}

/// Ensure `.gitignore` carries the anchored whole-directory ignore for the
/// root `.phronesis/` directory, migrating the legacy managed lines out.
/// Idempotent: a second run finds the entry present and nothing to migrate.
pub fn ensure_root_ignore(root: &Path, dry_run: bool) -> Result<IgnoreOutcome, SetupError> {
    reject_symlinks(root, Path::new(".gitignore"))?;
    let path = root.join(".gitignore");
    let original = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(SetupError::Io {
                action: "read",
                path: path.display().to_string(),
                source,
            });
        }
    };

    let had_trailing_newline = original.ends_with('\n');
    let mut outcome = IgnoreOutcome {
        dry_run,
        ..IgnoreOutcome::default()
    };
    let mut lines: Vec<String> = Vec::new();
    let mut saw_anchored = false;
    for line in original.lines() {
        // The legacy bare form is rewritten to the anchored whole-directory
        // entry; a later duplicate of the anchored entry collapses into it.
        let line = if line == ".phronesis/" {
            outcome.migrated_bare_lines += 1;
            ROOT_IGNORE_ENTRY.to_string()
        } else {
            line.to_string()
        };
        if is_legacy_managed(&line) {
            outcome.removed_legacy_lines += 1;
            continue;
        }
        if line == ROOT_IGNORE_ENTRY {
            if saw_anchored {
                outcome.deduped_anchored_lines += 1;
                continue;
            }
            saw_anchored = true;
        }
        lines.push(line);
    }
    if !saw_anchored {
        outcome.added_entry = true;
        if !lines.is_empty() && lines.last().map(String::as_str) != Some("") {
            lines.push(String::new());
        }
        lines.push(IGNORE_COMMENT.to_string());
        lines.push(ROOT_IGNORE_ENTRY.to_string());
    }

    if !outcome.changed() {
        return Ok(outcome);
    }
    if dry_run {
        return Ok(outcome);
    }

    let mut content = lines.join("\n");
    if had_trailing_newline || !content.is_empty() {
        content.push('\n');
    }
    std::fs::write(&path, content).map_err(|source| SetupError::Io {
        action: "write",
        path: path.display().to_string(),
        source,
    })?;
    Ok(outcome)
}

#[derive(Debug, Clone, Default)]
pub struct SetupReport {
    /// `config/phronesis/` exists and holds at least one preset file.
    pub presets_present: bool,
    /// Preset files written into local state (or that would be written).
    pub provisioned: Vec<String>,
    /// Local files already matching their preset (no action).
    pub identical: Vec<String>,
    /// Local files that differ from their preset: reported and kept local
    /// (customized files are never overwritten).
    pub conflicts: Vec<String>,
    pub ignore: IgnoreOutcome,
    pub dry_run: bool,
}

/// Provision local `.phronesis/` state from the tracked presets in
/// `config/phronesis/`.
///
/// For every preset file, the matching local file is created when missing,
/// left alone when identical (idempotent re-runs are no-ops), and reported
/// as a conflict when it differs — customized local files are preserved,
/// never overwritten. Runtime state the presets do not cover is untouched.
pub fn setup(root: &Path, dry_run: bool) -> Result<SetupReport, SetupError> {
    let mut report = SetupReport {
        dry_run,
        ..SetupReport::default()
    };

    reject_symlinks(root, Path::new(PRESETS_DIR))?;
    let presets = root.join(PRESETS_DIR);
    let mut preset_files = Vec::new();
    collect_files(&presets, &mut preset_files).map_err(|source| SetupError::Io {
        action: "scan",
        path: presets.display().to_string(),
        source,
    })?;
    preset_files.sort();
    report.presets_present = !preset_files.is_empty();
    // Inspect every destination before changing files, including ignore policy.
    for preset in &preset_files {
        if let Ok(relative) = preset.strip_prefix(&presets) {
            reject_symlinks(root, &Path::new(ROOT_STATE_DIR).join(relative))?;
        }
    }
    report.ignore = ensure_root_ignore(root, dry_run)?;

    for preset in preset_files {
        let rel = preset
            .strip_prefix(&presets)
            .map_err(|source| SetupError::Io {
                action: "resolve",
                path: preset.display().to_string(),
                source: std::io::Error::other(source),
            })?;
        let target = root.join(ROOT_STATE_DIR).join(rel);
        let rel_display = format!("{ROOT_STATE_DIR}/{}", rel.display());
        let preset_bytes = std::fs::read(&preset).map_err(|source| SetupError::Io {
            action: "read",
            path: preset.display().to_string(),
            source,
        })?;
        let local = match std::fs::read(&target) {
            Ok(bytes) => Some(bytes),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => None,
            Err(source) => {
                return Err(SetupError::Io {
                    action: "read",
                    path: target.display().to_string(),
                    source,
                });
            }
        };
        match local {
            None => {
                report.provisioned.push(rel_display);
                if dry_run {
                    continue;
                }
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(|source| SetupError::Io {
                        action: "create",
                        path: parent.display().to_string(),
                        source,
                    })?;
                }
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&target)
                    .map_err(|source| SetupError::Io {
                        action: "create without replacing",
                        path: target.display().to_string(),
                        source,
                    })?;
                file.write_all(&preset_bytes)
                    .map_err(|source| SetupError::Io {
                        action: "write",
                        path: target.display().to_string(),
                        source,
                    })?;
            }
            Some(bytes) if bytes == preset_bytes => report.identical.push(rel_display),
            Some(_) => report.conflicts.push(rel_display),
        }
    }
    Ok(report)
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "preset symbolic link is not a regular preset: {}",
                    path.display()
                ),
            ));
        }
        if kind.is_dir() {
            collect_files(&path, out)?;
        } else if kind.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

/// Render the report as init-style step lines for terminal output.
pub fn render_report(report: &SetupReport) -> String {
    let mut out = String::new();
    if report.dry_run {
        out.push_str("dry run (nothing written)\n");
    }
    if !report.presets_present {
        out.push_str("= no presets found in config/phronesis/ — nothing to provision\n");
    }
    for path in &report.provisioned {
        out.push_str(&format!(
            "{} {path}\n",
            if report.dry_run { "would +" } else { "+" }
        ));
    }
    for path in &report.identical {
        out.push_str(&format!("= {path} already matches its preset\n"));
    }
    for path in &report.conflicts {
        out.push_str(&format!(
            "! conflict: {path} differs from its preset — kept local (customized files are never overwritten)\n"
        ));
    }
    if report.ignore.added_entry {
        out.push_str(&format!(
            "{} {ROOT_IGNORE_ENTRY} ignore entry in .gitignore\n",
            if report.dry_run { "would +" } else { "+" }
        ));
    }
    if report.ignore.migrated_bare_lines > 0 {
        out.push_str(&format!(
            "~ migrated {} bare `.phronesis/` ignore line(s) to the anchored form\n",
            report.ignore.migrated_bare_lines
        ));
    }
    if report.ignore.removed_legacy_lines > 0 {
        out.push_str(&format!(
            "~ removed {} legacy `.phronesis` carveout line(s) superseded by the whole-directory ignore\n",
            report.ignore.removed_legacy_lines
        ));
    }
    if !report.ignore.changed() {
        out.push_str("= .gitignore already carries the anchored ignore entry\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn setup_provisions_missing_files() {
        let root = TempDir::new().unwrap();
        write(root.path(), "config/phronesis/kernel.md", "kernel\n");
        write(root.path(), "config/phronesis/nudges/x.md", "nudge\n");
        let report = setup(root.path(), false).unwrap();
        assert_eq!(report.provisioned.len(), 2);
        assert!(root.path().join(".phronesis/kernel.md").exists());
        assert!(root.path().join(".phronesis/nudges/x.md").exists());
        assert!(root.path().join(".gitignore").exists());
    }

    #[test]
    fn setup_is_idempotent_and_preserves_customized_files() {
        let root = TempDir::new().unwrap();
        write(root.path(), "config/phronesis/kernel.md", "kernel\n");
        setup(root.path(), false).unwrap();
        // Customize the local copy after provisioning.
        write(root.path(), ".phronesis/kernel.md", "locally customized\n");
        let second = setup(root.path(), false).unwrap();
        assert!(second.provisioned.is_empty());
        assert_eq!(second.identical, Vec::<String>::new());
        assert_eq!(second.conflicts.len(), 1);
        assert_eq!(
            std::fs::read_to_string(root.path().join(".phronesis/kernel.md")).unwrap(),
            "locally customized\n"
        );
    }

    #[test]
    fn setup_dry_run_writes_nothing() {
        let root = TempDir::new().unwrap();
        write(root.path(), "config/phronesis/kernel.md", "kernel\n");
        let report = setup(root.path(), true).unwrap();
        assert_eq!(report.provisioned.len(), 1);
        assert!(!root.path().join(".phronesis").exists());
        assert!(!root.path().join(".gitignore").exists());
    }

    #[test]
    fn ensure_root_ignore_migrates_legacy_lines_and_is_idempotent() {
        let root = TempDir::new().unwrap();
        std::fs::write(
            root.path().join(".gitignore"),
            "/target\n.phronesis/*\n!.phronesis/wiki/\n!.phronesis/wiki/**\n.phronesis/log.jsonl\n",
        )
        .unwrap();
        let first = ensure_root_ignore(root.path(), false).unwrap();
        assert!(first.added_entry);
        assert_eq!(first.removed_legacy_lines, 4);
        let content = std::fs::read_to_string(root.path().join(".gitignore")).unwrap();
        assert!(content.contains(ROOT_IGNORE_ENTRY));
        assert!(!content.contains("!.phronesis/wiki/"));
        assert!(!content.contains(".phronesis/log.jsonl"));
        assert!(content.contains("/target"));
        // Repeated init/setup must not undo the anchored policy.
        let second = ensure_root_ignore(root.path(), false).unwrap();
        assert!(!second.changed());
        let after = std::fs::read_to_string(root.path().join(".gitignore")).unwrap();
        assert_eq!(after, content);
    }

    #[test]
    fn ensure_root_ignore_preserves_unrelated_lines() {
        let root = TempDir::new().unwrap();
        std::fs::write(
            root.path().join(".gitignore"),
            "/target\n**/.phronesis/log.jsonl\n",
        )
        .unwrap();
        ensure_root_ignore(root.path(), false).unwrap();
        let content = std::fs::read_to_string(root.path().join(".gitignore")).unwrap();
        assert!(content.contains("**/.phronesis/log.jsonl"));
        assert!(content.contains(ROOT_IGNORE_ENTRY));
    }
}

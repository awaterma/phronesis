//! The confidence *subject* — the work unit a score is about.
//!
//! Milestone behavior: an **implicit work unit**. A monotonic id is minted on
//! demand and held in `.phronesis/outcomes/current` until a build/test cycle
//! settles it; the next edit after that opens a fresh unit. The explicit
//! `submit_suggestion` path (later commit) writes a caller-chosen id to the
//! same place via [`set`].

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SubjectError {
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

fn dir(root: &Path) -> PathBuf {
    root.join(".phronesis").join("outcomes")
}

fn current_path(root: &Path) -> PathBuf {
    dir(root).join("current")
}

/// The currently open subject id, if any.
pub fn current(root: &Path) -> Option<String> {
    std::fs::read_to_string(current_path(root))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Return the open subject, minting and persisting a fresh one if none is open.
pub fn open(root: &Path) -> Result<String, SubjectError> {
    if let Some(s) = current(root) {
        return Ok(s);
    }
    let id = mint();
    set(root, &id)?;
    Ok(id)
}

/// Set the open subject explicitly — the `submit_suggestion` path.
pub fn set(root: &Path, id: &str) -> Result<(), SubjectError> {
    let d = dir(root);
    std::fs::create_dir_all(&d).map_err(|e| SubjectError::Io {
        path: d.display().to_string(),
        source: e,
    })?;
    let p = current_path(root);
    std::fs::write(&p, id).map_err(|e| SubjectError::Io {
        path: p.display().to_string(),
        source: e,
    })
}

/// Settle (close) the current subject — the next edit mints a new one. Settling
/// an already-closed subject is a no-op.
pub fn settle(root: &Path) -> Result<(), SubjectError> {
    let p = current_path(root);
    match std::fs::remove_file(&p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(SubjectError::Io {
            path: p.display().to_string(),
            source: e,
        }),
    }
}

/// A fresh, monotonic-ish subject id. Nanos give practical uniqueness even when
/// a settle/open happens within the same wall-clock second.
fn mint() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("unit-{nanos}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_is_none_when_unset() {
        let dir = tempfile::tempdir().unwrap();
        assert!(current(dir.path()).is_none());
    }

    #[test]
    fn open_mints_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let id = open(dir.path()).unwrap();
        assert!(id.starts_with("unit-"));
        assert_eq!(current(dir.path()).as_deref(), Some(id.as_str()));
    }

    #[test]
    fn open_is_stable_until_settled() {
        let dir = tempfile::tempdir().unwrap();
        let first = open(dir.path()).unwrap();
        let second = open(dir.path()).unwrap();
        assert_eq!(first, second, "open returns the same id until settled");
    }

    #[test]
    fn settle_then_open_mints_a_new_id() {
        let dir = tempfile::tempdir().unwrap();
        let first = open(dir.path()).unwrap();
        settle(dir.path()).unwrap();
        assert!(current(dir.path()).is_none());
        let second = open(dir.path()).unwrap();
        assert_ne!(first, second, "a new unit is minted after settle");
    }

    #[test]
    fn set_overrides_the_open_subject() {
        let dir = tempfile::tempdir().unwrap();
        open(dir.path()).unwrap();
        set(dir.path(), "xlate-7").unwrap();
        assert_eq!(current(dir.path()).as_deref(), Some("xlate-7"));
    }

    #[test]
    fn settle_when_absent_is_ok() {
        let dir = tempfile::tempdir().unwrap();
        assert!(settle(dir.path()).is_ok());
    }

    #[test]
    fn set_errors_when_outcomes_dir_is_blocked_by_a_file() {
        // `.phronesis/outcomes` exists as a file → create_dir_all fails and
        // set() surfaces the IO error rather than panicking.
        let dir = tempfile::tempdir().unwrap();
        let phr = dir.path().join(".phronesis");
        std::fs::create_dir_all(&phr).unwrap();
        std::fs::write(phr.join("outcomes"), "blocking file").unwrap();
        assert!(matches!(set(dir.path(), "u"), Err(SubjectError::Io { .. })));
    }
}

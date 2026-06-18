//! Per-subject append-only ledger of outcome facts.
//!
//! Outcome adapters run in one stateless hook invocation; the gate runs in
//! another. The ledger is the durable bridge — append outcome facts keyed by
//! subject, read them back at gate time. Same exclusive-flock write discipline
//! as `action_log` (auto-released on fd close, whole-line atomicity).
//!
//! One file per subject: `.phronesis/outcomes/<subject>.jsonl`. When the shared
//! journey journal (SPEC-journey-facts) lands, this can fold into it; until
//! then it is the minimal standalone ledger the confidence SPEC calls for.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::outcomes::facts::OutcomeFact;

/// One ledger line: a neutral outcome fact plus when it was recorded.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LedgerEntry {
    pub ts: u64,
    pub predicate: String,
    pub args: Vec<String>,
}

#[derive(Debug, Error)]
pub enum LedgerError {
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

fn path_for(root: &Path, subject: &str) -> PathBuf {
    root.join(".phronesis")
        .join("outcomes")
        .join(format!("{}.jsonl", sanitize(subject)))
}

/// Subjects are ids we mint or accept from a tool; replace anything that isn't
/// `[A-Za-z0-9_-]` so a crafted subject can't traverse out of the outcomes dir.
fn sanitize(subject: &str) -> String {
    subject
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Append outcome facts for `subject` to its ledger. Empty input is a no-op.
pub fn append(root: &Path, subject: &str, facts: &[OutcomeFact]) -> Result<(), LedgerError> {
    if facts.is_empty() {
        return Ok(());
    }
    let path = path_for(root, subject);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| LedgerError::Io {
            path: parent.display().to_string(),
            source: e,
        })?;
    }

    let ts = now();
    let mut buf = String::new();
    for f in facts {
        let entry = LedgerEntry {
            ts,
            predicate: f.predicate.to_string(),
            args: f.args.clone(),
        };
        buf.push_str(&serde_json::to_string(&entry)?);
        buf.push('\n');
    }

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| LedgerError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
    file.lock_exclusive().map_err(|e| LedgerError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    let write_result = (&file)
        .write_all(buf.as_bytes())
        .map_err(|e| LedgerError::Io {
            path: path.display().to_string(),
            source: e,
        });
    let _ = FileExt::unlock(&file);
    write_result?;
    Ok(())
}

/// Read all ledger entries for `subject` in append order. Missing file → empty.
/// Malformed lines are skipped so a torn trailing write can't hide the rest.
pub fn read(root: &Path, subject: &str) -> Result<Vec<LedgerEntry>, LedgerError> {
    let path = path_for(root, subject);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(LedgerError::Io {
                path: path.display().to_string(),
                source: e,
            });
        }
    };
    Ok(content
        .lines()
        .filter_map(|l| serde_json::from_str::<LedgerEntry>(l).ok())
        .collect())
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_and_read_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        append(
            dir.path(),
            "u",
            &[OutcomeFact::build("u", true), OutcomeFact::test("u", 3, 0)],
        )
        .unwrap();
        let entries = read(dir.path(), "u").unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].predicate, "build_outcome");
        assert_eq!(entries[1].predicate, "test_outcome");
        assert_eq!(entries[1].args, vec!["u", "3", "0", "3"]);
    }

    #[test]
    fn read_missing_subject_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read(dir.path(), "nope").unwrap().is_empty());
    }

    #[test]
    fn append_empty_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), "u", &[]).unwrap();
        assert!(read(dir.path(), "u").unwrap().is_empty());
    }

    #[test]
    fn appends_accumulate() {
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), "u", &[OutcomeFact::build("u", false)]).unwrap();
        append(dir.path(), "u", &[OutcomeFact::build("u", true)]).unwrap();
        let entries = read(dir.path(), "u").unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].args[1], "fail");
        assert_eq!(entries[1].args[1], "pass");
    }

    #[test]
    fn read_skips_malformed_lines() {
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), "u", &[OutcomeFact::build("u", true)]).unwrap();
        let path = path_for(dir.path(), "u");
        let mut content = std::fs::read_to_string(&path).unwrap();
        content.push_str("{not json\n");
        std::fs::write(&path, content).unwrap();
        let entries = read(dir.path(), "u").unwrap();
        assert_eq!(
            entries.len(),
            1,
            "the one valid entry survives a torn write"
        );
    }

    #[test]
    fn subject_cannot_escape_the_outcomes_dir() {
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), "../../evil", &[OutcomeFact::build("x", true)]).unwrap();
        let outcomes = dir.path().join(".phronesis").join("outcomes");
        let files: Vec<_> = std::fs::read_dir(&outcomes)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            files
                .iter()
                .all(|f| !f.contains("..") && f.ends_with(".jsonl")),
            "sanitized filename stays inside the outcomes dir: {files:?}"
        );
    }
}

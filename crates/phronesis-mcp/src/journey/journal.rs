//! Append-only per-call journal at `.phronesis/journey/events.jsonl`.
//!
//! Same flock discipline as `action_log` / `outcomes::ledger`: an exclusive
//! advisory file lock (via `fs2::FileExt`) is held around each write, so
//! concurrent appenders serialize and cannot interleave at any line size.
//! POSIX flock auto-releases when the file descriptor is closed — including on
//! abnormal process exit — so there's no stuck-lock risk. Advisory locks
//! don't work on NFS or some network filesystems; this is acceptable because
//! `.phronesis/` always lives inside a local project workspace.
//!
//! One JSON Lines record per *executed* tool call (post-check only — blocked
//! calls never reach here, so the journal reflects what the agent actually
//! did).
//!
//! Reads tail-bias: `read_recent(n)` returns the last `n` records in append
//! order; `read_recent_subject(s, n)` filters those by subject. v1 reads the
//! whole file with a hard line cap; reverse-read is a future optimization.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// One line of the journey journal. Field order here is the serialization
/// order: `v`, `ts`, `sid`, `seq`, `tool`, `path`, `ext?`, `module?`,
/// `tags[]`, `subject?`. See SPEC §"The journal record".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalRecord {
    /// Record schema version. Bump when the on-disk shape changes.
    pub v: u32,
    /// Unix epoch seconds at the time the record was journaled.
    pub ts: u64,
    /// Session id (for `s` windows). Stamped by the SessionStart hook.
    pub sid: String,
    /// Monotonic per-project counter (for `Nc` windows).
    pub seq: u64,
    /// Tool name (e.g. `Edit`, `Write`, `Bash`).
    pub tool: String,
    /// File path or `<cmd>` for bash invocations.
    pub path: String,
    /// File extension when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ext: Option<String>,
    /// Resolved module name from `journey.json::modules` globs, when matched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    /// Tagger output for this record.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Optional work-unit id — the outcomes fold-in seam. Lands in Task 4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
}

#[derive(Debug, Error)]
pub enum JournalError {
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Hard cap on lines read by `read_recent*` regardless of caller-requested `n`.
/// Bounds the pathological case where retention misbehaves. See SPEC §"Cost".
pub const SUFFIX_HARD_CAP: usize = 10_000;

fn dir(root: &Path) -> PathBuf {
    root.join(".phronesis").join("journey")
}

fn events_path(root: &Path) -> PathBuf {
    dir(root).join("events.jsonl")
}

/// Append one record to the journey journal. Creates `.phronesis/journey/`
/// if missing; acquires an exclusive advisory lock around the write so
/// concurrent appenders serialize; releases the lock on fd close.
pub fn append(root: &Path, record: &JournalRecord) -> Result<(), JournalError> {
    let d = dir(root);
    std::fs::create_dir_all(&d).map_err(|e| JournalError::Io {
        path: d.display().to_string(),
        source: e,
    })?;
    let path = events_path(root);
    let mut line = serde_json::to_string(record)?;
    line.push('\n');

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| JournalError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
    file.lock_exclusive().map_err(|e| JournalError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    let write_result = (&file)
        .write_all(line.as_bytes())
        .map_err(|e| JournalError::Io {
            path: path.display().to_string(),
            source: e,
        });
    // Best-effort unlock; the lock is also released when `file` is dropped.
    let _ = FileExt::unlock(&file);
    write_result?;
    Ok(())
}

/// Read the last `min(n, SUFFIX_HARD_CAP)` records in append order. A
/// missing file is not an error — returns an empty vec. Malformed lines are
/// silently skipped so a torn trailing write doesn't hide good data.
pub fn read_recent(root: &Path, n: usize) -> Result<Vec<JournalRecord>, JournalError> {
    let limit = n.min(SUFFIX_HARD_CAP);
    let path = events_path(root);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(JournalError::Io {
                path: path.display().to_string(),
                source: e,
            });
        }
    };
    let all: Vec<JournalRecord> = content
        .lines()
        .filter_map(|l| serde_json::from_str::<JournalRecord>(l).ok())
        .collect();
    let start = all.len().saturating_sub(limit);
    Ok(all[start..].to_vec())
}

/// Read the last `n` records (bounded by `SUFFIX_HARD_CAP` before filtering)
/// whose `subject` matches. The per-work-unit read used by the outcomes
/// fold-in (lands in Task 4).
pub fn read_recent_subject(
    root: &Path,
    subject: &str,
    n: usize,
) -> Result<Vec<JournalRecord>, JournalError> {
    let all = read_recent(root, SUFFIX_HARD_CAP)?;
    let filtered: Vec<JournalRecord> = all
        .into_iter()
        .filter(|r| r.subject.as_deref() == Some(subject))
        .collect();
    let limit = n.min(filtered.len());
    let start = filtered.len() - limit;
    Ok(filtered[start..].to_vec())
}

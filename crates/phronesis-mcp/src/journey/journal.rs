//! Append-only per-call journal at `.phronesis/journey/events.jsonl`.
//!
//! Same flock discipline as `action_log`: an exclusive
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
    /// The tool call's process exit code, when the CLI's payload carried one
    /// (Bash/shell records only). Named `command_exit`, not `exit` — the
    /// action log's `exit` is the hook's own exit code; the two must not
    /// collide. Absent means the CLI genuinely didn't send one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_exit: Option<i32>,
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

/// Default write-side size cap for the journal. Journal records are small
/// (~200–500 bytes), so 16 MiB comfortably holds several times the
/// `SUFFIX_HARD_CAP` read window.
pub const MAX_JOURNAL_BYTES_DEFAULT: u64 = 16 * 1024 * 1024;
/// Upper bound on the env override, mirroring the action log's ceiling.
pub const MAX_JOURNAL_BYTES_CEILING: u64 = 1024 * 1024 * 1024;
/// Records retained unconditionally at the tail. Equal to `SUFFIX_HARD_CAP`
/// so compaction can never drop a record the readers could still see.
pub const COMPACT_TAIL_RECORDS: usize = SUFFIX_HARD_CAP;

fn dir(root: &Path) -> PathBuf {
    root.join(".phronesis").join("journey")
}

fn events_path(root: &Path) -> PathBuf {
    dir(root).join("events.jsonl")
}

fn max_journal_bytes() -> u64 {
    std::env::var("PHRONESIS_MAX_JOURNAL_BYTES")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(|v| v.min(MAX_JOURNAL_BYTES_CEILING))
        .unwrap_or(MAX_JOURNAL_BYTES_DEFAULT)
}

/// Compact the journal when it exceeds `max_bytes`: retain the most recent
/// `tail_records` records plus, for every subject appearing in the dropped
/// prefix, its most recent `outcome:*`-bearing record (so each work unit's
/// latest grounded build/test result survives for confidence banding).
/// Atomic rewrite (temp file + rename) under the same advisory lock the
/// appenders take; never blind-truncates. Returns whether a compaction ran.
pub fn maybe_compact(
    root: &Path,
    max_bytes: u64,
    tail_records: usize,
) -> Result<bool, JournalError> {
    let path = events_path(root);
    let meta = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(_) => return Ok(false),
    };
    if meta.len() < max_bytes {
        return Ok(false);
    }
    let io_err = |e: std::io::Error| JournalError::Io {
        path: path.display().to_string(),
        source: e,
    };
    let file = OpenOptions::new().read(true).open(&path).map_err(io_err)?;
    file.lock_exclusive().map_err(io_err)?;
    let result = compact_locked(&path, tail_records);
    let _ = FileExt::unlock(&file);
    result
}

fn compact_locked(path: &Path, tail_records: usize) -> Result<bool, JournalError> {
    let io_err = |e: std::io::Error| JournalError::Io {
        path: path.display().to_string(),
        source: e,
    };
    let content = std::fs::read_to_string(path).map_err(io_err)?;
    let all: Vec<JournalRecord> = content
        .lines()
        .filter_map(|l| serde_json::from_str::<JournalRecord>(l).ok())
        .collect();
    if all.len() <= tail_records {
        return Ok(false);
    }
    let split = all.len() - tail_records;
    let (prefix, tail) = all.split_at(split);
    // Latest outcome-bearing record per subject in the prefix, by index.
    let mut latest: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for (i, r) in prefix.iter().enumerate() {
        if let Some(s) = r.subject.as_deref()
            && r.tags.iter().any(|t| t.starts_with("outcome:"))
        {
            latest.insert(s, i);
        }
    }
    let mut keep: Vec<usize> = latest.into_values().collect();
    keep.sort_unstable();
    let mut out = String::new();
    for i in keep {
        out.push_str(&serde_json::to_string(&prefix[i])?);
        out.push('\n');
    }
    for r in tail {
        out.push_str(&serde_json::to_string(r)?);
        out.push('\n');
    }
    let tmp = path.with_extension("jsonl.tmp");
    let tmp_err = |e: std::io::Error| JournalError::Io {
        path: tmp.display().to_string(),
        source: e,
    };
    std::fs::write(&tmp, out).map_err(tmp_err)?;
    std::fs::rename(&tmp, path).map_err(tmp_err)?;
    Ok(true)
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
    // Write-side retention: best-effort and fail-open — a compaction error
    // must never block the append.
    if let Err(e) = maybe_compact(root, max_journal_bytes(), COMPACT_TAIL_RECORDS) {
        eprintln!("phronesis: journal compaction skipped: {e}");
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn record(seq: u64, command_exit: Option<i32>) -> JournalRecord {
        JournalRecord {
            v: 1,
            ts: seq,
            sid: "s".to_string(),
            seq,
            tool: "Bash".to_string(),
            path: "<cmd>".to_string(),
            ext: None,
            module: None,
            tags: vec![],
            subject: None,
            command_exit,
        }
    }

    #[test]
    fn command_exit_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), &record(1, Some(101))).unwrap();
        let recs = read_recent(dir.path(), 10).unwrap();
        assert_eq!(recs[0].command_exit, Some(101));
    }

    #[test]
    fn command_exit_none_is_omitted_from_serialization() {
        let line = serde_json::to_string(&record(1, None)).unwrap();
        assert!(
            !line.contains("command_exit"),
            "absent exit code must be omitted, not null: {line}"
        );
    }

    #[test]
    fn v1_line_without_command_exit_still_parses() {
        let line = r#"{"v":1,"ts":0,"sid":"s","seq":1,"tool":"Bash","path":"<cmd>","tags":[]}"#;
        let rec: JournalRecord = serde_json::from_str(line).unwrap();
        assert_eq!(rec.command_exit, None);
    }

    fn tagged(seq: u64, subject: &str, tags: &[&str]) -> JournalRecord {
        JournalRecord {
            subject: Some(subject.to_string()),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            ..record(seq, None)
        }
    }

    #[test]
    fn under_cap_is_untouched() {
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), &record(1, None)).unwrap();
        let compacted = maybe_compact(dir.path(), u64::MAX, 2).unwrap();
        assert!(!compacted);
        assert_eq!(read_recent(dir.path(), 100).unwrap().len(), 1);
    }

    #[test]
    fn missing_file_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!maybe_compact(dir.path(), 1, 2).unwrap());
    }

    #[test]
    fn over_cap_keeps_tail_and_latest_outcome_per_prefix_subject() {
        let dir = tempfile::tempdir().unwrap();
        // Prefix: subject "u" has two outcome records (seq 1 stale, seq 2
        // latest) and noise; subject "v" has one; seq 4 is outcome-less noise.
        append(dir.path(), &tagged(1, "u", &["outcome:compile_error"])).unwrap();
        append(dir.path(), &tagged(2, "u", &["outcome:compile_ok"])).unwrap();
        append(dir.path(), &tagged(3, "v", &["outcome:test_pass"])).unwrap();
        append(dir.path(), &record(4, None)).unwrap();
        // Tail of 2:
        append(dir.path(), &record(5, None)).unwrap();
        append(dir.path(), &record(6, None)).unwrap();
        let compacted = maybe_compact(dir.path(), 1, 2).unwrap();
        assert!(compacted);
        let recs = read_recent(dir.path(), 100).unwrap();
        let seqs: Vec<u64> = recs.iter().map(|r| r.seq).collect();
        assert_eq!(
            seqs,
            vec![2, 3, 5, 6],
            "latest outcome per prefix subject (2 for u, 3 for v) + tail, original order"
        );
    }

    #[test]
    fn confidence_read_still_works_after_compaction() {
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), &tagged(1, "u", &["outcome:compile_ok"])).unwrap();
        for seq in 2..=5 {
            append(dir.path(), &record(seq, None)).unwrap();
        }
        maybe_compact(dir.path(), 1, 2).unwrap();
        let for_u = read_recent_subject(dir.path(), "u", 10).unwrap();
        assert_eq!(for_u.len(), 1, "subject u's grounded outcome survives");
        assert!(for_u[0].tags.iter().any(|t| t == "outcome:compile_ok"));
    }

    #[test]
    fn append_still_succeeds_after_compaction() {
        let dir = tempfile::tempdir().unwrap();
        for seq in 1..=4 {
            append(dir.path(), &record(seq, None)).unwrap();
        }
        maybe_compact(dir.path(), 1, 2).unwrap();
        append(dir.path(), &record(5, None)).unwrap();
        let seqs: Vec<u64> = read_recent(dir.path(), 100).unwrap().iter().map(|r| r.seq).collect();
        assert_eq!(seqs, vec![3, 4, 5]);
    }

    #[test]
    fn malformed_lines_are_dropped_at_compaction() {
        let dir = tempfile::tempdir().unwrap();
        for seq in 1..=3 {
            append(dir.path(), &record(seq, None)).unwrap();
        }
        // Torn write in the middle of the prefix.
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(dir.path().join(".phronesis/journey/events.jsonl"))
            .unwrap();
        writeln!(f, "{{torn").unwrap();
        drop(f);
        append(dir.path(), &record(4, None)).unwrap();
        maybe_compact(dir.path(), 1, 2).unwrap();
        let seqs: Vec<u64> = read_recent(dir.path(), 100).unwrap().iter().map(|r| r.seq).collect();
        assert_eq!(seqs, vec![3, 4]);
    }
}

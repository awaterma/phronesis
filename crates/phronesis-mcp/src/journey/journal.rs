//! Append-only per-call journal at `.phronesis/journey/events.jsonl`.
//!
//! # Locking model — stable lock file
//!
//! Every mutation of `events.jsonl` (append, compaction) serializes on an
//! exclusive advisory lock (via `fs2::FileExt`) taken on a *stable sibling
//! lock file*, `.phronesis/journey/events.lock` — never on the journal's own
//! file descriptor. Compaction atomically replaces `events.jsonl` by rename,
//! so a lock held on the journal fd itself could outlive the file it guards;
//! the lock file's inode is never replaced, so whoever holds it is
//! unambiguously the only mutator. This removes the old
//! open/lock/revalidate-inode retry loop, whose bounded retries could in
//! theory let an append land in a stale, already-renamed-away file.
//!
//! The lock auto-releases when its fd is closed — including on abnormal
//! process exit — so there's no stuck-lock risk. `fs2` provides the same
//! advisory-exclusive semantics on non-unix platforms (`LockFileEx` on
//! Windows); the lock file is never read or written, so Windows' stricter
//! mandatory-lock behavior cannot interfere with journal I/O. Advisory locks
//! don't work on NFS or some network filesystems; this is acceptable because
//! `.phronesis/` always lives inside a local project workspace.
//!
//! Readers stay lock-free: compaction replaces the journal with a single
//! atomic `rename`, so any reader observes either the complete old file or
//! the complete new one, never a partially rewritten file.
//!
//! # Durability
//!
//! Compaction writes the full compacted journal to a temp file in the same
//! directory, `sync_all()`s it, then renames it over `events.jsonl`; the
//! temp file is best-effort removed on any failure. The parent directory is
//! deliberately *not* fsynced: the journal is best-effort telemetry (the
//! hook already treats append errors as fail-open), and after a power loss
//! the worst case is observing the pre-compaction journal — still valid
//! JSONL, merely over-sized, and healed by the next compaction. Plain
//! appends are not fsynced either, matching the action log; a crash can
//! lose the last line, which readers already tolerate (torn trailing lines
//! are skipped).
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
/// `tags[]`, `subject?`, `command_exit?`. See SPEC §"The journal record".
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

fn lock_path(root: &Path) -> PathBuf {
    dir(root).join("events.lock")
}

/// Exclusive-lock guard on the stable lock file. Holding this guard is the
/// sole license to mutate `events.jsonl`. The advisory lock is released on
/// drop (and by the OS when the fd closes, including on abnormal exit).
struct JournalLock {
    file: std::fs::File,
}

impl Drop for JournalLock {
    fn drop(&mut self) {
        // Best-effort explicit unlock; fd close releases it regardless.
        let _ = FileExt::unlock(&self.file);
    }
}

/// Open/create `.phronesis/journey/events.lock` and take a (blocking)
/// exclusive advisory lock on it. The lock file's inode is stable —
/// compaction renames over `events.jsonl`, never over the lock file — so
/// every holder is serialized against every other mutator with no
/// revalidation needed. Failure policy: any error opening or locking the
/// lock file surfaces as `JournalError::Io` naming `events.lock`; callers
/// propagate it (append) or treat it per their own documented policy.
fn acquire_lock(root: &Path) -> Result<JournalLock, JournalError> {
    let path = lock_path(root);
    let io_err = |e: std::io::Error| JournalError::Io {
        path: path.display().to_string(),
        source: e,
    };
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .map_err(io_err)?;
    file.lock_exclusive().map_err(io_err)?;
    Ok(JournalLock { file })
}

fn max_journal_bytes() -> u64 {
    std::env::var("PHRONESIS_MAX_JOURNAL_BYTES")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(|v| v.min(MAX_JOURNAL_BYTES_CEILING))
        .unwrap_or(MAX_JOURNAL_BYTES_DEFAULT)
}

/// True when the journal at `path` meets or exceeds `max_bytes`. A missing
/// file (or any metadata error) reads as under-cap: nothing to compact.
fn over_cap(path: &Path, max_bytes: u64) -> bool {
    std::fs::metadata(path)
        .map(|m| m.len() >= max_bytes)
        .unwrap_or(false)
}

/// Compact the journal when it exceeds `max_bytes`: retain the most recent
/// `tail_records` records plus, for every subject appearing in the dropped
/// prefix, its most recent `outcome:*`-bearing record (so each work unit's
/// latest grounded build/test result survives for confidence banding).
/// Atomic rewrite (temp file + fsync + rename) under the stable lock file
/// shared with appenders; never blind-truncates. Returns whether a
/// compaction ran.
pub fn maybe_compact(
    root: &Path,
    max_bytes: u64,
    tail_records: usize,
) -> Result<bool, JournalError> {
    let path = events_path(root);
    // Cheap unlocked pre-check: skip lock traffic when clearly under cap
    // (or when the journal — and therefore its directory — doesn't exist).
    if !over_cap(&path, max_bytes) {
        return Ok(false);
    }
    let _lock = acquire_lock(root)?;
    maybe_compact_locked(&path, max_bytes, tail_records)
}

/// Cap re-check + compaction body. The caller MUST hold the stable lock.
fn maybe_compact_locked(
    path: &Path,
    max_bytes: u64,
    tail_records: usize,
) -> Result<bool, JournalError> {
    // Re-check under the lock: another process may have compacted while we
    // waited for the grant, and its rewrite already enforced the cap.
    if !over_cap(path, max_bytes) {
        return Ok(false);
    }
    compact_locked(path, tail_records)
}

/// Write `content` to `tmp`, flushed to disk (`sync_all`) before returning,
/// so the subsequent rename never installs an unflushed file.
fn write_temp_synced(tmp: &Path, content: &str) -> std::io::Result<()> {
    let mut f = std::fs::File::create(tmp)?;
    f.write_all(content.as_bytes())?;
    f.sync_all()
}

fn compact_locked(path: &Path, tail_records: usize) -> Result<bool, JournalError> {
    let all = read_records(path)?;
    if all.len() <= tail_records {
        return Ok(false);
    }
    let out = compacted_content(&all, tail_records)?;
    install_compacted(path, &out)?;
    Ok(true)
}

fn read_records(path: &Path) -> Result<Vec<JournalRecord>, JournalError> {
    let io_err = |e: std::io::Error| JournalError::Io {
        path: path.display().to_string(),
        source: e,
    };
    let content = std::fs::read_to_string(path).map_err(io_err)?;
    Ok(content
        .lines()
        .filter_map(|l| serde_json::from_str::<JournalRecord>(l).ok())
        .collect())
}

fn compacted_content(all: &[JournalRecord], tail_records: usize) -> Result<String, JournalError> {
    let split = all.len() - tail_records;
    let (prefix, tail) = all.split_at(split);
    let keep = latest_outcome_indices(prefix);
    serialize_compaction(prefix, tail, &keep)
}

fn latest_outcome_indices(prefix: &[JournalRecord]) -> Vec<usize> {
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
    keep
}

fn serialize_compaction(
    prefix: &[JournalRecord],
    tail: &[JournalRecord],
    keep: &[usize],
) -> Result<String, JournalError> {
    let mut out = String::new();
    for &i in keep {
        out.push_str(&serde_json::to_string(&prefix[i])?);
        out.push('\n');
    }
    for r in tail {
        out.push_str(&serde_json::to_string(r)?);
        out.push('\n');
    }
    Ok(out)
}

fn install_compacted(path: &Path, out: &str) -> Result<(), JournalError> {
    let tmp = path.with_extension("jsonl.tmp");
    let tmp_err = |e: std::io::Error| JournalError::Io {
        path: tmp.display().to_string(),
        source: e,
    };
    if let Err(e) = write_temp_synced(&tmp, out) {
        // Best-effort cleanup of a partial temp file; the live journal is
        // untouched on this path.
        let _ = std::fs::remove_file(&tmp);
        return Err(tmp_err(e));
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(tmp_err(e));
    }
    // Parent-directory fsync deliberately omitted — see module docs
    // ("Durability").
    Ok(())
}

/// Append one record to the journey journal. Creates `.phronesis/journey/`
/// if missing; serializes on the stable lock file with all other mutators
/// (appenders and compaction), so a concurrent compaction can never rename
/// the journal out from under an in-flight append. Write-side retention
/// runs under that same lock and is fail-open — a compaction error is
/// reported to stderr and never blocks the append.
pub fn append(root: &Path, record: &JournalRecord) -> Result<(), JournalError> {
    let d = dir(root);
    std::fs::create_dir_all(&d).map_err(|e| JournalError::Io {
        path: d.display().to_string(),
        source: e,
    })?;
    let mut line = serde_json::to_string(record)?;
    line.push('\n');
    let path = events_path(root);
    let io_err = |e: std::io::Error| JournalError::Io {
        path: path.display().to_string(),
        source: e,
    };

    let _lock = acquire_lock(root)?;
    if let Err(e) = maybe_compact_locked(&path, max_journal_bytes(), COMPACT_TAIL_RECORDS) {
        eprintln!("phronesis: journal compaction skipped: {e}");
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(io_err)?;
    file.write_all(line.as_bytes()).map_err(io_err)?;
    Ok(())
}

/// Read the last `min(n, SUFFIX_HARD_CAP)` records in append order. A
/// missing file is not an error — returns an empty vec. Malformed lines are
/// silently skipped so a torn trailing write doesn't hide good data.
/// Lock-free by design: compaction's atomic rename means this always sees a
/// coherent whole file (old or new).
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

/// Subject-filtered over the whole journal (the journal is bounded by
/// compaction), capped at the last `min(n, SUFFIX_HARD_CAP)` matching
/// records — this is what makes the compactor's per-subject preserved
/// outcomes reachable. Lock-free, same rationale as `read_recent`.
pub fn read_recent_subject(
    root: &Path,
    subject: &str,
    n: usize,
) -> Result<Vec<JournalRecord>, JournalError> {
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
    let filtered: Vec<JournalRecord> = content
        .lines()
        .filter_map(|l| serde_json::from_str::<JournalRecord>(l).ok())
        .filter(|r| r.subject.as_deref() == Some(subject))
        .collect();
    let limit = n.min(SUFFIX_HARD_CAP).min(filtered.len());
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
        let seqs: Vec<u64> = read_recent(dir.path(), 100)
            .unwrap()
            .iter()
            .map(|r| r.seq)
            .collect();
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
        let seqs: Vec<u64> = read_recent(dir.path(), 100)
            .unwrap()
            .iter()
            .map(|r| r.seq)
            .collect();
        assert_eq!(seqs, vec![3, 4]);
    }

    #[test]
    fn preserved_prefix_outcome_is_readable_beyond_positional_window() {
        // A subject's compaction-preserved outcome sits BEFORE the tail;
        // read_recent_subject must still find it (filter-then-cap).
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), &tagged(1, "u", &["outcome:compile_ok"])).unwrap();
        for seq in 2..=6 {
            append(dir.path(), &record(seq, None)).unwrap();
        }
        maybe_compact(dir.path(), 1, 2).unwrap();
        // File is now [u's outcome (seq 1)] + tail [5, 6]; a positional
        // 2-record window would never reach seq 1.
        let for_u = read_recent_subject(dir.path(), "u", 2).unwrap();
        assert_eq!(for_u.len(), 1);
        assert_eq!(for_u[0].seq, 1);
    }

    #[test]
    fn append_lands_in_live_file_after_external_rename() {
        // Under the stable-lock design, append opens the live journal fresh
        // (under the lock) on every call — an external rename can never
        // orphan a write. Replaces the old fd-revalidation guarantee.
        let dir = tempfile::tempdir().unwrap();
        for seq in 1..=3 {
            append(dir.path(), &record(seq, None)).unwrap();
        }
        // Swap the journal exactly as compaction's rename does.
        let path = dir.path().join(".phronesis/journey/events.jsonl");
        let tmp = dir.path().join(".phronesis/journey/events.jsonl.tmp");
        std::fs::copy(&path, &tmp).unwrap();
        std::fs::rename(&tmp, &path).unwrap();
        append(dir.path(), &record(4, None)).unwrap();
        let seqs: Vec<u64> = read_recent(dir.path(), 100)
            .unwrap()
            .iter()
            .map(|r| r.seq)
            .collect();
        assert_eq!(seqs, vec![1, 2, 3, 4]);
    }

    #[test]
    fn at_cap_with_few_records_is_not_rewritten() {
        // Byte cap exceeded but parsed records <= tail_records: no rewrite.
        let dir = tempfile::tempdir().unwrap();
        for seq in 1..=3 {
            append(dir.path(), &record(seq, None)).unwrap();
        }
        assert!(!maybe_compact(dir.path(), 1, 10).unwrap());
        assert_eq!(read_recent(dir.path(), 100).unwrap().len(), 3);
    }

    // ---- Stable-lock concurrency suite (deterministic: barriers + bounded
    // ---- iteration counts, no sleeps) --------------------------------------

    #[test]
    #[cfg(unix)]
    fn lock_inode_is_stable_across_compaction() {
        // The invariant that makes fd revalidation unnecessary: compaction
        // renames over events.jsonl, never over events.lock. Replaces the
        // old fd_is_current_detects_rename_swap test.
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        for seq in 1..=4u64 {
            append(dir.path(), &record(seq, None)).unwrap();
        }
        let lock = dir.path().join(".phronesis/journey/events.lock");
        let before = std::fs::metadata(&lock).unwrap().ino();
        assert!(maybe_compact(dir.path(), 1, 2).unwrap());
        append(dir.path(), &record(5, None)).unwrap();
        let after = std::fs::metadata(&lock).unwrap().ino();
        assert_eq!(before, after, "the lock file inode must never be replaced");
    }

    #[test]
    fn concurrent_appenders_do_not_interleave_json() {
        use std::sync::{Arc, Barrier};
        const THREADS: usize = 8;
        const PER_THREAD: u64 = 25;
        let dir = Arc::new(tempfile::tempdir().unwrap());
        let barrier = Arc::new(Barrier::new(THREADS));
        let mut handles = Vec::new();
        for t in 0..THREADS as u64 {
            let dir = Arc::clone(&dir);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                for i in 0..PER_THREAD {
                    append(dir.path(), &record(t * 1000 + i, None)).unwrap();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        // Assert on the RAW file, not read_recent (which skips bad lines):
        // every line must parse, and every uniquely numbered seq must be
        // present exactly once.
        let mut seqs = read_raw_sequences(dir.path());
        assert_eq!(seqs.len(), THREADS * PER_THREAD as usize, "no lost append");
        seqs.sort_unstable();
        seqs.dedup();
        assert_eq!(
            seqs.len(),
            THREADS * PER_THREAD as usize,
            "no duplicated seq"
        );
    }

    fn read_raw_sequences(root: &Path) -> Vec<u64> {
        let raw = std::fs::read_to_string(root.join(".phronesis/journey/events.jsonl"))
            .expect("read raw journey journal");
        raw.lines()
            .map(|line| {
                serde_json::from_str::<JournalRecord>(line)
                    .expect("raw journal line must be intact JSON")
                    .seq
            })
            .collect()
    }

    #[test]
    fn appends_racing_compaction_are_never_lost() {
        // Every record carries a unique subject + outcome tag, so the
        // compaction policy preserves ALL of them regardless of tail size —
        // any missing seq is a genuinely lost append, not a policy drop.
        // Without the stable lock, a compaction rename could clobber a
        // concurrent append landing in the pre-rename inode.
        use std::sync::{Arc, Barrier};
        const APPENDERS: u64 = 4;
        const PER_THREAD: u64 = 25;
        let dir = Arc::new(tempfile::tempdir().unwrap());
        let barrier = Arc::new(Barrier::new(APPENDERS as usize + 1));
        let mut handles = Vec::new();
        for t in 0..APPENDERS {
            let dir = Arc::clone(&dir);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                for i in 0..PER_THREAD {
                    let seq = t * 1000 + i;
                    append(
                        dir.path(),
                        &tagged(seq, &format!("s{seq}"), &["outcome:test_pass"]),
                    )
                    .unwrap();
                }
            }));
        }
        {
            let dir = Arc::clone(&dir);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                for _ in 0..50 {
                    // max_bytes=1 forces a full rewrite attempt every call.
                    maybe_compact(dir.path(), 1, 1).unwrap();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let (actual, expected) = race_sequences(dir.path(), APPENDERS, PER_THREAD);
        assert_eq!(actual, expected, "every unique append survives the race");
    }

    fn race_sequences(root: &Path, appenders: u64, per_thread: u64) -> (Vec<u64>, Vec<u64>) {
        let records = read_recent(root, SUFFIX_HARD_CAP).expect("read compacted journey records");
        let mut actual: Vec<u64> = records.iter().map(|record| record.seq).collect();
        actual.sort_unstable();
        let mut expected: Vec<u64> = (0..appenders)
            .flat_map(|thread| (0..per_thread).map(move |index| thread * 1000 + index))
            .collect();
        expected.sort_unstable();
        (actual, expected)
    }

    #[test]
    fn repeated_compact_append_loop_loses_no_record() {
        // Sequential worst case: compaction rewrite after every append.
        // Unique subject + outcome tag per record => policy preserves all,
        // so any loss would be a locking/atomicity bug.
        let dir = tempfile::tempdir().unwrap();
        for seq in 0..200u64 {
            append(
                dir.path(),
                &tagged(seq, &format!("s{seq}"), &["outcome:compile_ok"]),
            )
            .unwrap();
            maybe_compact(dir.path(), 1, 1).unwrap();
        }
        let recs = read_recent(dir.path(), SUFFIX_HARD_CAP).unwrap();
        let seqs: Vec<u64> = recs.iter().map(|r| r.seq).collect();
        assert_eq!(
            seqs,
            (0..200).collect::<Vec<u64>>(),
            "append order preserved, nothing lost"
        );
    }

    #[test]
    fn reader_sees_valid_json_during_replacement() {
        // Readers are lock-free: the atomic rename must always present a
        // coherent whole file. Only the FINAL line of a snapshot may be a
        // torn in-progress append; a malformed non-final line would mean
        // the replacement was not atomic.
        use std::sync::{Arc, Barrier};
        let dir = Arc::new(tempfile::tempdir().unwrap());
        for seq in 0..20u64 {
            append(
                dir.path(),
                &tagged(seq, &format!("s{seq}"), &["outcome:test_pass"]),
            )
            .unwrap();
        }
        let path = dir.path().join(".phronesis/journey/events.jsonl");
        let barrier = Arc::new(Barrier::new(2));
        let mutator = {
            let dir = Arc::clone(&dir);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                for i in 0..100u64 {
                    append(
                        dir.path(),
                        &tagged(1000 + i, &format!("t{i}"), &["outcome:test_pass"]),
                    )
                    .unwrap();
                    maybe_compact(dir.path(), 1, 1).unwrap();
                }
            })
        };
        barrier.wait();
        for _ in 0..200 {
            let raw = std::fs::read_to_string(&path).unwrap();
            let lines: Vec<&str> = raw.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                if serde_json::from_str::<JournalRecord>(line).is_err() {
                    assert_eq!(
                        i,
                        lines.len() - 1,
                        "non-final malformed line during replacement: {line:?}"
                    );
                }
            }
            // The public reader never errors mid-replacement either.
            read_recent(dir.path(), 50).unwrap();
        }
        mutator.join().unwrap();
    }

    #[test]
    fn append_errors_when_lock_path_is_a_directory() {
        // Documented lock-failure policy: any error opening/locking
        // events.lock surfaces as JournalError::Io naming events.lock, and
        // append propagates it (the hook call site already treats append
        // errors as fail-open).
        let dir = tempfile::tempdir().unwrap();
        let journey = dir.path().join(".phronesis").join("journey");
        std::fs::create_dir_all(&journey).unwrap();
        std::fs::create_dir(journey.join("events.lock")).unwrap();
        let err = append(dir.path(), &record(1, None)).unwrap_err();
        match err {
            JournalError::Io { path, .. } => {
                assert!(path.contains("events.lock"), "path = {path}");
            }
            other => panic!("expected JournalError::Io, got {other:?}"),
        }
    }

    #[test]
    fn compaction_temp_error_propagates_and_journal_is_untouched() {
        // Documented temp-failure policy: the error surfaces as
        // JournalError::Io naming the temp path, the live journal is never
        // touched on that path, and appends keep working (append's internal
        // compaction is fail-open by policy).
        let dir = tempfile::tempdir().unwrap();
        for seq in 1..=4u64 {
            append(dir.path(), &record(seq, None)).unwrap();
        }
        // Block the temp path: File::create on a directory fails.
        let tmp = dir.path().join(".phronesis/journey/events.jsonl.tmp");
        std::fs::create_dir(&tmp).unwrap();
        let err = maybe_compact(dir.path(), 1, 2).unwrap_err();
        match err {
            JournalError::Io { path, .. } => {
                assert!(path.contains("events.jsonl.tmp"), "path = {path}");
            }
            other => panic!("expected JournalError::Io, got {other:?}"),
        }
        assert_eq!(
            read_recent(dir.path(), 100).unwrap().len(),
            4,
            "failed compaction must not touch the live journal"
        );
        append(dir.path(), &record(5, None)).unwrap();
        assert_eq!(read_recent(dir.path(), 100).unwrap().len(), 5);
    }

    #[test]
    fn successful_compaction_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        for seq in 1..=4u64 {
            append(dir.path(), &record(seq, None)).unwrap();
        }
        assert!(maybe_compact(dir.path(), 1, 2).unwrap());
        assert!(
            !dir.path()
                .join(".phronesis/journey/events.jsonl.tmp")
                .exists(),
            "temp file must not persist after a successful rename"
        );
    }
}

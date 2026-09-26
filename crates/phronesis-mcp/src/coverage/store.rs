use anyhow::{Context, Result, anyhow};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const COVERAGE_FORMAT: u32 = 1;

/// Region-id prefix a `hit_kind: "region"` record must carry.
const FUNCTION_REGION_PREFIX: &str = "fn:";
/// Region-id prefix a `hit_kind: "branch"` record must carry.
const BRANCH_REGION_PREFIX: &str = "branch:";

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HitRecord {
    pub v: u32,
    pub kind: String,
    pub test: String,
    pub region: String,
    pub file: String,
    pub start_line: u64,
    pub end_line: u64,
    pub hit_kind: String,
    pub revision: String,
    pub tool: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoverageIndex {
    pub format: u32,
    pub revision: String,
    pub imported_at: u64,
    pub tool: String,
}

/// On-disk index: the public [`CoverageIndex`] plus the integrity fields
/// that bind it to one exact records file. `write_store` fills them; a
/// reader recomputes them over the records bytes and refuses any mismatch.
/// Optional only so an index written before they existed parses and is
/// reported as unverifiable (corrupt) rather than as an I/O error.
#[derive(Debug, Serialize, Deserialize)]
struct IndexFile {
    #[serde(flatten)]
    index: CoverageIndex,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    records_fnv1a64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    record_count: Option<usize>,
}

/// Why a coverage store could not be trusted. `reason` is a stable
/// snake_case code (it becomes the second arg of the
/// `store_corrupt(coverage, <reason>)` fact, so rules can match on it);
/// `detail` is the human-readable explanation for stderr / CLI output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreCorruption {
    pub reason: &'static str,
    pub detail: String,
}

impl StoreCorruption {
    fn new(reason: &'static str, detail: impl Into<String>) -> Self {
        Self {
            reason,
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for StoreCorruption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "coverage store corrupt ({}): {}",
            self.reason, self.detail
        )
    }
}

impl std::error::Error for StoreCorruption {}

/// The three states a reader can observe. Only `Loaded` carries evidence,
/// and only after the records have been verified against the index.
#[derive(Debug, Clone, PartialEq)]
pub enum StoreState {
    /// Neither file exists: nothing has been imported.
    Missing,
    /// Index and records agree (digest, count, revision, tool) and every
    /// record passes [`validate_record`].
    Loaded {
        index: CoverageIndex,
        hits: Vec<HitRecord>,
    },
    /// Anything else. Consumers treat this as "no evidence" and say so.
    Corrupt(StoreCorruption),
}

/// The remedy for stale, legacy, or corrupt evidence. Re-collecting (which
/// re-imports) is the fix: an export from before per-site region ids is
/// refused by the importer, so re-importing an old export does not help.
pub const RECOLLECT_HINT: &str = "re-run `phr-mcp coverage collect` (it re-imports; an export collected elsewhere then goes through `phr-mcp coverage import`)";

pub fn store_paths(root: &Path) -> (PathBuf, PathBuf) {
    let dir = root.join(".phronesis");
    (dir.join("coverage.jsonl"), dir.join("coverage.index"))
}

/// Staleness, defined once for hydration and selection: the imported
/// revision is known to differ from HEAD. An unknown HEAD (no git) cannot
/// prove staleness, matching when `coverage_stale` asserts.
pub fn is_stale(index: &CoverageIndex, head_sha: Option<&str>) -> bool {
    head_sha.is_some_and(|head| !index.revision.eq_ignore_ascii_case(head))
}

/// FNV-1a 64 over the exact records bytes. An integrity check against torn
/// or mismatched writes, not an authenticity check: anyone who can rewrite
/// the records file can rewrite the index too.
fn fnv1a_64_hex(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn count_records(bytes: &[u8]) -> usize {
    bytes
        .split(|b| *b == b'\n')
        .filter(|l| !l.iter().all(u8::is_ascii_whitespace))
        .count()
}

/// Advisory lock serializing store replacement against reads, so a reader
/// never pairs one import's index with another's records. flock releases
/// on process exit, so a crashed writer leaves no stale lock.
fn open_lock(dir: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("coverage.lock"))
}

/// Atomically replace the coverage store: records first, then the index.
/// The index is the commit marker and records the records file's digest and
/// count, so a crash between the two renames (new records, old index) reads
/// back as corrupt — never as the old revision's evidence — until the next
/// import. A crash before the first index rename leaves records without an
/// index, which also reads as corrupt.
pub fn write_store(root: &Path, records: &[HitRecord], index: &CoverageIndex) -> Result<()> {
    let (records_path, index_path) = store_paths(root);
    let dir = records_path
        .parent()
        .context("coverage path has no parent")?;
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let lock_path = dir.join("coverage.lock");
    let lock = open_lock(dir).with_context(|| format!("opening {}", lock_path.display()))?;
    lock.lock_exclusive()
        .with_context(|| format!("locking {}", lock_path.display()))?;

    let mut buf: Vec<u8> = Vec::new();
    for rec in records {
        serde_json::to_writer(&mut buf, rec)?;
        buf.push(b'\n');
    }

    let tmp_records = records_path.with_file_name("coverage.jsonl.tmp");
    {
        let mut file = File::create(&tmp_records)?;
        file.write_all(&buf)?;
        file.sync_all()?;
    }
    fs::rename(&tmp_records, &records_path)?;

    let on_disk = IndexFile {
        index: index.clone(),
        records_fnv1a64: Some(fnv1a_64_hex(&buf)),
        record_count: Some(records.len()),
    };
    let tmp_index = index_path.with_file_name("coverage.index.tmp");
    {
        let mut file = File::create(&tmp_index)?;
        serde_json::to_writer(&mut file, &on_disk)?;
        file.sync_all()?;
    }
    fs::rename(&tmp_index, &index_path)?;

    Ok(())
}

fn read_optional(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// How long a read waits for an import holding the lock before reading
/// unlocked. Bounded so a hung or stopped import never hangs a hook.
const LOCK_WAIT: Duration = Duration::from_millis(200);
/// Poll interval while waiting for the lock.
const LOCK_POLL: Duration = Duration::from_millis(10);
/// Unlocked reads: how many attempts before a mismatch is reported.
const READ_ATTEMPTS: usize = 4;
/// Unlocked reads: pause before retry `n` (1-based) is `n` times this.
const RETRY_PAUSE: Duration = Duration::from_millis(25);

/// Read and verify the whole store. Never errors: every failure mode is a
/// [`StoreState::Corrupt`] with a stable reason code.
///
/// Reads take the store lock shared, so an import in flight is either
/// wholly before or wholly after the read. The wait is bounded
/// ([`LOCK_WAIT`]); if the lock cannot be taken in time (a hung import, a
/// read-only checkout) the read proceeds unlocked and retries, after a short
/// pause, any read whose index changed underneath it or that looks torn
/// (the window between an import's two renames) before reporting corrupt.
pub fn load_store(root: &Path) -> StoreState {
    load_store_with(root, LOCK_WAIT, &mut |attempt| {
        std::thread::sleep(RETRY_PAUSE * attempt as u32)
    })
}

/// [`load_store`] with the lock wait and the between-retry pause injected
/// (tests drive the unlocked retry deterministically through `pause`).
fn load_store_with(root: &Path, lock_wait: Duration, pause: &mut dyn FnMut(usize)) -> StoreState {
    let (records_path, _) = store_paths(root);
    let lock = records_path
        .parent()
        .filter(|dir| dir.is_dir())
        .and_then(|dir| open_lock(dir).ok())
        .filter(|lock| try_lock_shared_for(lock, lock_wait));
    if lock.is_some() {
        // Under the lock no import is mid-flight: one read is the truth.
        return load_store_once(root);
    }
    load_store_unlocked(root, pause)
}

fn try_lock_shared_for(lock: &File, wait: Duration) -> bool {
    let deadline = Instant::now() + wait;
    loop {
        if lock.try_lock_shared().is_ok() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(LOCK_POLL);
    }
}

/// Reasons an unlocked read can observe transiently while an import is
/// between its two renames (or, on the first import, before the index
/// exists).
fn is_transient(reason: &str) -> bool {
    matches!(
        reason,
        "digest_mismatch" | "count_mismatch" | "missing_index"
    )
}

fn load_store_unlocked(root: &Path, pause: &mut dyn FnMut(usize)) -> StoreState {
    let (_, index_path) = store_paths(root);
    let mut state = StoreState::Missing;
    for attempt in 0..READ_ATTEMPTS {
        if attempt > 0 {
            pause(attempt);
        }
        let before = read_optional(&index_path).ok().flatten();
        state = load_store_once(root);
        let after = read_optional(&index_path).ok().flatten();
        let torn = matches!(&state, StoreState::Corrupt(c) if is_transient(c.reason));
        if before == after && !torn {
            break;
        }
    }
    state
}

fn load_store_once(root: &Path) -> StoreState {
    match load_verified(root) {
        Ok(Some((index, hits))) => StoreState::Loaded { index, hits },
        Ok(None) => StoreState::Missing,
        Err(c) => StoreState::Corrupt(c),
    }
}

fn load_verified(
    root: &Path,
) -> std::result::Result<Option<(CoverageIndex, Vec<HitRecord>)>, StoreCorruption> {
    let (records_path, index_path) = store_paths(root);
    let index_bytes = read_optional(&index_path)
        .map_err(|e| StoreCorruption::new("index_unreadable", e.to_string()))?;
    let records_bytes = read_optional(&records_path)
        .map_err(|e| StoreCorruption::new("records_unreadable", e.to_string()))?;

    let (index_bytes, records_bytes) = match (index_bytes, records_bytes) {
        (None, None) => return Ok(None),
        (None, Some(_)) => {
            return Err(StoreCorruption::new(
                "missing_index",
                "coverage records exist without an index (interrupted import?)",
            ));
        }
        (Some(_), None) => {
            return Err(StoreCorruption::new(
                "missing_records",
                "coverage index exists without its records file",
            ));
        }
        (Some(i), Some(r)) => (i, r),
    };

    let file: IndexFile = serde_json::from_slice(&index_bytes)
        .map_err(|e| StoreCorruption::new("index_unreadable", e.to_string()))?;
    let (Some(digest), Some(count)) = (&file.records_fnv1a64, file.record_count) else {
        return Err(StoreCorruption::new(
            "unverifiable_index",
            "index carries no records digest (written by an older phr-mcp)",
        ));
    };
    let index = file.index;
    if index.format != COVERAGE_FORMAT {
        return Err(StoreCorruption::new(
            "unsupported_format",
            format!("index format {} (expected {COVERAGE_FORMAT})", index.format),
        ));
    }
    if !is_canonical_revision(&index.revision) {
        return Err(StoreCorruption::new(
            "invalid_index",
            "index revision is not a lowercase 40-hex sha",
        ));
    }
    if *digest != fnv1a_64_hex(&records_bytes) {
        return Err(StoreCorruption::new(
            "digest_mismatch",
            "records file does not match the index digest (interrupted import?)",
        ));
    }
    if count != count_records(&records_bytes) {
        return Err(StoreCorruption::new(
            "count_mismatch",
            format!("index expects {count} records"),
        ));
    }

    let text = std::str::from_utf8(&records_bytes)
        .map_err(|e| StoreCorruption::new("records_unreadable", e.to_string()))?;
    let mut hits = Vec::with_capacity(count);
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let line_num = i + 1;
        let rec: HitRecord = serde_json::from_str(line)
            .map_err(|e| StoreCorruption::new("invalid_record", format!("line {line_num}: {e}")))?;
        validate_record(&rec)
            .map_err(|e| StoreCorruption::new("invalid_record", format!("line {line_num}: {e}")))?;
        if rec.revision != index.revision {
            return Err(StoreCorruption::new(
                "revision_mismatch",
                format!("line {line_num}: record revision differs from the index revision"),
            ));
        }
        if rec.tool != index.tool {
            return Err(StoreCorruption::new(
                "tool_mismatch",
                format!("line {line_num}: record tool differs from the index tool"),
            ));
        }
        hits.push(rec);
    }
    Ok(Some((index, hits)))
}

/// The index of a verified store; `None` when nothing is imported or the
/// store is corrupt (use [`load_store`] to tell the two apart).
pub fn load_index(root: &Path) -> Option<CoverageIndex> {
    match load_store(root) {
        StoreState::Loaded { index, .. } => Some(index),
        StoreState::Missing | StoreState::Corrupt(_) => None,
    }
}

/// Verified hits: empty when nothing is imported, an error when the store
/// is corrupt.
pub fn load_hits(root: &Path) -> Result<Vec<HitRecord>> {
    match load_store(root) {
        StoreState::Loaded { hits, .. } => Ok(hits),
        StoreState::Missing => Ok(Vec::new()),
        StoreState::Corrupt(c) => Err(anyhow!(c)),
    }
}

fn is_canonical_revision(rev: &str) -> bool {
    rev.len() == 40 && rev.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

/// The one record validator, applied on import (before anything is written)
/// and on every read. Revisions may be any-case hex here; the importer
/// canonicalizes them to lowercase and the reader requires them to equal
/// the (lowercase) index revision.
pub fn validate_record(rec: &HitRecord) -> Result<()> {
    if rec.v != COVERAGE_FORMAT {
        return Err(anyhow!(
            "invalid format version: expected {}",
            COVERAGE_FORMAT
        ));
    }
    if rec.kind != "hit" {
        return Err(anyhow!("invalid kind: expected 'hit'"));
    }
    validate_identifier_field(&rec.test, "test")?;
    validate_identifier_field(&rec.region, "region")?;
    validate_identifier_field(&rec.tool, "tool")?;
    if rec.file.starts_with('/') {
        return Err(anyhow!("file path must be repo-relative (no leading '/')"));
    }
    if rec.file.split('/').any(|c| c == "..") {
        return Err(anyhow!("file path must not contain '..'"));
    }
    if rec.file.is_empty() {
        return Err(anyhow!("file path must be non-empty"));
    }
    if rec.revision.len() != 40 || !rec.revision.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(anyhow!("revision must be exactly 40 hex chars"));
    }
    let prefix = match rec.hit_kind.as_str() {
        "region" => FUNCTION_REGION_PREFIX,
        "branch" => BRANCH_REGION_PREFIX,
        _ => return Err(anyhow!("hit_kind must be 'region' or 'branch'")),
    };
    if !rec.region.starts_with(prefix) {
        return Err(anyhow!(
            "hit_kind '{}' requires a region id starting with '{prefix}'",
            rec.hit_kind
        ));
    }
    if rec.start_line > rec.end_line {
        return Err(anyhow!("start_line must be <= end_line"));
    }
    Ok(())
}

fn validate_identifier_field(field: &str, name: &str) -> Result<()> {
    if field.is_empty() {
        return Err(anyhow!("{name} must be non-empty"));
    }
    if field.len() > 256 {
        return Err(anyhow!("{name} must be <= 256 bytes"));
    }
    for c in field.chars() {
        if c.is_control() {
            return Err(anyhow!("{name} contains control characters"));
        }
        if !matches!(c, 'A'..='Z' | 'a'..='z' | '0'..='9' | '_' | ':' | '.' | '/' | '-') {
            return Err(anyhow!("{name} contains invalid character '{c}'"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn hit(rev: &str, n: usize) -> Vec<HitRecord> {
        (0..n)
            .map(|i| HitRecord {
                v: COVERAGE_FORMAT,
                kind: "hit".into(),
                test: format!("t{i}"),
                region: format!("fn:src/lib.rs::f{i}"),
                file: "src/lib.rs".into(),
                start_line: 1,
                end_line: 2,
                hit_kind: "region".into(),
                revision: rev.into(),
                tool: "cargo-llvm-cov".into(),
            })
            .collect()
    }

    fn index(rev: &str) -> CoverageIndex {
        CoverageIndex {
            format: COVERAGE_FORMAT,
            revision: rev.into(),
            imported_at: 1,
            tool: "cargo-llvm-cov".into(),
        }
    }

    fn external_lock(root: &Path) -> File {
        let lock = open_lock(&root.join(".phronesis")).expect("open lock");
        lock.lock_exclusive().expect("hold lock");
        lock
    }

    /// The lock alone keeps concurrent reads consistent: with a generous
    /// lock wait, a read never falls into the unlocked retry path. Removing
    /// the lock from either side makes `pause` fire (or a read corrupt).
    #[test]
    fn lock_isolates_reads_from_in_flight_imports() {
        let root = tempfile::tempdir().unwrap();
        let (a, b) = ("a".repeat(40), "b".repeat(40));
        write_store(root.path(), &hit(&a, 2), &index(&a)).unwrap();
        let done = Arc::new(AtomicBool::new(false));
        let writer = {
            let (root, done) = (root.path().to_path_buf(), Arc::clone(&done));
            let (a, b) = (a.clone(), b.clone());
            std::thread::spawn(move || {
                for i in 0..300 {
                    let (rev, n) = if i % 2 == 0 { (&b, 3) } else { (&a, 2) };
                    write_store(&root, &hit(rev, n), &index(rev)).unwrap();
                }
                done.store(true, Ordering::SeqCst);
            })
        };
        let mut retries = 0usize;
        let mut reads = 0usize;
        while !done.load(Ordering::SeqCst) {
            reads += 1;
            let state =
                load_store_with(root.path(), Duration::from_secs(10), &mut |_| retries += 1);
            assert!(
                matches!(state, StoreState::Loaded { .. }),
                "read {reads}: {state:?}"
            );
        }
        writer.join().unwrap();
        assert_eq!(
            retries, 0,
            "{retries} of {reads} reads needed the unlocked retry"
        );
    }

    /// A hung import holding the lock: the read gives up within the bound
    /// and reads unlocked.
    #[test]
    fn read_behind_a_held_lock_is_bounded() {
        let root = tempfile::tempdir().unwrap();
        let a = "a".repeat(40);
        write_store(root.path(), &hit(&a, 2), &index(&a)).unwrap();
        let _held = external_lock(root.path());
        let start = Instant::now();
        let state = load_store(root.path());
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "{:?}",
            start.elapsed()
        );
        assert!(matches!(state, StoreState::Loaded { .. }), "{state:?}");
    }

    /// Unlocked fallback caught between an import's two renames: the index
    /// is unchanged across the read, but the records are new. The read must
    /// retry (the import completes during the pause), not report corrupt.
    #[test]
    fn unlocked_read_retries_a_torn_store() {
        let root = tempfile::tempdir().unwrap();
        let (a, b) = ("a".repeat(40), "b".repeat(40));
        write_store(root.path(), &hit(&b, 3), &index(&b)).unwrap();
        let (_, index_path) = store_paths(root.path());
        let new_index = fs::read(&index_path).unwrap();
        write_store(root.path(), &hit(&a, 2), &index(&a)).unwrap();
        let old_index = fs::read(&index_path).unwrap();
        write_store(root.path(), &hit(&b, 3), &index(&b)).unwrap();
        fs::write(&index_path, &old_index).unwrap(); // records renamed, index not yet

        let _held = external_lock(root.path());
        let mut pauses = 0usize;
        let state = load_store_with(root.path(), Duration::ZERO, &mut |_| {
            pauses += 1;
            fs::write(&index_path, &new_index).unwrap(); // the import completes
        });
        assert_eq!(pauses, 1);
        match state {
            StoreState::Loaded { index, .. } => assert_eq!(index.revision, b),
            other => panic!("expected the completed import, got {other:?}"),
        }
    }

    /// A genuinely torn store (crashed import) is still reported after the
    /// bounded retries.
    #[test]
    fn unlocked_read_reports_a_persistently_torn_store() {
        let root = tempfile::tempdir().unwrap();
        let (a, b) = ("a".repeat(40), "b".repeat(40));
        write_store(root.path(), &hit(&a, 2), &index(&a)).unwrap();
        let (_, index_path) = store_paths(root.path());
        let old_index = fs::read(&index_path).unwrap();
        write_store(root.path(), &hit(&b, 3), &index(&b)).unwrap();
        fs::write(&index_path, &old_index).unwrap();
        let _held = external_lock(root.path());
        let mut pauses = 0usize;
        let state = load_store_with(root.path(), Duration::ZERO, &mut |_| pauses += 1);
        assert_eq!(pauses, READ_ATTEMPTS - 1);
        assert!(
            matches!(&state, StoreState::Corrupt(c) if c.reason == "digest_mismatch"),
            "{state:?}"
        );
    }
}

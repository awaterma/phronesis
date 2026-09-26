use anyhow::{Context, Result, anyhow};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

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
    fs::create_dir_all(dir)?;
    let lock = open_lock(dir)?;
    lock.lock_exclusive()?;

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

/// How many times a read retries when the index changed underneath it
/// (a writer that does not take the lock, e.g. an older binary).
const READ_ATTEMPTS: usize = 3;

/// Read and verify the whole store. Never errors: every failure mode is a
/// [`StoreState::Corrupt`] with a stable reason code.
///
/// Reads take the store lock shared, so an import in flight is either
/// wholly before or wholly after the read. If the lock cannot be taken
/// (read-only checkout) the read proceeds unlocked, and a read whose index
/// changed between the start and the end is retried rather than reported
/// as corrupt.
pub fn load_store(root: &Path) -> StoreState {
    let (records_path, _) = store_paths(root);
    let lock = records_path
        .parent()
        .filter(|dir| dir.is_dir())
        .and_then(|dir| open_lock(dir).ok())
        .filter(|lock| lock.lock_shared().is_ok());
    let state = load_store_unlocked(root);
    drop(lock);
    state
}

fn load_store_unlocked(root: &Path) -> StoreState {
    let (_, index_path) = store_paths(root);
    let mut state = StoreState::Missing;
    for _ in 0..READ_ATTEMPTS {
        let before = read_optional(&index_path).ok().flatten();
        state = load_store_once(root);
        let after = read_optional(&index_path).ok().flatten();
        if before == after {
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
            format!(
                "index carries no records digest (written by an older phr-mcp); {RECOLLECT_HINT}"
            ),
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

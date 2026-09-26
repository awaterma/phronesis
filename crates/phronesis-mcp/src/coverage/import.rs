use anyhow::{Context, Result, anyhow};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::coverage::region_map::{MAX_REGION_ID_BYTES, file_segment, is_qualified_region_id};
use crate::coverage::store::{COVERAGE_FORMAT, CoverageIndex, HitRecord, write_store};

#[derive(Debug)]
pub struct ImportSummary {
    pub records: usize,
    pub tests: usize,
    pub revision: String,
}

const MAX_EXPORT_SIZE: u64 = 5 * 1024 * 1024;

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
    if rec.hit_kind != "region" && rec.hit_kind != "branch" {
        return Err(anyhow!("hit_kind must be 'region' or 'branch'"));
    }
    if rec.start_line > rec.end_line {
        return Err(anyhow!("start_line must be <= end_line"));
    }
    validate_region_id(rec)
}

/// Region ids must be in the per-site grammar (SPEC-coverage-evidence
/// §3.2), agree with `hit_kind`, and be qualified with the record's own
/// file — otherwise a hit would join a site it never executed.
fn validate_region_id(rec: &HitRecord) -> Result<()> {
    if !is_qualified_region_id(&rec.region) {
        return Err(anyhow!(
            "region '{}' is not a qualified region id (fn:<file>::<item-path> or \
             branch:<file>::<item-path>:<anchor>); exports from before per-site ids \
             must be re-collected with `phr-mcp coverage collect`",
            rec.region
        ));
    }
    let prefix = if rec.hit_kind == "branch" {
        "branch:"
    } else {
        "fn:"
    };
    if !rec.region.starts_with(prefix) {
        return Err(anyhow!(
            "region '{}' does not match hit_kind '{}'",
            rec.region,
            rec.hit_kind
        ));
    }
    // A capped id (§3.2: over-long ids are truncated and hashed) may have
    // lost part of its file segment; every other id must start with it.
    let qualified = format!("{prefix}{}::", file_segment(&rec.file));
    if rec.region.len() < MAX_REGION_ID_BYTES && !rec.region.starts_with(&qualified) {
        return Err(anyhow!(
            "region '{}' is not qualified with the record's file '{}'",
            rec.region,
            rec.file
        ));
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

/// All-or-nothing import: parse and validate every record first; only when
/// the whole export passes (including single-revision consistency) is the
/// store replaced (SPEC-coverage-evidence §8).
pub fn import_export(root: &Path, export_path: &Path, now_unix: u64) -> Result<ImportSummary> {
    let records = read_records(export_path)?;
    let revision = single_revision(&records)?;
    write_store(
        root,
        &records,
        &CoverageIndex {
            format: COVERAGE_FORMAT,
            revision: revision.clone(),
            imported_at: now_unix,
            tool: records[0].tool.clone(),
        },
    )?;
    Ok(ImportSummary {
        records: records.len(),
        tests: records
            .iter()
            .map(|r| r.test.as_str())
            .collect::<HashSet<_>>()
            .len(),
        revision,
    })
}

fn read_records(export_path: &Path) -> Result<Vec<HitRecord>> {
    let metadata = fs::metadata(export_path)
        .with_context(|| format!("export file not found: {}", export_path.display()))?;
    if metadata.len() > MAX_EXPORT_SIZE {
        return Err(anyhow!(
            "export file exceeds {} byte limit",
            MAX_EXPORT_SIZE
        ));
    }
    read_lines(export_path)?
        .iter()
        .enumerate()
        .map(|(i, line)| parse_record(i, line))
        .collect()
}

fn read_lines(export_path: &Path) -> Result<Vec<String>> {
    let raw: Vec<String> = BufReader::new(File::open(export_path)?)
        .lines()
        .collect::<std::io::Result<_>>()?;
    Ok(raw.into_iter().filter(|l| !l.trim().is_empty()).collect())
}

fn parse_record(index: usize, line: &str) -> Result<HitRecord> {
    let line_num = index + 1;
    let rec: HitRecord = serde_json::from_str(line)
        .with_context(|| format!("malformed JSON at export line {line_num}"))?;
    match validate_record(&rec) {
        Ok(()) => Ok(rec),
        Err(e) => Err(anyhow!("export line {line_num}: {e}")),
    }
}

fn single_revision(records: &[HitRecord]) -> Result<String> {
    let first = records
        .first()
        .ok_or_else(|| anyhow!("no records found in export"))?;
    if records.iter().any(|r| r.revision != first.revision) {
        return Err(anyhow!("export mixes revisions"));
    }
    Ok(first.revision.clone())
}

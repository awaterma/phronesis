use anyhow::{Context, Result, anyhow};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::coverage::region_map::{file_segment, is_qualified_region_id};
use crate::coverage::store::{COVERAGE_FORMAT, CoverageIndex, HitRecord, write_store};

/// Re-exported: the one validator shared by import and every store read.
pub use crate::coverage::store::validate_record;

#[derive(Debug)]
pub struct ImportSummary {
    pub records: usize,
    pub tests: usize,
    pub revision: String,
}

const MAX_EXPORT_SIZE: u64 = 5 * 1024 * 1024;

/// Region ids must be in the per-site grammar (SPEC-coverage-evidence
/// §3.2), agree with `hit_kind`, and be qualified with the record's own
/// file — otherwise a hit would join a site it never executed. Import-only:
/// the shared read validator checks just the `hit_kind` prefix, so a
/// digest-valid store from before per-site ids reads as stale evidence
/// (`coverage_stale`), not as `store_corrupt`.
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
    // Capping (§3.2) shortens each segment in place and `file_segment`
    // applies the same cap, so every id — capped or not — starts with it.
    let qualified = format!("{prefix}{}::", file_segment(&rec.file));
    if !rec.region.starts_with(&qualified) {
        return Err(anyhow!(
            "region '{}' is not qualified with the record's file '{}'",
            rec.region,
            rec.file
        ));
    }
    Ok(())
}

/// All-or-nothing import: parse and validate every record first; only when
/// the whole export passes (including single-revision consistency) is the
/// store replaced (SPEC-coverage-evidence §8). Revisions are canonicalized
/// to lowercase (git prints lowercase, so an uppercase import would read as
/// permanently stale), exact duplicate records are collapsed, and an export
/// must name exactly one tool: the index records a single `tool` and region
/// identity is tool-specific, so a mixed export is rejected rather than
/// attributed to whichever tool came first.
pub fn import_export(root: &Path, export_path: &Path, now_unix: u64) -> Result<ImportSummary> {
    let records = dedupe(read_records(export_path)?);
    let revision = single_revision(&records)?;
    let tool = single_tool(&records)?;
    write_store(
        root,
        &records,
        &CoverageIndex {
            format: COVERAGE_FORMAT,
            revision: revision.clone(),
            imported_at: now_unix,
            tool,
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
    match validate_record(&rec).and_then(|()| validate_region_id(&rec)) {
        Ok(()) => Ok(HitRecord {
            revision: rec.revision.to_ascii_lowercase(),
            ..rec
        }),
        Err(e) => Err(anyhow!("export line {line_num}: {e}")),
    }
}

/// Drop exact duplicates, keeping first-occurrence order.
fn dedupe(records: Vec<HitRecord>) -> Vec<HitRecord> {
    let mut seen: HashSet<HitRecord> = HashSet::new();
    records
        .into_iter()
        .filter(|r| seen.insert(r.clone()))
        .collect()
}

fn single_tool(records: &[HitRecord]) -> Result<String> {
    let first = records
        .first()
        .ok_or_else(|| anyhow!("no records found in export"))?;
    if records.iter().any(|r| r.tool != first.tool) {
        return Err(anyhow!("export mixes tools"));
    }
    Ok(first.tool.clone())
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

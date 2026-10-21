//! Audit run/scan orchestration. Split from the original `audit.rs`;
//! functions moved verbatim.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::rules_file::{DiskRule, RulesFile};

use super::engine::{
    ScanFileInput, build_per_rule, filter_audit_rules, rule_has_ast_predicate,
    scan_file_into_accum, scan_pack_rules,
};
use super::types::{AuditOpts, AuditReport, Level, PerFileHits};

/// Shared scan core used by both [`run`] and [`run_profiled`].
/// When `times` is `Some`, timing points are recorded; when `None`
/// the `Instant::now()` calls still execute (unconditionally) but
/// stores are gated so there is no semantic difference for the caller.
pub(super) fn run_core(
    rules: &RulesFile,
    opts: &AuditOpts,
    mut times: Option<&mut AuditSectionTimes>,
) -> AuditReport {
    let total_start = Instant::now();
    let (pack_registry, mut incomplete) = match crate::language_packs::load(&opts.project_root) {
        Ok(registry) => (Some(registry), Vec::new()),
        Err(error) => (None, vec![error.to_string()]),
    };

    let audit_rules = filter_audit_rules(rules, opts.rule_filter.as_deref());
    if let Some(ref mut t) = times {
        t.audit_rules = audit_rules.len() as u32;
    }

    // For v1, audit every file the walker accepts. Most rules don't carry
    // an explicit file_pattern condition; default to scanning everything
    // and let the predicates self-filter. `Discovery` also reports files
    // a `.phronesisignore` entry excluded; structural rules still scan
    // those (below the size cap), only lexical rules skip them.
    let (discovery, files_scanned) = {
        let t = Instant::now();
        let d = if audit_rules.is_empty() {
            Discovery::default()
        } else {
            discover_files_with_excluded(&opts.scan_root, &["*"])
        };
        // Every file offered to any scan, excluded ones included.
        let n = (d.scanned.len() + d.excluded.len()) as u32;
        if let Some(ref mut t2) = times {
            t2.discover = t.elapsed();
            t2.files_scanned = n;
        }
        (d, n)
    };

    let structural_rules: Vec<&DiskRule> = audit_rules
        .iter()
        .copied()
        .filter(|r| {
            rule_has_ast_predicate(r)
                || r.conditions
                    .iter()
                    .any(|c| c.predicate.starts_with("pack_"))
        })
        .collect();
    let size_cap = crate::security::max_file_bytes();
    // A pack-owned source audit cannot read (or skips as over the cap) is
    // an unexamined file, not a clean one: with pack rules in play it makes
    // the audit incomplete.
    let has_pack_rules = audit_rules.iter().any(|r| {
        r.conditions
            .iter()
            .any(|c| c.predicate.starts_with("pack_"))
    });
    // The root-relative path of a source a structural-facts pack owns. Its
    // reads are bounded whatever rules are selected; a refused read is
    // reported, since lexical rules would otherwise miss the file silently.
    let pack_owned = |path: &std::path::Path| -> Option<String> {
        let registry = pack_registry.as_ref()?;
        let rel = path
            .strip_prefix(&opts.project_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let owned = registry
            .for_path_with(&rel, crate::language_packs::Capability::StructuralFacts)
            .map_or(true, |pack| pack.is_some());
        owned.then_some(rel)
    };
    let unexamined_pack_source = |path: &std::path::Path, why: &str| -> Option<String> {
        pack_owned(path).map(|rel| format!("{rel}: pack-owned source not audited: {why}"))
    };
    // A pack-owned source is read through the packs' bounded reader, so an
    // oversized one is refused before it is allocated, not after.
    let read_source = |path: &std::path::Path| -> Result<String, String> {
        match pack_owned(path) {
            Some(rel) => crate::language_packs::assets::read_bounded(
                &opts.project_root,
                None,
                &rel,
                crate::language_packs::limits::MAX_FILE_BYTES,
            )
            .map_err(|e| e.to_string())
            .and_then(|bytes| String::from_utf8(bytes).map_err(|e| e.to_string())),
            None => std::fs::read_to_string(path).map_err(|e| e.to_string()),
        }
    };

    // per_rule[rule_id] -> (level, BTreeMap<path -> PerFileHits>)
    let mut accum: BTreeMap<String, (Level, BTreeMap<PathBuf, PerFileHits>)> = BTreeMap::new();

    for path in &discovery.scanned {
        let t = Instant::now();
        let content = match read_source(path) {
            Ok(c) => c,
            Err(error) => {
                if let Some(ref mut t2) = times {
                    t2.read_files += t.elapsed();
                }
                incomplete.extend(unexamined_pack_source(path, &error));
                continue;
            }
        };
        if let Some(ref mut t2) = times {
            t2.read_files += t.elapsed();
        }
        if has_pack_rules
            && let Some(registry) = &pack_registry
            && let Err(error) = scan_pack_rules(
                &opts.project_root,
                path,
                &content,
                registry,
                &audit_rules,
                &mut accum,
            )
        {
            incomplete.push(error.to_string());
        }
        scan_file_into_accum(ScanFileInput {
            project_root: &opts.project_root,
            path,
            content: &content,
            rules: &audit_rules,
            accum: &mut accum,
            times: times.as_deref_mut(),
        });
    }

    // Excluded files: structural rules only, and only under the size cap —
    // an ignored vendored tree must not cost more than scanning it would.
    if !structural_rules.is_empty() {
        for path in &discovery.excluded {
            let over_cap = std::fs::metadata(path)
                .map(|m| m.len() > size_cap)
                .unwrap_or(true);
            if over_cap {
                incomplete.extend(unexamined_pack_source(
                    path,
                    &format!("larger than the {size_cap}-byte cap"),
                ));
                continue;
            }
            let t = Instant::now();
            let content = match read_source(path) {
                Ok(c) => c,
                Err(error) => {
                    if let Some(ref mut t2) = times {
                        t2.read_files += t.elapsed();
                    }
                    incomplete.extend(unexamined_pack_source(path, &error));
                    continue;
                }
            };
            if let Some(ref mut t2) = times {
                t2.read_files += t.elapsed();
            }
            if has_pack_rules
                && let Some(registry) = &pack_registry
                && let Err(error) = scan_pack_rules(
                    &opts.project_root,
                    path,
                    &content,
                    registry,
                    &structural_rules,
                    &mut accum,
                )
            {
                incomplete.push(error.to_string());
            }
            scan_file_into_accum(ScanFileInput {
                project_root: &opts.project_root,
                path,
                content: &content,
                rules: &structural_rules,
                accum: &mut accum,
                times: times.as_deref_mut(),
            });
        }
    }

    let mut lexical_excluded: Vec<PathBuf> = discovery
        .excluded
        .iter()
        .map(|p| {
            p.strip_prefix(&opts.scan_root)
                .map(Path::to_path_buf)
                .unwrap_or_else(|_| p.clone())
        })
        .collect();
    lexical_excluded.sort();

    let (per_rule, total) = {
        let t = Instant::now();
        let r = build_per_rule(accum);
        let total = total_start.elapsed();
        if let Some(ref mut t2) = times {
            t2.report_build = t.elapsed();
            t2.total = total;
        }
        (r, total)
    };

    AuditReport {
        generated_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        scan_duration_ms: total.as_millis() as u64,
        files_scanned,
        per_rule,
        lexical_excluded,
        incomplete,
    }
}

/// Per-section timing breakdown for `run_profiled`. All fields are
/// cumulative across the scan unless noted.
#[derive(Debug, Default, Clone, Copy)]
pub struct AuditSectionTimes {
    /// Time inside `discover_files` (walking the tree).
    pub discover: std::time::Duration,
    /// Sum of `fs::read_to_string` over every file (I/O + decode + alloc).
    pub read_files: std::time::Duration,
    /// Sum of `rust_test_block_keep_mask_for` over every Rust file.
    pub keep_mask: std::time::Duration,
    /// Sum of the rule × condition × line inner loop (substring matching,
    /// the suspected hot spot).
    pub match_loop: std::time::Duration,
    /// Final aggregation/sort/render setup.
    pub report_build: std::time::Duration,
    /// Total wall time of `run_profiled`.
    pub total: std::time::Duration,

    pub files_scanned: u32,
    pub audit_rules: u32,
    /// Total `line.matches(needle)` invocations performed.
    pub line_matches_evaluated: u64,
}

/// Run the audit over `opts.scan_root` using `rules`. Reads files, runs each
/// opted-in rule's predicates against the file contents, returns an
/// `AuditReport`. Never panics; unreadable files are skipped silently.
pub fn run(rules: &RulesFile, opts: &AuditOpts) -> AuditReport {
    run_core(rules, opts, None)
}

/// Profiling variant of [`run`] — same logic, returns per-section wall
/// times via [`AuditSectionTimes`]. Kept in tree as a permanent diagnostic
/// (analogous to the criterion bench in `phronesis`); no behavior change vs
/// `run`. Call this from a probe binary; production callers use `run`.
pub fn run_profiled(rules: &RulesFile, opts: &AuditOpts) -> (AuditReport, AuditSectionTimes) {
    let mut times = AuditSectionTimes::default();
    let report = run_core(rules, opts, Some(&mut times));
    (report, times)
}

/// Walk `root` and return all files whose extension matches one of
/// `extensions`. Respects .gitignore and other standard ignore files
/// via the `ignore` crate. Symlinks not followed.
///
/// `extensions` should be passed without the leading dot (e.g. `["rs", "swift"]`).
/// A wildcard (`["*"]`) returns every file the walker accepts.
pub fn discover_files(root: &Path, extensions: &[&str]) -> Vec<PathBuf> {
    discover_files_with_excluded(root, extensions).scanned
}

/// Files the walker accepted, split by whether a `.phronesisignore` entry
/// excluded them. Both walks honour `.gitignore`, hidden-file defaults and
/// walker errors identically, so a file that only `.gitignore` drops is in
/// neither set: `excluded` is `.phronesisignore` policy and nothing else.
#[derive(Debug, Default)]
pub struct Discovery {
    /// Sorted.
    pub scanned: Vec<PathBuf>,
    /// Excluded by `.phronesisignore` (at any directory level). Sorted.
    /// Structural rules still scan these; only lexical rules skip them.
    pub excluded: Vec<PathBuf>,
}

/// Like [`discover_files`], but also returns the files a `.phronesisignore`
/// excluded: one walk honours the custom ignore file, one does not, and the
/// difference is the policy exclusions. `.phronesisignore` files themselves
/// are never reported.
pub fn discover_files_with_excluded(root: &Path, extensions: &[&str]) -> Discovery {
    use ignore::WalkBuilder;
    use std::collections::BTreeSet;

    fn walk(root: &Path, extensions: &[&str], honour_phronesisignore: bool) -> BTreeSet<PathBuf> {
        let wildcard = extensions.contains(&"*");
        let mut builder = WalkBuilder::new(root);
        builder.follow_links(false);
        // Honour .gitignore even outside a git worktree (e.g. tempdirs in
        // tests) so both walks drop the same gitignored files and the set
        // difference is .phronesisignore policy only.
        builder.require_git(false);
        if honour_phronesisignore {
            builder.add_custom_ignore_filename(".phronesisignore");
        }
        let mut out = BTreeSet::new();
        for entry in builder.build().flatten() {
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let path = entry.into_path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if wildcard || extensions.contains(&ext) {
                out.insert(path);
            }
        }
        out
    }

    let with_ignore = walk(root, extensions, true);
    let without_ignore = walk(root, extensions, false);
    Discovery {
        excluded: without_ignore
            .difference(&with_ignore)
            .filter(|p| p.file_name().is_none_or(|n| n != ".phronesisignore"))
            .cloned()
            .collect(),
        scanned: with_ignore.into_iter().collect(),
    }
}

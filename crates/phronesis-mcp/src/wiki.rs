//! Wiki page primitives — ADR-style decision pages. Shared by `wiki_drift`
//! and by any future wiki-consuming module. Discovery is centralized here:
//! canonical decisions live in `docs/decisions/`, the legacy
//! `.phronesis/wiki/decisions/` location is still read for compatibility,
//! canonical ids take precedence, and differing duplicate copies surface as
//! diagnostics.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

/// Lifecycle status for a decision page. Spec defines exactly three values;
/// parsing into an enum makes typos (`Superseded`, `deprecated`, `draft`) a
/// load-time error instead of letting them silently fall through the drift
/// scoring as if they were `accepted`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DecisionStatus {
    Proposed,
    Accepted,
    Superseded,
}

impl DecisionStatus {
    /// Wire-format string used in JSON output and table rendering.
    pub fn as_str(&self) -> &'static str {
        match self {
            DecisionStatus::Proposed => "proposed",
            DecisionStatus::Accepted => "accepted",
            DecisionStatus::Superseded => "superseded",
        }
    }
}

/// Structured YAML frontmatter at the top of every decision page.
/// See SPEC-wiki-drift.md §"Decision page schema".
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionFrontmatter {
    pub id: String,
    pub date: String,
    pub status: DecisionStatus,
    #[serde(default)]
    pub enforces: Vec<String>,
    #[serde(default)]
    pub superseded_by: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// One decision page on disk: structured frontmatter + free-form body.
#[derive(Debug, Clone)]
pub struct Decision {
    pub frontmatter: DecisionFrontmatter,
    pub body: String,
    pub path: PathBuf,
}

#[derive(Debug, Error)]
pub enum WikiError {
    #[error("wiki directory not found at {0}")]
    DirMissing(String),
    #[error("failed to read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: missing or malformed frontmatter ({message})")]
    Frontmatter { path: String, message: String },
}

/// Default per-project wiki directory: `<project_root>/.phronesis/wiki/`.
pub fn default_wiki_dir(project_root: &Path) -> PathBuf {
    project_root.join(".phronesis").join("wiki")
}

/// Locate the closing `---` fence in the post-opening-fence content `rest`
/// and split into `(yaml, body)` slices. The fence must be exactly three
/// dashes followed by a line break or end-of-file; lookalike lines (`----`,
/// `--- text`) are skipped. Returns the static error message on failure so
/// the caller can wrap it with path context.
fn split_frontmatter(rest: &str) -> Result<(&str, &str), &'static str> {
    let mut search_from = 0usize;
    loop {
        let rel = rest[search_from..]
            .find("\n---")
            .ok_or("missing closing `---` fence")?;
        let idx = search_from + rel;
        // Body starts after the closing fence and the newline that follows it.
        let after_fence = &rest[idx + 4..]; // skip "\n---"
        if after_fence.is_empty() {
            return Ok((&rest[..idx], ""));
        }
        if let Some(body) = after_fence
            .strip_prefix('\n')
            .or_else(|| after_fence.strip_prefix("\r\n"))
        {
            return Ok((&rest[..idx], body));
        }
        search_from = idx + 1;
    }
}

/// Parse a single decision page. Expects:
///
/// ```text
/// ---
/// id: ...
/// date: ...
/// ---
///
/// body text
/// ```
pub fn parse_decision_file(path: &Path) -> Result<Decision, WikiError> {
    let content = std::fs::read_to_string(path).map_err(|source| WikiError::Io {
        path: path.display().to_string(),
        source,
    })?;

    // Frontmatter is bracketed by `---` on its own line at the top of the file.
    // We accept an optional leading whitespace before the opening fence.
    let trimmed = content.trim_start_matches('\u{FEFF}'); // strip UTF-8 BOM if present
    let rest = trimmed
        .strip_prefix("---\n")
        .or_else(|| trimmed.strip_prefix("---\r\n"))
        .ok_or_else(|| WikiError::Frontmatter {
            path: path.display().to_string(),
            message: "expected file to start with `---`".to_string(),
        })?;
    // The closing fence is a line consisting of exactly `---`. A plain
    // `find("\n---")` would also match a `----` divider or a block-scalar
    // line starting with `---` inside the YAML, truncating it mid-document —
    // so keep searching until the three dashes are followed by a line
    // break (or end of file). See `split_frontmatter` for the loop logic.
    let (yaml, body) = split_frontmatter(rest).map_err(|message| WikiError::Frontmatter {
        path: path.display().to_string(),
        message: message.to_string(),
    })?;
    let body = body.to_string();

    let frontmatter: DecisionFrontmatter =
        serde_norway::from_str(yaml).map_err(|e| WikiError::Frontmatter {
            path: path.display().to_string(),
            message: e.to_string(),
        })?;

    Ok(Decision {
        frontmatter,
        body,
        path: path.to_path_buf(),
    })
}

/// Walk a wiki decisions directory and parse every `*.md` file (except
/// `README.md`). Returns decisions sorted by `date` field, newest first.
///
/// Subdirectories are ignored — the convention is flat
/// `<date>-<slug>.md` files.
pub fn walk_decisions(wiki_dir: &Path) -> Result<Vec<Decision>, WikiError> {
    if !wiki_dir.exists() {
        return Err(WikiError::DirMissing(wiki_dir.display().to_string()));
    }
    let entries = std::fs::read_dir(wiki_dir).map_err(|source| WikiError::Io {
        path: wiki_dir.display().to_string(),
        source,
    })?;

    let mut decisions = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| WikiError::Io {
            path: wiki_dir.display().to_string(),
            source,
        })?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        if path.file_name().and_then(|n| n.to_str()) == Some("README.md") {
            continue;
        }
        decisions.push(parse_decision_file(&path)?);
    }

    // Newest first. String comparison works because dates are ISO YYYY-MM-DD.
    decisions.sort_by(|a, b| b.frontmatter.date.cmp(&a.frontmatter.date));
    Ok(decisions)
}

/// Walk a decisions directory tolerantly: parse errors are returned
/// alongside the decisions instead of aborting the walk, so lenient
/// consumers can report them without discarding the whole corpus.
fn walk_decisions_lenient(wiki_dir: &Path) -> (Vec<Decision>, Vec<UnreadableDecision>) {
    let mut decisions = Vec::new();
    let mut unreadable = Vec::new();
    let entries = match std::fs::read_dir(wiki_dir) {
        Ok(entries) => entries,
        Err(error) => {
            return (
                decisions,
                vec![UnreadableDecision {
                    path: wiki_dir.to_path_buf(),
                    message: error.to_string(),
                }],
            );
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                unreadable.push(UnreadableDecision {
                    path: wiki_dir.to_path_buf(),
                    message: error.to_string(),
                });
                continue;
            }
        };
        let path = entry.path();
        if !path.is_file()
            || path.extension().and_then(|e| e.to_str()) != Some("md")
            || path.file_name().and_then(|n| n.to_str()) == Some("README.md")
        {
            continue;
        }
        match parse_decision_file(&path) {
            Ok(decision) => decisions.push(decision),
            Err(error) => unreadable.push(UnreadableDecision {
                path,
                message: error.to_string(),
            }),
        }
    }
    decisions.sort_by(|a, b| b.frontmatter.date.cmp(&a.frontmatter.date));
    (decisions, unreadable)
}

/// Canonical decisions directory, relative to the project root:
/// `docs/decisions/`. Tracked like any other documentation.
pub const CANONICAL_DECISIONS_REL: &str = "docs/decisions";

/// Legacy decisions directory, relative to the project root:
/// `.phronesis/wiki/decisions/`. Pre-0.42 projects keep their decisions
/// here; `init` still creates it when the canonical directory is absent.
pub const LEGACY_DECISIONS_REL: &str = ".phronesis/wiki/decisions";

/// `<project_root>/docs/decisions/`.
pub fn canonical_decisions_dir(project_root: &Path) -> PathBuf {
    project_root.join(CANONICAL_DECISIONS_REL)
}

/// `<project_root>/.phronesis/wiki/decisions/`.
pub fn legacy_decisions_dir(project_root: &Path) -> PathBuf {
    project_root.join(LEGACY_DECISIONS_REL)
}

/// One decision id claimable by two locations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionConflict {
    pub id: String,
    pub canonical: PathBuf,
    pub legacy: PathBuf,
}

/// Two files in one location claiming the same decision id. Recorded by the
/// lenient resolution (graph extraction) and rejected by the strict one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateId {
    pub id: String,
    pub location: PathBuf,
    pub first: PathBuf,
    pub second: PathBuf,
}

/// A decision file whose frontmatter could not be parsed. Lenient consumers
/// (graph extraction) surface it as a visible diagnostic instead of
/// discarding the whole corpus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadableDecision {
    pub path: PathBuf,
    pub message: String,
}

/// Decisions resolved across locations, plus the diagnostics produced while
/// resolving them.
#[derive(Debug, Clone, Default)]
pub struct DecisionSet {
    /// Resolved decisions (canonical precedence by id), newest first.
    pub decisions: Vec<Decision>,
    /// Same id in both locations with differing file content. The canonical
    /// copy governs; the diagnostic tells the operator to reconcile.
    pub conflicts: Vec<DecisionConflict>,
    /// Same id twice within one location (lenient mode only). All copies of
    /// such an id are dropped — no copy is trusted to govern.
    pub duplicates: Vec<DuplicateId>,
    /// Files that failed to parse (lenient mode only). The rest of the
    /// corpus still resolves; the errors stay visible.
    pub unreadable: Vec<UnreadableDecision>,
}

/// How strict resolution treats two files in one location claiming the same
/// decision id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuplicatePolicy {
    /// Return [`DecisionsError::DuplicateId`] (drift and CLI consumers:
    /// a duplicate id makes the corpus untrustworthy).
    Reject,
    /// Drop all copies of the id, record them in
    /// [`DecisionSet::duplicates`], and keep going (graph extraction must
    /// stay total; the duplicate surfaces as a diagnostic edge).
    DropWithDiagnostic,
}

#[derive(Debug, Error)]
pub enum DecisionsError {
    #[error("no decisions directory found; checked {0} and {1}")]
    NoDecisionsDir(String, String),
    #[error("decisions directory not found at {0}")]
    DirMissing(String),
    #[error("duplicate decision id `{id}` in {location}: {first} and {second} both claim it")]
    DuplicateId {
        id: String,
        location: String,
        first: String,
        second: String,
    },
    #[error(transparent)]
    Wiki(#[from] WikiError),
}

/// Resolve the decision corpus for `project_root`.
///
/// With `override_dir`, exactly that directory is scanned (callers that let
/// the operator point elsewhere keep full control; no merging). Otherwise
/// the canonical `docs/decisions/` and legacy `.phronesis/wiki/decisions/`
/// directories are scanned in that order when they exist. Canonical
/// documents take precedence on duplicate ids; a legacy copy with differing
/// content is recorded as a conflict diagnostic and an identical copy is
/// silently superseded.
pub fn resolve_decisions(
    project_root: &Path,
    override_dir: Option<&Path>,
    policy: DuplicatePolicy,
) -> Result<DecisionSet, DecisionsError> {
    let dirs: Vec<PathBuf> = match override_dir {
        Some(dir) => vec![dir.to_path_buf()],
        None => {
            let canonical = canonical_decisions_dir(project_root);
            let legacy = legacy_decisions_dir(project_root);
            let mut dirs = Vec::new();
            if canonical.is_dir() {
                dirs.push(canonical.clone());
            }
            if legacy.is_dir() {
                dirs.push(legacy.clone());
            }
            if dirs.is_empty() {
                return Err(DecisionsError::NoDecisionsDir(
                    canonical.display().to_string(),
                    legacy.display().to_string(),
                ));
            }
            dirs
        }
    };

    let mut scanned: Vec<(PathBuf, Vec<Decision>)> = Vec::with_capacity(dirs.len());
    let mut duplicates = Vec::new();
    let mut set_errors: Vec<UnreadableDecision> = Vec::new();
    for dir in &dirs {
        if !dir.is_dir() {
            return Err(DecisionsError::DirMissing(dir.display().to_string()));
        }
        let (decisions, unreadable) = match policy {
            DuplicatePolicy::Reject => (walk_decisions(dir)?, Vec::new()),
            DuplicatePolicy::DropWithDiagnostic => walk_decisions_lenient(dir),
        };
        unreadable.into_iter().for_each(|u| set_errors.push(u));
        let mut seen: std::collections::BTreeMap<&str, PathBuf> = std::collections::BTreeMap::new();
        let mut duplicated_ids: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for decision in &decisions {
            match seen.get(decision.frontmatter.id.as_str()) {
                Some(first) => match policy {
                    DuplicatePolicy::Reject => {
                        return Err(DecisionsError::DuplicateId {
                            id: decision.frontmatter.id.clone(),
                            location: dir.display().to_string(),
                            first: first.display().to_string(),
                            second: decision.path.display().to_string(),
                        });
                    }
                    DuplicatePolicy::DropWithDiagnostic => {
                        duplicated_ids.insert(decision.frontmatter.id.clone());
                        duplicates.push(DuplicateId {
                            id: decision.frontmatter.id.clone(),
                            location: dir.clone(),
                            first: first.clone(),
                            second: decision.path.clone(),
                        });
                    }
                },
                None => {
                    seen.insert(decision.frontmatter.id.as_str(), decision.path.clone());
                }
            }
        }
        // Lenient mode drops every copy of an ambiguous id: no copy is
        // trusted to govern.
        let kept = if duplicated_ids.is_empty() {
            decisions
        } else {
            decisions
                .into_iter()
                .filter(|d| !duplicated_ids.contains(d.frontmatter.id.as_str()))
                .collect()
        };
        scanned.push((dir.clone(), kept));
    }

    let mut by_id: std::collections::BTreeMap<String, Decision> = std::collections::BTreeMap::new();
    let ambiguous: std::collections::HashSet<&str> = duplicates
        .iter()
        .map(|duplicate| duplicate.id.as_str())
        .collect();
    let mut conflicts = Vec::new();
    // `dirs` lists canonical before legacy, so the first claim on an id is
    // the canonical copy (or, under an override, the only copy — no merging
    // can occur with a single directory).
    for (_, decisions) in scanned {
        for decision in decisions {
            let id = decision.frontmatter.id.clone();
            if ambiguous.contains(id.as_str()) {
                continue;
            }
            match by_id.get(&id) {
                None => {
                    by_id.insert(id, decision);
                }
                Some(existing) => {
                    if file_contents_differ(&existing.path, &decision.path) {
                        conflicts.push(DecisionConflict {
                            id,
                            canonical: existing.path.clone(),
                            legacy: decision.path.clone(),
                        });
                    }
                }
            }
        }
    }

    let mut decisions: Vec<Decision> = by_id.into_values().collect();
    decisions.sort_by(|a, b| b.frontmatter.date.cmp(&a.frontmatter.date));
    Ok(DecisionSet {
        decisions,
        conflicts,
        duplicates,
        unreadable: set_errors,
    })
}

fn file_contents_differ(a: &Path, b: &Path) -> bool {
    match (std::fs::read_to_string(a), std::fs::read_to_string(b)) {
        (Ok(a), Ok(b)) => a != b,
        _ => true,
    }
}

/// Lenient entry for consumers that must stay total (graph extraction feeds
/// freshness and hook-time provenance and cannot fail closed on a decision
/// layout problem): a missing corpus yields an empty set and within-location
/// duplicates are dropped with a recorded diagnostic.
pub fn resolve_decisions_lenient(project_root: &Path) -> DecisionSet {
    resolve_decisions(project_root, None, DuplicatePolicy::DropWithDiagnostic).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(dir: &TempDir, name: &str, content: &str) -> PathBuf {
        let p = dir.path().join(name);
        fs::write(&p, content).expect("write fixture");
        p
    }

    #[test]
    fn parse_minimal_decision() {
        let dir = TempDir::new().unwrap();
        let p = write(
            &dir,
            "2026-05-29-api-naming.md",
            "---\n\
             id: api-naming\n\
             date: 2026-05-29\n\
             status: accepted\n\
             ---\n\
             \n\
             ## Decision\n\
             Use snake_case for API fields.\n",
        );
        let d = parse_decision_file(&p).expect("parse");
        assert_eq!(d.frontmatter.id, "api-naming");
        assert_eq!(d.frontmatter.date, "2026-05-29");
        assert_eq!(d.frontmatter.status, DecisionStatus::Accepted);
        assert!(d.frontmatter.enforces.is_empty());
        assert!(d.frontmatter.superseded_by.is_none());
        assert!(d.body.contains("## Decision"));
        assert!(d.body.contains("Use snake_case for API fields"));
        assert_eq!(d.path, p);
    }

    #[test]
    fn parse_decision_with_enforces_list_and_tags() {
        let dir = TempDir::new().unwrap();
        let p = write(
            &dir,
            "x.md",
            "---\n\
             id: workspace-cargo\n\
             date: 2026-05-29\n\
             status: accepted\n\
             enforces:\n  - warn-cargo-build-without-workspace\n  - some-other-rule\n\
             tags: [build, hygiene]\n\
             ---\n\
             body\n",
        );
        let d = parse_decision_file(&p).expect("parse");
        assert_eq!(
            d.frontmatter.enforces,
            vec![
                "warn-cargo-build-without-workspace".to_string(),
                "some-other-rule".to_string()
            ]
        );
        assert_eq!(
            d.frontmatter.tags,
            vec!["build".to_string(), "hygiene".to_string()]
        );
    }

    #[test]
    fn parse_decision_with_superseded_by() {
        let dir = TempDir::new().unwrap();
        let p = write(
            &dir,
            "x.md",
            "---\n\
             id: old\n\
             date: 2026-01-01\n\
             status: superseded\n\
             superseded_by: new-decision\n\
             ---\n\
             ",
        );
        let d = parse_decision_file(&p).expect("parse");
        assert_eq!(
            d.frontmatter.superseded_by,
            Some("new-decision".to_string())
        );
    }

    #[test]
    fn parse_decision_missing_frontmatter_errors() {
        let dir = TempDir::new().unwrap();
        let p = write(&dir, "x.md", "no frontmatter here\njust prose\n");
        match parse_decision_file(&p) {
            Err(WikiError::Frontmatter { .. }) => {}
            other => panic!("expected Frontmatter error, got {:?}", other),
        }
    }

    #[test]
    fn parse_decision_missing_required_field_errors() {
        let dir = TempDir::new().unwrap();
        let p = write(
            &dir,
            "x.md",
            "---\nid: x\ndate: 2026-01-01\n---\nbody\n", // missing `status`
        );
        match parse_decision_file(&p) {
            Err(WikiError::Frontmatter { message, .. }) => assert!(message.contains("status")),
            other => panic!("expected Frontmatter error, got {:?}", other),
        }
    }

    #[test]
    fn parse_decision_unknown_field_errors() {
        // deny_unknown_fields catches typos that would otherwise silently drop.
        let dir = TempDir::new().unwrap();
        let p = write(
            &dir,
            "x.md",
            "---\nid: x\ndate: 2026-01-01\nstatus: accepted\nstauts: typo\n---\n",
        );
        assert!(matches!(
            parse_decision_file(&p),
            Err(WikiError::Frontmatter { .. })
        ));
    }

    #[test]
    fn parse_decision_unknown_status_value_errors() {
        // Status is enum-typed (proposed/accepted/superseded). Typos like
        // `Superseded` or `deprecated` must fail loudly, not silently fall
        // through to active-drift scoring.
        let dir = TempDir::new().unwrap();
        let p = write(
            &dir,
            "x.md",
            "---\nid: x\ndate: 2026-01-01\nstatus: Superseded\n---\n",
        );
        assert!(matches!(
            parse_decision_file(&p),
            Err(WikiError::Frontmatter { .. })
        ));
    }

    #[test]
    fn parse_decision_fence_with_trailing_text_is_not_a_fence() {
        // `--- see appendix` is not a closing fence. The old find("\n---")
        // accepted it, silently truncating the YAML and leaking the line's
        // tail into the body. Now the parser keeps searching and, absent a
        // real fence, reports it missing.
        let dir = TempDir::new().unwrap();
        let p = write(
            &dir,
            "x.md",
            "---\nid: x\ndate: 2026-01-01\nstatus: accepted\n--- see appendix\n",
        );
        let err = parse_decision_file(&p).expect_err("trailing text is not a fence");
        assert!(err.to_string().contains("missing closing `---` fence"));
    }

    #[test]
    fn parse_decision_four_dash_divider_is_not_a_fence() {
        // A `----` divider must not close the frontmatter (the old code
        // matched it and prepended a stray `-` to the body).
        let dir = TempDir::new().unwrap();
        let p = write(
            &dir,
            "x.md",
            "---\nid: x\ndate: 2026-01-01\nstatus: accepted\n----\n",
        );
        let err = parse_decision_file(&p).expect_err("---- is not a fence");
        assert!(err.to_string().contains("missing closing `---` fence"));
    }

    #[test]
    fn parse_decision_real_fence_after_lookalike_line() {
        // A lookalike line is skipped; the real fence further down still
        // closes the frontmatter. The YAML in between is then judged by the
        // parser (here it is invalid, and the error names the YAML problem
        // rather than silently truncating at the lookalike).
        let dir = TempDir::new().unwrap();
        let p = write(
            &dir,
            "x.md",
            "---\nid: x\ndate: 2026-01-01\nstatus: accepted\n--- stray\n---\n\nbody\n",
        );
        let err = parse_decision_file(&p).expect_err("lookalike makes the YAML invalid");
        let msg = err.to_string();
        assert!(!msg.contains("missing closing"), "fence was found: {msg}");
    }

    #[test]
    fn parse_decision_fence_at_eof_without_trailing_newline() {
        let dir = TempDir::new().unwrap();
        let p = write(
            &dir,
            "x.md",
            "---\nid: x\ndate: 2026-01-01\nstatus: accepted\n---",
        );
        let d = parse_decision_file(&p).expect("fence at EOF closes frontmatter");
        assert_eq!(d.frontmatter.id, "x");
        assert!(d.body.is_empty());
    }

    #[test]
    fn parse_decision_crlf_line_endings() {
        let dir = TempDir::new().unwrap();
        let p = write(
            &dir,
            "x.md",
            "---\r\nid: x\r\ndate: 2026-01-01\r\nstatus: accepted\r\n---\r\nbody\r\n",
        );
        let d = parse_decision_file(&p).expect("CRLF fences parse");
        assert_eq!(d.frontmatter.id, "x");
        assert!(d.body.contains("body"));
    }

    #[test]
    fn walk_decisions_returns_empty_for_empty_dir() {
        let dir = TempDir::new().unwrap();
        let wiki = dir.path().join("decisions");
        fs::create_dir(&wiki).unwrap();
        let decisions = walk_decisions(&wiki).unwrap();
        assert!(decisions.is_empty());
    }

    #[test]
    fn walk_decisions_missing_dir_errors() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("nope");
        match walk_decisions(&missing) {
            Err(WikiError::DirMissing(_)) => {}
            other => panic!("expected DirMissing, got {:?}", other),
        }
    }

    #[test]
    fn walk_decisions_parses_md_files_and_skips_readme() {
        let dir = TempDir::new().unwrap();
        let wiki = dir.path().join("decisions");
        fs::create_dir(&wiki).unwrap();
        fs::write(wiki.join("README.md"), "# not a decision\n").unwrap();
        fs::write(
            wiki.join("2026-05-29-a.md"),
            "---\nid: a\ndate: 2026-05-29\nstatus: accepted\n---\nbody\n",
        )
        .unwrap();
        fs::write(
            wiki.join("2026-05-28-b.md"),
            "---\nid: b\ndate: 2026-05-28\nstatus: accepted\n---\nbody\n",
        )
        .unwrap();
        fs::write(wiki.join("notes.txt"), "non-md, should be skipped").unwrap();

        let decisions = walk_decisions(&wiki).unwrap();
        // README.md and notes.txt skipped; both .md decisions parsed.
        let ids: Vec<&str> = decisions
            .iter()
            .map(|d| d.frontmatter.id.as_str())
            .collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"a"));
        assert!(ids.contains(&"b"));
    }

    #[test]
    fn walk_decisions_sorted_by_date_desc() {
        let dir = TempDir::new().unwrap();
        let wiki = dir.path().join("decisions");
        fs::create_dir(&wiki).unwrap();
        fs::write(
            wiki.join("c.md"),
            "---\nid: c\ndate: 2025-01-01\nstatus: accepted\n---\n",
        )
        .unwrap();
        fs::write(
            wiki.join("a.md"),
            "---\nid: a\ndate: 2026-06-01\nstatus: accepted\n---\n",
        )
        .unwrap();
        fs::write(
            wiki.join("b.md"),
            "---\nid: b\ndate: 2026-03-15\nstatus: accepted\n---\n",
        )
        .unwrap();
        let decisions = walk_decisions(&wiki).unwrap();
        let dates: Vec<&str> = decisions
            .iter()
            .map(|d| d.frontmatter.date.as_str())
            .collect();
        // Newest first.
        assert_eq!(dates, vec!["2026-06-01", "2026-03-15", "2025-01-01"]);
    }
}

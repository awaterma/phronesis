//! Source dispatch and availability resolution.
//! See `docs/specs/SPEC-drift-consolidation.md` §1 and §5.

use std::path::Path;

use super::types::{
    AggregateReport, Availability, DriftItem, DriftReport, MissingReason, Source, Totals,
    uncovered_count,
};

pub const DEFAULT_LIMIT: usize = 5;
pub const MAX_LIMIT: usize = 50;

/// Everything a source might need, built by the caller from request
/// parameters and workspace state. The drift core stays pure over this.
pub struct SourceInputs<'a> {
    pub project_root: &'a Path,
    pub claude_md: Option<&'a Path>,
    pub memory_dir: Option<&'a Path>,
    pub wiki_dir: Option<&'a Path>,
    /// Include the draft-rule `suggestion` on each item. Off by default:
    /// a draft is several hundred bytes of escaped JSON, and measured
    /// against this repository it was 64% of the whole response (6,014 of
    /// 9,374 bytes across 10 items). Spending most of a bounded summary on
    /// payload the caller did not ask for defeats the point of bounding it.
    pub suggest: bool,
}

/// Drop `suggestion` unless the caller asked for it.
///
/// Applied here rather than in each source adapter so the adapters stay
/// pure projections of their native reports, and so a source added later
/// cannot forget the gate.
fn apply_suggest(mut items: Vec<DriftItem>, suggest: bool) -> Vec<DriftItem> {
    if !suggest {
        for item in &mut items {
            item.suggestion = None;
        }
    }
    items
}

pub fn clamp_limit(requested: usize) -> usize {
    requested.clamp(1, MAX_LIMIT)
}

/// Sort by descending triage urgency, then truncate. Sorting first means a
/// truncated response keeps the items that matter, rather than whichever
/// happened to be scanned first.
///
/// Personal entries are deprioritized below actionable drift so they cannot
/// crowd out real findings. They share `Family::Uncovered` with actionable
/// items (a personal entry scoring below threshold is correctly `Uncovered`),
/// so the category axis breaks the tie: non-Personal sorts first within the
/// same family.
pub fn apply_limit(mut items: Vec<DriftItem>, limit: usize) -> (Vec<DriftItem>, bool) {
    items.sort_by(|a, b| {
        let fam_cmp = b.verdict.family().cmp(&a.verdict.family());
        if fam_cmp != std::cmp::Ordering::Equal {
            return fam_cmp;
        }
        let a_personal = a.category == Some(super::types::Category::Personal);
        let b_personal = b.category == Some(super::types::Category::Personal);
        a_personal.cmp(&b_personal)
    });
    let truncated = items.len() > limit;
    items.truncate(limit);
    (items, truncated)
}

/// Run one source. Never returns Err: an absent corpus is reported as
/// `Missing`, an unreadable one as `Errored`.
pub fn run_source(source: Source, inputs: &SourceInputs<'_>) -> DriftReport {
    match source {
        Source::ClaudeMd => run_claude_md(inputs),
        Source::Memory => run_memory(inputs),
        Source::Wiki => run_wiki(inputs),
        // Registered by SPEC-rule-staleness; until then there is no graph
        // binding source to consult.
        Source::Code => DriftReport::missing(Source::Code, MissingReason::NoGraph),
    }
}

fn run_claude_md(inputs: &SourceInputs<'_>) -> DriftReport {
    // `claude_md_drift::run` resolves CLAUDE.md from the project root
    // itself, so the only job here is the availability pre-check: report a
    // missing file rather than letting the source raise
    // `DriftError::ClaudeMdMissing`.
    if !inputs.project_root.join("CLAUDE.md").exists() {
        return DriftReport::missing(Source::ClaudeMd, MissingReason::NoFile);
    }
    match crate::claude_md_drift::run(inputs.project_root) {
        Ok(report) => {
            let items = apply_suggest(crate::claude_md_drift::into_items(&report), inputs.suggest);
            DriftReport {
                source: Source::ClaudeMd,
                availability: Availability::Present {
                    scanned: report.items.len(),
                },
                uncovered_count: uncovered_count(&items),
                items,
            }
        }
        Err(e) => DriftReport::errored(Source::ClaudeMd, e.to_string()),
    }
}

fn run_memory(inputs: &SourceInputs<'_>) -> DriftReport {
    let dir = match inputs.memory_dir {
        Some(p) => p.to_path_buf(),
        None => crate::memory_drift::default_memory_dir(inputs.project_root),
    };
    if !dir.is_dir() {
        return DriftReport::missing(Source::Memory, MissingReason::NoDir);
    }
    match crate::memory_drift::run_with_dir(inputs.project_root, &dir) {
        Ok(report) => {
            let items = apply_suggest(crate::memory_drift::into_items(&report), inputs.suggest);
            DriftReport {
                source: Source::Memory,
                availability: Availability::Present {
                    scanned: report.items.len(),
                },
                uncovered_count: uncovered_count(&items),
                items,
            }
        }
        Err(e) => DriftReport::errored(Source::Memory, e.to_string()),
    }
}

fn run_wiki(inputs: &SourceInputs<'_>) -> DriftReport {
    let dir = match inputs.wiki_dir {
        Some(p) => p.to_path_buf(),
        None => crate::wiki::default_wiki_dir(inputs.project_root).join("decisions"),
    };
    if !dir.is_dir() {
        return DriftReport::missing(Source::Wiki, MissingReason::NoDir);
    }
    match crate::wiki_drift::run_with_dir(inputs.project_root, &dir) {
        Ok(report) => {
            let items = apply_suggest(crate::wiki_drift::into_items(&report), inputs.suggest);
            DriftReport {
                source: Source::Wiki,
                availability: Availability::Present {
                    scanned: report.items.len(),
                },
                uncovered_count: uncovered_count(&items),
                items,
            }
        }
        Err(e) => DriftReport::errored(Source::Wiki, e.to_string()),
    }
}

/// Run several sources. One source failing never suppresses the others.
pub fn run_all(sources: &[Source], inputs: &SourceInputs<'_>, limit: usize) -> AggregateReport {
    let limit = clamp_limit(limit);
    let mut totals = Totals::default();
    let mut truncated_any = false;
    let mut reports = Vec::with_capacity(sources.len());

    for &source in sources {
        let mut report = run_source(source, inputs);
        match report.availability {
            Availability::Present { .. } => totals.sources_present += 1,
            Availability::Missing { .. } => totals.sources_missing += 1,
            Availability::Errored { .. } => totals.sources_errored += 1,
        }
        totals.uncovered_total += report.uncovered_count;
        for item in &report.items {
            if item.category == Some(super::types::Category::Personal) {
                continue;
            }
            *totals.by_family.entry(item.verdict.family()).or_insert(0) += 1;
        }
        let (kept, truncated) = apply_limit(std::mem::take(&mut report.items), limit);
        report.items = kept;
        truncated_any |= truncated;
        reports.push(report);
    }

    AggregateReport {
        sources: reports,
        totals,
        truncated: truncated_any,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drift::{Availability, MissingReason, Source};

    /// Every corpus path is pinned to a nonexistent location *inside* the
    /// temp dir.
    ///
    /// Leaving `memory_dir: None` would make `run_memory` fall back to
    /// `memory_drift::default_memory_dir`, which reads the real
    /// `dirs::home_dir()`. On a machine that happens to have a matching
    /// `~/.claude/projects/<encoded>/memory`, the source would be `Present`
    /// and the "all absent" assertions would fail for reasons unrelated to
    /// the code under test.
    struct Paths {
        memory: std::path::PathBuf,
        wiki: std::path::PathBuf,
    }

    fn absent_paths(root: &std::path::Path) -> Paths {
        Paths {
            memory: root.join("no-such-memory-dir"),
            wiki: root.join("no-such-wiki-dir"),
        }
    }

    fn empty_inputs<'a>(root: &'a std::path::Path, p: &'a Paths) -> SourceInputs<'a> {
        SourceInputs {
            project_root: root,
            claude_md: None,
            memory_dir: Some(&p.memory),
            wiki_dir: Some(&p.wiki),
            suggest: false,
        }
    }

    /// A project whose memory dir holds one uncovered entry that
    /// `memory_drift::suggest_rule` will actually draft a rule for.
    ///
    /// Two conditions have to hold, and both are easy to miss: the entry must
    /// land in `Bucket::Actionable`, which `classify` grants only when the
    /// text carries a predicate-shaped trigger (`memory_drift.rs:264-283`) —
    /// hence the `gh pr` invocation, which matches the command-shaped family
    /// — and it must score below the coverage threshold, hence a rules file
    /// whose only rule shares no vocabulary with it.
    fn project_with_one_memory_entry(root: &std::path::Path) -> std::path::PathBuf {
        std::fs::create_dir_all(root.join(".phronesis")).expect("mkdir");
        std::fs::write(
            root.join(".phronesis").join("rules.json"),
            r#"{"rules":[{"id":"r","phase":"pre","when":[{"new_content_contains":"zzz"}],"then":{"log":"y"}}]}"#,
        )
        .expect("rules");
        let mem = root.join("memory");
        std::fs::create_dir_all(&mem).expect("mkdir memory");
        std::fs::write(
            mem.join("thing.md"),
            "---\nname: always-review-before-merge\ndescription: run `gh pr checks` before merging\nmetadata:\n  type: feedback\n---\n\nAlways run `gh pr checks` and wait for green before merging.\n",
        )
        .expect("entry");
        mem
    }

    #[test]
    fn suggestions_are_omitted_unless_requested() {
        // A draft rule is ~800 bytes of escaped JSON per item. Emitting it
        // unconditionally spent most of the response on payload the caller
        // did not ask for, which defeats the bounded-summary promise the
        // `all` source makes.
        let d = tempfile::tempdir().expect("tempdir");
        let mem = project_with_one_memory_entry(d.path());
        let inputs = SourceInputs {
            project_root: d.path(),
            claude_md: None,
            memory_dir: Some(&mem),
            wiki_dir: None,
            suggest: false,
        };
        let report = run_source(Source::Memory, &inputs);
        assert!(
            !report.items.is_empty(),
            "fixture must produce at least one item"
        );
        assert!(
            report.items.iter().all(|i| i.suggestion.is_none()),
            "suggestions must be withheld by default"
        );
    }

    #[test]
    fn suggestions_are_present_when_requested() {
        let d = tempfile::tempdir().expect("tempdir");
        let mem = project_with_one_memory_entry(d.path());
        let inputs = SourceInputs {
            project_root: d.path(),
            claude_md: None,
            memory_dir: Some(&mem),
            wiki_dir: None,
            suggest: true,
        };
        let report = run_source(Source::Memory, &inputs);
        assert!(
            report.items.iter().any(|i| i.suggestion.is_some()),
            "suggest: true must produce at least one draft rule"
        );
    }

    #[test]
    fn a_missing_corpus_is_reported_not_raised() {
        let d = tempfile::tempdir().expect("tempdir");
        let paths = absent_paths(d.path());
        let report = run_source(Source::Wiki, &empty_inputs(d.path(), &paths));
        assert!(matches!(
            report.availability,
            Availability::Missing {
                reason: MissingReason::NoDir
            }
        ));
        assert_eq!(report.uncovered_count, 0);
        assert!(report.items.is_empty());
    }

    #[test]
    fn code_source_is_missing_until_rule_staleness_lands() {
        let d = tempfile::tempdir().expect("tempdir");
        let paths = absent_paths(d.path());
        let report = run_source(Source::Code, &empty_inputs(d.path(), &paths));
        assert!(matches!(
            report.availability,
            Availability::Missing {
                reason: MissingReason::NoGraph
            }
        ));
    }

    #[test]
    fn run_all_succeeds_when_every_corpus_is_absent() {
        let d = tempfile::tempdir().expect("tempdir");
        let paths = absent_paths(d.path());
        let agg = run_all(Source::ALL, &empty_inputs(d.path(), &paths), 5);
        assert_eq!(agg.sources.len(), 4);
        assert_eq!(agg.totals.sources_missing, 4);
        assert_eq!(agg.totals.sources_present, 0);
        assert_eq!(agg.totals.uncovered_total, 0);
        assert!(!agg.truncated);
    }

    #[test]
    fn sources_are_returned_in_stable_order() {
        let d = tempfile::tempdir().expect("tempdir");
        let paths = absent_paths(d.path());
        let agg = run_all(Source::ALL, &empty_inputs(d.path(), &paths), 5);
        let order: Vec<Source> = agg.sources.iter().map(|r| r.source).collect();
        assert_eq!(order, Source::ALL.to_vec());
    }

    #[test]
    fn a_malformed_source_does_not_suppress_a_healthy_one() {
        // Spec §5.2. An operator asking "what drift exists" must not get a
        // hard failure because one of four corpora is malformed —
        // particularly when the malformed corpus is what they are trying
        // to diagnose.
        let d = tempfile::tempdir().expect("tempdir");
        let root = d.path();
        std::fs::create_dir_all(root.join(".phronesis")).expect("mkdir");
        std::fs::write(
            root.join(".phronesis").join("rules.json"),
            r#"{"rules":[{"id":"r","phase":"pre","when":[{"new_content_contains":"x"}],"then":{"log":"y"}}]}"#,
        )
        .expect("rules");
        std::fs::write(root.join("CLAUDE.md"), "- Always prefer iterators\n").expect("claude");

        // A decisions dir that exists but holds an unparseable page.
        let wiki = root.join(".phronesis").join("wiki").join("decisions");
        std::fs::create_dir_all(&wiki).expect("mkdir wiki");
        std::fs::write(wiki.join("broken.md"), "no frontmatter here\n").expect("broken adr");

        let memory = root.join("no-such-memory-dir");
        let inputs = SourceInputs {
            project_root: root,
            claude_md: None,
            memory_dir: Some(&memory),
            wiki_dir: Some(&wiki),
            suggest: false,
        };

        let agg = run_all(Source::ALL, &inputs, 5);
        let claude = agg
            .sources
            .iter()
            .find(|r| r.source == Source::ClaudeMd)
            .expect("claude_md report");
        assert!(
            matches!(claude.availability, Availability::Present { .. }),
            "a healthy source must survive a malformed sibling: {:?}",
            claude.availability
        );
        assert!(
            !claude.items.is_empty(),
            "the healthy source must still return items"
        );
    }

    #[test]
    fn limit_truncates_items_and_sets_the_flag() {
        let items: Vec<crate::drift::DriftItem> = (0..10)
            .map(|i| crate::drift::DriftItem {
                subject: format!("item-{i}"),
                verdict: crate::drift::Verdict::Uncovered,
                category: None,
                suggestion: None,
                evidence: crate::drift::Evidence::Heuristic {
                    score: 0.0,
                    threshold: 0.15,
                    matched_rules: vec![],
                },
            })
            .collect();
        let (kept, truncated) = apply_limit(items, 3);
        assert_eq!(kept.len(), 3);
        assert!(truncated);
    }

    #[test]
    fn limit_is_clamped_to_fifty() {
        assert_eq!(clamp_limit(0), 1);
        assert_eq!(clamp_limit(5), 5);
        assert_eq!(clamp_limit(9999), 50);
    }

    #[test]
    fn items_are_ordered_by_family_urgency_before_truncation() {
        let mk = |v: crate::drift::Verdict, s: &str| crate::drift::DriftItem {
            subject: s.to_string(),
            verdict: v,
            category: None,
            suggestion: None,
            evidence: crate::drift::Evidence::Heuristic {
                score: 0.0,
                threshold: 0.15,
                matched_rules: vec![],
            },
        };
        let items = vec![
            mk(crate::drift::Verdict::Covered, "covered"),
            mk(crate::drift::Verdict::Uncovered, "uncovered"),
        ];
        let (kept, _) = apply_limit(items, 1);
        assert_eq!(
            kept[0].subject, "uncovered",
            "most urgent must survive truncation"
        );
    }

    #[test]
    fn personal_entries_do_not_crowd_out_actionable_drift() {
        // F3: Personal entries share Family::Uncovered with actionable items.
        // Without a category-aware sort, alphabetical scan order can fill
        // every slot with Personal entries, hiding real drift.
        let mk = |v: crate::drift::Verdict, cat: Option<crate::drift::Category>, s: &str| {
            crate::drift::DriftItem {
                subject: s.to_string(),
                verdict: v,
                category: cat,
                suggestion: None,
                evidence: crate::drift::Evidence::Heuristic {
                    score: 0.0,
                    threshold: 0.15,
                    matched_rules: vec![],
                },
            }
        };
        let items = vec![
            mk(
                crate::drift::Verdict::Uncovered,
                Some(crate::drift::Category::Personal),
                "aaa-personal",
            ),
            mk(
                crate::drift::Verdict::Uncovered,
                Some(crate::drift::Category::Personal),
                "bbb-personal",
            ),
            mk(
                crate::drift::Verdict::Uncovered,
                None,
                "actionable-drift",
            ),
        ];
        let (kept, _) = apply_limit(items, 1);
        assert_eq!(
            kept[0].subject, "actionable-drift",
            "actionable drift must survive truncation over Personal entries"
        );
    }

    #[test]
    fn by_family_excludes_personal_entries() {
        // F4: by_family must agree with uncovered_total, which excludes
        // Personal. Without this, the JSON envelope can report
        // {"by_family": {"uncovered": N}} while uncovered_total is 0.
        use crate::drift::{Category, DriftItem, Evidence, Verdict};

        let mk = |v: Verdict, cat: Option<Category>| DriftItem {
            subject: "s".to_string(),
            verdict: v,
            category: cat,
            suggestion: None,
            evidence: Evidence::Heuristic {
                score: 0.0,
                threshold: 0.15,
                matched_rules: vec![],
            },
        };
        let items = vec![
            mk(Verdict::Uncovered, Some(Category::Personal)),
            mk(Verdict::Uncovered, Some(Category::Personal)),
            mk(Verdict::Uncovered, None),
        ];
        let report = DriftReport {
            source: Source::Memory,
            availability: Availability::Present { scanned: 3 },
            uncovered_count: uncovered_count(&items),
            items,
        };
        let mut totals = Totals::default();
        for item in &report.items {
            if item.category == Some(crate::drift::Category::Personal) {
                continue;
            }
            *totals.by_family.entry(item.verdict.family()).or_insert(0) += 1;
        }
        totals.uncovered_total = report.uncovered_count;
        assert_eq!(totals.uncovered_total, 1, "one non-Personal uncovered item");
        assert_eq!(
            totals.by_family.get(&crate::drift::Family::Uncovered),
            Some(&1),
            "by_family must exclude Personal, matching uncovered_total"
        );
    }
}

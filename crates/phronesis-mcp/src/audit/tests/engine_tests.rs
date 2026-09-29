//! Tests for the audit engine, run, diagnostics, and types. Split from
//! the original `audit.rs` `mod tests`.

use crate::audit::*;
use crate::rules_file::{DiskAction, DiskCondition, DiskRule, RulesFile};
use std::path::{Path, PathBuf};

pub(super) fn rule(id: &str, content_match: &str, action: &str) -> DiskRule {
    DiskRule {
        id: id.to_string(),
        phase: "pre".to_string(),
        priority: 10,
        conditions: vec![DiskCondition {
            predicate: "new_content_contains".to_string(),
            args: vec![content_match.to_string()],
            script: None,
        }],
        actions: vec![DiskAction {
            action_type: action.to_string(),
            params: vec![format!("{} fired", id)],
            ..Default::default()
        }],
        silent: None,
        audit: Some(true),
        doc_excepted: None,
    }
}

#[test]
fn run_finds_content_matches_in_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("a.rs"),
        "fn main() { let x = foo.unwrap(); }",
    )
    .unwrap();
    let rules = RulesFile {
        rules: vec![rule("no-unwrap", ".unwrap()", "constraint_violation")],
    };
    let report = run(
        &rules,
        &AuditOpts {
            project_root: dir.path().to_path_buf(),
            scan_root: dir.path().to_path_buf(),
            rule_filter: None,
        },
    );
    assert_eq!(report.per_rule.len(), 1);
    assert_eq!(report.per_rule[0].rule_id, "no-unwrap");
    assert_eq!(report.per_rule[0].hits, 1);
    assert_eq!(report.per_rule[0].level, Level::Block);
    assert_eq!(report.per_rule[0].files.len(), 1);
    assert_eq!(report.per_rule[0].files[0].lines, vec![1]);
}

#[test]
fn run_skips_non_audit_rules() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), ".unwrap()").unwrap();
    let mut r = rule("no-unwrap", ".unwrap()", "constraint_violation");
    r.audit = None; // not opted in
    let rules = RulesFile { rules: vec![r] };
    let report = run(
        &rules,
        &AuditOpts {
            project_root: dir.path().to_path_buf(),
            scan_root: dir.path().to_path_buf(),
            rule_filter: None,
        },
    );
    assert!(report.per_rule.is_empty());
}

#[test]
fn run_respects_rule_filter() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), ".unwrap() panic!()").unwrap();
    let rules = RulesFile {
        rules: vec![
            rule("no-unwrap", ".unwrap()", "constraint_violation"),
            rule("no-panic", "panic!(", "constraint_violation"),
        ],
    };
    let report = run(
        &rules,
        &AuditOpts {
            project_root: dir.path().to_path_buf(),
            scan_root: dir.path().to_path_buf(),
            rule_filter: Some("no-panic".to_string()),
        },
    );
    assert_eq!(report.per_rule.len(), 1);
    assert_eq!(report.per_rule[0].rule_id, "no-panic");
}

#[test]
fn rule_filter_matches_or_expansions() {
    assert!(rule_matches_filter(
        "warn-untested-risky-call",
        "warn-untested-risky-call"
    ));
    assert!(rule_matches_filter(
        "warn-untested-risky-call#or0",
        "warn-untested-risky-call"
    ));
    assert!(rule_matches_filter(
        "warn-untested-risky-call#or12",
        "warn-untested-risky-call"
    ));
    assert!(rule_matches_filter(
        "warn-untested-risky-call#or0-or1",
        "warn-untested-risky-call"
    ));
    assert!(!rule_matches_filter(
        "warn-untested-risky-call#or0-",
        "warn-untested-risky-call"
    ));
    assert!(!rule_matches_filter(
        "warn-untested-risky-call#or",
        "warn-untested-risky-call"
    ));
    assert!(!rule_matches_filter(
        "warn-untested-risky-call-v2",
        "warn-untested-risky-call"
    ));
    assert!(!rule_matches_filter(
        "warn-untested",
        "warn-untested-risky-call"
    ));
}

#[test]
fn run_rule_filter_selects_all_or_branches() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), ".unwrap() panic!()").unwrap();
    let rules = RulesFile {
        rules: vec![
            rule("risky#or0", ".unwrap()", "constraint_violation"),
            rule("risky#or1", "panic!(", "constraint_violation"),
            rule("other", "panic!(", "constraint_violation"),
        ],
    };
    let report = run(
        &rules,
        &AuditOpts {
            project_root: dir.path().to_path_buf(),
            scan_root: dir.path().to_path_buf(),
            rule_filter: Some("risky".to_string()),
        },
    );
    let mut ids: Vec<&str> = report.per_rule.iter().map(|r| r.rule_id.as_str()).collect();
    ids.sort();
    assert_eq!(ids, vec!["risky#or0", "risky#or1"]);
}

#[test]
fn empty_diagnostic_names_unmatched_rule_filter() {
    let rules = RulesFile {
        rules: vec![
            rule("warn-untested-risky-call#or0", "x", "warning"),
            rule("warn-untested-risky-call#or1", "x", "warning"),
            rule("no-unwrap", "x", "warning"),
        ],
    };
    let report = AuditReport {
        generated_at: 0,
        scan_duration_ms: 0,
        files_scanned: 0,
        per_rule: vec![],
    };
    let diag = empty_result_diagnostic(
        &report,
        &rules,
        Some("warn-untested-risky"),
        Path::new("/src"),
    )
    .expect("diagnostic");
    assert!(
        diag.contains("no opted-in rule matches `warn-untested-risky`"),
        "{diag}"
    );
    assert!(
        diag.contains("Did you mean: warn-untested-risky-call?"),
        "{diag}"
    );
    assert!(!diag.contains("walked 0 files"), "{diag}");

    // A matching filter (via #orN) with zero files scanned still reports
    // the walker diagnostic, not the unmatched-rule one.
    let diag = empty_result_diagnostic(
        &report,
        &rules,
        Some("warn-untested-risky-call"),
        Path::new("/src"),
    )
    .expect("diagnostic");
    assert!(diag.contains("walked 0 files"), "{diag}");
}

#[test]
fn file_line_count_above_counts_production_lines_only_for_rust() {
    // A Rust file with 100 lines, half of which sit inside a
    // #[cfg(test)] mod tests block. With a threshold of 75, the rule
    // should NOT fire — production line count is 50.
    let dir = tempfile::tempdir().unwrap();
    let prod_lines = "let _ = 1;\n".repeat(50);
    let test_block = format!(
        "#[cfg(test)]\nmod tests {{\n{}\n}}\n",
        "    let _ = 1;\n".repeat(50)
    );
    std::fs::write(
        dir.path().join("a.rs"),
        format!("{}{}", prod_lines, test_block),
    )
    .unwrap();

    let r = DiskRule {
        id: "audit-file-loc-high".to_string(),
        phase: "audit".to_string(),
        priority: 3,
        conditions: vec![
            DiskCondition {
                predicate: "file_extension_is".to_string(),
                args: vec!["rs".to_string()],
                script: None,
            },
            DiskCondition {
                predicate: "file_line_count_above".to_string(),
                args: vec!["75".to_string()],
                script: None,
            },
        ],
        actions: vec![DiskAction {
            action_type: "constraint_warning".to_string(),
            params: vec!["too big".to_string()],
            ..Default::default()
        }],
        silent: None,
        audit: Some(true),
        doc_excepted: None,
    };
    let rules = RulesFile { rules: vec![r] };
    let report = run(
        &rules,
        &AuditOpts {
            project_root: dir.path().to_path_buf(),
            scan_root: dir.path().to_path_buf(),
            rule_filter: None,
        },
    );
    assert!(
        report.per_rule.is_empty(),
        "rule should not fire when prod LOC is under threshold even if total exceeds it"
    );
}

#[test]
fn run_does_not_strip_test_blocks_for_non_rust_files() {
    // The mask is only applied to .rs files. For other extensions the
    // content goes through as-is (the test-block convention is
    // Rust-specific).
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.py"), "#[cfg(test)]\n.clone()").unwrap();
    let rules = RulesFile {
        rules: vec![rule("warn-clone", ".clone()", "constraint_warning")],
    };
    let report = run(
        &rules,
        &AuditOpts {
            project_root: dir.path().to_path_buf(),
            scan_root: dir.path().to_path_buf(),
            rule_filter: None,
        },
    );
    assert_eq!(report.per_rule[0].hits, 1);
}

#[test]
fn run_distinguishes_warn_from_block() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), ".clone() .clone()").unwrap();
    let rules = RulesFile {
        rules: vec![rule("warn-clone", ".clone()", "constraint_warning")],
    };
    let report = run(
        &rules,
        &AuditOpts {
            project_root: dir.path().to_path_buf(),
            scan_root: dir.path().to_path_buf(),
            rule_filter: None,
        },
    );
    assert_eq!(report.per_rule[0].level, Level::Warn);
    assert_eq!(report.per_rule[0].hits, 2);
}

#[test]
fn run_honors_file_path_matches_gate() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::create_dir_all(dir.path().join("target")).unwrap();
    std::fs::write(dir.path().join("src").join("a.rs"), ".unwrap()").unwrap();
    std::fs::write(dir.path().join("target").join("b.rs"), ".unwrap()").unwrap();

    let r = DiskRule {
        id: "no-unwrap-src".to_string(),
        phase: "pre".to_string(),
        priority: 10,
        conditions: vec![
            DiskCondition {
                predicate: "new_content_contains".to_string(),
                args: vec![".unwrap()".to_string()],
                script: None,
            },
            DiskCondition {
                predicate: "file_path_matches".to_string(),
                args: vec!["src".to_string()],
                script: None,
            },
        ],
        actions: vec![DiskAction {
            action_type: "constraint_violation".to_string(),
            params: vec!["no unwrap".to_string()],
            ..Default::default()
        }],
        silent: None,
        audit: Some(true),
        doc_excepted: None,
    };
    let rules = RulesFile { rules: vec![r] };
    let report = run(
        &rules,
        &AuditOpts {
            project_root: dir.path().to_path_buf(),
            scan_root: dir.path().to_path_buf(),
            rule_filter: None,
        },
    );
    assert_eq!(report.per_rule.len(), 1);
    assert_eq!(report.per_rule[0].hits, 1);
    // Only the src/ file should fire; target/ is filtered out by the gate.
    let path = &report.per_rule[0].files[0].path;
    assert!(path.to_string_lossy().contains("src"), "got {:?}", path);
    assert!(!path.to_string_lossy().contains("target"));
}

#[test]
fn run_honors_file_extension_is_gate() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "print(\"hi\")").unwrap();
    std::fs::write(dir.path().join("b.rhai"), "print(\"hi\")").unwrap();

    let r = DiskRule {
        id: "no-rhai-print".to_string(),
        phase: "pre".to_string(),
        priority: 10,
        conditions: vec![
            DiskCondition {
                predicate: "new_content_contains".to_string(),
                args: vec!["print(".to_string()],
                script: None,
            },
            DiskCondition {
                predicate: "file_extension_is".to_string(),
                args: vec!["rhai".to_string()],
                script: None,
            },
        ],
        actions: vec![DiskAction {
            action_type: "constraint_violation".to_string(),
            params: vec!["no print in rhai".to_string()],
            ..Default::default()
        }],
        silent: None,
        audit: Some(true),
        doc_excepted: None,
    };
    let rules = RulesFile { rules: vec![r] };
    let report = run(
        &rules,
        &AuditOpts {
            project_root: dir.path().to_path_buf(),
            scan_root: dir.path().to_path_buf(),
            rule_filter: None,
        },
    );
    assert_eq!(report.per_rule.len(), 1);
    assert_eq!(report.per_rule[0].hits, 1);
    let path = &report.per_rule[0].files[0].path;
    assert!(path.to_string_lossy().ends_with("b.rhai"), "got {:?}", path);
}

#[test]
fn run_skips_rule_with_unsupported_predicate() {
    // A rule mixing new_content_contains with a predicate that audit
    // can't evaluate (here `function_added`, which is a diff-context
    // predicate without a corresponding fact in SyntaxFacts::all_facts)
    // must be skipped entirely — not fire on content match alone, which
    // would be unsafe.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), ".unwrap()").unwrap();
    let r = DiskRule {
        id: "mixed".to_string(),
        phase: "pre".to_string(),
        priority: 10,
        conditions: vec![
            DiskCondition {
                predicate: "new_content_contains".to_string(),
                args: vec![".unwrap()".to_string()],
                script: None,
            },
            DiskCondition {
                predicate: "function_added".to_string(),
                args: vec!["?file".to_string(), "?fn".to_string()],
                script: None,
            },
        ],
        actions: vec![DiskAction {
            action_type: "constraint_violation".to_string(),
            params: vec!["nope".to_string()],
            ..Default::default()
        }],
        silent: None,
        audit: Some(true),
        doc_excepted: None,
    };
    let rules = RulesFile { rules: vec![r] };
    let report = run(
        &rules,
        &AuditOpts {
            project_root: dir.path().to_path_buf(),
            scan_root: dir.path().to_path_buf(),
            rule_filter: None,
        },
    );
    assert!(
        report.per_rule.is_empty(),
        "rule with unsupported predicate must be skipped: {:?}",
        report.per_rule
    );
}

#[test]
fn rank_recognizes_constraint_warning_action_type() {
    // The hook + init.rs use `constraint_warning` (not `warn_violation`)
    // for warning actions. Make sure audit maps it correctly.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "dbg!(x)").unwrap();
    let r = DiskRule {
        id: "warn-dbg".to_string(),
        phase: "pre".to_string(),
        priority: 5,
        conditions: vec![DiskCondition {
            predicate: "new_content_contains".to_string(),
            args: vec!["dbg!(".to_string()],
            script: None,
        }],
        actions: vec![DiskAction {
            action_type: "constraint_warning".to_string(),
            params: vec!["no dbg".to_string()],
            ..Default::default()
        }],
        silent: None,
        audit: Some(true),
        doc_excepted: None,
    };
    let rules = RulesFile { rules: vec![r] };
    let report = run(
        &rules,
        &AuditOpts {
            project_root: dir.path().to_path_buf(),
            scan_root: dir.path().to_path_buf(),
            rule_filter: None,
        },
    );
    assert_eq!(report.per_rule.len(), 1);
    assert_eq!(report.per_rule[0].level, Level::Warn);
}

#[test]
fn run_records_multiple_line_numbers_per_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("a.rs"),
        "line one\n.unwrap()\nline three\n.unwrap()\n",
    )
    .unwrap();
    let rules = RulesFile {
        rules: vec![rule("no-unwrap", ".unwrap()", "constraint_violation")],
    };
    let report = run(
        &rules,
        &AuditOpts {
            project_root: dir.path().to_path_buf(),
            scan_root: dir.path().to_path_buf(),
            rule_filter: None,
        },
    );
    assert_eq!(report.per_rule[0].hits, 2);
    assert_eq!(report.per_rule[0].files[0].lines, vec![2, 4]);
}

#[test]
fn run_returns_empty_when_no_audit_rules_in_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "fn clean(){}").unwrap();
    let rules = RulesFile {
        rules: vec![rule("no-unwrap", ".unwrap()", "constraint_violation")],
    };
    let report = run(
        &rules,
        &AuditOpts {
            project_root: dir.path().to_path_buf(),
            scan_root: dir.path().to_path_buf(),
            rule_filter: None,
        },
    );
    assert!(report.per_rule.is_empty());
    assert!(report.files_scanned >= 1);
}

#[test]
fn run_groups_hits_across_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), ".unwrap()").unwrap();
    std::fs::write(dir.path().join("b.rs"), ".unwrap()\n.unwrap()").unwrap();
    let rules = RulesFile {
        rules: vec![rule("no-unwrap", ".unwrap()", "constraint_violation")],
    };
    let report = run(
        &rules,
        &AuditOpts {
            project_root: dir.path().to_path_buf(),
            scan_root: dir.path().to_path_buf(),
            rule_filter: None,
        },
    );
    assert_eq!(report.per_rule[0].hits, 3);
    assert_eq!(report.per_rule[0].files.len(), 2);
}

#[test]
fn discover_files_returns_only_matching_extensions() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "fn x(){}").unwrap();
    std::fs::write(dir.path().join("b.md"), "# hi").unwrap();
    std::fs::write(dir.path().join("c.rs"), "fn y(){}").unwrap();
    let mut got = discover_files(dir.path(), &["rs"]);
    got.sort();
    let names: Vec<_> = got
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    assert_eq!(names, vec!["a.rs", "c.rs"]);
}

#[test]
fn discover_files_respects_gitignore() {
    let dir = tempfile::tempdir().unwrap();
    // .gitignore needs the dir to look like a real repo for `ignore` to honor it
    std::fs::create_dir_all(dir.path().join(".git")).unwrap();
    std::fs::write(dir.path().join(".gitignore"), "ignored.rs\n").unwrap();
    std::fs::write(dir.path().join("keep.rs"), "fn x(){}").unwrap();
    std::fs::write(dir.path().join("ignored.rs"), "fn y(){}").unwrap();
    let got = discover_files(dir.path(), &["rs"]);
    let names: Vec<_> = got
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    assert_eq!(names, vec!["keep.rs"]);
}

#[test]
fn discover_files_wildcard_returns_all_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "x").unwrap();
    std::fs::write(dir.path().join("b.md"), "x").unwrap();
    let got = discover_files(dir.path(), &["*"]);
    assert_eq!(got.len(), 2);
}

fn empty_report() -> AuditReport {
    AuditReport {
        generated_at: 0,
        scan_duration_ms: 0,
        files_scanned: 0,
        per_rule: Vec::new(),
    }
}

fn report_with_hits() -> AuditReport {
    AuditReport {
        generated_at: 0,
        scan_duration_ms: 0,
        files_scanned: 5,
        per_rule: vec![RuleAudit {
            rule_id: "r".into(),
            level: Level::Warn,
            hits: 1,
            files: vec![FileAudit {
                path: PathBuf::from("a.rs"),
                lines: vec![1],
                details: vec![],
            }],
        }],
    }
}

#[test]
fn empty_diagnostic_returns_none_when_report_has_hits() {
    let r = report_with_hits();
    assert!(empty_result_diagnostic(&r, &opted_in(5), None, Path::new("/proj")).is_none());
}

/// A rules file with `n` opted-in rules.
fn opted_in(n: usize) -> RulesFile {
    RulesFile {
        rules: (0..n)
            .map(|i| rule(&format!("r{i}"), "x", "warning"))
            .collect(),
    }
}

#[test]
fn empty_diagnostic_flags_zero_audit_tagged_rules() {
    let r = empty_report();
    let msg = empty_result_diagnostic(&r, &opted_in(0), None, Path::new("/proj"))
        .expect("zero audit-tagged rules must produce a diagnostic");
    assert!(
        msg.contains("audit: true"),
        "message must name the recoverable cause; got: {msg}"
    );
}

#[test]
fn empty_diagnostic_flags_walker_zero_files_when_rules_opted_in() {
    let r = empty_report();
    let msg = empty_result_diagnostic(&r, &opted_in(5), None, Path::new("/proj/src"))
        .expect("opted-in rules but zero scanned files must produce a diagnostic");
    assert!(
        msg.contains("0 files") && msg.contains("/proj/src"),
        "message must call out walker scope and scan_root; got: {msg}"
    );
    assert!(
        msg.contains(".gitignore") || msg.contains(".phronesisignore"),
        "message should point at ignore files as a likely cause; got: {msg}"
    );
}

#[test]
fn empty_diagnostic_silent_for_legitimately_clean_audit() {
    // Rules opted in, files walked, simply no violations — the honest
    // zero. No diagnostic; the normal renderer's "no violations" line
    // covers it.
    let r = AuditReport {
        files_scanned: 50,
        ..empty_report()
    };
    assert!(empty_result_diagnostic(&r, &opted_in(3), None, Path::new("/proj")).is_none());
}

#[test]
fn run_profiled_matches_run_and_populates_section_times() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("a.rs"),
        "fn main() { let x = foo.unwrap(); }",
    )
    .unwrap();
    let rules = RulesFile {
        rules: vec![rule("no-unwrap", ".unwrap()", "constraint_violation")],
    };
    let opts = AuditOpts {
        project_root: dir.path().to_path_buf(),
        scan_root: dir.path().to_path_buf(),
        rule_filter: None,
    };
    let report = run(&rules, &opts);
    let (profiled, times) = run_profiled(&rules, &opts);

    // Structural equality between the two paths.
    assert_eq!(report.files_scanned, profiled.files_scanned);
    assert_eq!(report.per_rule.len(), profiled.per_rule.len());
    for (r, p) in report.per_rule.iter().zip(profiled.per_rule.iter()) {
        assert_eq!(r.rule_id, p.rule_id);
        assert_eq!(r.hits, p.hits);
        assert_eq!(r.files.len(), p.files.len());
        // FileAudit does not derive PartialEq; compare fields individually.
        for (a, b) in r.files.iter().zip(p.files.iter()) {
            assert_eq!(a.path, b.path);
            assert_eq!(a.lines, b.lines);
            assert_eq!(a.details, b.details);
        }
    }

    // Timing fields populated.
    assert_eq!(times.files_scanned, profiled.files_scanned);
    assert!(times.audit_rules >= 1, "audit_rules must be counted");
    assert!(
        times.total >= times.match_loop,
        "total {:?} must be >= match_loop {:?}",
        times.total,
        times.match_loop
    );
}

#[test]
fn discovery_reports_only_phronesisignore_exclusions_at_any_level() {
    use crate::audit::run::discover_files_with_excluded;
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir_all(root.join("src/deep")).expect("mkdir");
    std::fs::write(root.join("src/kept.rs"), "fn a() {}\n").expect("write");
    std::fs::write(root.join("src/dropped.rs"), "fn b() {}\n").expect("write");
    std::fs::write(root.join("src/deep/nested.rs"), "fn c() {}\n").expect("write");
    // Excluded by .gitignore AND .phronesisignore: not a policy exclusion.
    std::fs::write(root.join("src/generated.rs"), "fn g() {}\n").expect("write");
    // Hidden file: the walker skips it by default; never reported either.
    std::fs::write(root.join("src/.hidden.rs"), "fn h() {}\n").expect("write");
    std::fs::write(root.join(".gitignore"), "src/generated.rs\n").expect("write");
    std::fs::write(
        root.join(".phronesisignore"),
        "src/dropped.rs\nsrc/generated.rs\n",
    )
    .expect("write");
    std::fs::write(root.join("src/deep/.phronesisignore"), "nested.rs\n").expect("write");

    let d = discover_files_with_excluded(root, &["rs"]);
    let names = |v: &[std::path::PathBuf]| -> Vec<String> {
        v.iter()
            .map(|p| {
                p.strip_prefix(root)
                    .expect("under root")
                    .to_string_lossy()
                    .to_string()
            })
            .collect()
    };
    assert_eq!(names(&d.scanned), vec!["src/kept.rs"]);
    assert_eq!(
        names(&d.excluded),
        vec!["src/deep/nested.rs", "src/dropped.rs"]
    );
}

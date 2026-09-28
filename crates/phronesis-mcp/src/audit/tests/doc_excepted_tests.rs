//! Tests for the audit engine — doc excepted tests. Split from the original `audit.rs`
//! `mod tests` / `engine_tests.rs`.

use super::engine_tests::rule;
use crate::audit::*;
use crate::rules_file::{DiskAction, DiskCondition, DiskRule, RulesFile};

    #[test]
    fn doc_excepted_rule_skips_matches_preceded_by_doc_comment() {
        let dir = tempfile::tempdir().unwrap();
        // Two `#[allow(dead_code)]` attributes: the first carries a
        // `///` doc-comment justification immediately above (should be
        // exempt); the second does not (should be flagged).
        std::fs::write(
            dir.path().join("a.rs"),
            "/// Documented exception: planned API surface.\n\
             #[allow(dead_code)]\n\
             struct A;\n\
             \n\
             #[allow(dead_code)]\n\
             struct B;\n",
        )
        .unwrap();
        let mut r = rule(
            "audit-allow-dead-code-in-src",
            "#[allow(dead_code)]",
            "constraint_warning",
        );
        r.doc_excepted = Some(true);
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
        // Only one hit: the undocumented `#[allow(dead_code)]` on line 5.
        assert_eq!(report.per_rule[0].hits, 1);
        assert_eq!(report.per_rule[0].files[0].lines, vec![5]);
    }

    #[test]
    fn doc_excepted_rule_skips_past_stacked_attributes() {
        let dir = tempfile::tempdir().unwrap();
        // A `///` doc-comment block, followed by another attribute
        // (`#[serde(default)]`), then the `#[allow(dead_code)]`. The
        // exception walker should skip past the intermediate attribute
        // to find the doc-comment.
        std::fs::write(
            dir.path().join("a.rs"),
            "/// Documented exception.\n\
             #[serde(default)]\n\
             #[allow(dead_code)]\n\
             struct A;\n",
        )
        .unwrap();
        let mut r = rule(
            "audit-allow-dead-code-in-src",
            "#[allow(dead_code)]",
            "constraint_warning",
        );
        r.doc_excepted = Some(true);
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
            "doc-comment above stacked attributes should still exempt"
        );
    }

    #[test]
    fn doc_excepted_rule_with_blank_line_between_still_exempts() {
        let dir = tempfile::tempdir().unwrap();
        // A `///` doc-comment with one blank line between it and the
        // `#[allow(...)]` should still count as documentation — the
        // helper walks past blanks to find the nearest non-blank line.
        std::fs::write(
            dir.path().join("a.rs"),
            "/// Documented.\n\
             \n\
             #[allow(dead_code)]\n\
             struct A;\n",
        )
        .unwrap();
        let mut r = rule(
            "audit-allow-dead-code-in-src",
            "#[allow(dead_code)]",
            "constraint_warning",
        );
        r.doc_excepted = Some(true);
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
            "documented exception should be skipped even with blank line"
        );
    }

    #[test]
    fn doc_excepted_rule_does_not_exempt_when_flag_false() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.rs"),
            "/// Documented.\n#[allow(dead_code)]\nstruct A;\n",
        )
        .unwrap();
        // doc_excepted defaults to None/false → rule fires anyway.
        let r = rule(
            "audit-allow-dead-code-in-src",
            "#[allow(dead_code)]",
            "constraint_warning",
        );
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
    }

    #[test]
    fn run_skips_lines_inside_test_blocks_for_rust_files() {
        let dir = tempfile::tempdir().unwrap();
        // Two hits: one in production (line 1) and one inside a #[cfg(test)]
        // module. Only the production hit should be reported.
        let content =
            "fn prod() { x.clone(); }\n\n#[cfg(test)]\nmod tests {\n    fn t() { y.clone(); }\n}\n";
        std::fs::write(dir.path().join("a.rs"), content).unwrap();
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
        assert_eq!(report.per_rule.len(), 1, "rule must fire on production hit");
        assert_eq!(report.per_rule[0].hits, 1, "test-block hit must be skipped");
        assert_eq!(
            report.per_rule[0].files[0].lines,
            vec![1],
            "reported line should be the production line, not shifted by stripping"
        );
    }

    #[test]
    fn whole_file_rule_fires_once_per_matching_file_via_line_count_gate() {
        // file_line_count_above is a gate; with no content predicate the rule
        // is "whole-file" and should emit exactly one hit per matching file
        // at line 1.
        let dir = tempfile::tempdir().unwrap();
        let big = "x\n".repeat(100);
        let small = "x\n".repeat(5);
        std::fs::write(dir.path().join("big.rs"), &big).unwrap();
        std::fs::write(dir.path().join("small.rs"), &small).unwrap();

        let r = DiskRule {
            id: "audit-too-big".to_string(),
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
                    args: vec!["50".to_string()],
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
        assert_eq!(report.per_rule.len(), 1);
        assert_eq!(report.per_rule[0].hits, 1, "only big.rs should fire");
        assert_eq!(report.per_rule[0].files.len(), 1);
        assert!(
            report.per_rule[0].files[0]
                .path
                .to_string_lossy()
                .ends_with("big.rs")
        );
        assert_eq!(report.per_rule[0].files[0].lines, vec![1]);
    }

    #[test]
    fn doc_excepted_whole_file_rule_skips_files_with_exemption_marker() {
        // A Rust file whose top-of-file `//!` doc-comment block carries a
        // `phronesis-allow: <rule-id>` marker should be exempt from the
        // named whole-file (gate-only) rule when the rule opts in.
        let dir = tempfile::tempdir().unwrap();
        let exempt_content = format!(
            "//! Module doc.\n//!\n//! phronesis-allow: audit-too-big (intentional god-file)\n\nfn x() {{}}\n{}",
            "let _ = 1;\n".repeat(100)
        );
        let plain_content = "let _ = 1;\n".repeat(102);
        std::fs::write(dir.path().join("exempt.rs"), &exempt_content).unwrap();
        std::fs::write(dir.path().join("plain.rs"), &plain_content).unwrap();

        let r = DiskRule {
            id: "audit-too-big".to_string(),
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
                    args: vec!["50".to_string()],
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
            doc_excepted: Some(true),
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
        // Only plain.rs should fire; exempt.rs carries the marker.
        assert_eq!(report.per_rule.len(), 1);
        assert_eq!(report.per_rule[0].hits, 1);
        assert!(
            report.per_rule[0].files[0]
                .path
                .to_string_lossy()
                .ends_with("plain.rs")
        );
    }

    #[test]
    fn doc_excepted_whole_file_marker_must_match_rule_id() {
        // An exemption marker naming a DIFFERENT rule must not exempt
        // the file from this rule.
        let dir = tempfile::tempdir().unwrap();
        let content = format!(
            "//! phronesis-allow: some-other-rule\n\n{}",
            "let _ = 1;\n".repeat(100)
        );
        std::fs::write(dir.path().join("a.rs"), &content).unwrap();
        let r = DiskRule {
            id: "audit-too-big".to_string(),
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
                    args: vec!["50".to_string()],
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
            doc_excepted: Some(true),
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
        assert_eq!(
            report.per_rule.len(),
            1,
            "marker for a different rule must not exempt this one"
        );
    }


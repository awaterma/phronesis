//! Tests for the audit engine — ast tests. Split from the original `audit.rs`
//! `mod tests` / `engine_tests.rs`.

use super::engine_tests::rule;
use crate::audit::*;
use crate::rules_file::{DiskAction, DiskCondition, DiskRule, RulesFile};

    // ── AST predicate evaluation in audit (Phase 3.5) ─────────────────────────

    fn ast_let_binding_rule() -> DiskRule {
        DiskRule {
            id: "audit-rust-let-binding-count-high".to_string(),
            phase: "audit".to_string(),
            priority: 3,
            conditions: vec![DiskCondition {
                predicate: "function_let_binding_count_high".to_string(),
                args: vec!["?file".to_string(), "?fn".to_string(), "?count".to_string()],
                script: None,
            }],
            actions: vec![DiskAction {
                action_type: "constraint_warning".to_string(),
                params: vec!["`?fn` in ?file has ?count outer-scope `let` bindings.".to_string()],
                ..Default::default()
            }],
            silent: None,
            audit: Some(true),
            doc_excepted: None,
        }
    }

    /// Mirror of `ast_let_binding_rule` for the let-mut predicate.
    /// Lets the audit tests exercise the let_mut variant of the
    /// block-pattern rule pair end-to-end through the AST branch.
    fn ast_let_mut_rule() -> DiskRule {
        DiskRule {
            id: "audit-rust-let-mut-count-high".to_string(),
            phase: "audit".to_string(),
            priority: 3,
            conditions: vec![DiskCondition {
                predicate: "function_let_mut_count_high".to_string(),
                args: vec!["?file".to_string(), "?fn".to_string(), "?count".to_string()],
                script: None,
            }],
            actions: vec![DiskAction {
                action_type: "constraint_warning".to_string(),
                params: vec![
                    "`?fn` in ?file has ?count outer-scope `let mut` declarations.".to_string(),
                ],
                ..Default::default()
            }],
            silent: None,
            audit: Some(true),
            doc_excepted: None,
        }
    }

    #[test]
    fn run_evaluates_python_and_typescript_ast_predicates() {
        // End-to-end: the .py/.ts extension dispatch reaches the new
        // tree-sitter extractors and their predicates audit like the
        // Rust ones do.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("svc.py"),
            "def fetch(url=load_default()):\n    \"\"\"F.\"\"\"\n    print(url)\n    try:\n        go(url)\n    except:\n        pass\n    try:\n        go(url)\n    except ValueError:\n        pass\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("svc.ts"),
            "function load(a: any): any { return a; }\n",
        )
        .unwrap();
        let mk = |id: &str, predicate: &str, args: Vec<&str>| DiskRule {
            id: id.to_string(),
            phase: "audit".to_string(),
            priority: 3,
            conditions: vec![DiskCondition {
                predicate: predicate.to_string(),
                args: args.into_iter().map(String::from).collect(),
                script: None,
            }],
            actions: vec![DiskAction {
                action_type: "constraint_warning".to_string(),
                params: vec![format!("{} hit in ?file", predicate)],
                ..Default::default()
            }],
            silent: None,
            audit: Some(true),
            doc_excepted: None,
        };
        let rules = RulesFile {
            rules: vec![
                mk(
                    "audit-python-bare-except",
                    "python_bare_except",
                    vec!["?file", "?fn"],
                ),
                mk(
                    "audit-python-print-call",
                    "python_print_call",
                    vec!["?file", "?fn"],
                ),
                mk(
                    "audit-python-call-default",
                    "python_call_in_default_arg",
                    vec!["?file", "?fn", "?param", "?callee"],
                ),
                mk(
                    "audit-python-handler-pass",
                    "python_exception_handler_passes",
                    vec!["?file", "?fn", "?exception"],
                ),
                mk(
                    "audit-ts-explicit-any",
                    "ts_explicit_any",
                    vec!["?file", "?fn", "?count"],
                ),
            ],
        };
        let report = run(
            &rules,
            &AuditOpts {
                project_root: dir.path().to_path_buf(),
                scan_root: dir.path().to_path_buf(),
                rule_filter: None,
            },
        );
        let ids: Vec<&str> = report.per_rule.iter().map(|r| r.rule_id.as_str()).collect();
        assert!(
            ids.contains(&"audit-python-bare-except"),
            "python predicate must audit; got {ids:?}"
        );
        assert!(
            ids.contains(&"audit-python-print-call"),
            "python print predicate must audit; got {ids:?}"
        );
        assert!(
            ids.contains(&"audit-python-call-default"),
            "python default-call predicate must audit; got {ids:?}"
        );
        assert!(
            ids.contains(&"audit-python-handler-pass"),
            "python handler predicate must audit; got {ids:?}"
        );
        assert!(
            ids.contains(&"audit-ts-explicit-any"),
            "typescript predicate must audit; got {ids:?}"
        );
    }

    #[test]
    fn run_evaluates_ast_predicate_function_let_binding_count_high() {
        // Positive case: a function with 8+ outer-scope `let` bindings should
        // produce one audit hit on the let-binding rule.
        let dir = tempfile::tempdir().unwrap();
        let src = "\
fn ladder() {
    let a = 1;
    let b = 2;
    let c = 3;
    let d = 4;
    let e = 5;
    let f = 6;
    let g = 7;
    let h = 8;
    let _ = (a, b, c, d, e, f, g, h);
}
";
        std::fs::write(dir.path().join("a.rs"), src).unwrap();
        let rules = RulesFile {
            rules: vec![ast_let_binding_rule()],
        };
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
            "expected one rule with hits, got {:?}",
            report.per_rule,
        );
        assert_eq!(
            report.per_rule[0].rule_id,
            "audit-rust-let-binding-count-high"
        );
        assert_eq!(report.per_rule[0].hits, 1);
        assert_eq!(report.per_rule[0].level, Level::Warn);
        assert_eq!(report.per_rule[0].files.len(), 1);
        assert!(
            report.per_rule[0].files[0]
                .path
                .to_string_lossy()
                .ends_with("a.rs")
        );
    }

    #[test]
    fn run_ast_predicate_hit_carries_function_name_and_count() {
        // The audit must name the offending function and its count, not
        // just emit a placeholder line `1` per hit. `ladder` has 9
        // outer-scope `let` bindings (a..h plus the trailing `let _ =`),
        // which clears the 8-binding threshold; the per-hit detail should
        // read "ladder (9 let bindings)" — the *actual* count, not the
        // threshold.
        let dir = tempfile::tempdir().unwrap();
        let src = "\
fn ladder() {
    let a = 1;
    let b = 2;
    let c = 3;
    let d = 4;
    let e = 5;
    let f = 6;
    let g = 7;
    let h = 8;
    let _ = (a, b, c, d, e, f, g, h);
}
";
        std::fs::write(dir.path().join("a.rs"), src).unwrap();
        let rules = RulesFile {
            rules: vec![ast_let_binding_rule()],
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
        let details = &report.per_rule[0].files[0].details;
        assert!(
            details.iter().any(|d| d == "ladder (9 let bindings)"),
            "expected named detail 'ladder (9 let bindings)', got {details:?}"
        );
    }

    #[test]
    fn run_silences_ast_predicate_on_block_pattern_adopter() {
        // Silence case (LOAD-BEARING): a function using the block pattern —
        // `let x = { let a; let b; ...; tmp }` — has 8+ total `let`s but only
        // one OUTER-scope let. The extractor halts at the child block, so
        // SyntaxFacts contains no fact for this function, and the audit must
        // report zero hits. This is the spec's core property; the entire
        // reason the feature exists.
        let dir = tempfile::tempdir().unwrap();
        let src = "\
fn block_adopter() {
    let result = {
        let a = 1;
        let b = 2;
        let c = 3;
        let d = 4;
        let e = 5;
        let f = 6;
        let g = 7;
        let h = 8;
        (a, b, c, d, e, f, g, h)
    };
    let _ = result;
}
";
        std::fs::write(dir.path().join("a.rs"), src).unwrap();
        let rules = RulesFile {
            rules: vec![ast_let_binding_rule()],
        };
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
            "block-pattern adopter must NOT fire let-binding rule (spec property), got {:?}",
            report.per_rule,
        );
    }

    #[test]
    fn run_ast_predicate_does_not_fire_on_non_rust_files() {
        // SyntaxFacts::extract only parses Rust and Swift; other extensions
        // get a default (empty) SyntaxFacts. A `.py` file with many `let`-ish
        // lines must not produce hits from a Rust AST predicate.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.py"), "let a = 1\nlet b = 2\nlet c = 3\n").unwrap();
        let rules = RulesFile {
            rules: vec![ast_let_binding_rule()],
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
    }

    #[test]
    fn run_ast_and_content_rules_coexist_on_same_file() {
        // Regression: adding AST evaluation must not break content predicates.
        // A file with both a heavy ladder() function AND a `.unwrap()` should
        // fire both rules independently.
        let dir = tempfile::tempdir().unwrap();
        let src = "\
fn ladder() {
    let a = 1;
    let b = 2;
    let c = 3;
    let d = 4;
    let e = 5;
    let f = 6;
    let g = 7;
    let h = 8;
    let _ = foo.unwrap();
}
";
        std::fs::write(dir.path().join("a.rs"), src).unwrap();
        let rules = RulesFile {
            rules: vec![
                ast_let_binding_rule(),
                rule("no-unwrap", ".unwrap()", "constraint_violation"),
            ],
        };
        let report = run(
            &rules,
            &AuditOpts {
                project_root: dir.path().to_path_buf(),
                scan_root: dir.path().to_path_buf(),
                rule_filter: None,
            },
        );
        let rule_ids: Vec<&str> = report.per_rule.iter().map(|r| r.rule_id.as_str()).collect();
        assert!(
            rule_ids.contains(&"audit-rust-let-binding-count-high"),
            "let-binding rule should fire: {:?}",
            rule_ids
        );
        assert!(
            rule_ids.contains(&"no-unwrap"),
            "content rule should still fire: {:?}",
            rule_ids
        );
    }

    #[test]
    fn run_evaluates_ast_predicate_function_let_mut_count_high() {
        // Mirrors run_evaluates_ast_predicate_function_let_binding_count_high
        // for the let-mut variant. Closes a previously-untested gap: the audit
        // branch was generic but only the let-binding rule had end-to-end
        // coverage through the AST path. A function with 3+ outer-scope
        // `let mut`s should produce one hit on the let-mut rule.
        let dir = tempfile::tempdir().unwrap();
        let src = "\
fn mut_ladder() {
    let mut a = vec![];
    let mut b = String::new();
    let mut c = 0;
    a.push(1);
    b.push_str(\"x\");
    c += 1;
    let _ = (a, b, c);
}
";
        std::fs::write(dir.path().join("a.rs"), src).unwrap();
        let rules = RulesFile {
            rules: vec![ast_let_mut_rule()],
        };
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
            "expected one rule with hits, got {:?}",
            report.per_rule,
        );
        assert_eq!(report.per_rule[0].rule_id, "audit-rust-let-mut-count-high");
        assert_eq!(report.per_rule[0].hits, 1);
        assert_eq!(report.per_rule[0].level, Level::Warn);
    }

    #[test]
    fn run_silences_ast_let_mut_rule_on_block_pattern_adopter() {
        // Mirror of the binding-rule silence test, but for the let-mut
        // variant. A function that scopes its `let mut`s inside a block
        // expression must NOT fire the let-mut rule — the walker halts at
        // the child block, so SyntaxFacts contains no fact.
        let dir = tempfile::tempdir().unwrap();
        let src = "\
fn mut_adopter() {
    let result = {
        let mut a = vec![];
        let mut b = String::new();
        let mut c = 0;
        a.push(1);
        b.push_str(\"x\");
        c += 1;
        (a, b, c)
    };
    let _ = result;
}
";
        std::fs::write(dir.path().join("a.rs"), src).unwrap();
        let rules = RulesFile {
            rules: vec![ast_let_mut_rule()],
        };
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
            "block-pattern adopter must NOT fire let-mut rule (spec property), got {:?}",
            report.per_rule,
        );
    }

    #[test]
    fn run_mixed_ast_and_content_rule_uses_ast_predicate_only() {
        // Pins the documented behavior in the AST-branch comment: when a
        // single rule's `when` clause combines an AST predicate AND a
        // content predicate, the AST branch handles the rule and the
        // `continue` drops the content predicate. Effectively, AST takes
        // priority and the content predicate is ignored.
        //
        // Two sub-cases prove the property:
        //  (a) AST signal present, content signal absent → rule still fires
        //      (proves the content predicate is not required)
        //  (b) Content signal present, AST signal absent → rule does NOT fire
        //      (proves the rule doesn't fall through to content evaluation)
        //
        // No shipped rule currently mixes the two predicate kinds. This test
        // exists so the behavior can't drift unnoticed if one ever does, and
        // so a future change to AND-semantics (instead of AST-priority)
        // forces an explicit test update.
        let mixed_rule = || DiskRule {
            id: "mixed-rule".to_string(),
            phase: "audit".to_string(),
            priority: 3,
            conditions: vec![
                DiskCondition {
                    predicate: "function_let_binding_count_high".to_string(),
                    args: vec!["?file".to_string(), "?fn".to_string(), "?count".to_string()],
                    script: None,
                },
                DiskCondition {
                    predicate: "new_content_contains".to_string(),
                    args: vec!["MARKER_NEVER_PRESENT_IN_FIXTURE".to_string()],
                    script: None,
                },
            ],
            actions: vec![DiskAction {
                action_type: "constraint_warning".to_string(),
                params: vec!["mixed rule fired".to_string()],
                ..Default::default()
            }],
            silent: None,
            audit: Some(true),
            doc_excepted: None,
        };

        // Sub-case (a): AST signal hits (8 outer-scope lets), content marker
        // is deliberately absent. AST-priority semantics → rule fires.
        let dir_a = tempfile::tempdir().unwrap();
        std::fs::write(
            dir_a.path().join("a.rs"),
            "\
fn ladder() {
    let a = 1; let b = 2; let c = 3; let d = 4;
    let e = 5; let f = 6; let g = 7; let h = 8;
    let _ = (a, b, c, d, e, f, g, h);
}
",
        )
        .unwrap();
        let report_a = run(
            &RulesFile {
                rules: vec![mixed_rule()],
            },
            &AuditOpts {
                project_root: dir_a.path().to_path_buf(),
                scan_root: dir_a.path().to_path_buf(),
                rule_filter: None,
            },
        );
        assert_eq!(
            report_a.per_rule.len(),
            1,
            "AST predicate alone should fire the mixed rule (content predicate is ignored); got {:?}",
            report_a.per_rule,
        );
        assert_eq!(report_a.per_rule[0].rule_id, "mixed-rule");

        // Sub-case (b): content marker present, AST signal absent (short fn).
        // AST-priority semantics → rule does NOT fire.
        let dir_b = tempfile::tempdir().unwrap();
        std::fs::write(
            dir_b.path().join("b.rs"),
            "\
// MARKER_NEVER_PRESENT_IN_FIXTURE
fn short() {
    let _ = 1;
}
",
        )
        .unwrap();
        let report_b = run(
            &RulesFile {
                rules: vec![mixed_rule()],
            },
            &AuditOpts {
                project_root: dir_b.path().to_path_buf(),
                scan_root: dir_b.path().to_path_buf(),
                rule_filter: None,
            },
        );
        assert!(
            report_b.per_rule.is_empty(),
            "content predicate alone must NOT fire a mixed rule (AST takes priority); got {:?}",
            report_b.per_rule,
        );
    }


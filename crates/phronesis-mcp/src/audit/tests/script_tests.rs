//! Tests for the audit engine — script tests. Split from the original `audit.rs`
//! `mod tests` / `engine_tests.rs`.

use super::super::engine::audit_path_facts;
use super::engine_tests::rule;
use crate::audit::*;
use crate::rules_file::{DiskAction, DiskCondition, DiskRule, RulesFile};
use std::collections::HashMap;

fn add_script(rule: &mut DiskRule, script: &str) {
    rule.conditions.push(DiskCondition {
        predicate: "__script__".to_string(),
        args: Vec::new(),
        script: Some(script.to_string()),
    });
}

#[test]
fn run_scopes_builtin_script_path_exclusion_to_each_file() {
    let dir = tempfile::tempdir().unwrap();
    let commands = dir.path().join("commands");
    let integration = dir.path().join("integration");
    std::fs::create_dir_all(&commands).unwrap();
    std::fs::create_dir_all(&integration).unwrap();
    std::fs::write(commands.join("example.py"), "print('match')\n").unwrap();
    std::fs::write(integration.join("example.py"), "print('match')\n").unwrap();

    let mut r = rule("no-print", "print(", "constraint_warning");
    add_script(
        &mut r,
        "!facts_contain('file_path_matches', ['integration'])",
    );
    let report = run(
        &RulesFile { rules: vec![r] },
        &AuditOpts {
            project_root: dir.path().to_path_buf(),
            scan_root: dir.path().to_path_buf(),
            rule_filter: None,
        },
    );

    assert_eq!(report.per_rule.len(), 1);
    assert_eq!(report.per_rule[0].hits, 1);
    let path = report.per_rule[0].files[0].path.to_string_lossy();
    assert!(path.ends_with("commands/example.py"), "got {path}");
}

#[test]
fn run_scopes_ast_rule_with_builtin_script_path_exclusion() {
    let dir = tempfile::tempdir().unwrap();
    let commands = dir.path().join("commands");
    let integration = dir.path().join("integration");
    std::fs::create_dir_all(&commands).unwrap();
    std::fs::create_dir_all(&integration).unwrap();
    let source = "def undocumented():\n    pass\n";
    std::fs::write(commands.join("example.py"), source).unwrap();
    std::fs::write(integration.join("example.py"), source).unwrap();

    let r = DiskRule {
        id: "missing-docstring".to_string(),
        phase: "audit".to_string(),
        priority: 3,
        conditions: vec![
            DiskCondition {
                predicate: "python_function_missing_docstring".to_string(),
                args: vec!["?file".to_string(), "?fn".to_string()],
                script: None,
            },
            DiskCondition {
                predicate: "__script__".to_string(),
                args: Vec::new(),
                script: Some("!facts_contain('file_path_matches', ['integration'])".to_string()),
            },
        ],
        actions: vec![DiskAction {
            action_type: "constraint_warning".to_string(),
            params: vec!["missing docstring".to_string()],
            ..Default::default()
        }],
        silent: None,
        audit: Some(true),
        doc_excepted: None,
    };
    let report = run(
        &RulesFile { rules: vec![r] },
        &AuditOpts {
            project_root: dir.path().to_path_buf(),
            scan_root: dir.path().to_path_buf(),
            rule_filter: None,
        },
    );

    assert_eq!(report.per_rule.len(), 1);
    assert_eq!(report.per_rule[0].hits, 1);
    assert!(
        report.per_rule[0].files[0]
            .path
            .to_string_lossy()
            .ends_with("commands/example.py")
    );
    assert_eq!(report.per_rule[0].files[0].details, vec!["undocumented"]);
}

#[test]
fn builtin_script_path_facts_are_fresh_in_either_evaluation_order() {
    let dir = tempfile::tempdir().unwrap();
    let commands = dir.path().join("commands/example.py");
    let integration = dir.path().join("integration/example.py");
    let script = "!facts_contain('file_path_matches', ['integration'])";
    let evaluator = phr::BuiltinScriptEvaluator::new();
    let bindings = HashMap::new();

    for (first, second) in [(&integration, &commands), (&commands, &integration)] {
        let first_result = evaluator
            .evaluate(script, &audit_path_facts(dir.path(), first), &bindings)
            .unwrap();
        let second_result = evaluator
            .evaluate(script, &audit_path_facts(dir.path(), second), &bindings)
            .unwrap();
        assert_eq!(first_result, first == &commands);
        assert_eq!(second_result, second == &commands);
    }
}

#[test]
fn run_ands_multiple_builtin_script_guards() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("example.py"), "print('match')\n").unwrap();
    std::fs::write(dir.path().join("example.rs"), "print!(\"match\");\n").unwrap();

    let mut r = rule("python-only", "print", "constraint_warning");
    add_script(&mut r, "facts_contain('file_extension_is', ['py'])");
    add_script(&mut r, "facts_count('file_extension_is', ['py']) == 1");
    add_script(
        &mut r,
        "!facts_contain('file_path_matches', ['integration'])",
    );
    let report = run(
        &RulesFile { rules: vec![r] },
        &AuditOpts {
            project_root: dir.path().to_path_buf(),
            scan_root: dir.path().to_path_buf(),
            rule_filter: None,
        },
    );

    assert_eq!(report.per_rule[0].hits, 1);
    assert!(
        report.per_rule[0].files[0]
            .path
            .to_string_lossy()
            .ends_with("example.py")
    );
}

#[test]
fn run_allows_passing_script_only_whole_file_rule() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("example.py"), "pass\n").unwrap();
    let mut r = rule("python-file", "unused", "constraint_warning");
    r.conditions.clear();
    add_script(&mut r, "facts_contain('file_extension_is', ['py'])");

    let report = run(
        &RulesFile { rules: vec![r] },
        &AuditOpts {
            project_root: dir.path().to_path_buf(),
            scan_root: dir.path().to_path_buf(),
            rule_filter: None,
        },
    );

    assert_eq!(report.per_rule[0].hits, 1);
    assert_eq!(report.per_rule[0].files[0].lines, vec![1]);
}

#[test]
fn script_diagnostics_reject_unsupported_scripts_once_per_rule() {
    let mut malformed = rule("malformed", "x", "constraint_warning");
    add_script(&mut malformed, "facts_contain(");
    let mut bound = rule("bound", "x", "constraint_warning");
    add_script(
        &mut bound,
        "facts_contain('file_path_matches', ['?segment'])",
    );
    let mut rhai = rule("rhai", "x", "constraint_warning");
    add_script(&mut rhai, "facts.len > 0");
    let rules = RulesFile {
        rules: vec![malformed, bound, rhai],
    };

    let diagnostics = script_diagnostics(&rules, None);
    assert_eq!(diagnostics.len(), 3, "{diagnostics:?}");
    assert!(diagnostics.iter().any(|d| d.contains("malformed")));
    assert!(diagnostics.iter().any(|d| d.contains("bound")));
    assert!(diagnostics.iter().any(|d| d.contains("rhai")));

    let filtered = script_diagnostics(&rules, Some("bound"));
    assert_eq!(filtered.len(), 1, "{filtered:?}");
    assert!(filtered[0].contains("bound"));
}

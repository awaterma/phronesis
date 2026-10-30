//! Focused characterization for the private source-metadata sidecar:
//! `CompiledPolicy::compile_source` must preserve original source rule
//! metadata (`binds`) and the expansion association across OR unfolding,
//! without widening the public `DiskRule` shape, and without losing
//! duplicate or OR-expansion diagnostics.

use phronesis_governance::policy::{
    CompiledPolicy, DiskAction, PolicyDiagnostic, PolicyError, SourceRule, WhenClause,
};

fn source(id: &str, binds: Option<bool>, when: WhenClause) -> SourceRule {
    SourceRule {
        id: id.to_string(),
        phase: "pre".to_string(),
        priority: 5,
        when: vec![when],
        then: DiskAction {
            action_type: "log".to_string(),
            params: vec!["m".to_string()],
            data: None,
        },
        silent: None,
        audit: None,
        doc_excepted: None,
        binds,
    }
}

fn or_clause(a: &str, b: &str) -> WhenClause {
    WhenClause::Or(vec![
        WhenClause::Leaf(phronesis_governance::policy::DiskCondition {
            predicate: "new_content_contains".to_string(),
            args: vec![a.to_string()],
            script: None,
        }),
        WhenClause::Leaf(phronesis_governance::policy::DiskCondition {
            predicate: "new_content_contains".to_string(),
            args: vec![b.to_string()],
            script: None,
        }),
    ])
}

fn flat_rule(id: &str) -> CompiledPolicy {
    CompiledPolicy::compile(vec![phronesis_governance::policy::DiskRule {
        id: id.to_string(),
        phase: "pre".to_string(),
        priority: 1,
        conditions: vec![phronesis_governance::policy::DiskCondition {
            predicate: "p".to_string(),
            args: vec!["x".to_string()],
            script: None,
        }],
        actions: vec![DiskAction {
            action_type: "log".to_string(),
            params: vec!["m".to_string()],
            data: None,
        }],
        silent: None,
        audit: None,
        doc_excepted: None,
    }])
    .expect("valid flat rule")
}

#[test]
fn binds_none_true_false_survive_or_expansion() {
    for binds in [None, Some(true), Some(false)] {
        let policy =
            CompiledPolicy::compile_source(&[source("r", binds, or_clause("a", "b"))])
                .expect("compiles");
        assert_eq!(policy.rules().len(), 2, "{binds:?}");
        let first = policy.source_origin("r#or0").expect("sidecar entry");
        assert_eq!(first.source_rule_id, "r");
        assert_eq!(first.binds, binds);
        assert_eq!(first.expansion, Some(vec![0]));
        let second = policy.source_origin("r#or1").expect("sidecar entry");
        assert_eq!(second.source_rule_id, "r");
        assert_eq!(second.binds, binds);
        assert_eq!(second.expansion, Some(vec![1]));
    }
}

#[test]
fn unexpanded_rule_keeps_source_id_without_expansion() {
    let leaf = WhenClause::Leaf(phronesis_governance::policy::DiskCondition {
        predicate: "file_path_matches".to_string(),
        args: vec!["src/".to_string()],
        script: None,
    });
    let policy = CompiledPolicy::compile_source(&[source("solo", Some(false), leaf)])
        .expect("compiles");
    assert_eq!(policy.rules().len(), 1);
    let origin = policy.source_origin("solo").expect("sidecar entry");
    assert_eq!(origin.source_rule_id, "solo");
    assert_eq!(origin.binds, Some(false));
    assert_eq!(origin.expansion, None);
}

#[test]
fn flat_compile_route_has_no_source_metadata() {
    let policy = flat_rule("flat");
    assert!(policy.source_origin("flat").is_none());
    assert!(policy.source_origin("missing").is_none());
}

#[test]
fn identical_duplicate_sources_dedupe_and_keep_first_origin() {
    let a = source("dup", Some(false), or_clause("a", "b"));
    // Same flat content, different source-only metadata: the flat DiskRules
    // are identical (binds is not on DiskRule), so the loader dedupes and
    // the sidecar keeps the FIRST copy's origin.
    let b = source("dup", Some(true), or_clause("a", "b"));
    let policy = CompiledPolicy::compile_source(&[a, b]).expect("compiles");
    assert_eq!(policy.rules().len(), 2);
    assert_eq!(
        policy.diagnostics(),
        &[
            PolicyDiagnostic::DuplicateIdenticalRule {
                rule_id: "dup#or0".to_string(),
                message: "lists rule `dup#or0` more than once with identical content; using one copy — delete the extras".to_string(),
            },
            PolicyDiagnostic::DuplicateIdenticalRule {
                rule_id: "dup#or1".to_string(),
                message: "lists rule `dup#or1` more than once with identical content; using one copy — delete the extras".to_string(),
            }
        ]
    );
    // The first copy's metadata wins, matching the kept rule.
    assert_eq!(
        policy.source_origin("dup#or0").map(|o| o.binds),
        Some(Some(false))
    );
    assert_eq!(
        policy.source_origin("dup#or1").map(|o| o.binds),
        Some(Some(false))
    );
}

#[test]
fn differing_duplicate_sources_are_invalid() {
    let a = source("dup", None, or_clause("a", "b"));
    let b = SourceRule {
        priority: 9,
        ..source("dup", None, or_clause("a", "b"))
    };
    let err = CompiledPolicy::compile_source(&[a, b]).unwrap_err();
    assert!(
        matches!(
            &err,
            PolicyError::Invalid { source_id, message }
                if source_id == "compiled" && message.contains("duplicate rule id `dup#or0`")
        ),
        "{err:?}"
    );
}

#[test]
fn differing_flat_rules_from_two_sources_collide() {
    let a = source("shared", None, or_clause("a", "b"));
    let b = SourceRule {
        phase: "post".to_string(),
        ..source("shared", None, or_clause("a", "b"))
    };
    let err = CompiledPolicy::compile_source(&[a, b]).unwrap_err();
    assert!(matches!(err, PolicyError::Invalid { .. }));
}

#[test]
fn large_or_expansion_diagnostic_is_not_lost() {
    let alts = (0..7)
        .map(|i| {
            WhenClause::Leaf(phronesis_governance::policy::DiskCondition {
                predicate: "new_content_contains".to_string(),
                args: vec![format!("f{i}()")],
                script: None,
            })
        })
        .collect::<Vec<_>>();
    let wide = SourceRule {
        when: vec![
            WhenClause::Or(alts.clone()),
            WhenClause::Or(alts.clone()),
            WhenClause::Or(alts),
        ],
        ..source("wide", Some(false), WhenClause::Or(vec![]))
    };
    let policy = CompiledPolicy::compile_source(&[wide]).expect("compiles");
    // 7 * 7 * 7 = 343 flat rules, above the 32 threshold.
    assert_eq!(policy.rules().len(), 343);
    let large = policy
        .diagnostics()
        .iter()
        .find(|d| matches!(d, PolicyDiagnostic::LargeOrExpansion { .. }))
        .expect("OR diagnostic preserved");
    assert_eq!(large.rule_id(), "wide");
    assert!(large.message().contains("expands to 343 rules via OR"));
}

#[test]
fn empty_or_is_an_unfold_error_naming_the_source_rule() {
    let err = CompiledPolicy::compile_source(&[source(
        "broken",
        None,
        WhenClause::Or(vec![]),
    )])
    .unwrap_err();
    assert!(
        matches!(
            &err,
            PolicyError::Unfold { source_id, message }
                if source_id == "broken" && message.contains("empty `or`")
        ),
        "{err:?}"
    );
}
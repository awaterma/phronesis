use phronesis_rhai::{FactProviderEvent, RhaiFactProvider};

fn edit_event() -> FactProviderEvent {
    FactProviderEvent {
        phase: "pre".to_string(),
        tool_name: "Edit".to_string(),
        file_path: "src/parser/mod.rs".to_string(),
        old_content: "fn parse() {}".to_string(),
        new_content: "fn parse() { value.unwrap(); }".to_string(),
        command: String::new(),
        output: String::new(),
        files: Vec::new(),
    }
}

#[test]
fn provider_emits_extensible_predicate_facts() {
    let script = r#"
        if event.tool_name == "Edit" && event.file_path.starts_with("src/parser/") {
            emit_fact("parser_changed", [event.file_path]);
        }
        if event.new_content.contains("unwrap()") {
            emit_fact("unsafe_unwrap_added", [event.file_path]);
        }
    "#;

    let facts = RhaiFactProvider::new()
        .evaluate(script, &edit_event())
        .expect("provider evaluates");

    assert_eq!(facts.len(), 2);
    assert_eq!(facts[0].predicate, "parser_changed");
    assert_eq!(facts[0].args, ["src/parser/mod.rs"]);
    assert_eq!(facts[1].predicate, "unsafe_unwrap_added");
}

#[test]
fn provider_rejects_invalid_fact_shapes() {
    let provider = RhaiFactProvider::new();
    assert!(
        provider
            .evaluate(r#"emit_fact("../bad", ["x"]);"#, &edit_event())
            .is_err()
    );
    assert!(
        provider
            .evaluate(r#"emit_fact("valid", [#{ nested: true }]);"#, &edit_event())
            .is_err()
    );
}

#[test]
fn provider_can_classify_a_multi_file_change_set() {
    let provider = RhaiFactProvider::new();
    let event = FactProviderEvent {
        phase: "pre".to_string(),
        tool_name: "apply_patch".to_string(),
        files: vec!["src/lib.rs".to_string(), "tests/lib.rs".to_string()],
        ..FactProviderEvent::default()
    };
    let script = r#"
        let production = 0;
        let tests = 0;
        for path in event.files {
            if path.contains("/src/") || path.starts_with("src/") {
                production += 1;
            }
            if path.contains("/tests/") || path.starts_with("tests/") {
                tests += 1;
            }
        }
        if production > 0 { emit_fact("production_change", []); }
        if tests > 0 { emit_fact("test_change", []); }
    "#;

    let facts = provider
        .evaluate(script, &event)
        .expect("change-set provider should evaluate");

    assert_eq!(facts.len(), 2);
    assert_eq!(facts[0].predicate, "production_change");
    assert_eq!(facts[1].predicate, "test_change");
}

#[test]
fn repository_change_set_provider_marks_missing_test_path() {
    let provider = RhaiFactProvider::new();
    let script = include_str!("../../../.phronesis/predicates/change_set.rhai");
    let source_only = FactProviderEvent {
        phase: "pre".to_string(),
        tool_name: "apply_patch".to_string(),
        files: vec!["crates/phronesis/src/network.rs".to_string()],
        ..FactProviderEvent::default()
    };
    let with_test = FactProviderEvent {
        files: vec![
            "crates/phronesis/src/network.rs".to_string(),
            "crates/phronesis/tests/rete_smoke.rs".to_string(),
        ],
        ..source_only.clone()
    };

    let source_only_facts = provider
        .evaluate(script, &source_only)
        .expect("repository provider should evaluate");
    assert!(
        source_only_facts
            .iter()
            .any(|fact| { fact.predicate == "change_set_production_without_test" })
    );

    let with_test_facts = provider
        .evaluate(script, &with_test)
        .expect("repository provider should evaluate");
    assert!(
        !with_test_facts
            .iter()
            .any(|fact| { fact.predicate == "change_set_production_without_test" })
    );
    assert!(
        with_test_facts
            .iter()
            .any(|fact| fact.predicate == "change_set_has_test")
    );
}

#[test]
fn provider_scope_is_fresh_between_evaluations() {
    let provider = RhaiFactProvider::new();
    let script = r#"emit_fact("one_per_run", [event.file_path]);"#;
    assert_eq!(provider.evaluate(script, &edit_event()).unwrap().len(), 1);
    assert_eq!(provider.evaluate(script, &edit_event()).unwrap().len(), 1);
}

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
fn provider_scope_is_fresh_between_evaluations() {
    let provider = RhaiFactProvider::new();
    let script = r#"emit_fact("one_per_run", [event.file_path]);"#;
    assert_eq!(provider.evaluate(script, &edit_event()).unwrap().len(), 1);
    assert_eq!(provider.evaluate(script, &edit_event()).unwrap().len(), 1);
}

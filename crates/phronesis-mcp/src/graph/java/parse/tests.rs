use super::*;

#[test]
fn qualified_record_patterns_preserve_declarations_imports_and_method_calls() {
    let source = r#"
        package app;
        import model.Outer;
        class Patterns {
            static int inspect(Object value) {
                if (value instanceof Outer.Item(String text)) {
                    return text.length();
                }
                return switch (value) {
                    case Outer.Item(String text) -> text.length();
                    default -> 0;
                };
            }
        }
    "#;
    let parsed = parse("Patterns.java", source);
    assert!(!parsed.parse_failed, "{parsed:?}");
    assert_eq!(parsed.skipped, 0);
    assert_eq!(parsed.types, ["Patterns"]);
    assert_eq!(parsed.imports, [ImportDecl::Type("model.Outer".into())]);
    assert_eq!(parsed.methods.len(), 1);
    assert!(parsed.value_names.contains("text"));
    assert_eq!(parsed.methods[0].calls.len(), 2);
    assert!(
        parsed.methods[0]
            .calls
            .iter()
            .all(|call| call.name == "length")
    );
    // The grammar extension must not convert malformed patterns into evidence.
    let broken = source.replace("Outer.Item(String text)", "Outer.Item(String text");
    assert!(parse("Patterns.java", &broken).parse_failed);
}

#[test]
fn declarations_follow_packages_and_member_paths_not_files_or_local_classes() {
    let out = parse(
        "wrong/layout.java",
        r#"
        package actual /* directory is irrelevant */ . names;
        class Outer {
            class Inner { record Item(int value) {} }
            void method() {
                class Local {}
                Object x = new Object() { class AnonymousMember {} };
            }
        }
        interface Contract { @interface Marker {} }
        enum Mode { ON; class Nested {} }
        "#,
    );
    assert!(!out.parse_failed, "{out:?}");
    assert_eq!(out.skipped, 0);
    assert_eq!(out.package, "actual.names");
    assert_eq!(
        out.types,
        [
            "Contract",
            "Contract.Marker",
            "Mode",
            "Mode.Nested",
            "Outer",
            "Outer.Inner",
            "Outer.Inner.Item"
        ]
    );
    assert_eq!(out.methods.len(), 1);
    assert_eq!(out.methods[0].declaring_type, "Outer");
}

#[test]
fn comments_strings_and_annotations_cannot_invent_imports_or_packages() {
    let out = parse(
        "Example.java",
        r#"
        // package invented; import phantom.Type;
        class Example {
            String message = "import other.Type; package fake;";
            @SuppressWarnings("@Test") void helper() {}
        }
        "#,
    );
    assert!(!out.parse_failed);
    assert_eq!(out.package, "");
    assert!(out.imports.is_empty());
    assert!(!out.methods[0].test_annotation);
}

#[test]
fn parses_each_import_form_including_java_25_module_import() {
    let out = parse(
        "Example.java",
        r#"
        import p.Outer;
        import p.Outer.Inner;
        import p /* comment */ . *;
        import static p.Outer.make;
        import static p.Outer.*;
        import module java.sql;
        class Example {}
        "#,
    );
    assert!(!out.parse_failed, "{out:?}");
    assert_eq!(
        out.imports,
        [
            ImportDecl::Type("p.Outer".into()),
            ImportDecl::Type("p.Outer.Inner".into()),
            ImportDecl::Wildcard("p".into()),
            ImportDecl::StaticMember("p.Outer.make".into()),
            ImportDecl::StaticWildcard("p.Outer".into()),
            ImportDecl::Module,
        ]
    );
}

#[test]
fn module_info_is_skipped_but_package_info_keeps_annotated_package_and_imports() {
    let out = parse(
        "src/module-info.java",
        "module example { requires java.sql; }",
    );
    assert!(out.module_info);
    assert!(!out.parse_failed);
    assert!(out.types.is_empty());
    let out = parse("p/package-info.java", "@Marker package p; import x.Marker;");
    assert_eq!(out.package, "p");
    assert_eq!(out.imports, [ImportDecl::Type("x.Marker".into())]);
    assert!(out.methods.is_empty());
}

#[test]
fn only_method_annotations_identify_tests_and_nested_bodies_do_not_supply_calls() {
    let out = parse(
        "Checks.java",
        r#"
        class Checks {
            @org.junit.jupiter.api.Test void verifies() {
                Service.run(1);
                object.get();
                class Local { void method() { bogus(); } }
                Runnable deferred = () -> later();
                Object anonymous = new Object() { void hidden() { phantom(); } };
            }
            @ParameterizedTest(name = "{0}") void parameterized(String value) { run(value); }
            @RepeatedTest(2) void repeated() {}
            @TestFactory Object factory() { return null; }
            void helper() { Service.run(); }
        }
        "#,
    );
    assert!(!out.parse_failed, "{out:?}");
    assert_eq!(out.methods.len(), 5);
    assert!(out.methods[..4].iter().all(|method| method.test_annotation));
    assert!(!out.methods[4].test_annotation);
    assert_eq!(
        out.methods[0].calls,
        [
            Call {
                receiver: Some("Service".into()),
                constructed_type: None,
                name: "run".into(),
                arity: 1
            },
            Call {
                receiver: Some("object".into()),
                constructed_type: None,
                name: "get".into(),
                arity: 0
            },
        ]
    );
    assert_eq!(out.methods[1].arity, 1);
}

#[test]
fn module_import_identifiers_preserve_contextual_words_but_reject_reserved_words() {
    for name in ["java.sql", "module.record", "open.var", "_name.$generated"] {
        let source = format!("import module {name}; class A {{}}");
        let out = parse("A.java", &source);
        assert!(!out.parse_failed, "{name}: {out:?}");
        assert_eq!(out.imports, [ImportDecl::Module]);
    }
    for name in [
        "class",
        "for",
        "int",
        "true",
        "false",
        "null",
        "_",
        "java.class",
    ] {
        let source = format!("import module {name}; class A {{}}");
        assert!(parse("A.java", &source).parse_failed, "{name}");
    }
}

#[test]
fn malformed_declarations_do_not_become_default_package_evidence() {
    for source in [
        "package ; class A {}",
        "package a; package b; class A {}",
        "class Broken { void f( }",
        "import module java..sql; class A {}",
        "import module 123; class A {}",
        "import module java.sql class A {}",
        "import module class; class A {}",
        "import module java.for; class A {}",
        "import module int; class A {}",
        "import module true; class A {}",
        "import module _; class A {}",
    ] {
        let out = parse("A.java", source);
        assert!(out.parse_failed, "{source}: {out:?}");
        assert_eq!(out.skipped, 1);
        assert!(out.types.is_empty());
    }
}

#[test]
fn enhanced_for_variables_are_value_bindings() {
    let parsed = parse(
        "Check.java",
        "class Check { void check(java.util.List<Worker> workers) { for (Worker Service : workers) { Service.run(); } } }",
    );
    assert!(!parsed.parse_failed);
    assert!(parsed.value_names.contains("workers"));
    assert!(parsed.value_names.contains("Service"));
}

#[test]
fn orchard_modifiers_preserve_static_methods_and_test_annotations() {
    let out = parse(
        "Check.java",
        "class Check { @Test public static void check() {} public void instance() {} }",
    );
    assert!(!out.parse_failed);
    assert!(out.methods[0].is_static);
    assert!(out.methods[0].test_annotation);
    assert!(!out.methods[1].is_static);
}

#[test]
fn module_spelling_in_ordinary_imports_is_not_a_module_import() {
    let out = parse(
        "Check.java",
        "import module; import module.Type; import module.*; class Check {}",
    );
    assert!(!out.parse_failed, "{out:?}");
    assert_eq!(
        out.imports,
        [
            ImportDecl::Type("module".into()),
            ImportDecl::Type("module.Type".into()),
            ImportDecl::Wildcard("module".into())
        ]
    );
}

#[test]
fn unicode_string_literals_preserve_complete_file_evidence() {
    // Orchard 0.5.9–0.5.15 reject the Kelvin sign in two Bazel corpus files.
    let out = parse(
        "Names.java",
        r#"class Names { String name = "K"; void check() { value.get(); } }"#,
    );
    assert!(!out.parse_failed, "{out:?}");
    assert_eq!(out.types, ["Names"]);
    assert_eq!(out.methods.len(), 1);
    assert_eq!(out.methods[0].calls.len(), 1);
}

use super::*;

fn discover_files(builds: &[(&str, &str)], files: &[&str]) -> Discovery {
    discover(
        &builds
            .iter()
            .map(|(file, body)| (file.to_string(), body.to_string()))
            .collect(),
        &files
            .iter()
            .map(|file| file.to_string())
            .collect::<Vec<_>>(),
    )
}

#[test]
fn review_fixtures_keep_list_selects_and_reject_scalar_list_attributes() {
    let out = discover_files(
        &[
            (
                "BUILD",
                "java_library(name='app', srcs=['App.java'], deps=select({':a':['//lib:api'], '//conditions:default':[]}))",
            ),
            (
                "lib/BUILD",
                "java_library(name='api', srcs=['Api.java'], exports=select({':a':['//exported:api'], '//conditions:default':[]}))",
            ),
            (
                "exported/BUILD",
                "java_library(name='api', srcs=['Exported.java'])",
            ),
        ],
        &["App.java", "lib/Api.java", "exported/Exported.java"],
    );
    assert_eq!(out.diagnostics.count("unsupported_syntax_skipped"), 0);
    assert_eq!(out.diagnostics.count("select_branch_unioned"), 2);
    assert!(out.files["App.java"].visible_files.contains("lib/Api.java"));
    assert!(
        out.files["App.java"]
            .visible_files
            .contains("exported/Exported.java")
    );
    for expression in [
        "'App.java'",
        "select({':a':'App.java', '//conditions:default':['Other.java']})",
    ] {
        let build = format!("java_library(name='app', srcs={expression})");
        let rejected = discover_files(&[("BUILD", &build)], &["App.java", "Other.java"]);
        assert_eq!(rejected.diagnostics.count("unsupported_syntax_skipped"), 1);
        assert!(
            rejected
                .files
                .values()
                .all(|file| file.context == Context::Unclaimed)
        );
    }
}

#[test]
fn same_package_labels_can_omit_colons_in_sources_dependencies_and_aliases() {
    let out = discover_files(
        &[(
            "app/BUILD",
            r#"
filegroup(name='sources', srcs=['App.java'])
java_library(name='app', srcs=['sources'], deps=['api'])
alias(name='api', actual='implementation')
java_library(name='implementation', srcs=['Api.java'], exports=['exported'])
java_library(name='exported', srcs=['Exported.java'])
java_library(name='unrelated', srcs=['Unrelated.java'])
"#,
        )],
        &[
            "app/App.java",
            "app/Api.java",
            "app/Exported.java",
            "app/Unrelated.java",
        ],
    );
    assert_eq!(out.files["app/App.java"].context, Context::Production);
    assert_eq!(
        *out.files["app/App.java"].visible_files,
        BTreeSet::from([
            "app/App.java".into(),
            "app/Api.java".into(),
            "app/Exported.java".into(),
        ])
    );
    assert_eq!(out.diagnostics.count("unresolved_label"), 0);
}

#[test]
fn globs_selects_bindings_and_test_claims_are_order_independent() {
    let body = r#"
COMMON = glob(["src/**/*.java"], exclude = ["**/data/**"])
java_test(name = "test", srcs = COMMON + select({":a": ["Extra.java"], "//conditions:default": []}))
java_library(name = "lib", srcs = glob(include = ["src/*.java"]))
"#;
    let out = discover_files(
        &[("BUILD", body)],
        &[
            "src/A.java",
            "src/nested/B.java",
            "src/data/Data.java",
            "Extra.java",
        ],
    );
    assert_eq!(out.files["src/A.java"].context, Context::Production);
    assert_eq!(out.files["src/nested/B.java"].context, Context::Test);
    assert_eq!(out.files["Extra.java"].context, Context::Test);
    assert_eq!(out.files["src/data/Data.java"].context, Context::Unclaimed);
    assert_eq!(out.diagnostics.count("select_branch_unioned"), 1);
    assert_eq!(out.diagnostics.count("dual_claimed_file"), 1);
}

#[test]
fn nested_build_boundaries_and_filegroups_preserve_test_classification() {
    let out = discover_files(
        &[
            (
                "BUILD",
                "java_library(name='all', srcs=glob(['**/*.java']))",
            ),
            (
                "nested/BUILD.bazel",
                "filegroup(name='files', srcs=['Test.java'])\njava_test(name='tests', srcs=[':files'])",
            ),
        ],
        &["Root.java", "nested/Test.java"],
    );
    assert_eq!(out.files["Root.java"].unit, "//");
    assert_eq!(out.files["nested/Test.java"].unit, "//nested");
    assert_eq!(out.files["nested/Test.java"].context, Context::Test);
    assert!(
        !out.files["Root.java"]
            .visible_files
            .contains("nested/Test.java")
    );
}

#[test]
fn dependencies_are_target_specific_and_follow_exports_but_not_transitive_deps() {
    let out = discover_files(
        &[
            (
                "BUILD",
                "java_library(name='a', srcs=['A.java'], deps=['//x:api'])\njava_library(name='b', srcs=['B.java'], deps=['//y:api'])",
            ),
            (
                "x/BUILD",
                "java_library(name='api', srcs=['X.java'], deps=['//hidden:impl'], exports=['//exported:api'])",
            ),
            ("y/BUILD", "java_library(name='api', srcs=['Y.java'])"),
            (
                "hidden/BUILD",
                "java_library(name='impl', srcs=['Hidden.java'])",
            ),
            (
                "exported/BUILD",
                "java_library(name='api', srcs=['Exported.java'])",
            ),
        ],
        &[
            "A.java",
            "B.java",
            "x/X.java",
            "y/Y.java",
            "hidden/Hidden.java",
            "exported/Exported.java",
        ],
    );
    assert_eq!(
        *out.files["A.java"].visible_files,
        BTreeSet::from([
            "A.java".into(),
            "x/X.java".into(),
            "exported/Exported.java".into()
        ])
    );
    assert_eq!(
        *out.files["B.java"].visible_files,
        BTreeSet::from(["B.java".into(), "y/Y.java".into()])
    );
}

#[test]
fn unknown_macros_claim_sources_but_entry_points_and_runtime_deps_do_not() {
    let out = discover_files(
        &[(
            "BUILD",
            r#"
custom_lib(name="runner", srcs=["Runner.java"], timeout=30)
java_test(name="suite", test_class="p.Runner", runtime_deps=[":runner"])
my_java_lib("positional", ["Unclaimed.java"])
"#,
        )],
        &["Runner.java", "Unclaimed.java"],
    );
    assert_eq!(out.files["Runner.java"].context, Context::Production);
    assert_eq!(out.files["Unclaimed.java"].context, Context::Unclaimed);
    assert_eq!(
        out.test_entry_points,
        [("//:suite".into(), "p.Runner".into())]
    );
}

#[test]
fn unsupported_syntax_unbound_names_and_cyclic_filegroups_are_counted() {
    let out = discover_files(
        &[(
            "BUILD",
            r#"
java_library(name="forward", srcs=MISSING + ["A.java"])
MISSING = ["B.java"]
filegroup(name="first", srcs=[":second"])
filegroup(name="second", srcs=[":first"])
java_test(name="cycle", srcs=[":first"])
java_library(name="bad", srcs=[f for f in ["B.java"]])
for f in ["B.java"]:
    java_library(name="loop", srcs=[f])
"#,
        )],
        &["A.java", "B.java"],
    );
    assert_eq!(out.files["A.java"].context, Context::Production);
    assert_eq!(out.files["B.java"].context, Context::Unclaimed);
    assert_eq!(out.diagnostics.count("unbound_identifier"), 1);
    assert_eq!(out.diagnostics.count("unsupported_syntax_skipped"), 2);
    assert!(out.diagnostics.count("unresolved_label") > 0);
}

#[test]
fn cross_package_src_labels_and_escaping_literals_cannot_claim_files() {
    let out = discover_files(
        &[(
            "app/BUILD",
            "java_library(name='bad', srcs=['../Other.java', '//other:files'])",
        )],
        &["Other.java", "app/A.java"],
    );
    assert!(!out.files.contains_key("Other.java"));
    assert_eq!(out.files["app/A.java"].context, Context::Unclaimed);
    assert_eq!(out.diagnostics.count("unresolved_label"), 2);
}

#[test]
fn dependency_aliases_follow_actual_targets_and_cycles_are_visible() {
    let out = discover_files(
        &[
            (
                "BUILD",
                "alias(name='api', actual='//lib:api')\njava_library(name='app', srcs=['App.java'], deps=[':api'])\nalias(name='cycle', actual=':cycle')\njava_library(name='bad', srcs=['Bad.java'], deps=[':cycle'])",
            ),
            ("lib/BUILD", "java_library(name='api', srcs=['Api.java'])"),
        ],
        &["App.java", "Bad.java", "lib/Api.java"],
    );
    assert!(out.files["App.java"].visible_files.contains("lib/Api.java"));
    assert!(!out.files["Bad.java"].visible_files.contains("lib/Api.java"));
    assert_eq!(out.diagnostics.count("unresolved_label"), 1);
}

#[test]
fn conditional_aliases_union_targets_without_mistaking_shared_targets_for_cycles() {
    let out = discover_files(
        &[
            (
                "BUILD",
                r#"
CHOICES = select({'//conditions:default': '//lib:api', ':platform': ':indirect', ':other': '//lib:other'})
alias(name='api', actual=CHOICES)
alias(name='indirect', actual='//lib:api')
alias(name='cyclic', actual=select({':platform': ':cyclic', '//conditions:default': '//lib:api'}))
java_library(name='app', srcs=['App.java'], deps=[':api', ':cyclic'])
alias(name='invalid', actual=select({':platform': ['//lib:api'], '//conditions:default': '//lib:api'}))
"#,
            ),
            (
                "lib/BUILD",
                "java_library(name='api', srcs=['Api.java'])\njava_library(name='other', srcs=['Other.java'])",
            ),
        ],
        &["App.java", "lib/Api.java", "lib/Other.java"],
    );
    assert!(out.files["App.java"].visible_files.contains("lib/Api.java"));
    assert!(
        out.files["App.java"]
            .visible_files
            .contains("lib/Other.java")
    );
    assert_eq!(out.diagnostics.count("select_branch_unioned"), 2);
    assert_eq!(out.diagnostics.count("unresolved_label"), 1);
    assert_eq!(out.diagnostics.count("unsupported_syntax_skipped"), 1);
}

#[test]
fn exponential_values_exhaust_budget_before_materialization() {
    for initial in [
        "['A.java']",
        "'A.java'",
        "select({':a': ':target', ':b': ':other'})",
    ] {
        let mut body = format!("x0 = {initial}\n");
        for i in 1..40 {
            let previous = i - 1;
            if initial.starts_with("select") {
                body.push_str(&format!(
                    "x{i} = select({{':a': x{previous}, ':b': x{previous}}})\n"
                ));
            } else {
                body.push_str(&format!("x{i} = x{previous} + x{previous}\n"));
            }
        }
        body.push_str("java_library(name='after', srcs=['A.java'])\n");
        let mut diagnostics = Diagnostics::default();
        let calls = eval::evaluate("BUILD", &body, &[], &mut diagnostics);
        assert!(
            calls.is_empty(),
            "evaluation continued after exhaustion: {initial}"
        );
        assert_eq!(diagnostics.count("evaluation_budget_exceeded"), 1);
    }
}

#[test]
fn large_literal_source_lists_remain_supported() {
    let sources = (0..10_000)
        .map(|i| format!("'Source{i}.java'"))
        .collect::<Vec<_>>()
        .join(",");
    let body = format!("SRCS = [{sources}]\njava_library(name='large', srcs=SRCS)");
    let mut diagnostics = Diagnostics::default();
    let calls = eval::evaluate("BUILD", &body, &[], &mut diagnostics);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].srcs.len(), 10_000);
    assert_eq!(diagnostics.count("evaluation_budget_exceeded"), 0);
}

#[test]
fn identical_claims_share_visibility_and_multiple_claims_union_it() {
    let out = discover_files(
        &[(
            "BUILD",
            "java_library(name='a', srcs=['A.java', 'B.java', 'Both.java'], deps=[':dep'])\njava_library(name='b', srcs=['C.java', 'Both.java'])\njava_library(name='dep', srcs=['Dep.java'])",
        )],
        &["A.java", "B.java", "C.java", "Both.java", "Dep.java"],
    );
    assert!(Arc::ptr_eq(
        &out.files["A.java"].visible_files,
        &out.files["B.java"].visible_files
    ));
    assert_eq!(
        *out.files["Both.java"].visible_files,
        BTreeSet::from([
            "A.java".into(),
            "B.java".into(),
            "Both.java".into(),
            "C.java".into(),
            "Dep.java".into()
        ])
    );
    assert!(!out.files["C.java"].visible_files.contains("Dep.java"));
}

#![cfg(all(feature = "go-grammar", feature = "rhai"))]

use phronesis_mcp::coverage::{
    language::go,
    lcov::{self, LcovOptions},
    region_map::changed_regions_in,
};
use std::fs;

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("fixture");
    fs::write(
        dir.path().join("go.mod"),
        "module example.invalid/cov\n\ngo 1.26\n",
    )
    .expect("module");
    fs::write(dir.path().join("calc.go"), SOURCE).expect("source");
    dir
}
const SOURCE: &str = "package calc\ntype A struct{}\ntype B struct{}\nfunc (a A) Read() int {\n return 1\n}\nfunc (b B) Read() int {\n return 2\n}\nfunc One() int { return 3 }\n";

#[test]
fn go_pack_regions_distinguish_receivers_and_join_changed_regions() {
    let dir = fixture();
    let sites = go::function_sites(dir.path(), "calc.go", SOURCE).expect("sites");
    assert_eq!(sites.len(), 3);
    let a = sites
        .iter()
        .find(|s| s.item_path.ends_with("::A::Read"))
        .expect("A");
    let b = sites
        .iter()
        .find(|s| s.item_path.ends_with("::B::Read"))
        .expect("B");
    assert_ne!(a.region_id("calc.go"), b.region_id("calc.go"));
    let changed = changed_regions_in(
        dir.path(),
        "calc.go",
        SOURCE,
        &SOURCE.replace("return 1", "return 10"),
    )
    .expect("change");
    assert_eq!(changed.functions, vec![a.region_id("calc.go")]);
}

#[test]
fn bazel_dat_requires_explicit_identity_and_prefix_and_preserves_receivers() {
    let dir = fixture();
    let cov = dir.path().join("cov");
    fs::create_dir(&cov).expect("cov");
    fs::write(
        cov.join("coverage.dat"),
        "TN:\nSF:workspace/calc.go\nDA:5,1\nDA:8,0\nDA:10,1\nend_of_record\n",
    )
    .expect("lcov");
    assert!(lcov::records_from_lcov_dir(dir.path(), &cov, "go", "abc").is_err());
    let opts = LcovOptions {
        test_id: Some("go:go::example.invalid/cov::TestRead".into()),
        workspace_prefix: Some("workspace".into()),
    };
    let (hits, _) = lcov::records_from_lcov_dir_with_options(dir.path(), &cov, "go", "abc", &opts)
        .expect("import");
    assert_eq!(hits.len(), 2);
    assert!(hits.iter().any(|h| h.region.ends_with("::A::Read")));
    assert!(!hits.iter().any(|h| h.region.ends_with("::B::Read")));
    assert!(hits.iter().any(|h| h.region.ends_with("::One")));
    fs::write(cov.join("second.info"), "TN:another\n").expect("second");
    assert!(
        lcov::records_from_lcov_dir_with_options(dir.path(), &cov, "go", "abc", &opts)
            .expect_err("single report required")
            .to_string()
            .contains("exactly one")
    );
}

#[test]
fn explicit_identity_conflicts_and_unsafe_prefixes_fail() {
    let dir = fixture();
    let cov = dir.path().join("cov");
    fs::create_dir(&cov).expect("cov");
    fs::write(
        cov.join("coverage.dat"),
        "TN:first\nSF:calc.go\nDA:5,1\nend_of_record\n",
    )
    .expect("lcov");
    let mut opts = LcovOptions {
        test_id: Some("second".into()),
        workspace_prefix: None,
    };
    assert!(
        lcov::records_from_lcov_dir_with_options(dir.path(), &cov, "go", "abc", &opts)
            .expect_err("conflict")
            .to_string()
            .contains("conflicts")
    );
    for prefix in ["../workspace", "/workspace", "workspace/../x", ""] {
        opts.workspace_prefix = Some(prefix.into());
        assert!(opts.validate().is_err());
    }
}

#[cfg(unix)]
#[test]
fn source_and_report_symlinks_are_not_followed_even_in_absolute_fallback() {
    use std::os::unix::fs::symlink;
    let dir = fixture();
    let outside = tempfile::tempdir().expect("outside");
    fs::write(outside.path().join("other.go"), SOURCE).expect("outside source");
    symlink(outside.path(), dir.path().join("bazel-out")).expect("link");
    symlink(dir.path(), dir.path().join("cycle")).expect("cycle");
    assert!(matches!(
        lcov::relativize(dir.path(), "bazel-out/other.go"),
        lcov::Relativized::Missing
    ));
    assert!(matches!(
        lcov::relativize(dir.path(), "/old/workspace/other.go"),
        lcov::Relativized::Missing
    ));
    assert!(
        matches!(lcov::relativize(dir.path(),"/old/workspace/calc.go"),lcov::Relativized::Path(p) if p=="calc.go")
    );
    let cov = dir.path().join("cov");
    fs::create_dir(&cov).expect("cov");
    symlink(outside.path().join("other.go"), cov.join("coverage.dat")).expect("report link");
    assert!(lcov::report_paths(&cov).expect("paths").is_empty());
}

#[test]
fn shared_line_declarations_do_not_receive_ambiguous_hits() {
    let dir = fixture();
    let source = "package calc\nfunc First() int { return 1 }; func Second() int { return 2 }\n";
    let sites = go::function_sites(dir.path(), "calc.go", source).expect("sites");
    assert_eq!(sites.len(), 2);
    let src = lcov::LcovSource {
        path: "calc.go".into(),
        line_hits: vec![(2, 1)],
        function_hits: vec![],
    };
    assert!(lcov::hit_sites(&sites, &src, &go::LANGUAGE).0.is_empty());
}

#[test]
fn escaped_pack_identity_is_valid_and_injective_in_coverage_store() {
    assert_ne!(
        go::region_item("go:go::pkg%3A::Read"),
        go::region_item("go:go::pkg_p3A::Read")
    );
    let dir = fixture();
    let sites = go::function_sites(dir.path(), "calc.go", SOURCE).expect("sites");
    for site in sites {
        let record = phronesis_mcp::coverage::store::HitRecord {
            v: phronesis_mcp::coverage::store::COVERAGE_FORMAT,
            kind: "hit".into(),
            test: "go:go::example.invalid/cov::TestRead".into(),
            region: site.region_id("calc.go"),
            file: "calc.go".into(),
            start_line: site.start_line,
            end_line: site.end_line,
            hit_kind: "region".into(),
            revision: "a".repeat(40),
            tool: "go-test".into(),
        };
        phronesis_mcp::coverage::store::validate_record(&record).expect("valid pack region");
    }
}

#[test]
fn top_level_initializer_cannot_credit_an_unrun_function_on_the_same_line() {
    let dir = fixture();
    let source = "package calc\nvar x = func() int { return 1 }(); func F() int { return 2 }\n";
    let sites = go::function_sites(dir.path(), "calc.go", source).expect("sites");
    assert_eq!(sites.len(), 1);
    let src = lcov::LcovSource {
        path: "calc.go".into(),
        line_hits: vec![(2, 1)],
        function_hits: vec![],
    };
    assert!(lcov::hit_sites(&sites, &src, &go::LANGUAGE).0.is_empty());
}

#[test]
fn wrapped_signature_with_single_line_body_receives_hits() {
    let dir = fixture();
    let source = "package calc\nfunc F(\n) int { return 2 }\n";
    let sites = go::function_sites(dir.path(), "calc.go", source).expect("sites");
    assert_eq!(sites.len(), 1);
    let src = lcov::LcovSource {
        path: "calc.go".into(),
        line_hits: vec![(3, 1)],
        function_hits: vec![],
    };
    assert_eq!(lcov::hit_sites(&sites, &src, &go::LANGUAGE).0.len(), 1);
    assert_ne!(go::region_item("foo_p"), go::region_item("foo%p"));
}

#[test]
fn skipping_an_ambiguous_opening_row_does_not_credit_the_closing_row() {
    let dir = fixture();
    let source = "package calc\nvar x = func() int { return 1 }(); func F() {\n}; var y = func() int { return 2 }()\n";
    let sites = go::function_sites(dir.path(), "calc.go", source).expect("sites");
    assert_eq!(sites.len(), 1);
    let src = lcov::LcovSource {
        path: "calc.go".into(),
        line_hits: vec![(2, 1), (3, 1)],
        function_hits: vec![],
    };
    assert!(lcov::hit_sites(&sites, &src, &go::LANGUAGE).0.is_empty());
}

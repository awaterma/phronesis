//! One row per language: what a "function region" is, which files are
//! production sources, how a test id is namespaced, whether a one-line
//! function may be attributed from `FNDA`, and how `select` renders a
//! runnable command. Adding a language is one row here plus one module.

use crate::coverage::region_map::{FunctionSite, extract_function_sites, python_function_sites};

pub mod typescript;

pub struct CoverageLanguage {
    pub id: &'static str,
    pub extensions: &'static [&'static str],
    pub test_id_prefix: &'static str,
    pub function_sites: fn(&str) -> anyhow::Result<Vec<FunctionSite>>,
    /// Per-path extraction when the grammar choice depends on the file
    /// (TSX vs TypeScript); `extract_function_sites_for` prefers it.
    pub function_sites_for_path: Option<fn(&str, &str) -> anyhow::Result<Vec<FunctionSite>>>,
    pub is_wanted_source: fn(&str) -> bool,
    pub is_one_liner: fn(&FunctionSite) -> bool,
    pub one_liner_needs_fnda: bool,
    pub render_command: fn(&str, &str) -> Option<String>,
}

pub mod rust {
    use crate::coverage::region_map::FunctionSite;

    pub fn is_wanted_source(rel: &str) -> bool {
        rel.contains("/src/") && !rel.contains("/src/bin/") && !rel.ends_with("build.rs")
    }

    /// The llvm-cov path never consults `hit_sites`; a Rust row takes no
    /// one-liner path.
    pub fn is_one_liner(_site: &FunctionSite) -> bool {
        false
    }

    pub fn render_command(_test_id: &str, _file: &str) -> Option<String> {
        None
    }
}

pub mod python {
    use crate::coverage::region_map::FunctionSite;

    pub fn is_wanted_source(rel: &str) -> bool {
        let name = rel.rsplit('/').next().unwrap_or(rel);
        name != "conftest.py" && crate::graph::python::classify_python_file(rel) != "test"
    }

    /// Part F's contract: the `def` line is the body start exactly when the
    /// function is a one-liner (a split one-liner still has body_start==start).
    pub fn is_one_liner(site: &FunctionSite) -> bool {
        site.body_start_line == site.start_line
    }

    /// Moved verbatim from `select.rs` (Part F).
    pub fn render_command(test: &str, file: &str) -> Option<String> {
        let module = file.strip_suffix(".py")?.replace('/', "::");
        let marker = format!("::{module}::");
        let suffix = test.rsplit_once(&marker)?.1;
        Some(format!("python -m pytest {file}::{suffix}"))
    }
}

pub static LANGUAGES: &[CoverageLanguage] = &[
    CoverageLanguage {
        id: "rust",
        extensions: &["rs"],
        test_id_prefix: "",
        function_sites: extract_function_sites,
        function_sites_for_path: None,
        is_wanted_source: rust::is_wanted_source,
        is_one_liner: rust::is_one_liner,
        one_liner_needs_fnda: false,
        render_command: rust::render_command,
    },
    CoverageLanguage {
        id: "python",
        extensions: &["py"],
        test_id_prefix: "python:",
        function_sites: python_function_sites,
        function_sites_for_path: None,
        is_wanted_source: python::is_wanted_source,
        is_one_liner: python::is_one_liner,
        one_liner_needs_fnda: true,
        render_command: python::render_command,
    },
    CoverageLanguage {
        id: "typescript",
        extensions: &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"],
        test_id_prefix: "typescript:",
        function_sites: typescript::function_sites,
        function_sites_for_path: Some(typescript::function_sites_for_path),
        is_wanted_source: typescript::is_wanted_source,
        is_one_liner: typescript::is_one_liner,
        one_liner_needs_fnda: true,
        render_command: typescript::render_command,
    },
];

pub fn language_for_path(rel: &str) -> Option<&'static CoverageLanguage> {
    let ext = std::path::Path::new(rel).extension()?.to_str()?;
    LANGUAGES.iter().find(|l| l.extensions.contains(&ext))
}

/// Longest matching non-empty prefix wins; the empty prefix (Rust's bare
/// libtest names) is the fallback only when no namespaced prefix matches.
pub fn language_for_test_id(test_id: &str) -> Option<&'static CoverageLanguage> {
    LANGUAGES
        .iter()
        .filter(|l| !l.test_id_prefix.is_empty() && test_id.starts_with(l.test_id_prefix))
        .max_by_key(|l| l.test_id_prefix.len())
        .or_else(|| LANGUAGES.iter().find(|l| l.test_id_prefix.is_empty()))
}

#[cfg(test)]
mod python_command_tests {
    use super::python::render_command as python_command;
    #[test]
    fn renders_graph_python_id_as_a_pytest_node_id() {
        assert_eq!(
            python_command(
                "python:pkg::tests::test_store::test_load",
                "tests/test_store.py"
            )
            .as_deref(),
            Some("python -m pytest tests/test_store.py::test_load")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_dispatches_rust_python_and_typescript_and_rejects_the_rest() {
        assert_eq!(
            language_for_path("crates/x/src/lib.rs").map(|l| l.id),
            Some("rust")
        );
        assert_eq!(
            language_for_path("pkg/store.py").map(|l| l.id),
            Some("python")
        );
        assert_eq!(language_for_path("a/b.txt").map(|l| l.id), None);
        assert_eq!(
            language_for_path("src/x.ts").map(|l| l.id),
            Some("typescript")
        );
        assert_eq!(
            language_for_path("src/x.js").map(|l| l.id),
            Some("typescript")
        );
        assert_eq!(
            language_for_test_id("python:pkg::tests::test_store::test_load").map(|l| l.id),
            Some("python")
        );
        assert_eq!(
            language_for_test_id("typescript:myapp::tests::store::Store loads").map(|l| l.id),
            Some("typescript")
        );
        assert_eq!(
            language_for_test_id("outcomes::tests::a_bare_libtest_name").map(|l| l.id),
            Some("rust")
        );
    }

    #[test]
    fn registry_rows_carry_part_f_behaviour_unchanged() {
        let py = language_for_path("pkg/store.py").expect("python row");
        assert!(py.one_liner_needs_fnda);
        assert!((py.is_wanted_source)("pkg/store.py"));
        assert!(!(py.is_wanted_source)("tests/test_store.py"));
        assert!(!(py.is_wanted_source)("pkg/conftest.py"));
        assert_eq!(
            (py.render_command)(
                "python:pkg::tests::test_store::test_load",
                "tests/test_store.py"
            ),
            Some("python -m pytest tests/test_store.py::test_load".to_string())
        );
        let rs = language_for_path("crates/x/src/lib.rs").expect("rust row");
        assert!(!rs.one_liner_needs_fnda);
        assert!((rs.is_wanted_source)("crates/x/src/lib.rs"));
        assert!(!(rs.is_wanted_source)("crates/x/src/bin/main.rs"));
        assert_eq!(
            (rs.render_command)("outcomes::tests::x", "crates/x/src/lib.rs"),
            None
        );
        // Revision 2 pins: the one-liner predicate is per language.
        let one_line = FunctionSite {
            item_path: "f".into(),
            start_line: 4,
            body_start_line: 4,
            end_line: 4,
        };
        let split_one_liner = FunctionSite {
            item_path: "g".into(),
            start_line: 4,
            body_start_line: 4,
            end_line: 5,
        };
        assert!(
            (py.is_one_liner)(&one_line) && (py.is_one_liner)(&split_one_liner),
            "Python: body_start==start, even when the one-liner spills onto later lines"
        );
        assert!(
            !(rs.is_one_liner)(&one_line) && !(rs.is_one_liner)(&split_one_liner),
            "Rust never takes the one-liner path"
        );
        assert_eq!(
            (py.function_sites)("def f():\n    pass\n")
                .expect("py")
                .len(),
            1
        );
        assert_eq!((rs.function_sites)("fn f() {}\n").expect("rs").len(), 1);
    }

    #[test]
    fn relativize_is_language_neutral_and_refuses_shared_suffixes() {
        use crate::coverage::lcov::{Relativized, relativize};
        let dir = tempfile::tempdir().expect("tempdir");
        let r = dir.path();
        for rel in [
            "app/src/x.ts",
            "lib/app/src/x.ts",
            "app/Sources/X.swift",
            "lib/app/Sources/X.swift",
            "app/src/main/java/com/x/S.java",
            "lib/app/src/main/java/com/x/S.java",
            "app/x.lua",
            "lib/app/x.lua",
        ] {
            let p = r.join(rel);
            std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
            std::fs::write(&p, "").expect("write");
        }
        for sf in [
            "/workspaces/app/src/x.ts",
            "/workspaces/app/Sources/X.swift",
            "/workspaces/app/src/main/java/com/x/S.java",
            "/workspaces/app/x.lua",
        ] {
            assert!(
                matches!(relativize(r, sf), Relativized::Ambiguous(_)),
                "{sf} must be ambiguous"
            );
        }
        assert_eq!(
            relativize(r, "/workspaces/lib/app/src/x.ts"),
            Relativized::Path("lib/app/src/x.ts".into())
        );
    }
}

//! Tests for unit discovery and manifest parsing.

#[cfg(test)]
mod core_tests {
    use crate::graph::unit::*;
    use tempfile::TempDir;

    // ─── manifest parsing ───────────────────────────────────────────

    #[test]
    fn the_package_name_is_read_from_the_package_section() {
        let m = parse_cargo_manifest("[package]\nname = \"phronesis-mcp\"\n");
        assert_eq!(m.package.as_deref(), Some("phronesis-mcp"));
    }

    #[test]
    fn a_workspace_package_section_does_not_supply_the_package_name() {
        // `[workspace.package]` holds inherited *defaults*, not this crate's
        // identity. Reading it would name every member after the workspace.
        let m = parse_cargo_manifest("[workspace.package]\nname = \"nope\"\n");
        assert_eq!(m.package, None);
    }

    #[test]
    fn a_virtual_workspace_manifest_declares_no_package() {
        let m = parse_cargo_manifest("[workspace]\nresolver = \"2\"\nmembers = [\"crates/*\"]\n");
        assert_eq!(m.package, None);
    }

    #[test]
    fn a_simple_dependency_maps_its_alias_to_itself() {
        let m = parse_cargo_manifest("[dependencies]\nserde = \"1\"\n");
        assert_eq!(m.deps.get("serde").map(String::as_str), Some("serde"));
    }

    #[test]
    fn a_renamed_dependency_resolves_to_its_real_package() {
        // Without this, `use phr::…` finds no unit and the edge vanishes.
        let m = parse_cargo_manifest(
            "[dependencies]\nphr = { version = \"0.22\", path = \"../phronesis\", package = \"phronesis\" }\n",
        );
        assert_eq!(m.deps.get("phr").map(String::as_str), Some("phronesis"));
    }

    #[test]
    fn a_multi_line_inline_table_is_read_to_its_closing_brace() {
        let m = parse_cargo_manifest(
            "[dependencies]\nphr = {\n  version = \"0.22\",\n  package = \"phronesis\",\n}\n",
        );
        assert_eq!(m.deps.get("phr").map(String::as_str), Some("phronesis"));
    }

    #[test]
    fn a_key_merely_ending_in_package_is_not_a_rename() {
        let m = parse_cargo_manifest("[dependencies]\nfoo = { default-package = \"bar\" }\n");
        assert_eq!(m.deps.get("foo").map(String::as_str), Some("foo"));
    }

    #[test]
    fn a_dependency_sub_table_registers_its_alias() {
        let m = parse_cargo_manifest("[dependencies.serde]\nversion = \"1\"\n");
        assert_eq!(m.deps.get("serde").map(String::as_str), Some("serde"));
    }

    #[test]
    fn a_dependency_sub_table_honours_its_package_key() {
        let m = parse_cargo_manifest("[dependencies.phr]\npackage = \"phronesis\"\n");
        assert_eq!(m.deps.get("phr").map(String::as_str), Some("phronesis"));
    }

    #[test]
    fn dev_dependencies_count_as_dependencies() {
        let m = parse_cargo_manifest("[dev-dependencies]\ntempfile = \"3\"\n");
        assert!(m.deps.contains_key("tempfile"));
    }

    #[test]
    fn target_specific_dependencies_count_as_dependencies() {
        let m = parse_cargo_manifest("[target.'cfg(unix)'.dependencies]\nlibc = \"0.2\"\n");
        assert!(m.deps.contains_key("libc"));
    }

    #[test]
    fn workspace_dependencies_are_kept_separate_from_local_ones() {
        let m =
            parse_cargo_manifest("[workspace.dependencies]\nphr = { package = \"phronesis\" }\n");
        assert!(m.deps.is_empty());
        assert_eq!(
            m.workspace_deps.get("phr").map(String::as_str),
            Some("phronesis")
        );
    }

    #[test]
    fn a_comment_is_not_mistaken_for_a_value() {
        let m = parse_cargo_manifest("[package]\nname = \"real\" # not \"fake\"\n");
        assert_eq!(m.package.as_deref(), Some("real"));
    }

    #[test]
    fn a_hash_inside_a_string_does_not_start_a_comment() {
        let m = parse_cargo_manifest("[package]\nname = \"a#b\"\n");
        assert_eq!(m.package.as_deref(), Some("a#b"));
    }

    // ─── unit identity ──────────────────────────────────────────────

    fn unit(name: &str, root: &str, deps: &[(&str, &str)]) -> Unit {
        Unit {
            lang: LANG_RUST,
            name: name.into(),
            root: root.into(),
            deps: deps
                .iter()
                .map(|(a, p)| ((*a).to_string(), (*p).to_string()))
                .collect(),
            // Rust targets compute their own root; these fields are Python's.
            import_root: String::new(),
            packages: Vec::new(),
            ts: TsConfig::default(),
            files: Vec::new(),
            test_target: false,
        }
    }

    #[test]
    fn a_unit_id_carries_both_language_and_package() {
        assert_eq!(unit("phronesis", "", &[]).id(), "rust:phronesis");
    }

    #[test]
    fn a_file_resolves_to_the_unit_that_contains_it() {
        let m = UnitMap::from_units(vec![
            unit("phronesis", "crates/phronesis", &[]),
            unit("phronesis-mcp", "crates/phronesis-mcp", &[]),
        ]);
        assert_eq!(
            m.resolve("crates/phronesis-mcp/src/lib.rs").map(Unit::id),
            Some("rust:phronesis-mcp".to_string())
        );
    }

    #[test]
    fn the_innermost_unit_wins_for_a_nested_workspace() {
        let m = UnitMap::from_units(vec![
            unit("outer", "", &[]),
            unit("inner", "crates/inner", &[]),
        ]);
        assert_eq!(
            m.resolve("crates/inner/src/lib.rs").map(Unit::id),
            Some("rust:inner".to_string())
        );
    }

    #[test]
    fn a_root_named_as_a_prefix_of_another_does_not_capture_it() {
        // `crates/phronesis` must not claim `crates/phronesis-mcp/...`.
        let m = UnitMap::from_units(vec![
            unit("phronesis", "crates/phronesis", &[]),
            unit("phronesis-mcp", "crates/phronesis-mcp", &[]),
        ]);
        assert_eq!(
            m.resolve("crates/phronesis-mcp/src/lib.rs").map(Unit::id),
            Some("rust:phronesis-mcp".to_string())
        );
    }

    #[test]
    fn a_file_under_no_unit_gets_the_unnamed_context() {
        let m = UnitMap::from_units(vec![unit("inner", "crates/inner", &[])]);
        assert_eq!(m.context_for("other/src/a.rs"), UnitContext::unnamed());
    }

    #[test]
    fn a_packages_library_and_default_binary_are_distinct_units() {
        let m = UnitMap::from_units(vec![unit("app", "crates/app", &[])]);
        assert_eq!(
            m.context_for("crates/app/src/lib.rs").id,
            "rust:app".to_string()
        );
        assert_eq!(
            m.context_for("crates/app/src/main.rs").id,
            "rust:app#bin:app".to_string()
        );
    }

    #[test]
    fn a_targets_module_base_is_its_crate_root_file() {
        let m = UnitMap::from_units(vec![unit("app", "crates/app", &[])]);
        for (file, base) in [
            ("crates/app/src/lib.rs", "crates/app/src/lib"),
            ("crates/app/src/graph/store.rs", "crates/app/src/lib"),
            ("crates/app/src/main.rs", "crates/app/src/main"),
            ("crates/app/src/bin/tool.rs", "crates/app/src/bin/tool"),
            ("crates/app/benches/sync.rs", "crates/app/benches/sync"),
            ("crates/app/tests/hooks/helper.rs", "crates/app/tests/hooks"),
            ("crates/app/build.rs", "crates/app/build"),
        ] {
            assert_eq!(m.context_for(file).module_base, base, "{file}");
        }
    }

    #[test]
    fn a_root_package_states_its_module_base_without_a_leading_slash() {
        let m = UnitMap::from_units(vec![unit("app", "", &[])]);
        assert_eq!(m.context_for("src/lib.rs").module_base, "src/lib");
    }

    #[test]
    fn a_file_under_no_unit_has_no_module_base() {
        // Nothing is known to strip, so the extractor keeps its own
        // `src/`-anchored heuristic.
        let m = UnitMap::from_units(vec![unit("inner", "crates/inner", &[])]);
        assert!(m.context_for("other/a.rs").module_base.is_empty());
    }

    #[test]
    fn a_binary_can_reach_its_packages_library_by_extern_name() {
        let m = UnitMap::from_units(vec![unit("my-app", "crates/app", &[])]);
        let ctx = m.context_for("crates/app/src/main.rs");
        assert_eq!(
            ctx.siblings.get("my_app").map(String::as_str),
            Some("rust:my-app")
        );
    }

    #[test]
    fn an_empty_map_names_every_file_unnamed() {
        assert_eq!(
            UnitMap::default().context_for("src/a.rs").id,
            "rust:crate".to_string()
        );
    }

    // ─── sibling resolution ─────────────────────────────────────────

    #[test]
    fn a_dependency_on_a_sibling_unit_becomes_a_reachable_namespace() {
        let m = UnitMap::from_units(vec![
            unit("phronesis", "crates/phronesis", &[]),
            unit(
                "phronesis-mcp",
                "crates/phronesis-mcp",
                &[("phr", "phronesis")],
            ),
        ]);
        let ctx = m.context_for("crates/phronesis-mcp/src/lib.rs");
        assert_eq!(
            ctx.siblings.get("phr").map(String::as_str),
            Some("rust:phronesis")
        );
    }

    #[test]
    fn a_third_party_dependency_is_not_a_sibling() {
        // A node with no definitions hanging off it is worse than no node.
        let m = UnitMap::from_units(vec![unit("mine", "", &[("serde", "serde")])]);
        assert!(m.context_for("src/a.rs").siblings.is_empty());
    }

    #[test]
    fn a_unit_is_never_its_own_sibling() {
        let m = UnitMap::from_units(vec![unit("mine", "", &[("mine", "mine")])]);
        assert!(m.context_for("src/a.rs").siblings.is_empty());
    }

    // ─── discovery from disk ────────────────────────────────────────

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(p, body).expect("write");
    }

    #[test]
    fn discovery_finds_every_member_of_a_workspace() {
        let d = TempDir::new().expect("tempdir");
        write(
            d.path(),
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/*\"]\n",
        );
        write(d.path(), "crates/a/Cargo.toml", "[package]\nname = \"a\"\n");
        write(d.path(), "crates/b/Cargo.toml", "[package]\nname = \"b\"\n");
        let m = UnitMap::discover(d.path());
        assert_eq!(
            m.resolve("crates/a/src/lib.rs").map(Unit::id).as_deref(),
            Some("rust:a")
        );
        assert_eq!(
            m.resolve("crates/b/src/lib.rs").map(Unit::id).as_deref(),
            Some("rust:b")
        );
    }

    #[test]
    fn two_crates_with_same_named_modules_get_distinct_namespaces() {
        // The bug this module exists to fix: without a unit prefix both files
        // claim `crate::wme` and merge into one node.
        let d = TempDir::new().expect("tempdir");
        write(d.path(), "crates/a/Cargo.toml", "[package]\nname = \"a\"\n");
        write(d.path(), "crates/b/Cargo.toml", "[package]\nname = \"b\"\n");
        let m = UnitMap::discover(d.path());
        assert_ne!(
            m.context_for("crates/a/src/wme.rs").id,
            m.context_for("crates/b/src/wme.rs").id
        );
    }

    #[test]
    fn a_workspace_level_alias_is_inherited_by_members() {
        let d = TempDir::new().expect("tempdir");
        write(
            d.path(),
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/*\"]\n[workspace.dependencies]\nphr = { package = \"core-lib\" }\n",
        );
        write(
            d.path(),
            "crates/app/Cargo.toml",
            "[package]\nname = \"app\"\n[dependencies]\nphr.workspace = true\n",
        );
        write(
            d.path(),
            "crates/core-lib/Cargo.toml",
            "[package]\nname = \"core-lib\"\n",
        );
        let ctx = UnitMap::discover(d.path()).context_for("crates/app/src/lib.rs");
        assert_eq!(
            ctx.siblings.get("phr").map(String::as_str),
            Some("rust:core-lib")
        );
    }

    #[test]
    fn a_virtual_root_manifest_defines_no_unit() {
        let d = TempDir::new().expect("tempdir");
        write(
            d.path(),
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/*\"]\n",
        );
        assert!(UnitMap::discover(d.path()).resolve("src/a.rs").is_none());
    }
}

#[cfg(test)]
mod python_tests {
    use crate::graph::unit::*;
    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(p, body).expect("write");
    }

    #[test]
    fn a_pyproject_declares_its_distribution_name() {
        let m = parse_pyproject_manifest("[project]\nname = \"py-side\"\nversion = \"0.1.0\"\n");
        assert_eq!(m.package.as_deref(), Some("py-side"));
    }

    #[test]
    fn a_poetry_pyproject_declares_its_distribution_name() {
        // Poetry predates PEP 621 and is still widespread.
        let m = parse_pyproject_manifest("[tool.poetry]\nname = \"legacy\"\n");
        assert_eq!(m.package.as_deref(), Some("legacy"));
    }

    #[test]
    fn a_pyproject_without_a_name_declares_no_distribution() {
        // A build-backend-only pyproject.toml is common and names nothing.
        let m = parse_pyproject_manifest("[build-system]\nrequires = [\"setuptools\"]\n");
        assert_eq!(m.package, None);
    }

    #[test]
    fn an_unclaimed_python_file_falls_back_to_a_python_namespace() {
        // The fallback must follow the file's language. Naming a stray .py
        // file `rust:crate::…` would merge it with Rust modules and make the
        // language tag a lie exactly where there is least evidence.
        let m = UnitMap::default();
        assert_eq!(m.context_for("scripts/tool.py").id, "python:project");
    }

    #[test]
    fn discovery_finds_a_python_distribution() {
        let d = TempDir::new().expect("tempdir");
        write(d.path(), "pyproject.toml", "[project]\nname = \"pyside\"\n");
        let m = UnitMap::discover(d.path());
        assert_eq!(
            m.resolve("src/pyside/utils.py").map(Unit::id).as_deref(),
            Some("python:pyside")
        );
    }

    #[test]
    fn a_sibling_distributions_packages_are_reachable_namespaces() {
        // Python has no distribution-level import aliasing: source writes the
        // *package* name, which need not match the distribution name, so the
        // sibling map has to be keyed by what is actually on disk.
        let d = TempDir::new().expect("tempdir");
        write(
            d.path(),
            "libs/core/pyproject.toml",
            "[project]\nname = \"core-lib\"\n",
        );
        write(d.path(), "libs/core/src/core/__init__.py", "");
        write(
            d.path(),
            "libs/app/pyproject.toml",
            "[project]\nname = \"app\"\n",
        );
        write(d.path(), "libs/app/src/app/__init__.py", "");

        let ctx = UnitMap::discover(d.path()).context_for("libs/app/src/app/__init__.py");
        assert_eq!(
            ctx.siblings.get("core").map(String::as_str),
            Some("python:core-lib"),
            "keyed by package name on disk, valued by distribution unit id"
        );
    }

    #[test]
    fn a_distribution_is_never_its_own_sibling() {
        let d = TempDir::new().expect("tempdir");
        write(d.path(), "pyproject.toml", "[project]\nname = \"solo\"\n");
        write(d.path(), "src/solo/__init__.py", "");
        let ctx = UnitMap::discover(d.path()).context_for("src/solo/mod.py");
        assert!(ctx.siblings.is_empty(), "{:?}", ctx.siblings);
    }

    #[test]
    fn a_python_file_never_resolves_to_a_cargo_package() {
        // Both manifests can sit in one repository, and the innermost-root
        // rule alone would hand a .py file whichever package is nearer.
        let d = TempDir::new().expect("tempdir");
        write(d.path(), "pyproject.toml", "[project]\nname = \"pyside\"\n");
        write(
            d.path(),
            "crates/inner/Cargo.toml",
            "[package]\nname = \"inner\"\n",
        );
        let m = UnitMap::discover(d.path());
        assert_eq!(
            m.resolve("crates/inner/helper.py").map(Unit::id).as_deref(),
            Some("python:pyside"),
            "a .py file under a Cargo package still belongs to the Python distribution"
        );
        assert_eq!(
            m.resolve("crates/inner/src/lib.rs")
                .map(Unit::id)
                .as_deref(),
            Some("rust:inner")
        );
    }
}

#[cfg(test)]
mod python_layout_tests {
    use crate::graph::unit::*;
    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(p, body).expect("write");
    }

    fn py_project(layout: &[&str]) -> TempDir {
        let d = TempDir::new().expect("tempdir");
        write(d.path(), "pyproject.toml", "[project]\nname = \"pyside\"\n");
        for rel in layout {
            write(d.path(), rel, "");
        }
        d
    }

    #[test]
    fn a_src_layout_roots_modules_at_the_src_directory() {
        // src/pyside/utils.py is the module `pyside.utils`, so `src` must be
        // stripped but `pyside` must not.
        let d = py_project(&["src/pyside/__init__.py"]);
        let m = UnitMap::discover(d.path());
        assert_eq!(m.context_for("src/pyside/utils.py").module_base, "src");
    }

    #[test]
    fn a_flat_layout_roots_modules_at_the_project_directory() {
        // Without a src/ directory the import root is the project root
        // itself: pyside/utils.py is `pyside.utils`.
        let d = py_project(&["pyside/__init__.py"]);
        let m = UnitMap::discover(d.path());
        assert!(m.context_for("pyside/utils.py").module_base.is_empty());
    }

    #[test]
    fn a_python_unit_has_no_compilation_target_suffix() {
        // Cargo's #bin:/#test: split exists because those compile as separate
        // crates. Python has no equivalent — a test module is just a module.
        let d = py_project(&["src/pyside/__init__.py"]);
        let m = UnitMap::discover(d.path());
        assert_eq!(m.context_for("src/pyside/utils.py").id, "python:pyside");
        assert_eq!(m.context_for("tests/test_utils.py").id, "python:pyside");
    }
}

#[cfg(test)]
mod typescript_tests {
    use crate::graph::unit::*;

    #[test]
    fn typescript_extensions_map_to_the_typescript_language() {
        for path in ["a.ts", "a.tsx", "a.mts", "a.cts"] {
            assert_eq!(lang_of_path(path), Some(LANG_TYPESCRIPT), "{path}");
        }
    }

    #[test]
    fn an_unclaimed_typescript_file_falls_back_to_a_typescript_namespace() {
        // The fallback follows the file's own language, as Python's does.
        let m = UnitMap::default();
        assert_eq!(m.context_for("scripts/tool.ts").id, "typescript:project");
    }

    #[test]
    fn a_package_json_declares_its_package_name() {
        let m = parse_package_json(r#"{"name": "myapp", "version": "1.0.0"}"#);
        assert_eq!(m.package.as_deref(), Some("myapp"));
    }

    #[test]
    fn a_scoped_package_name_is_kept_whole() {
        // `@org/pkg` is one name, not a path. Splitting it would invent a unit.
        let m = parse_package_json(r#"{"name": "@yourorg/billing"}"#);
        assert_eq!(m.package.as_deref(), Some("@yourorg/billing"));
    }

    #[test]
    fn a_package_json_without_a_name_declares_no_package() {
        // Common in app roots that are not published.
        let m = parse_package_json(r#"{"private": true, "scripts": {}}"#);
        assert_eq!(m.package, None);
    }

    #[test]
    fn malformed_package_json_declares_no_package() {
        // Degrades to "no unit" rather than guessing a name.
        assert_eq!(parse_package_json("{not json").package, None);
    }

    #[test]
    fn a_tsconfig_without_compiler_options_yields_defaults() {
        assert_eq!(parse_tsconfig("{}"), TsConfig::default());
    }

    #[test]
    fn base_url_is_normalized_without_dot_slash_or_trailing_slash() {
        let c = parse_tsconfig(r#"{"compilerOptions": {"baseUrl": "./src/"}}"#);
        assert_eq!(c.base_url, "src");
    }

    #[test]
    fn base_url_of_dot_means_the_unit_root() {
        let c = parse_tsconfig(r#"{"compilerOptions": {"baseUrl": "."}}"#);
        assert_eq!(c.base_url, "");
    }

    #[test]
    fn paths_keep_their_wildcards_verbatim() {
        let c = parse_tsconfig(
            r#"{"compilerOptions": {"baseUrl": "src", "paths": {"@app/*": ["app/*"]}}}"#,
        );
        assert_eq!(c.paths.get("@app/*"), Some(&vec!["app/*".to_string()]));
    }

    #[test]
    fn a_path_alias_may_have_several_targets() {
        // TypeScript tries each in order; resolution must keep them all.
        let c = parse_tsconfig(r#"{"compilerOptions": {"paths": {"~/*": ["lib/*", "vendor/*"]}}}"#);
        assert_eq!(
            c.paths.get("~/*"),
            Some(&vec!["lib/*".to_string(), "vendor/*".to_string()])
        );
    }

    #[test]
    fn a_tsconfig_with_comments_still_parses() {
        // tsconfig.json is JSONC in practice; TypeScript itself allows comments.
        let c = parse_tsconfig(
            "{\n  // the source root\n  \"compilerOptions\": {\"baseUrl\": \"src\"}\n}",
        );
        assert_eq!(c.base_url, "src");
    }

    #[test]
    fn a_tsconfig_with_trailing_commas_still_parses() {
        let c = parse_tsconfig(r#"{"compilerOptions": {"baseUrl": "src",},}"#);
        assert_eq!(c.base_url, "src");
    }

    #[test]
    fn malformed_tsconfig_yields_defaults_rather_than_guesses() {
        assert_eq!(parse_tsconfig("{not json"), TsConfig::default());
    }

    #[test]
    fn a_double_slash_inside_a_string_is_not_treated_as_a_comment() {
        // A url like "https://example.com" must survive strip_jsonc intact;
        // treating the `//` as a comment would truncate the value mid-string
        // and break JSON parsing entirely (or worse, silently corrupt it).
        let c = parse_tsconfig(
            r#"{"compilerOptions": {"baseUrl": "src"}, "//comment": "https://example.com"}"#,
        );
        assert_eq!(c.base_url, "src");
    }

    #[test]
    fn the_tsc_init_banner_with_a_url_in_a_block_comment_still_parses() {
        // The literal header `tsc --init` writes. A `//` inside the block
        // comment must not be read as a line-comment start — that would eat
        // the closing `*/` and corrupt everything after it.
        let c = parse_tsconfig(
            "{\n  /* Visit https://aka.ms/tsconfig to read more about this file */\n  \"compilerOptions\": { \"baseUrl\": \"src\" }\n}",
        );
        assert_eq!(c.base_url, "src");
    }

    #[test]
    fn a_block_comment_on_its_own_line_is_stripped() {
        let c = parse_tsconfig(
            "{\n  /* explanatory */\n  \"compilerOptions\": {\"baseUrl\": \"src\"}\n}",
        );
        assert_eq!(c.base_url, "src");
    }

    #[test]
    fn a_slash_star_inside_a_string_is_not_treated_as_a_comment_start() {
        let c = parse_tsconfig(
            r#"{"compilerOptions": {"baseUrl": "src"}, "note": "see /* not a comment */ here"}"#,
        );
        assert_eq!(c.base_url, "src");
    }

    #[test]
    fn a_comma_and_brace_inside_a_string_path_target_survive_intact() {
        // The trailing-comma pass must be string-aware too: `,}` inside a
        // string value is data, not a container closer.
        let c = parse_tsconfig(r#"{"compilerOptions": {"paths": {"@x/*": ["a,}b/*"]}}}"#);
        assert_eq!(c.paths.get("@x/*"), Some(&vec!["a,}b/*".to_string()]));
    }

    // ─── discovery from disk ────────────────────────────────────────

    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(p, body).expect("write");
    }

    #[test]
    fn discovery_finds_a_typescript_package() {
        let d = TempDir::new().expect("tempdir");
        write(d.path(), "package.json", r#"{"name": "myapp"}"#);
        write(d.path(), "src/index.ts", "");
        let m = UnitMap::discover(d.path());
        assert_eq!(
            m.resolve("src/index.ts").map(Unit::id).as_deref(),
            Some("typescript:myapp")
        );
    }

    #[test]
    fn two_independent_packages_in_one_tree_are_two_units() {
        // Not a monorepo — just two projects. The common case.
        let d = TempDir::new().expect("tempdir");
        write(d.path(), "frontend/package.json", r#"{"name": "web"}"#);
        write(d.path(), "frontend/src/a.ts", "");
        write(d.path(), "server/package.json", r#"{"name": "api"}"#);
        write(d.path(), "server/src/a.ts", "");
        let m = UnitMap::discover(d.path());
        assert_eq!(
            m.resolve("frontend/src/a.ts").map(Unit::id).as_deref(),
            Some("typescript:web")
        );
        assert_eq!(
            m.resolve("server/src/a.ts").map(Unit::id).as_deref(),
            Some("typescript:api")
        );
    }

    #[test]
    fn node_modules_never_defines_a_unit() {
        // Every dependency ships a package.json. Walking them would mint
        // hundreds of units and index tens of thousands of files.
        let d = TempDir::new().expect("tempdir");
        write(d.path(), "package.json", r#"{"name": "myapp"}"#);
        write(
            d.path(),
            "node_modules/left-pad/package.json",
            r#"{"name": "left-pad"}"#,
        );
        write(d.path(), "node_modules/left-pad/index.ts", "");
        let m = UnitMap::discover(d.path());
        assert_eq!(
            m.resolve("node_modules/left-pad/index.ts").map(Unit::id),
            None,
            "a dependency's file belongs to no unit of ours"
        );
    }

    #[test]
    fn a_units_file_index_excludes_node_modules() {
        let d = TempDir::new().expect("tempdir");
        write(d.path(), "package.json", r#"{"name": "myapp"}"#);
        write(d.path(), "src/a.ts", "");
        write(d.path(), "node_modules/dep/b.ts", "");
        let ctx = UnitMap::discover(d.path()).context_for("src/a.ts");
        assert!(
            ctx.files.contains(&"src/a.ts".to_string()),
            "{:?}",
            ctx.files
        );
        assert!(
            !ctx.files.iter().any(|f| f.contains("node_modules")),
            "{:?}",
            ctx.files
        );
    }

    #[test]
    fn a_units_tsconfig_is_read_into_its_context() {
        let d = TempDir::new().expect("tempdir");
        write(d.path(), "package.json", r#"{"name": "myapp"}"#);
        write(
            d.path(),
            "tsconfig.json",
            r#"{"compilerOptions": {"baseUrl": "src", "paths": {"@app/*": ["app/*"]}}}"#,
        );
        write(d.path(), "src/a.ts", "");
        let ctx = UnitMap::discover(d.path()).context_for("src/a.ts");
        assert_eq!(ctx.ts.base_url, "src");
        assert_eq!(ctx.ts.paths.get("@app/*"), Some(&vec!["app/*".to_string()]));
    }

    #[test]
    fn an_extends_chain_is_followed() {
        // Shared base configs are standard; not following `extends` loses the
        // aliases that most projects define exactly once, in the base.
        let d = TempDir::new().expect("tempdir");
        write(d.path(), "package.json", r#"{"name": "myapp"}"#);
        write(
            d.path(),
            "tsconfig.base.json",
            r#"{"compilerOptions": {"baseUrl": "src", "paths": {"@app/*": ["app/*"]}}}"#,
        );
        write(
            d.path(),
            "tsconfig.json",
            r#"{"extends": "./tsconfig.base.json"}"#,
        );
        write(d.path(), "src/a.ts", "");
        let ctx = UnitMap::discover(d.path()).context_for("src/a.ts");
        assert_eq!(ctx.ts.base_url, "src");
        assert_eq!(ctx.ts.paths.get("@app/*"), Some(&vec!["app/*".to_string()]));
    }

    #[test]
    fn a_child_tsconfig_overrides_what_it_extends() {
        let d = TempDir::new().expect("tempdir");
        write(d.path(), "package.json", r#"{"name": "myapp"}"#);
        write(
            d.path(),
            "base.json",
            r#"{"compilerOptions": {"baseUrl": "old"}}"#,
        );
        write(
            d.path(),
            "tsconfig.json",
            r#"{"extends": "./base.json", "compilerOptions": {"baseUrl": "src"}}"#,
        );
        write(d.path(), "src/a.ts", "");
        let ctx = UnitMap::discover(d.path()).context_for("src/a.ts");
        assert_eq!(ctx.ts.base_url, "src");
    }

    #[test]
    fn a_self_referential_extends_chain_terminates() {
        // A cycle here should not hang or overflow the stack — the depth cap
        // must actually stop recursion.
        let d = TempDir::new().expect("tempdir");
        write(d.path(), "package.json", r#"{"name": "myapp"}"#);
        write(
            d.path(),
            "tsconfig.json",
            r#"{"extends": "./tsconfig.json", "compilerOptions": {"baseUrl": "src"}}"#,
        );
        write(d.path(), "src/a.ts", "");
        let ctx = UnitMap::discover(d.path()).context_for("src/a.ts");
        // The file's own baseUrl still applies; the cycle just stops feeding
        // more "parent" data back in.
        assert_eq!(ctx.ts.base_url, "src");
    }

    #[test]
    fn a_nested_units_files_do_not_leak_into_the_outer_units_index() {
        // A unit's `files` must contain only files that resolve to that
        // unit. Without a boundary at the nested package.json, the outer
        // unit's index would claim the inner unit's sources too, and a later
        // import resolution against that index could name a file under the
        // wrong unit's id/module_base entirely.
        let d = TempDir::new().expect("tempdir");
        write(d.path(), "package.json", r#"{"name": "root"}"#);
        write(d.path(), "src/root.ts", "");
        write(
            d.path(),
            "packages/inner/package.json",
            r#"{"name": "inner"}"#,
        );
        write(d.path(), "packages/inner/src/a.ts", "");
        let m = UnitMap::discover(d.path());

        let outer = m.context_for("src/root.ts");
        assert_eq!(outer.id, "typescript:root");
        assert!(
            outer.files.contains(&"src/root.ts".to_string()),
            "{:?}",
            outer.files
        );
        assert!(
            !outer.files.contains(&"packages/inner/src/a.ts".to_string()),
            "outer unit's files leaked the inner unit's source: {:?}",
            outer.files
        );

        let inner = m.context_for("packages/inner/src/a.ts");
        assert_eq!(inner.id, "typescript:inner");
        assert!(
            inner.files.contains(&"packages/inner/src/a.ts".to_string()),
            "{:?}",
            inner.files
        );
    }

    #[test]
    fn a_file_created_after_discovery_is_invisible_until_rediscovered() {
        // `resolve` and `context_for` are membership-based against the
        // `files` index captured at `discover` time. Nothing in this crate
        // caches a `UnitMap` across saves today (sync always rediscovers
        // fresh before resolving), but the invariant is public API and must
        // be pinned: a stale map does not silently join the wrong unit, it
        // falls back to the unnamed context.
        let d = TempDir::new().expect("tempdir");
        write(d.path(), "package.json", r#"{"name": "root"}"#);
        write(d.path(), "src/a.ts", "");
        let m = UnitMap::discover(d.path());

        write(d.path(), "src/late.ts", "");

        assert_eq!(
            m.context_for("src/late.ts").id,
            "typescript:project",
            "a file created after discovery must not resolve into the unit \
             it would belong to on a fresh discovery"
        );
    }
}

#[cfg(test)]
mod swift_tests {
    use crate::graph::unit::*;
    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(p, body).expect("write");
    }

    const MANIFEST: &str = r#"// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "Demo",
    products: [.library(name: "App", targets: ["App"])],
    dependencies: [.package(url: "https://example.com/x.git", from: "1.0.0")],
    targets: [
        .target(
            name: "App",
            dependencies: [.product(name: "X", package: "x")]
        ),
        .executableTarget(name: "demo-cli", dependencies: ["App"]),
        .target(name: "Core", path: "Lib/Core/"),
        .testTarget(name: "AppTests", dependencies: ["App"]),
        .testTarget(name: "CoreSpecs", dependencies: ["Core"], path: "./Specs/Core"),
    ]
)
"#;

    #[test]
    fn targets_default_to_sources_and_tests_directories() {
        let t = parse_package_swift(MANIFEST);
        let find = |n: &str| t.iter().find(|t| t.name == n).expect(n);
        assert_eq!(find("App").path, "Sources/App");
        assert!(!find("App").is_test);
        assert_eq!(find("demo-cli").path, "Sources/demo-cli");
        assert_eq!(find("AppTests").path, "Tests/AppTests");
        assert!(find("AppTests").is_test);
    }

    #[test]
    fn an_explicit_path_overrides_the_default_and_is_normalized() {
        let t = parse_package_swift(MANIFEST);
        let find = |n: &str| t.iter().find(|t| t.name == n).expect(n);
        assert_eq!(find("Core").path, "Lib/Core");
        assert_eq!(find("CoreSpecs").path, "Specs/Core");
        assert!(find("CoreSpecs").is_test);
    }

    #[test]
    fn a_product_is_not_a_target() {
        let t = parse_package_swift(MANIFEST);
        assert_eq!(t.len(), 5, "{t:?}");
        assert!(!t.iter().any(|t| t.name == "X"));
    }

    #[test]
    fn discovery_maps_files_to_their_swiftpm_target() {
        let d = TempDir::new().expect("tempdir");
        write(d.path(), "pkg/Package.swift", MANIFEST);
        let m = UnitMap::discover(d.path());

        let ctx = m.context_for("pkg/Sources/App/Overlay/NPCOverlay.swift");
        assert_eq!(ctx.id, "swift:App");
        assert_eq!(ctx.module_base, "pkg/Sources/App");
        assert!(!ctx.test_target);
        assert_eq!(
            ctx.siblings.get("AppTests").map(String::as_str),
            Some("swift:AppTests")
        );
        assert!(!ctx.siblings.contains_key("App"), "{:?}", ctx.siblings);

        let tests = m.context_for("pkg/Tests/AppTests/Helpers.swift");
        assert_eq!(tests.id, "swift:AppTests");
        assert!(tests.test_target);
        assert_eq!(
            tests.siblings.get("App").map(String::as_str),
            Some("swift:App")
        );

        let spec = m.context_for("pkg/Specs/Core/Thing.swift");
        assert_eq!(spec.id, "swift:CoreSpecs");
        assert!(spec.test_target);

        let stray = m.context_for("pkg/Scripts/tool.swift");
        assert_eq!(stray, UnitContext::unnamed_for(LANG_SWIFT));
    }

    #[test]
    fn a_target_prefix_does_not_capture_a_longer_sibling() {
        // `Sources/App` must not claim `Sources/AppKitShim/...`.
        let d = TempDir::new().expect("tempdir");
        write(
            d.path(),
            "Package.swift",
            ".target(name: \"App\"),\n.target(name: \"AppKitShim\"),\n",
        );
        let m = UnitMap::discover(d.path());
        assert_eq!(
            m.context_for("Sources/AppKitShim/A.swift").id,
            "swift:AppKitShim"
        );
        assert_eq!(m.context_for("Sources/App/A.swift").id, "swift:App");
    }
}

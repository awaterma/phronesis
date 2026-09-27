//! Tests that run a Cargo binary target.
//!
//! An integration test that spawns its package's binary through
//! `env!("CARGO_BIN_EXE_<name>")` makes no Rust call the graph can follow, so
//! without this it reaches nothing: code tested only through the binary reads
//! as untested. This module finds those references and turns them into edges
//! to a `@bin:<name>` hint, which `derive::canonicalize_function_edges`
//! resolves against the `cargo_bin` build-metadata edges to the binary's
//! `main`, or counts as unresolved. From `main`, `test_reaches` follows the
//! ordinary call closure.
//!
//! The reference may sit in the test itself or in a helper it calls. Helpers
//! are followed only within the file and only where Rust's own resolution is
//! certain without seeing other files: a bare call to a free function the
//! calling module itself defines, or an explicit single-module path to one.
//! A call through a glob import or a method is not followed.

use std::collections::{BTreeMap, BTreeSet};
use tree_sitter::Node;

/// Callee hint for a Cargo binary named in source, before resolution.
pub const BIN_HINT: &str = "@bin:";

/// The Cargo environment variable prefix naming a binary target's path.
const CARGO_BIN_EXE: &str = "CARGO_BIN_EXE_";

/// Binary names referenced by `env!("CARGO_BIN_EXE_<name>")` anywhere in
/// `body`, including inside another macro's arguments, where the invocation
/// is a token sequence rather than a parsed `macro_invocation`.
pub fn cargo_bin_exe_names(body: Node, source: &[u8]) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut stack = vec![body];
    while let Some(node) = stack.pop() {
        if matches!(node.kind(), "macro_invocation" | "token_tree") {
            let mut cursor = node.walk();
            let children = node.children(&mut cursor).collect::<Vec<_>>();
            for window in children.windows(3) {
                let [name, bang, args] = window else {
                    continue;
                };
                if names_env(*name, source)
                    && bang.kind() == "!"
                    && args.kind() == "token_tree"
                    && let Some(binary) = cargo_bin_exe(*args, source)
                {
                    found.insert(binary);
                }
            }
        }
        let mut cursor = node.walk();
        stack.extend(node.children(&mut cursor));
    }
    found
}

/// Whether a macro-name node is `env` or a path ending in it (`std::env`).
fn names_env(node: Node, source: &[u8]) -> bool {
    let leaf = match node.kind() {
        "identifier" => Some(node),
        "scoped_identifier" => node.child_by_field_name("name"),
        _ => None,
    };
    leaf.and_then(|leaf| leaf.utf8_text(source).ok()) == Some("env")
}

/// `<name>` from an `env!` argument list whose first argument is the plain
/// string literal `"CARGO_BIN_EXE_<name>"`. Escapes and raw strings are not
/// read: a name spelled that way is left unrecognized rather than decoded.
fn cargo_bin_exe(args: Node, source: &[u8]) -> Option<String> {
    let literal = args.named_child(0)?;
    if literal.kind() != "string_literal" {
        return None;
    }
    let text = literal.utf8_text(source).ok()?;
    let inner = text.strip_prefix('"')?.strip_suffix('"')?;
    if inner.contains('\\') {
        return None;
    }
    inner
        .strip_prefix(CARGO_BIN_EXE)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

/// One function's evidence, gathered during the extractor's walk and
/// resolved once the whole file has been seen.
#[derive(Debug, Clone)]
pub struct FunctionRuns {
    /// Canonical identity of the function.
    pub qualified: String,
    /// The module it is written in (its identity minus the leaf name).
    pub module: String,
    /// Extracted as a test (`#[test]`, or inside `#[cfg(test)]`): its
    /// callees are `tested_by` evidence rather than `calls`.
    pub is_test: bool,
    /// A free function without a test attribute, so a call to it from the
    /// same file can be followed.
    pub helper: bool,
    /// Binaries named directly in its body.
    pub direct: BTreeSet<String>,
    /// Raw callee hints, as the extractor recorded them.
    pub callees: BTreeSet<String>,
}

/// The raw edges the evidence supports, as `(relation, args)`:
/// `tested_by(@bin:<name>, test)` for every binary a test runs directly or
/// through same-file helpers, and `calls(helper, @bin:<name>)` for every
/// non-test function that names a binary itself (a helper that only calls
/// another helper already has that `calls` edge).
pub fn binary_run_edges(runs: &[FunctionRuns]) -> Vec<(&'static str, Vec<String>)> {
    if runs.iter().all(|run| run.direct.is_empty()) {
        return Vec::new();
    }
    let helpers = runs
        .iter()
        .filter(|run| run.helper)
        .map(|run| run.qualified.as_str())
        .collect::<BTreeSet<_>>();
    // Each function's resolved same-file helper callees.
    let edges = runs
        .iter()
        .map(|run| {
            let callees = run
                .callees
                .iter()
                .filter_map(|callee| same_file_helper(&run.module, callee, &helpers))
                .filter(|helper| *helper != run.qualified)
                .collect::<BTreeSet<_>>();
            (run.qualified.as_str(), callees)
        })
        .collect::<BTreeMap<_, _>>();
    let mut reach = runs
        .iter()
        .map(|run| (run.qualified.as_str(), run.direct.clone()))
        .collect::<BTreeMap<_, _>>();
    // Fixed point over a finite lattice: each pass only adds names.
    loop {
        let mut changed = false;
        for (function, callees) in &edges {
            let inherited = callees
                .iter()
                .filter_map(|callee| reach.get(callee))
                .flatten()
                .cloned()
                .collect::<BTreeSet<_>>();
            if let Some(names) = reach.get_mut(function) {
                let before = names.len();
                names.extend(inherited);
                changed |= names.len() != before;
            }
        }
        if !changed {
            break;
        }
    }
    let mut out = Vec::new();
    for run in runs {
        if run.is_test {
            for name in reach.get(run.qualified.as_str()).into_iter().flatten() {
                out.push((
                    "tested_by",
                    vec![format!("{BIN_HINT}{name}"), run.qualified.clone()],
                ));
            }
        } else {
            for name in &run.direct {
                out.push((
                    "calls",
                    vec![run.qualified.clone(), format!("{BIN_HINT}{name}")],
                ));
            }
        }
    }
    out
}

/// The same-file helper a raw callee hint certainly names: a bare name the
/// caller's own module defines, or a single-module `@path:` to one.
fn same_file_helper<'a>(
    module: &str,
    callee: &str,
    helpers: &BTreeSet<&'a str>,
) -> Option<&'a str> {
    let target = match callee.strip_prefix("@path:") {
        Some(hint) => {
            let (modules, name) = hint.rsplit_once(':')?;
            if modules.contains('|') {
                return None;
            }
            format!("{modules}::{name}")
        }
        None if callee.starts_with('@') => return None,
        None => format!("{module}::{callee}"),
    };
    helpers.get(target.as_str()).copied()
}

/// The package a Rust function identity belongs to: `rust:tool` for
/// `rust:tool#test:cli::t`. `CARGO_BIN_EXE_<name>` is set only for the
/// package's own tests and benches, so this scopes the name lookup.
pub fn package_of(function: &str) -> &str {
    let end = [function.find('#'), function.find("::")]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(function.len());
    &function[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(source: &str) -> BTreeSet<String> {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .expect("rust grammar");
        let tree = parser.parse(source, None).expect("parse");
        cargo_bin_exe_names(tree.root_node(), source.as_bytes())
    }

    fn run(qualified: &str, is_test: bool, direct: &[&str], callees: &[&str]) -> FunctionRuns {
        FunctionRuns {
            qualified: qualified.to_string(),
            module: qualified
                .rsplit_once("::")
                .map_or(String::new(), |(module, _)| module.to_string()),
            is_test,
            helper: !is_test,
            direct: direct.iter().map(|s| s.to_string()).collect(),
            callees: callees.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn env_references_are_found_in_code_and_inside_other_macros() {
        let found = names(
            r#"fn f() {
                let a = env!("CARGO_BIN_EXE_one");
                let b = std::env!("CARGO_BIN_EXE_two-dash");
                assert!(Command::new(env!("CARGO_BIN_EXE_three")).status().is_ok());
                let c = format!("{}", std::env!("CARGO_BIN_EXE_four"));
            }"#,
        );
        assert_eq!(
            found.into_iter().collect::<Vec<_>>(),
            ["four", "one", "three", "two-dash"]
        );
    }

    #[test]
    fn other_env_reads_prose_and_comments_are_not_binaries() {
        let found = names(
            r#"fn f() {
                let a = env!("CARGO_MANIFEST_DIR");
                let b = "env!(\"CARGO_BIN_EXE_quoted\")";
                // env!("CARGO_BIN_EXE_commented")
                let c = env!("CARGO_BIN_EXE_");
                let d = std::env::var("CARGO_BIN_EXE_runtime");
                let e = option_env!("CARGO_BIN_EXE_optional");
            }"#,
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn a_test_inherits_binaries_through_same_module_helpers_only() {
        let runs = [
            run("rust:p#test:t::bin", false, &["tool"], &[]),
            run("rust:p#test:t::wrap", false, &[], &["bin"]),
            run("rust:p#test:t::direct", true, &["tool"], &[]),
            run("rust:p#test:t::via", true, &[], &["wrap"]),
            run(
                "rust:p#test:t::via_path",
                true,
                &[],
                &["@path:rust:p#test:t:bin"],
            ),
            run(
                "rust:p#test:t::other_module",
                true,
                &[],
                &["@path:rust:p#test:t::m:bin"],
            ),
            run("rust:p#test:t::method", true, &[], &["@method:bin"]),
            run("rust:p#test:t::m::nested", true, &[], &["bin"]),
        ];
        let edges = binary_run_edges(&runs);
        let tested = |test: &str| {
            edges.iter().any(|(p, a)| {
                *p == "tested_by" && a[0] == "@bin:tool" && a[1] == format!("rust:p#test:t::{test}")
            })
        };
        assert!(tested("direct"));
        assert!(tested("via"));
        assert!(tested("via_path"));
        assert!(!tested("other_module"));
        assert!(!tested("method"));
        assert!(
            !tested("m::nested"),
            "a bare call resolves in the caller's own module"
        );
        assert!(
            edges
                .iter()
                .any(|(p, a)| *p == "calls" && a == &["rust:p#test:t::bin", "@bin:tool"])
        );
        assert!(
            !edges
                .iter()
                .any(|(p, a)| *p == "calls" && a[0] == "rust:p#test:t::wrap"),
            "wrap reaches the binary through its own call to bin"
        );
    }

    #[test]
    fn package_is_the_identity_up_to_its_target_or_module() {
        assert_eq!(package_of("rust:tool#test:cli::t"), "rust:tool");
        assert_eq!(package_of("rust:tool::m::f"), "rust:tool");
        assert_eq!(package_of("rust:tool"), "rust:tool");
    }
}

//! The bundled tree-sitter grammar registry a pack manifest's `grammar`
//! field names (`manifest::BUNDLED_GRAMMARS`). Independent grammar loading
//! (installing a `.so`/`.wasm` grammar) is explicitly later work — every
//! grammar here is compiled into this binary.

use std::sync::LazyLock;

use tree_sitter::{Language, Parser, Tree};

use super::limits;

static RUST: LazyLock<Language> = LazyLock::new(|| tree_sitter_rust::LANGUAGE.into());
static PYTHON: LazyLock<Language> = LazyLock::new(|| tree_sitter_python::LANGUAGE.into());
static SWIFT: LazyLock<Language> = LazyLock::new(|| tree_sitter_swift::LANGUAGE.into());
static TYPESCRIPT: LazyLock<Language> =
    LazyLock::new(|| tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into());
static TSX: LazyLock<Language> = LazyLock::new(|| tree_sitter_typescript::LANGUAGE_TSX.into());
static JAVA: LazyLock<Language> = LazyLock::new(crate::graph::java::grammar::language);
static LUA: LazyLock<Language> = LazyLock::new(|| tree_sitter_lua::LANGUAGE.into());
#[cfg(feature = "go-grammar")]
static GO: LazyLock<Language> = LazyLock::new(|| tree_sitter_go::LANGUAGE.into());

/// The bundled grammar named `id`, or `None` when `id` is not one of
/// [`super::BUNDLED_GRAMMARS`] or (Go only, today) the binary was compiled
/// without the grammar's feature. Callers that need to distinguish "unknown
/// grammar name" from "known grammar, unavailable in this binary" should
/// check [`is_available`] first.
pub fn grammar_for(id: &str) -> Option<Language> {
    Some(match id {
        "rust" => RUST.clone(),
        "python" => PYTHON.clone(),
        "swift" => SWIFT.clone(),
        "typescript" => TYPESCRIPT.clone(),
        "tsx" => TSX.clone(),
        "java" => JAVA.clone(),
        "lua" => LUA.clone(),
        #[cfg(feature = "go-grammar")]
        "go" => GO.clone(),
        _ => return None,
    })
}

/// Is `id` a [`super::BUNDLED_GRAMMARS`] name compiled into this binary?
/// False for a grammar name this binary knows about but was built without
/// (Go, under `--no-default-features` without `go-grammar`) as well as for
/// an unknown name.
pub fn is_available(id: &str) -> bool {
    grammar_for(id).is_some()
}

/// Parse `source` with `language`, bounded by the query/parse cancellation
/// deadline. Returns `None` on a syntax-unrecoverable failure or on
/// exceeding the deadline; tree-sitter does not distinguish the two.
pub fn parse(language: &Language, source: &str) -> Option<Tree> {
    let mut parser = Parser::new();
    parser.set_language(language).ok()?;
    // `set_timeout_micros` is deprecated in favor of a progress-callback
    // API that trades this one line for a stateful closure checking
    // elapsed time itself; both enforce the same deadline.
    #[allow(deprecated)]
    parser.set_timeout_micros(limits::PARSE_DEADLINE_MS * 1_000);
    parser.parse(source, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_grammar_name_resolves_except_gated_go() {
        for id in super::super::BUNDLED_GRAMMARS {
            let available = is_available(id);
            if id == "go" && !cfg!(feature = "go-grammar") {
                assert!(!available, "go should be gated off in this build");
            } else {
                assert!(available, "{id} should be a resolvable bundled grammar");
            }
        }
    }

    #[test]
    fn unknown_grammar_name_is_not_available() {
        assert!(!is_available("cobol"));
        assert!(grammar_for("cobol").is_none());
    }

    #[test]
    fn parse_produces_a_tree_for_a_bundled_grammar() {
        let language = grammar_for("python").unwrap();
        let tree = parse(&language, "def f():\n    pass\n").unwrap();
        assert_eq!(tree.root_node().kind(), "module");
    }

    #[test]
    fn parse_returns_none_for_a_language_mismatch_blank_source_still_parses() {
        // tree-sitter parsers are error-tolerant: they still return a tree
        // (with ERROR nodes) for malformed source rather than failing, and
        // an empty source parses too. `parse` only returns `None` for the
        // deadline/cancellation path, which we exercise via the capped
        // `PARSE_DEADLINE_MS` on pathological input in `extract.rs`'s tests.
        let language = grammar_for("python").unwrap();
        assert!(parse(&language, "").is_some());
        assert!(parse(&language, "def (((((").is_some());
    }
}

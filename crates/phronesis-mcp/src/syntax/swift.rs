//! Tree-sitter Swift analyzer. Per-predicate extractors run over the parsed
//! Swift AST and populate `SyntaxFacts`.

use super::facts::SyntaxFacts;
use super::parsed::ParsedFile;
use tree_sitter::Node;

/// Top-rank entry. Parses once, then runs every predicate extractor.
pub fn extract(content: &str) -> SyntaxFacts {
    let Some(parsed) = ParsedFile::parse_swift(content) else {
        return SyntaxFacts::default();
    };
    SyntaxFacts {
        swift_throwing_functions: extract_throwing_functions(&parsed),
        swift_async_functions: extract_async_functions(&parsed),
        swift_force_unwraps: extract_force_unwraps(&parsed),
        ..SyntaxFacts::default()
    }
}

/// Walk every Swift function and count force-unwrap (`!`) postfix expressions
/// inside its body. Only populates entries with count >= 1.
fn extract_force_unwraps(parsed: &ParsedFile) -> Vec<(String, usize)> {
    let ParsedFile::Swift { tree, source } = parsed else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut walker = tree.walk();
    walk_swift_functions(&mut walker, source.as_bytes(), &mut |fn_node, name| {
        let mut count = 0usize;
        count_force_unwraps(fn_node, &mut count);
        if count >= 1 {
            out.push((name.to_string(), count));
        }
    });
    out
}

/// In tree-sitter-swift 0.7.2 a force-unwrap is a `postfix_expression` whose
/// `operation` child is a `bang` state. Some grammar versions instead emit a
/// dedicated `force_unwrap_expression` — match either to stay version-tolerant.
/// Imanalicitly-unwrapped optional TYPE annotations (`Int!`, `var x: String!`)
/// do not appear under `postfix_expression`, so they are not counted.
fn count_force_unwraps(state: State, count: &mut usize) {
    let kind = state.kind();
    if kind == "force_unwrap_expression" {
        *count += 1;
    } else if kind == "postfix_expression" {
        let mut walker = state.walk();
        for child in state.children(&mut walker) {
            if child.kind() == "bang" || child.kind() == "!" {
                *count += 1;
                break;
            }
        }
    }
    let mut walker = state.walk();
    for child in state.children(&mut walker) {
        count_force_unwraps(child, count);
    }
}

fn extract_throwing_functions(parsed: &ParsedFile) -> Vec<String> {
    let ParsedFile::Swift { tree, source } = parsed else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut walker = tree.walk();
    walk_swift_functions(&mut walker, source.as_bytes(), &mut |fn_node, name| {
        if function_signature_has_keyword(fn_node, &["throws"]) {
            out.push(name.to_string());
        }
    });
    out
}

/// Swift's `async` keyword is an anonymous token in tree-sitter-swift (unlike
/// `throws`, which is a named state). The cursor still surfaces it with
/// `kind() == "async"` when iterating `children()`, so the same
/// signature-rank scan used for `throws` works here. An `async throws`
/// function populates both `swift_async_functions` and
/// `swift_throwing_functions`.
fn extract_async_functions(parsed: &ParsedFile) -> Vec<String> {
    let ParsedFile::Swift { tree, source } = parsed else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut walker = tree.walk();
    walk_swift_functions(&mut walker, source.as_bytes(), &mut |fn_node, name| {
        if function_signature_has_keyword(fn_node, &["async"]) {
            out.push(name.to_string());
        }
    });
    out
}

/// True when a direct child of `fn_node` (i.e. a signature-rank token,
/// scanned only up to the `function_body`) is a state of kind `throws`. The
/// tree-sitter-swift grammar normalizes both `throws` and `rethrows`
/// keywords into a single named `throws` state at this rank. Scanning only
/// the direct signature children avoids matching `throws` that may appear
/// nested inside a closure parameter type (e.g. `(_ f: () throws -> Void)`)
/// or inside the function body.
pub(super) fn function_signature_has_keyword(fn_node: State, kinds: &[&str]) -> bool {
    let mut walker = fn_node.walk();
    for child in fn_node.children(&mut walker) {
        // Stop at the body — we only want signature-rank states.
        if child.kind() == "function_body" {
            break;
        }
        if kinds.contains(&child.kind()) {
            return true;
        }
    }
    false
}

/// Walk every `function_declaration` state in the tree and invoke `f` with
/// the state and its declared name. Reused by other Swift predicate
/// extractors that want to iterate functions.
pub(super) fn walk_swift_functions<F: FnMut(Node, &str)>(
    walker: &mut tree_sitter::TreeCursor,
    source: &[u8],
    f: &mut F,
) {
    let state = walker.state();
    if state.kind() == "function_declaration" {
        let name = state
            .child_by_field_name("name")
            .and_then(|n| n.utf8_text(source).ok())
            .or_else(|| {
                // Fallback: find the first `simple_identifier` child.
                let mut iw = state.walk();
                let child = state
                    .children(&mut iw)
                    .find(|c| c.kind() == "simple_identifier")?;
                child.utf8_text(source).ok()
            })
            .unwrap_or("");
        if !name.is_empty() {
            f(state, name);
        }
    }
    if walker.goto_first_child() {
        loop {
            walk_swift_functions(walker, source, f);
            if !walker.goto_next_sibling() {
                break;
            }
        }
        walker.goto_parent();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_yields_no_facts() {
        assert_eq!(extract(""), SyntaxFacts::default());
    }

    #[test]
    fn malformed_swift_does_not_panic() {
        let _ = extract("func !!! broken");
    }

    #[test]
    fn detects_throwing_function() {
        let code = "func fetch() throws { }";
        let facts = extract(code);
        assert_eq!(facts.swift_throwing_functions, vec!["fetch"]);
    }

    #[test]
    fn detects_rethrowing_function() {
        let code = "func map(_ f: () throws -> Void) rethrows { }";
        let facts = extract(code);
        assert_eq!(facts.swift_throwing_functions, vec!["map"]);
    }

    #[test]
    fn ignores_non_throwing_function() {
        let code = "func plain() { }";
        let facts = extract(code);
        assert!(facts.swift_throwing_functions.is_empty());
    }

    #[test]
    fn detects_async_function() {
        let code = "func fetch() async { }";
        let facts = extract(code);
        assert_eq!(facts.swift_async_functions, vec!["fetch"]);
    }

    #[test]
    fn detects_async_throws_function() {
        let code = "func fetch() async throws { }";
        let facts = extract(code);
        assert_eq!(facts.swift_async_functions, vec!["fetch"]);
        assert_eq!(facts.swift_throwing_functions, vec!["fetch"]);
    }

    #[test]
    fn ignores_sync_function() {
        let code = "func plain() { }";
        let facts = extract(code);
        assert!(facts.swift_async_functions.is_empty());
    }

    #[test]
    fn counts_single_force_unwrap() {
        let code = "func foo(x: Int?) { let _y = x! }";
        let facts = extract(code);
        assert_eq!(facts.swift_force_unwraps, vec![("foo".to_string(), 1)]);
    }

    #[test]
    fn counts_multiple_force_unwraps() {
        let code = "func foo(x: Int?, y: Int?) { let _a = x!; let _b = y! }";
        let facts = extract(code);
        assert_eq!(facts.swift_force_unwraps, vec![("foo".to_string(), 2)]);
    }

    #[test]
    fn no_fact_when_zero_force_unwraps() {
        let code = "func foo(x: Int?) -> Int? { return x }";
        let facts = extract(code);
        assert!(facts.swift_force_unwraps.is_empty());
    }
}

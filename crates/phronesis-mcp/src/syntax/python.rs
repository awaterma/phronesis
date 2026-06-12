//! Tree-sitter Python analyzer. Per-predicate extractors run over the parsed
//! Python AST and populate `SyntaxFacts`. Code outside any `def` is reported
//! under the pseudo-function name `<module>`.

use super::facts::SyntaxFacts;
use super::parsed::ParsedFile;
use tree_sitter::Node;

/// Parameter-count threshold: a `def` with this many parameters or more is
/// flagged. `self` / `cls` first parameters are not counted.
const PARAM_COUNT_THRESHOLD: usize = 6;

/// Top-level entry. Parses once, then runs every predicate extractor.
pub fn extract(content: &str) -> SyntaxFacts {
    let Some(parsed) = ParsedFile::parse_python(content) else {
        return SyntaxFacts::default();
    };
    let ParsedFile::Python { tree, source } = &parsed else {
        return SyntaxFacts::default();
    };
    let root = tree.root_node();
    let src = source.as_bytes();

    SyntaxFacts {
        python_bare_excepts: extract_bare_excepts(root, src),
        python_mutable_default_args: extract_mutable_defaults(root, src),
        python_function_param_counts_high: extract_param_counts_high(root, src),
        python_functions_missing_docstring: extract_missing_docstrings(root, src),
        ..SyntaxFacts::default()
    }
}

/// Name of the nearest enclosing `function_definition`, or `<module>`.
fn enclosing_function_name(node: Node, src: &[u8]) -> String {
    let mut cur = node;
    while let Some(parent) = cur.parent() {
        if parent.kind() == "function_definition"
            && let Some(name) = parent.child_by_field_name("name")
            && let Ok(text) = name.utf8_text(src)
        {
            return text.to_string();
        }
        cur = parent;
    }
    "<module>".to_string()
}

fn walk<'a>(root: Node<'a>, f: &mut dyn FnMut(Node<'a>)) {
    let mut walker = root.walk();
    let mut reached_root = false;
    while !reached_root {
        f(walker.node());
        if walker.goto_first_child() {
            continue;
        }
        loop {
            if walker.goto_next_sibling() {
                break;
            }
            if !walker.goto_parent() {
                reached_root = true;
                break;
            }
        }
    }
}

/// `except:` with no exception type — swallows everything including
/// KeyboardInterrupt/SystemExit. A bare `except_clause` has no child between
/// the `except` keyword and the `:`.
fn extract_bare_excepts(root: Node, src: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    walk(root, &mut |node| {
        if node.kind() != "except_clause" {
            return;
        }
        // Children of a typed clause include an expression (identifier,
        // attribute, tuple, as_pattern, ...) before the colon; a bare
        // clause has only the keyword, the colon, and the block.
        let mut walker = node.walk();
        let has_filter = node
            .children(&mut walker)
            .any(|c| !matches!(c.kind(), "except" | ":" | "block" | "comment"));
        if !has_filter {
            out.push(enclosing_function_name(node, src));
        }
    });
    out
}

/// `def f(x=[])` / `def f(x={})` / `def f(x=set())` — the default is created
/// once at def time and shared across calls.
fn extract_mutable_defaults(root: Node, src: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    walk(root, &mut |node| {
        if !matches!(node.kind(), "default_parameter" | "typed_default_parameter") {
            return;
        }
        let Some(value) = node.child_by_field_name("value") else {
            return;
        };
        let mutable = match value.kind() {
            "list" | "dictionary" | "set" => true,
            "call" => value
                .child_by_field_name("function")
                .and_then(|f| f.utf8_text(src).ok())
                .is_some_and(|name| matches!(name, "list" | "dict" | "set")),
            _ => false,
        };
        if mutable
            && let Some(name) = node.child_by_field_name("name")
            && let Ok(param) = name.utf8_text(src)
        {
            let fn_name = enclosing_function_name(node, src);
            out.push((fn_name, param.to_string()));
        }
    });
    out
}

fn extract_param_counts_high(root: Node, src: &[u8]) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    walk(root, &mut |node| {
        if node.kind() != "function_definition" {
            return;
        }
        let Some(params) = node.child_by_field_name("parameters") else {
            return;
        };
        let mut walker = params.walk();
        let mut count = 0usize;
        for (idx, child) in params
            .children(&mut walker)
            .filter(|c| {
                matches!(
                    c.kind(),
                    "identifier"
                        | "typed_parameter"
                        | "default_parameter"
                        | "typed_default_parameter"
                )
            })
            .enumerate()
        {
            // Skip a leading self/cls receiver.
            if idx == 0
                && child
                    .utf8_text(src)
                    .is_ok_and(|t| t == "self" || t == "cls" || t.starts_with("self:"))
            {
                continue;
            }
            count += 1;
        }
        if count >= PARAM_COUNT_THRESHOLD
            && let Some(name) = node.child_by_field_name("name")
            && let Ok(fn_name) = name.utf8_text(src)
        {
            out.push((fn_name.to_string(), count));
        }
    });
    out
}

/// Public `def`s (name not starting with `_`) whose body does not begin
/// with a docstring.
fn extract_missing_docstrings(root: Node, src: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    walk(root, &mut |node| {
        if node.kind() != "function_definition" {
            return;
        }
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };
        let Ok(fn_name) = name.utf8_text(src) else {
            return;
        };
        if fn_name.starts_with('_') {
            return;
        }
        let Some(body) = node.child_by_field_name("body") else {
            return;
        };
        let mut walker = body.walk();
        let first_stmt = body.children(&mut walker).find(|c| c.kind() != "comment");
        let has_docstring = first_stmt.is_some_and(|stmt| {
            stmt.kind() == "expression_statement"
                && stmt.child(0).is_some_and(|c| c.kind() == "string")
        });
        if !has_docstring {
            out.push(fn_name.to_string());
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_except_is_flagged_with_enclosing_function() {
        let facts = extract(
            "def fetch(url):\n    \"\"\"Fetch.\"\"\"\n    try:\n        go(url)\n    except:\n        pass\n",
        );
        assert_eq!(facts.python_bare_excepts, vec!["fetch".to_string()]);
    }

    #[test]
    fn typed_except_is_not_flagged() {
        let facts = extract(
            "def fetch(url):\n    \"\"\"Fetch.\"\"\"\n    try:\n        go(url)\n    except ValueError as e:\n        raise e\n",
        );
        assert!(facts.python_bare_excepts.is_empty());
    }

    #[test]
    fn module_level_bare_except_reports_module() {
        let facts = extract("try:\n    setup()\nexcept:\n    pass\n");
        assert_eq!(facts.python_bare_excepts, vec!["<module>".to_string()]);
    }

    #[test]
    fn mutable_default_list_and_dict_call_are_flagged() {
        let facts = extract(
            "def add(item, bucket=[]):\n    \"\"\"Add.\"\"\"\n    bucket.append(item)\n\ndef make(opts=dict()):\n    \"\"\"Make.\"\"\"\n    return opts\n",
        );
        assert_eq!(
            facts.python_mutable_default_args,
            vec![
                ("add".to_string(), "bucket".to_string()),
                ("make".to_string(), "opts".to_string())
            ]
        );
    }

    #[test]
    fn immutable_defaults_are_fine() {
        let facts = extract(
            "def add(item, n=0, name=\"x\", flag=None):\n    \"\"\"Add.\"\"\"\n    return n\n",
        );
        assert!(facts.python_mutable_default_args.is_empty());
    }

    #[test]
    fn param_count_threshold_skips_self() {
        let facts = extract(
            "class C:\n    def m(self, a, b, c, d, e, f):\n        \"\"\"M.\"\"\"\n        return a\n",
        );
        assert_eq!(
            facts.python_function_param_counts_high,
            vec![("m".to_string(), 6)]
        );

        let under = extract(
            "class C:\n    def m(self, a, b, c, d, e):\n        \"\"\"M.\"\"\"\n        return a\n",
        );
        assert!(under.python_function_param_counts_high.is_empty());
    }

    #[test]
    fn missing_docstring_only_for_public_defs() {
        let facts = extract(
            "def documented():\n    \"\"\"Doc.\"\"\"\n    return 1\n\ndef naked():\n    return 2\n\ndef _private():\n    return 3\n",
        );
        assert_eq!(
            facts.python_functions_missing_docstring,
            vec!["naked".to_string()]
        );
    }
}

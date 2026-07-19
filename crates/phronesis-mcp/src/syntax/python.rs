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
        python_print_calls: extract_print_calls(root, src),
        python_call_in_default_args: extract_call_in_default_args(root, src),
        python_exception_handler_passes: extract_handler_passes(root, src),
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
                        | "list_splat_pattern"
                        | "dictionary_splat_pattern"
                )
            })
            .enumerate()
        {
            // Skip a leading self/cls receiver.
            if idx == 0
                && child.utf8_text(src).is_ok_and(|t| {
                    t == "self" || t == "cls" || t.starts_with("self:") || t.starts_with("cls:")
                })
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

/// Recognize a call whose callee is the bare identifier `print`.
/// Excludes: `x.print()` (attribute access), `sprint()` (non-exact name),
/// comments, and string literals.
/// Rationale: Python logging HOWTO, <https://docs.python.org/3/howto/logging.html>.
fn extract_print_calls(root: Node, src: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    walk(root, &mut |node| {
        if node.kind() != "call" {
            return;
        }
        let Some(func) = node.child_by_field_name("function") else {
            return;
        };
        // Must be an identifier named exactly "print" (not attribute, not prefix).
        if func.kind() == "identifier"
            && let Ok(name) = func.utf8_text(src)
            && name == "print"
        {
            out.push(enclosing_function_name(node, src));
        }
    });
    out
}

/// `def f(x=some())` — default argument whose value is a call expression.
/// Records (fn_name, param_name, callee_name). Immutable constructors
/// (list, dict, set) are also included so projects can selectively ignore
/// them; the distinction is visible in the callee field.
/// Upstream: Bugbear B008, <https://docs.astral.sh/ruff/rules/function-call-in-default-argument/>.
fn extract_call_in_default_args(root: Node, src: &[u8]) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    walk(root, &mut |node| {
        if !matches!(node.kind(), "default_parameter" | "typed_default_parameter") {
            return;
        }
        let Some(value) = node.child_by_field_name("value") else {
            return;
        };
        if value.kind() != "call" {
            return;
        }
        let Some(func) = value.child_by_field_name("function") else {
            return;
        };
        let Ok(callee) = func.utf8_text(src) else {
            return;
        };
        if let Some(name) = node.child_by_field_name("name")
            && let Ok(param) = name.utf8_text(src)
        {
            let fn_name = enclosing_function_name(node, src);
            out.push((fn_name, param.to_string(), callee.to_string()));
        }
    });
    out
}

/// Typed exception handlers whose body is only `pass`, comments, or
/// ellipsis (`...`). Excludes bare handlers (those are caught by the
/// bare-except rule). The effective body check: strip comments and
/// ellipsis expressions; if nothing remains, the handler swallows.
/// Upstream: Bugbear B110, <https://docs.astral.sh/ruff/rules/try-except-pass/>;
/// this predicate is narrower because it only reports typed handlers.
fn extract_handler_passes(root: Node, src: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    walk(root, &mut |node| {
        if node.kind() != "except_clause" {
            return;
        }
        // Skip bare except clauses — they have no exception type child.
        let has_filter = node
            .children(&mut node.walk())
            .any(|c| !matches!(c.kind(), "except" | ":" | "block" | "comment"));
        if !has_filter {
            return;
        }
        // Typed handler — check the body.
        let Some(block) = node
            .children(&mut node.walk())
            .find(|c| c.kind() == "block")
        else {
            return;
        };
        let is_empty_body = block.children(&mut block.walk()).all(|stmt| {
            matches!(stmt.kind(), "comment" | "pass_statement" | "ellipsis")
                || (stmt.kind() == "expression_statement"
                    && stmt.child(0).is_some_and(|c| c.kind() == "ellipsis"))
        });
        if is_empty_body {
            // Extract the exception type string.
            let exc_type = node
                .children(&mut node.walk())
                .find(|c| !matches!(c.kind(), "except" | ":" | "block" | "comment"))
                .and_then(|c| c.utf8_text(src).ok())
                .unwrap_or("?");
            out.push((enclosing_function_name(node, src), exc_type.to_string()));
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

    // ─── python_print_call tests ───────────────────────────────────

    #[test]
    fn print_call_is_flagged() {
        let facts = extract("def process():\n    \"\"\"Process.\"\"\"\n    print('hello')\n");
        assert_eq!(facts.python_print_calls, vec!["process".to_string()]);
    }

    #[test]
    fn print_call_in_nested_function() {
        let facts = extract(
            "def outer():\n    \"\"\"Outer.\"\"\"\n    def inner():\n        print('hi')\n",
        );
        assert_eq!(facts.python_print_calls, vec!["inner".to_string()]);
    }

    #[test]
    fn print_call_excludes_attribute_access() {
        let facts = extract(
            "def foo():\n    \"\"\"Foo.\"\"\"\n    logger.print('x')\n    printer.print('y')\n",
        );
        assert!(facts.python_print_calls.is_empty());
    }

    #[test]
    fn print_call_excludes_similar_names() {
        let facts = extract(
            "def foo():\n    \"\"\"Foo.\"\"\"\n    sprint('x')\n    printx('y')\n    xprint('z')\n",
        );
        assert!(facts.python_print_calls.is_empty());
    }

    #[test]
    fn print_call_excludes_comments() {
        let facts = extract("def foo():\n    # print('in comment')\n    pass\n");
        assert!(facts.python_print_calls.is_empty());
    }

    #[test]
    fn print_call_excludes_strings() {
        let facts = extract("def foo():\n    x = \"print('in string')\"\n    pass\n");
        assert!(facts.python_print_calls.is_empty());
    }

    #[test]
    fn print_call_multiple_in_one_function() {
        let facts = extract("def foo():\n    \"\"\"Foo.\"\"\"\n    print('a')\n    print('b')\n");
        assert_eq!(facts.python_print_calls.len(), 2);
        assert!(facts.python_print_calls.iter().all(|n| *n == "foo"));
    }

    #[test]
    fn print_call_module_level() {
        let facts = extract("print('module level')\n");
        assert_eq!(facts.python_print_calls, vec!["<module>".to_string()]);
    }

    #[test]
    fn print_call_async_function() {
        let facts = extract("async def fetch():\n    \"\"\"Fetch.\"\"\"\n    print('async')\n");
        assert_eq!(facts.python_print_calls, vec!["fetch".to_string()]);
    }

    #[test]
    fn print_call_in_class() {
        let facts = extract(
            "class C:\n    def method(self):\n        \"\"\"Method.\"\"\"\n        print('x')\n",
        );
        assert_eq!(facts.python_print_calls, vec!["method".to_string()]);
    }

    // ─── python_call_in_default_args tests ─────────────────────────

    #[test]
    fn call_in_default_arg_is_flagged() {
        let facts = extract(
            "def make(default=[]):\n    \"\"\"Make.\"\"\"\n    return default\n\ndef make2(default=list()):\n    \"\"\"Make2.\"\"\"\n    return default\n\ndef make3(f=get_default()):\n    \"\"\"Make3.\"\"\"\n    return f\n",
        );
        // [] is a list literal (not a call), so only make2 and make3 match.
        assert_eq!(
            facts.python_call_in_default_args,
            vec![
                (
                    "make2".to_string(),
                    "default".to_string(),
                    "list".to_string()
                ),
                (
                    "make3".to_string(),
                    "f".to_string(),
                    "get_default".to_string()
                ),
            ]
        );
    }

    #[test]
    fn call_in_default_arg_nested_call() {
        let facts = extract("def make(f=config.read()):\n    \"\"\"Make.\"\"\"\n    return f\n");
        assert_eq!(
            facts.python_call_in_default_args,
            vec![(
                "make".to_string(),
                "f".to_string(),
                "config.read".to_string()
            )]
        );
    }

    #[test]
    fn no_call_in_default_arg() {
        let facts = extract("def make(f=1, g='x', h=None):\n    \"\"\"Make.\"\"\"\n    return f\n");
        assert!(facts.python_call_in_default_args.is_empty());
    }

    #[test]
    fn typed_default_parameter_with_call() {
        let facts = extract("def make(f: list = list()):\n    \"\"\"Make.\"\"\"\n    return f\n");
        assert_eq!(
            facts.python_call_in_default_args,
            vec![("make".to_string(), "f".to_string(), "list".to_string())]
        );
    }

    // ─── python_exception_handler_passes tests ─────────────────────

    #[test]
    fn typed_handler_with_only_pass_is_flagged() {
        let facts = extract(
            "def foo():\n    \"\"\"Foo.\"\"\"\n    try:\n        go()\n    except ValueError:\n        pass\n",
        );
        assert_eq!(
            facts.python_exception_handler_passes,
            vec![("foo".to_string(), "ValueError".to_string())]
        );
    }

    #[test]
    fn typed_handler_with_ellipsis_is_flagged() {
        let facts = extract(
            "def foo():\n    \"\"\"Foo.\"\"\"\n    try:\n        go()\n    except ValueError:\n        ...\n",
        );
        assert_eq!(
            facts.python_exception_handler_passes,
            vec![("foo".to_string(), "ValueError".to_string())]
        );
    }

    #[test]
    fn typed_handler_with_comment_only_is_flagged() {
        let facts = extract(
            "def foo():\n    \"\"\"Foo.\"\"\"\n    try:\n        go()\n    except ValueError:\n        # intentional no-op\n        pass\n",
        );
        assert!(
            facts
                .python_exception_handler_passes
                .iter()
                .any(|(fn_, _)| fn_ == "foo"),
            "expected handler_passes for foo"
        );
    }

    #[test]
    fn typed_handler_with_real_body_is_not_flagged() {
        let facts = extract(
            "def foo():\n    \"\"\"Foo.\"\"\"\n    try:\n        go()\n    except ValueError:\n        log_error(e)\n",
        );
        assert!(facts.python_exception_handler_passes.is_empty());
    }

    #[test]
    fn bare_handler_not_flagged_as_passes() {
        let facts = extract(
            "def foo():\n    \"\"\"Foo.\"\"\"\n    try:\n        go()\n    except:\n        pass\n",
        );
        assert!(facts.python_exception_handler_passes.is_empty());
    }

    #[test]
    fn tuple_exception_type_flagged() {
        let facts = extract(
            "def foo():\n    \"\"\"Foo.\"\"\"\n    try:\n        go()\n    except (ValueError, TypeError):\n        pass\n",
        );
        // The exception type string will be the text of the tuple node.
        assert!(
            facts
                .python_exception_handler_passes
                .iter()
                .any(|(fn_, _)| fn_ == "foo"),
            "expected handler_passes for foo"
        );
    }

    #[test]
    fn print_call_in_string_literal_not_flagged() {
        let facts = extract("def foo():\n    \"\"\"Foo.\"\"\"\n    x = \"print('not a call')\"\n");
        assert!(facts.python_print_calls.is_empty());
    }

    #[test]
    fn malformed_source_partial_except_not_crashed() {
        // tree-sitter is resilient; this shouldn't panic
        let facts = extract("def foo():\n    try:\n        pass\n    except\n");
        // May extract partial facts or empty; should not panic
        let _ = facts.python_bare_excepts;
        let _ = facts.python_print_calls;
    }

    #[test]
    fn positional_only_and_keyword_only_params_counted() {
        let facts = extract("def f(a, /, b, *, c):\n    \"\"\"F.\"\"\"\n    return a\n");
        // a, b, c = 3 params, should not trigger threshold
        assert!(facts.python_function_param_counts_high.is_empty());
    }

    #[test]
    fn varargs_and_kwargs_counted_in_param_threshold() {
        let facts = extract("def f(a, b, c, d, *args, **kwargs):\n    \"\"\"F.\"\"\"\n    pass\n");
        assert_eq!(
            facts.python_function_param_counts_high,
            vec![("f".to_string(), 6)]
        );
    }

    #[test]
    fn cls_not_counted_in_param_threshold() {
        let facts = extract(
            "class C:\n    def m(cls, a, b, c, d, e, f):\n        \"\"\"M.\"\"\"\n        pass\n",
        );
        assert_eq!(
            facts.python_function_param_counts_high,
            vec![("m".to_string(), 6)]
        );
    }
}

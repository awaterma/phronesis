//! Tree-sitter TypeScript analyzer. Per-predicate extractors run over the
//! parsed TS/TSX AST and populate `SyntaxFacts`. Code outside any named
//! function is reported under the pseudo-function name `<module>`.

use super::facts::SyntaxFacts;
use super::parsed::ParsedFile;
use tree_sitter::Node;

/// Parameter-count threshold, matching the Rust convention (5).
const PARAM_COUNT_THRESHOLD: usize = 5;

/// Top-level entry. `tsx` selects the TSX grammar for `.tsx` files.
pub fn extract(content: &str, tsx: bool) -> SyntaxFacts {
    let Some(parsed) = ParsedFile::parse_typescript(content, tsx) else {
        return SyntaxFacts::default();
    };
    let ParsedFile::TypeScript { tree, source } = &parsed else {
        return SyntaxFacts::default();
    };
    let root = tree.root_node();
    let src = source.as_bytes();

    SyntaxFacts {
        ts_explicit_anys: count_per_function(root, src, &mut |n| {
            n.kind() == "predefined_type" && n.utf8_text(src) == Ok("any")
        }),
        ts_non_null_assertions: count_per_function(root, src, &mut |n| {
            n.kind() == "non_null_expression"
        }),
        ts_suppression_comment_count: count_suppression_comments(root, src),
        ts_function_param_counts_high: extract_param_counts_high(root, src),
        ..SyntaxFacts::default()
    }
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

/// Name of the nearest enclosing function-ish state: function_declaration /
/// method_definition (by `name` field), or an arrow function / function
/// expression bound through a `variable_declarator`. Falls back to `<module>`.
fn enclosing_function_name(node: Node, src: &[u8]) -> String {
    let mut cur = node;
    while let Some(parent) = cur.parent() {
        match parent.kind() {
            "function_declaration" | "method_definition" | "generator_function_declaration" => {
                if let Some(name) = parent.child_by_field_name("name")
                    && let Ok(text) = name.utf8_text(src)
                {
                    return text.to_string();
                }
            }
            "arrow_function" | "function_expression" => {
                if let Some(decl) = parent.parent()
                    && decl.kind() == "variable_declarator"
                    && let Some(name) = decl.child_by_field_name("name")
                    && let Ok(text) = name.utf8_text(src)
                {
                    return text.to_string();
                }
            }
            _ => {}
        }
        cur = parent;
    }
    "<module>".to_string()
}

/// Count nodes matching `pred`, grouped by enclosing function name.
fn count_per_function(
    root: Node,
    src: &[u8],
    pred: &mut dyn FnMut(Node) -> bool,
) -> Vec<(String, usize)> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    walk(root, &mut |node| {
        if pred(node) {
            let name = enclosing_function_name(node, src);
            match counts.iter_mut().find(|(n, _)| *n == name) {
                Some((_, c)) => *c += 1,
                None => counts.push((name, 1)),
            }
        }
    });
    counts
}

/// `// @ts-ignore`, `// @ts-expect-error`, and `// @ts-nocheck` comments —
/// each one turns the type checker off somewhere.
fn count_suppression_comments(root: Node, src: &[u8]) -> usize {
    let mut count = 0usize;
    walk(root, &mut |node| {
        if node.kind() == "comment"
            && node.utf8_text(src).is_ok_and(|t| {
                t.contains("@ts-ignore")
                    || t.contains("@ts-expect-error")
                    || t.contains("@ts-nocheck")
            })
        {
            count += 1;
        }
    });
    count
}

fn extract_param_counts_high(root: Node, src: &[u8]) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    walk(root, &mut |node| {
        if !matches!(
            node.kind(),
            "function_declaration" | "method_definition" | "arrow_function" | "function_expression"
        ) {
            return;
        }
        let Some(params) = node.child_by_field_name("parameters") else {
            return;
        };
        let mut walker = params.walk();
        let count = params
            .children(&mut walker)
            .filter(|c| matches!(c.kind(), "required_parameter" | "optional_parameter"))
            .count();
        if count >= PARAM_COUNT_THRESHOLD {
            // Name resolution wants a state *inside* the function.
            let probe = params.child(0).unwrap_or(params);
            out.push((enclosing_function_name(probe, src), count));
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_any_counted_per_function() {
        let facts = extract(
            "function load(a: any, b: string): any { return a; }\nconst top: any = 1;\n",
            false,
        );
        assert_eq!(
            facts.ts_explicit_anys,
            vec![("load".to_string(), 2), ("<module>".to_string(), 1)]
        );
    }

    #[test]
    fn non_null_assertions_counted() {
        let facts = extract(
            "function f(x?: string) { return x!.length + x!.length; }\n",
            false,
        );
        assert_eq!(facts.ts_non_null_assertions, vec![("f".to_string(), 2)]);
    }

    #[test]
    fn suppression_comments_counted() {
        let facts = extract(
            "// @ts-ignore\nconst a = bad();\n// @ts-expect-error\nconst b = alsoBad();\n// plain comment\n",
            false,
        );
        assert_eq!(facts.ts_suppression_comment_count, 2);
    }

    #[test]
    fn param_count_threshold_flags_wide_signatures() {
        let facts = extract(
            "function wide(a: number, b: number, c: number, d: number, e: number) { return a; }\nfunction narrow(a: number, b: number) { return b; }\n",
            false,
        );
        assert_eq!(
            facts.ts_function_param_counts_high,
            vec![("wide".to_string(), 5)]
        );
    }

    #[test]
    fn arrow_function_bound_to_const_gets_its_name() {
        let facts = extract("const handler = (e: any) => e;\n", false);
        assert_eq!(facts.ts_explicit_anys, vec![("handler".to_string(), 1)]);
    }

    #[test]
    fn tsx_parses_with_tsx_grammar() {
        let facts = extract(
            "function App(props: any) { return <div>{props.x}</div>; }\n",
            true,
        );
        assert_eq!(facts.ts_explicit_anys, vec![("App".to_string(), 1)]);
    }
}

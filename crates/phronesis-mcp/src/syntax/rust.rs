//! Tree-sitter Rust analyzer. Each predicate has its own private extractor
//! function taking `&ParsedFile`; `extract()` parses once and runs them all.

use std::sync::LazyLock;

use streaming_iterator::StreamingIterator;
use tree_sitter::{Query, QueryCursor};

use super::facts::SyntaxFacts;
use super::parsed::ParsedFile;

static PUB_FN_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(
        &tree_sitter_rust::LANGUAGE.into(),
        r#"
        (function_item
          (visibility_modifier) @vis
          name: (identifier) @name)
        "#,
    )
    .expect("PUB_FN_QUERY compiles")
});

static FN_QUERY: LazyLock<Query> = LazyLock::new(|| {
    Query::new(
        &tree_sitter_rust::LANGUAGE.into(),
        r#"
        (function_item
          name: (identifier) @name
          return_type: (generic_type
            type: (type_identifier) @return_outer
            type_arguments: (type_arguments) @return_args))
        "#,
    )
    .expect("FN_QUERY compiles")
});

/// Top-rank entry. Parses once, then runs every predicate extractor.
pub fn extract(content: &str) -> SyntaxFacts {
    let Some(parsed) = ParsedFile::parse_rust(content) else {
        return SyntaxFacts::default();
    };
    let function_param_types = extract_function_param_types(&parsed);
    let vec_ref_params = function_param_types
        .iter()
        .filter(|(_, _, ty)| ty.starts_with("&Vec<") || ty.starts_with("&mut Vec<"))
        .map(|(fn_name, param, _)| (fn_name.clone(), param.clone()))
        .collect();
    // Group by fn name; emit only when count meets/exceeds the threshold.
    // Functions with `&self` are not penalized — the param extractor already
    // skips it, matching the spirit of "method with N business params."
    const PARAM_COUNT_THRESHOLD: usize = 5;
    let mut per_fn: std::collections::BTreeMap<String, usize> =
        std::collections::BTreeMap::new();
    for (fn_name, _, _) in &function_param_types {
        *per_fn.entry(fn_name.clone()).or_insert(0) += 1;
    }
    let function_param_counts_high: Vec<(String, usize)> = per_fn
        .into_iter()
        .filter(|(_, c)| *c >= PARAM_COUNT_THRESHOLD)
        .collect();
    SyntaxFacts {
        functions_returning_result_string: extract_result_string_returns(&parsed),
        public_functions: extract_public_functions(&parsed),
        async_functions: extract_async_functions(&parsed),
        function_param_types,
        vec_ref_params,
        function_param_counts_high,
        function_clone_counts: extract_function_clone_counts(&parsed),
        function_clone_counts_high: extract_function_clone_counts_high(&parsed),
        pub_fns_without_doc_comment: extract_pub_fns_without_doc_comment(&parsed),
        tests_without_assertion: extract_tests_without_assertion(&parsed),
        struct_derives: extract_struct_derives(&parsed),
        engine_eval_string_literals: extract_engine_eval_string_literals(&parsed),
        ..SyntaxFacts::default()
    }
}

/// One `(struct, trait)` entry per `#[derive(...)]` argument across the file.
/// Recognizes `derive` attributes attached directly to a `struct_item` as a
/// preceding sibling `attribute_item` under the same parent.
///
/// AST shape (verified against tree-sitter-rust 0.23):
///   `attribute_item -> attribute -> identifier("derive") + arguments: token_tree`
///   `token_tree` contains `identifier` children for each derived trait.
///
/// Grammar caveat: `attribute_item.attribute` and `attribute.path` are regular
/// children, not fields, in tree-sitter-rust 0.23. Only `attribute.arguments`
/// is exposed as a field.
fn extract_struct_derives(parsed: &ParsedFile) -> Vec<(String, String)> {
    let ParsedFile::Rust { tree, source } = parsed else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut walker = tree.walk();
    walk_struct_items(&mut walker, source.as_bytes(), &mut |struct_state, name| {
        // Look for sibling attribute_item states immediately preceding the struct.
        // Attributes are children of the struct's parent state (source_file / mod_item),
        // appearing as siblings before the struct_item.
        let Some(parent) = struct_state.parent() else {
            return;
        };
        let mut parent_walker = parent.walk();
        let mut prev_attrs: Vec<tree_sitter::Node> = Vec::new();
        for child in parent.children(&mut parent_walker) {
            if child.id() == struct_state.id() {
                break;
            }
            if child.kind() == "attribute_item" {
                prev_attrs.push(child);
            } else if matches!(child.kind(), "line_comment" | "block_comment") {
                // Comments don't separate attrs from the item they decorate.
            } else {
                // Anything else (use_declaration, extern_crate_declaration, item kinds,
                // expression_statement wrapping a top-rank macro, etc.) flushes pending
                // attributes. Inverting the policy this way means we don't have to
                // enumerate every kind tree-sitter-rust escoreoses, and remains correct
                // when the grammar grows new item kinds.
                prev_attrs.clear();
            }
        }
        for attr_state in prev_attrs {
            collect_derives_from_attr(attr_state, source.as_bytes(), name, &mut out);
        }
    });
    out
}

/// Walk an `attribute_item` and, if it's a `#[derive(...)]`, push one
/// `(struct_name, trait_name)` per identifier in the token_tree.
fn collect_derives_from_attr(
    attr: tree_sitter::Node,
    source: &[u8],
    struct_name: &str,
    out: &mut Vec<(String, String)>,
) {
    // attribute_item -> attribute (regular child, no field name)
    let mut aw = attr.walk();
    let Some(inner) = attr.children(&mut aw).find(|c| c.kind() == "attribute") else {
        return;
    };
    // The attribute's path is an `identifier` child (or `scoped_identifier`);
    // its arguments are exposed via the `arguments` field.
    let mut is_derive = false;
    let mut iw = inner.walk();
    for child in inner.children(&mut iw) {
        if child.kind() == "identifier" {
            if child.utf8_text(source).unwrap_or("") == "derive" {
                is_derive = true;
            }
            break;
        } else if child.kind() == "scoped_identifier" {
            // e.g. `core::prelude::v1::derive` — last segment is the path tail.
            let txt = child.utf8_text(source).unwrap_or("");
            if txt.rsplit("::").next() == Some("derive") {
                is_derive = true;
            }
            break;
        }
    }
    if !is_derive {
        return;
    }
    let Some(args) = inner.child_by_field_name("arguments") else {
        return;
    };
    if args.kind() != "token_tree" {
        return;
    }
    // Walk the token_tree; identifiers inside it are derived traits.
    let mut tw = args.walk();
    for child in args.children(&mut tw) {
        if child.kind() == "identifier" {
            let trait_name = child.utf8_text(source).unwrap_or("");
            if !trait_name.is_empty() {
                out.push((struct_name.to_string(), trait_name.to_string()));
            }
        }
    }
}

/// Recursively visits every `struct_item` in the tree, calling
/// `f(struct_state, name)` for each. Analogous to `walk_function_items`.
fn walk_struct_items<F: FnMut(tree_sitter::Node, &str)>(
    walker: &mut tree_sitter::TreeCursor,
    source: &[u8],
    f: &mut F,
) {
    let state = walker.state();
    if state.kind() == "struct_item" {
        let name = state
            .child_by_field_name("name")
            .and_then(|n| n.utf8_text(source).ok())
            .unwrap_or("");
        if !name.is_empty() {
            f(state, name);
        }
    }
    if walker.goto_first_child() {
        loop {
            walk_struct_items(walker, source, f);
            if !walker.goto_next_sibling() {
                break;
            }
        }
        walker.goto_parent();
    }
}

/// (fn_name, count) — number of `.clone()` MethodCall invocations in the
/// function's body (and inside closures within it). Only emits a fact when
/// `count >= 1`.
///
/// Scope: counts `expr.clone()` syntactic form only. UFCS calls like
/// `Clone::clone(&x)` or `<T as Clone>::clone(&x)` are NOT counted — those
/// are rare in practice and parse as scoped-path calls rather than field
/// expressions. Nested `fn` definitions inside the body are walked
/// separately and do not double-attribute their clones to the outer fn.
fn extract_function_clone_counts(parsed: &ParsedFile) -> Vec<(String, usize)> {
    let ParsedFile::Rust { tree, source } = parsed else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut walker = tree.walk();
    walk_function_items(&mut walker, source.as_bytes(), &mut |fn_node, name| {
        let Some(body) = fn_node.child_by_field_name("body") else {
            return;
        };
        let mut count = 0usize;
        count_clone_calls(body, source.as_bytes(), &mut count);
        if count >= 1 {
            out.push((name.to_string(), count));
        }
    });
    out
}

/// Functions with 3 or more `.clone()` calls. Threshold is fixed.
pub(crate) fn extract_function_clone_counts_high(parsed: &ParsedFile) -> Vec<(String, usize)> {
    const HIGH_THRESHOLD: usize = 3;
    extract_function_clone_counts(parsed)
        .into_iter()
        .filter(|(_, count)| *count >= HIGH_THRESHOLD)
        .collect()
}

/// Recursively count `x.clone()` method calls beneath `state`.
fn count_clone_calls(state: tree_sitter::Node, source: &[u8], count: &mut usize) {
    // Nested fn / closure: don't count inner-function clones toward the
    // enclosing function. Closures stay in scope (they execute as part of
    // the outer function's work); nested fns are independent units that
    // get walked separately by walk_function_items.
    if state.kind() == "function_item" {
        return;
    }
    if state.kind() == "call_expression" {
        if let Some(func) = state.child_by_field_name("function") {
            if func.kind() == "field_expression" {
                if let Some(field) = func.child_by_field_name("field") {
                    if field.utf8_text(source).unwrap_or("") == "clone" {
                        *count += 1;
                    }
                }
            }
        }
    }
    let mut walker = state.walk();
    for child in state.children(&mut walker) {
        count_clone_calls(child, source, count);
    }
}

/// (fn_name, param_name, type_text). `type_text` is whitespace-normalized so
/// rules can match against a stable form: runs of whitespace collapse to one
/// space, and whitespace adjacent to `<`, `>`, `,`, `:`, or `&` is stripped.
/// Examanales: `& String` → `&String`, `Vec< u8 >` → `Vec<u8>`,
/// `dyn Trait + Send` → `dyn Trait + Send`, `&'a str` → `&'a str`.
fn extract_function_param_types(parsed: &ParsedFile) -> Vec<(String, String, String)> {
    let ParsedFile::Rust { tree, source } = parsed else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut walker = tree.walk();
    walk_function_items(&mut walker, source.as_bytes(), &mut |fn_node, name| {
        let Some(params) = fn_node.child_by_field_name("parameters") else {
            return;
        };
        let mut param_walker = params.walk();
        for child in params.children(&mut param_walker) {
            if child.kind() != "parameter" {
                continue;
            }
            let Some(pat) = child.child_by_field_name("pattern") else {
                continue;
            };
            let Some(ty) = child.child_by_field_name("type") else {
                continue;
            };
            let pat_text = pat.utf8_text(source.as_bytes()).unwrap_or("");
            let ty_text = ty.utf8_text(source.as_bytes()).unwrap_or("");
            let collapsed: String = ty_text.split_whitespace().collect::<Vec<_>>().join(" ");
            let normalized = collapsed
                .replace(" <", "<")
                .replace("< ", "<")
                .replace(" >", ">")
                .replace("> ", ">")
                .replace(" ,", ",")
                .replace(", ", ",")
                .replace(" :", ":")
                .replace(": ", ":")
                .replace("& ", "&");
            out.push((name.to_string(), pat_text.to_string(), normalized));
        }
    });
    out
}

/// `async fn` — walks function_item states and checks whether their
/// `function_modifiers` child contains the `async` keyword.
fn extract_async_functions(parsed: &ParsedFile) -> Vec<String> {
    let ParsedFile::Rust { tree, source } = parsed else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut walker = tree.walk();
    walk_function_items(&mut walker, source.as_bytes(), &mut |fn_node, name| {
        let mut child_walker = fn_node.walk();
        for child in fn_node.children(&mut child_walker) {
            if child.kind() == "function_modifiers" {
                let text = child.utf8_text(source.as_bytes()).unwrap_or("");
                if text.split_whitespace().any(|w| w == "async") {
                    out.push(name.to_string());
                    return;
                }
            }
        }
    });
    out
}

/// Recursively visits every `function_item` in the tree. Trait method
/// declarations without bodies (`function_signature_item`) are intentionally
/// skipped — predicates here describe implementations, not signatures.
/// Calls `f(fn_node, name)` for each match. Lets predicates share traversal logic.
fn walk_function_items<F: FnMut(tree_sitter::Node, &str)>(
    walker: &mut tree_sitter::TreeCursor,
    source: &[u8],
    f: &mut F,
) {
    let state = walker.state();
    if state.kind() == "function_item" {
        let name = state
            .child_by_field_name("name")
            .and_then(|n| n.utf8_text(source).ok())
            .unwrap_or("");
        if !name.is_empty() {
            f(state, name);
        }
    }
    if walker.goto_first_child() {
        loop {
            walk_function_items(walker, source, f);
            if !walker.goto_next_sibling() {
                break;
            }
        }
        walker.goto_parent();
    }
}

/// `pub fn` only — `pub(crate)`, `pub(super)`, etc. are deliberately excluded
/// because they don't escoreose API outside the crate boundary.
fn extract_public_functions(parsed: &ParsedFile) -> Vec<String> {
    let ParsedFile::Rust { tree, source } = parsed else {
        return Vec::new();
    };
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&PUB_FN_QUERY, tree.root_node(), source.as_bytes());
    let mut out = Vec::new();
    while let Some(m) = matches.next() {
        let mut vis: Option<&str> = None;
        let mut name: Option<&str> = None;
        for cap in m.captures {
            let text = cap.node.utf8_text(source.as_bytes()).unwrap_or("");
            match PUB_FN_QUERY.capture_names()[cap.index as usize] {
                "vis" => vis = Some(text.trim()),
                "name" => name = Some(text),
                _ => {}
            }
        }
        if vis == Some("pub") {
            if let Some(n) = name {
                out.push(n.to_string());
            }
        }
    }
    out
}

/// `pub fn` declarations whose preceding region contains no doc comment.
/// Exemanats:
/// - functions inside `impl Trait for Type` blocks (trait provides docs)
/// - functions with `#[test]` attribute
pub(crate) fn extract_pub_fns_without_doc_comment(parsed: &ParsedFile) -> Vec<String> {
    let ParsedFile::Rust { tree, source } = parsed else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = tree.walk();
    walk_pub_fns_without_doc_comment(&mut cursor, source.as_bytes(), &mut out);
    out
}

fn walk_pub_fns_without_doc_comment(
    cursor: &mut tree_sitter::TreeCursor,
    source: &[u8],
    out: &mut Vec<String>,
) {
    let state = cursor.state();
    if state.kind() == "function_item"
        && is_pub_fn_node(state, source)
        && !is_test_fn(state, source)
        && !is_inside_trait_impl(state)
        && !has_preceding_doc_comment(state, source)
    {
        if let Some(name) = function_name(state, source) {
            out.push(name);
        }
    }
    if cursor.goto_first_child() {
        loop {
            walk_pub_fns_without_doc_comment(cursor, source, out);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        cursor.goto_parent();
    }
}

/// Walk backward through preceding siblings. Returns true if a doc comment
/// (`///` or `/**`) or `#[doc = "..."]` attribute is found. Regular `//`
/// comments and non-doc attributes are walked past; any other state kind
/// terminates the walk with `false`.
fn has_preceding_doc_comment(state: tree_sitter::Node, source: &[u8]) -> bool {
    let mut prev = state.prev_sibling();
    while let Some(sib) = prev {
        let text = sib.utf8_text(source).unwrap_or("");
        match sib.kind() {
            "line_comment" => {
                if text.trim_start().starts_with("///") {
                    return true;
                }
                prev = sib.prev_sibling();
            }
            "block_comment" => {
                if text.trim_start().starts_with("/**") {
                    return true;
                }
                prev = sib.prev_sibling();
            }
            "attribute_item" => {
                if text.trim_start().starts_with("#[doc") {
                    return true;
                }
                prev = sib.prev_sibling();
            }
            _ => return false,
        }
    }
    false
}

/// True if any preceding-sibling attribute is `#[test]` or `#[tokio::test...]`.
fn is_test_fn(state: tree_sitter::Node, source: &[u8]) -> bool {
    let mut prev = state.prev_sibling();
    while let Some(sib) = prev {
        match sib.kind() {
            "attribute_item" => {
                let text = sib.utf8_text(source).unwrap_or("");
                if text.contains("#[test]") || text.contains("#[tokio::test") {
                    return true;
                }
                prev = sib.prev_sibling();
            }
            "line_comment" | "block_comment" => {
                // Doc or regular comments don't break the attribute chain;
                // a documented #[test] should still be recognized as a test.
                prev = sib.prev_sibling();
            }
            _ => return false,
        }
    }
    false
}

/// True iff `state` is inside an `impl Trait for Type` block. A plain
/// inherent `impl Type { ... }` does NOT count (its methods are the type's
/// own public surface and warrant docs).
fn is_inside_trait_impl(state: tree_sitter::Node) -> bool {
    let mut p = state.parent();
    while let Some(parent) = p {
        if parent.kind() == "impl_item" {
            return parent.child_by_field_name("trait").is_some();
        }
        p = parent.parent();
    }
    false
}

/// True if `state` is a `function_item` whose visibility_modifier is exactly
/// `pub` (not `pub(crate)`, `pub(super)`, etc.). Mirrors `extract_public_functions`.
fn is_pub_fn_node(state: tree_sitter::Node, source: &[u8]) -> bool {
    let mut c = state.walk();
    for child in state.children(&mut c) {
        if child.kind() == "visibility_modifier" {
            let text = child.utf8_text(source).unwrap_or("").trim();
            return text == "pub";
        }
    }
    false
}

const ASSERTION_MACROS: &[&str] = &[
    "assert",
    "assert_eq",
    "assert_ne",
    "assert_matches",
    "debug_assert",
    "debug_assert_eq",
    "debug_assert_ne",
    "panic",
    "unreachable",
    "todo",
];

/// `#[test]` functions whose body has no assertion-macro invocation
/// and no `?` operator.
pub(crate) fn extract_tests_without_assertion(parsed: &ParsedFile) -> Vec<String> {
    let ParsedFile::Rust { tree, source } = parsed else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = tree.walk();
    walk_tests_without_assertion(&mut cursor, source.as_bytes(), &mut out);
    out
}

fn walk_tests_without_assertion(
    cursor: &mut tree_sitter::TreeCursor,
    source: &[u8],
    out: &mut Vec<String>,
) {
    let state = cursor.state();
    if state.kind() == "function_item" && is_test_fn(state, source) {
        if let Some(name) = function_name(state, source) {
            if !body_has_assertion(state, source) {
                out.push(name);
            }
        }
    }
    if cursor.goto_first_child() {
        loop {
            walk_tests_without_assertion(cursor, source, out);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        cursor.goto_parent();
    }
}

fn body_has_assertion(fn_node: tree_sitter::Node, source: &[u8]) -> bool {
    let Some(body) = fn_node.child_by_field_name("body") else {
        return false;
    };
    has_assertion_or_exception(body, source)
}

fn has_assertion_or_exception(state: tree_sitter::Node, source: &[u8]) -> bool {
    match state.kind() {
        "macro_invocation" => {
            // Macro name is at field "macro" (identifier or scoped_identifier).
            if let Some(name) = state.child_by_field_name("macro") {
                let text = name.utf8_text(source).unwrap_or("");
                let bare = text.rsplit("::").next().unwrap_or(text);
                if ASSERTION_MACROS.contains(&bare) {
                    return true;
                }
            }
        }
        "try_expression" => return true, // the `?` operator
        _ => {}
    }
    let mut cursor = state.walk();
    for child in state.children(&mut cursor) {
        if has_assertion_or_exception(child, source) {
            return true;
        }
    }
    false
}

/// Functions that call `engine.eval(...)` or `engine.eval::<T>(...)` with
/// a string literal as the first argument.
pub(crate) fn extract_engine_eval_string_literals(parsed: &ParsedFile) -> Vec<String> {
    let ParsedFile::Rust { tree, source } = parsed else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = tree.walk();
    walk_engine_eval_string_literals(&mut cursor, source.as_bytes(), &mut out, None);
    out
}

fn walk_engine_eval_string_literals(
    cursor: &mut tree_sitter::TreeCursor,
    source: &[u8],
    out: &mut Vec<String>,
    enclosing_fn: Option<String>,
) {
    let state = cursor.state();
    let new_enclosing = if state.kind() == "function_item" {
        function_name(state, source).or(enclosing_fn.clone())
    } else {
        enclosing_fn.clone()
    };

    if state.kind() == "call_expression" {
        if let Some(name) = new_enclosing.as_deref() {
            if call_is_eval_with_string_literal(state, source) && !out.iter().any(|n| n == name) {
                out.push(name.to_string());
            }
        }
    }

    if cursor.goto_first_child() {
        loop {
            walk_engine_eval_string_literals(cursor, source, out, new_enclosing.clone());
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        cursor.goto_parent();
    }
}

/// True iff `state` is a call_expression of shape `<expr>.eval(<string_literal>, ...)`
/// or `<expr>.eval::<T>(<string_literal>, ...)`.
fn call_is_eval_with_string_literal(state: tree_sitter::Node, source: &[u8]) -> bool {
    let Some(function) = state.child_by_field_name("function") else {
        return false;
    };

    // Unwrap turbofish: generic_function -> field_expression
    let field_expr = match function.kind() {
        "field_expression" => function,
        "generic_function" => {
            let mut c = function.walk();
            let inner = function
                .children(&mut c)
                .find(|n| n.kind() == "field_expression");
            match inner {
                Some(n) => n,
                None => return false,
            }
        }
        _ => return false,
    };

    let Some(field) = field_expr.child_by_field_name("field") else {
        return false;
    };
    if field.utf8_text(source).unwrap_or("") != "eval" {
        return false;
    }

    let Some(args) = state.child_by_field_name("arguments") else {
        return false;
    };
    let mut c = args.walk();
    for child in args.children(&mut c) {
        if matches!(child.kind(), "(" | ")" | ",") {
            continue;
        }
        return child.kind() == "string_literal";
    }
    false
}

fn function_name(state: tree_sitter::Node, source: &[u8]) -> Option<String> {
    state.child_by_field_name("name")
        .and_then(|n| n.utf8_text(source).ok())
        .map(|s| s.to_string())
}

/// Functions whose return type is `Result<_, String>` (error state is String).
fn extract_result_string_returns(parsed: &ParsedFile) -> Vec<String> {
    let ParsedFile::Rust { tree, source } = parsed else {
        return Vec::new();
    };
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&FN_QUERY, tree.root_node(), source.as_bytes());
    let mut out = Vec::new();

    while let Some(m) = matches.next() {
        let mut name: Option<&str> = None;
        let mut outer: Option<&str> = None;
        let mut args_node: Option<tree_sitter::Node> = None;
        for cap in m.captures {
            let text = cap.node.utf8_text(source.as_bytes()).unwrap_or("");
            match FN_QUERY.capture_names()[cap.index as usize] {
                "name" => name = Some(text),
                "return_outer" => outer = Some(text),
                "return_args" => args_node = Some(cap.node),
                _ => {}
            }
        }

        if outer != Some("Result") {
            continue;
        }
        let Some(args) = args_node else { continue };

        let count = args.named_child_count();
        if count < 2 {
            continue;
        }
        let Some(last) = args.named_child(count - 1) else {
            continue;
        };
        if last.kind() != "type_identifier" {
            continue;
        }
        let last_text = last.utf8_text(source.as_bytes()).unwrap_or("");
        if last_text != "String" {
            continue;
        }
        if let Some(n) = name {
            out.push(n.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_result_string_return() {
        let code = "fn foo() -> Result<u32, String> { Ok(1) }";
        let facts = extract(code);
        assert_eq!(facts.functions_returning_result_string, vec!["foo"]);
    }

    #[test]
    fn ignores_result_with_error_type() {
        let code = "fn ok() -> Result<u32, MyError> { Ok(1) }";
        let facts = extract(code);
        assert!(facts.functions_returning_result_string.is_empty());
    }

    #[test]
    fn ignores_non_result_returns() {
        let code = "fn x() -> u32 { 1 } fn y() -> String { String::new() }";
        let facts = extract(code);
        assert!(facts.functions_returning_result_string.is_empty());
    }

    #[test]
    fn detects_multiple_offenders() {
        let code = "\
fn one() -> Result<(), String> { Ok(()) }
fn two() -> Result<u32, String> { Ok(0) }
fn safe() -> Result<u32, MyError> { Ok(0) }
";
        let facts = extract(code);
        assert_eq!(facts.functions_returning_result_string, vec!["one", "two"]);
    }

    #[test]
    fn empty_input_yields_no_facts() {
        assert_eq!(extract(""), SyntaxFacts::default());
    }

    #[test]
    fn invalid_rust_does_not_panic() {
        let _ = extract("fn !!! broken values here");
    }

    #[test]
    fn does_not_flag_single_arg_result_string() {
        let code = "fn get_name() -> Result<String> { Ok(String::new()) }";
        let facts = extract(code);
        assert!(facts.functions_returning_result_string.is_empty());
    }

    #[test]
    fn does_not_flag_scoped_result_with_single_arg() {
        let code = "fn get_name() -> anyhow::Result<String> { Ok(String::new()) }";
        let facts = extract(code);
        assert!(facts.functions_returning_result_string.is_empty());
    }

    #[test]
    fn flags_result_string_string_pair() {
        let code = "fn weird() -> Result<String, String> { Ok(String::new()) }";
        let facts = extract(code);
        assert_eq!(facts.functions_returning_result_string, vec!["weird"]);
    }

    #[test]
    fn does_not_flag_result_with_complex_success_and_typed_error() {
        let code = "fn make() -> Result<Vec<u8>, MyError> { Ok(vec![]) }";
        let facts = extract(code);
        assert!(facts.functions_returning_result_string.is_empty());
    }

    #[test]
    fn does_not_flag_result_with_string_in_success_via_generic() {
        let code = "fn list() -> Result<Vec<String>, MyError> { Ok(vec![]) }";
        let facts = extract(code);
        assert!(facts.functions_returning_result_string.is_empty());
    }

    #[test]
    fn still_flags_standard_result_unit_string() {
        let code = "fn fail() -> Result<(), String> { Err(\"oops\".into()) }";
        let facts = extract(code);
        assert_eq!(facts.functions_returning_result_string, vec!["fail"]);
    }

    #[test]
    fn detects_pub_function() {
        let code = "pub fn foo() {}";
        let facts = extract(code);
        assert_eq!(facts.public_functions, vec!["foo"]);
    }

    #[test]
    fn ignores_private_function() {
        let code = "fn foo() {}";
        let facts = extract(code);
        assert!(facts.public_functions.is_empty());
    }

    #[test]
    fn ignores_pub_crate_function() {
        let code = "pub(crate) fn foo() {}";
        let facts = extract(code);
        assert!(
            facts.public_functions.is_empty(),
            "pub(crate) must not count as fully public: {:?}",
            facts.public_functions
        );
    }

    #[test]
    fn detects_pub_impl_method() {
        let code = "impl S { pub fn bar(&self) {} }";
        let facts = extract(code);
        assert_eq!(facts.public_functions, vec!["bar"]);
    }

    #[test]
    fn detects_async_function() {
        let code = "async fn foo() {}";
        let facts = extract(code);
        assert_eq!(facts.async_functions, vec!["foo"]);
    }

    #[test]
    fn detects_pub_async_function() {
        let code = "pub async fn foo() {}";
        let facts = extract(code);
        assert_eq!(facts.async_functions, vec!["foo"]);
    }

    #[test]
    fn ignores_sync_function() {
        let code = "fn foo() {}";
        let facts = extract(code);
        assert!(facts.async_functions.is_empty());
    }

    #[test]
    fn detects_async_impl_method() {
        let code = "impl S { async fn foo(&self) {} }";
        let facts = extract(code);
        assert_eq!(facts.async_functions, vec!["foo"]);
    }

    #[test]
    fn extracts_single_param_type() {
        let code = "fn foo(name: &String) {}";
        let facts = extract(code);
        assert_eq!(
            facts.function_param_types,
            vec![("foo".to_string(), "name".to_string(), "&String".to_string())]
        );
    }

    #[test]
    fn extracts_multiple_param_types() {
        let code = "fn foo(a: u32, b: Vec<u8>) {}";
        let facts = extract(code);
        assert_eq!(
            facts.function_param_types,
            vec![
                ("foo".to_string(), "a".to_string(), "u32".to_string()),
                ("foo".to_string(), "b".to_string(), "Vec<u8>".to_string()),
            ]
        );
    }

    #[test]
    fn normalizes_whitespace_in_param_type() {
        let code = "fn foo(name: & String) {}";
        let facts = extract(code);
        assert_eq!(
            facts.function_param_types,
            vec![("foo".to_string(), "name".to_string(), "&String".to_string())],
            "internal whitespace should be normalized so rules can match consistently"
        );
    }

    #[test]
    fn preserves_dyn_trait_internal_spacing() {
        let code = "fn foo(x: &dyn Trait) {}";
        let facts = extract(code);
        assert_eq!(
            facts.function_param_types,
            vec![("foo".to_string(), "x".to_string(), "&dyn Trait".to_string())],
            "dyn Trait should keep its internal space; only spaces adjacent to & / < / > / , / : are stripped"
        );
    }

    #[test]
    fn preserves_lifetime_spacing() {
        let code = "fn foo<'a>(s: &'a str) {}";
        let facts = extract(code);
        assert_eq!(
            facts.function_param_types,
            vec![("foo".to_string(), "s".to_string(), "&'a str".to_string())],
            "lifetime-bound references should preserve the space before the type name"
        );
    }

    #[test]
    fn function_param_counts_high_fires_at_threshold() {
        let code = "fn small(a: u32, b: u32) {} fn big(a: u32, b: u32, c: u32, d: u32, e: u32) {} fn xl(a: u32, b: u32, c: u32, d: u32, e: u32, f: u32) {}";
        let facts = extract(code);
        assert_eq!(
            facts.function_param_counts_high,
            vec![("big".to_string(), 5), ("xl".to_string(), 6)],
            "threshold is 5: small (2) skipped, big (5) and xl (6) flagged"
        );
    }

    #[test]
    fn function_param_counts_high_ignores_self() {
        // `&self` does not count toward the parameter total (matches the
        // intent of 'methods with N business params').
        let code = "impl S { fn method(&self, a: u32, b: u32, c: u32, d: u32) {} }";
        let facts = extract(code);
        assert!(
            facts.function_param_counts_high.is_empty(),
            "4 business params + &self should NOT fire the threshold-5 rule"
        );
    }

    #[test]
    fn vec_ref_params_flagged_for_shared_and_mut_refs() {
        let code = "fn a(xs: &Vec<u8>) {} fn b(ys: &mut Vec<String>) {} fn c(zs: Vec<i32>) {} fn d(ws: &[u8]) {}";
        let facts = extract(code);
        assert_eq!(
            facts.vec_ref_params,
            vec![
                ("a".to_string(), "xs".to_string()),
                ("b".to_string(), "ys".to_string()),
            ],
            "only &Vec<T> and &mut Vec<T> should be flagged; owned Vec<T> and &[T] should be left alone"
        );
    }

    #[test]
    fn skips_self_parameter() {
        let code = "impl S { fn foo(&self, x: u32) {} }";
        let facts = extract(code);
        assert_eq!(
            facts.function_param_types,
            vec![("foo".to_string(), "x".to_string(), "u32".to_string())]
        );
    }

    #[test]
    fn no_param_types_for_zero_arg_fn() {
        let code = "fn foo() {}";
        let facts = extract(code);
        assert!(facts.function_param_types.is_empty());
    }

    #[test]
    fn counts_single_clone_call() {
        let code = "fn foo(x: &String) { let _y = x.clone(); }";
        let facts = extract(code);
        assert_eq!(facts.function_clone_counts, vec![("foo".to_string(), 1)]);
    }

    #[test]
    fn counts_multiple_clone_calls() {
        let code = "fn foo(x: &String, y: &String) { let _a = x.clone(); let _b = y.clone(); }";
        let facts = extract(code);
        assert_eq!(facts.function_clone_counts, vec![("foo".to_string(), 2)]);
    }

    #[test]
    fn no_fact_when_zero_clones() {
        let code = "fn foo() {}";
        let facts = extract(code);
        assert!(facts.function_clone_counts.is_empty());
    }

    #[test]
    fn ignores_clone_in_other_function() {
        let code = "\
fn a() { let x = String::new(); let _ = x.clone(); }
fn b() {}
";
        let facts = extract(code);
        assert_eq!(facts.function_clone_counts, vec![("a".to_string(), 1)]);
    }

    #[test]
    fn nested_fn_does_not_double_count_clones() {
        // Nested fn: outer.clone() inside fn outer; inner.clone() inside fn inner.
        // Each clone attributes to its lexically-enclosing fn only.
        let code = "\
fn outer(x: &String) {
    let _a = x.clone();
    fn inner(y: &String) {
        let _b = y.clone();
    }
}
";
        let facts = extract(code);
        // Sort for stable comparison since tree-sitter walks lexically.
        let mut counts = facts.function_clone_counts;
        counts.sort();
        assert_eq!(
            counts,
            vec![("inner".to_string(), 1), ("outer".to_string(), 1)],
            "nested fn clones must attribute to inner only, not also to outer"
        );
    }

    #[test]
    fn closure_clones_attribute_to_enclosing_fn() {
        // Closure body executes as part of the outer fn's call chain, so its
        // clones DO count toward the outer fn.
        let code = "fn outer(x: &String) { let _f = || { let _y = x.clone(); }; }";
        let facts = extract(code);
        assert_eq!(facts.function_clone_counts, vec![("outer".to_string(), 1)]);
    }

    #[test]
    fn clone_count_high_does_not_fire_below_threshold() {
        let code = "fn foo() { let _ = x.clone(); let _ = y.clone(); }"; // 2 clones
        let facts = extract(code);
        assert!(
            facts.function_clone_counts_high.is_empty(),
            "count 2 must NOT fire; got {:?}",
            facts.function_clone_counts_high
        );
    }

    #[test]
    fn clone_count_high_fires_at_threshold() {
        let code = "fn foo() { let a = x.clone(); let b = x.clone(); let c = x.clone(); }";
        let facts = extract(code);
        assert_eq!(
            facts.function_clone_counts_high,
            vec![("foo".to_string(), 3)]
        );
    }

    #[test]
    fn clone_count_high_reports_count_above_threshold() {
        let code = "fn foo() {
            let _a = x.clone(); let _b = x.clone(); let _c = x.clone();
            let _d = x.clone(); let _e = x.clone(); let _f = x.clone();
            let _g = x.clone(); let _h = x.clone();
        }";
        let facts = extract(code);
        assert_eq!(
            facts.function_clone_counts_high,
            vec![("foo".to_string(), 8)]
        );
    }

    #[test]
    fn pub_fn_with_doc_comment_is_not_flagged() {
        let code = "/// Frobnicate the foo.\npub fn frobnicate() {}";
        let facts = extract(code);
        assert!(facts.pub_fns_without_doc_comment.is_empty());
    }

    #[test]
    fn pub_fn_without_any_comment_is_flagged() {
        let code = "pub fn naked() {}";
        let facts = extract(code);
        assert_eq!(facts.pub_fns_without_doc_comment, vec!["naked".to_string()]);
    }

    #[test]
    fn pub_fn_with_doc_comment_then_attribute_is_not_flagged() {
        let code = "/// Frobnicate.\n#[inline]\npub fn frobnicate() {}";
        let facts = extract(code);
        assert!(
            facts.pub_fns_without_doc_comment.is_empty(),
            "attribute between doc and fn must NOT block; got {:?}",
            facts.pub_fns_without_doc_comment
        );
    }

    #[test]
    fn pub_fn_inside_trait_impl_is_exempt() {
        let code = "impl SomeTrait for Foo { pub fn method() {} }";
        let facts = extract(code);
        assert!(
            facts.pub_fns_without_doc_comment.is_empty(),
            "trait-impl methods inherit docs from the trait"
        );
    }

    #[test]
    fn pub_fn_with_test_attribute_is_exempt() {
        let code = "#[test]\npub fn test_something() {}";
        let facts = extract(code);
        assert!(
            facts.pub_fns_without_doc_comment.is_empty(),
            "tests don't need public docs"
        );
    }

    #[test]
    fn extracts_single_derive() {
        let code = "#[derive(Debug)]\nstruct Foo {}";
        let facts = extract(code);
        assert_eq!(
            facts.struct_derives,
            vec![("Foo".to_string(), "Debug".to_string())]
        );
    }

    #[test]
    fn extracts_multiple_derives() {
        let code = "#[derive(Debug, Clone, PartialEq)]\nstruct Foo {}";
        let facts = extract(code);
        assert_eq!(
            facts.struct_derives,
            vec![
                ("Foo".to_string(), "Debug".to_string()),
                ("Foo".to_string(), "Clone".to_string()),
                ("Foo".to_string(), "PartialEq".to_string()),
            ]
        );
    }

    #[test]
    fn ignores_non_derive_attributes() {
        let code = "#[serde(rename = \"foo\")]\nstruct Foo {}";
        let facts = extract(code);
        assert!(facts.struct_derives.is_empty());
    }

    #[test]
    fn no_facts_for_struct_without_derives() {
        let code = "struct Foo {}";
        let facts = extract(code);
        assert!(facts.struct_derives.is_empty());
    }

    #[test]
    fn does_not_leak_attr_through_use_declaration() {
        // Mid-edit/illegal Rust: derive on a use statement. Must not leak the
        // derive onto the next struct.
        let code = "#[derive(Clone)]\nuse std::io;\nstruct Foo {}";
        let facts = extract(code);
        assert!(
            facts.struct_derives.is_empty(),
            "derive attached to a use should not propagate to Foo: {:?}",
            facts.struct_derives
        );
    }

    #[test]
    fn extracts_derive_on_nested_struct() {
        // Lock in that the walker handles nested structs (struct inside a fn body).
        let code = "fn outer() { #[derive(Debug)] struct Inner {} }";
        let facts = extract(code);
        assert_eq!(
            facts.struct_derives,
            vec![("Inner".to_string(), "Debug".to_string())]
        );
    }

    #[test]
    fn empty_test_body_is_flagged() {
        let code = "#[test]\nfn empty() {}";
        let facts = extract(code);
        assert_eq!(facts.tests_without_assertion, vec!["empty".to_string()]);
    }

    #[test]
    fn test_with_assert_eq_is_not_flagged() {
        let code = "#[test]\nfn good() { assert_eq!(1, 1); }";
        let facts = extract(code);
        assert!(facts.tests_without_assertion.is_empty());
    }

    #[test]
    fn test_with_exception_operator_is_not_flagged() {
        let code = "#[test]\nfn good() -> Result<(), String> { let _ = parse(\"\")?; Ok(()) }";
        let facts = extract(code);
        assert!(
            facts.tests_without_assertion.is_empty(),
            "? operator on Result counts as assertion"
        );
    }

    #[test]
    fn test_with_non_assert_macro_is_flagged() {
        // println! is not an assertion macro.
        let code = "#[test]\nfn weak() { println!(\"hello\"); }";
        let facts = extract(code);
        assert_eq!(facts.tests_without_assertion, vec!["weak".to_string()]);
    }

    #[test]
    fn engine_eval_string_literal_is_flagged() {
        let code = r#"
fn host() {
    let engine = rhai::Engine::new();
    let result: i64 = engine.eval("40 + 2").unwrap();
}
"#;
        let facts = extract(code);
        assert_eq!(facts.engine_eval_string_literals, vec!["host".to_string()]);
    }

    #[test]
    fn engine_eval_variable_is_not_flagged() {
        let code = r#"
fn host() {
    let engine = rhai::Engine::new();
    let script = load_script();
    let result: i64 = engine.eval(&script).unwrap();
}
"#;
        let facts = extract(code);
        assert!(
            facts.engine_eval_string_literals.is_empty(),
            "non-literal arg must not flag"
        );
    }

    #[test]
    fn engine_eval_turbofish_string_literal_is_flagged() {
        let code = r#"
fn host() {
    let engine = rhai::Engine::new();
    let _: i64 = engine.eval::<i64>("40 + 2").unwrap();
}
"#;
        let facts = extract(code);
        assert_eq!(facts.engine_eval_string_literals, vec!["host".to_string()]);
    }

    #[test]
    fn engine_eval_include_str_is_not_flagged() {
        let code = r#"
fn host() {
    let engine = rhai::Engine::new();
    let _: i64 = engine.eval(include_str!("script.rhai")).unwrap();
}
"#;
        let facts = extract(code);
        assert!(
            facts.engine_eval_string_literals.is_empty(),
            "include_str! macro arg is not a string literal"
        );
    }

    #[test]
    fn empty_test_with_doc_comment_is_still_flagged() {
        // Regression: is_test_fn used to bail on line_comments preceding #[test],
        // so documented-but-empty tests slipped past warn-empty-test.
        let code = "/// Tests the foo widget.\n#[test]\nfn documented_empty() {}";
        let facts = extract(code);
        assert_eq!(
            facts.tests_without_assertion,
            vec!["documented_empty".to_string()]
        );
    }
}

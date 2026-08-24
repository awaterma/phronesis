use super::super::parsed::ParsedFile;
use super::walk::{in_test_code, walk_function_items};

/// (fn_name, count) — number of `.clone()` MethodCall invocations in the
/// function's body (and inside closures within it). Only emits a fact when
/// `count >= 1`.
///
/// Scope: counts `expr.clone()` syntactic form only. UFCS calls like
/// `Clone::clone(&x)` or `<T as Clone>::clone(&x)` are NOT counted — those
/// are rare in practice and parse as scoped-path calls rather than field
/// expressions. Nested `fn` definitions inside the body are walked
/// separately and do not double-attribute their clones to the outer fn.
pub(super) fn extract_function_clone_counts(parsed: &ParsedFile) -> Vec<(String, usize)> {
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
pub(super) fn extract_function_clone_counts_high(parsed: &ParsedFile) -> Vec<(String, usize)> {
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
    if state.kind() == "call_expression"
        && let Some(func) = state.child_by_field_name("function")
        && func.kind() == "field_expression"
        && let Some(field) = func.child_by_field_name("field")
        && field.utf8_text(source).unwrap_or("") == "clone"
    {
        *count += 1;
    }
    let mut walker = state.walk();
    for child in state.children(&mut walker) {
        count_clone_calls(child, source, count);
    }
}

/// Recursive walk that respects function-scope boundaries: halts at
/// nested `function_item` and `closure_expression` nodes, and at any
/// `block` node that appears in *expression position* (e.g. the value of
/// a `let_declaration` — the block pattern shape `let x = { ... }`).
///
/// Why the expression-position halt: a `let` inside a block expression
/// used as a value is the very shape we want to *suggest* (the block
/// pattern), so counting it toward the outer function would punish the
/// pattern this rule surfaces. Closures own their scope. Nested
/// functions are walked separately by `walk_function_items`.
///
/// Recursion still descends into `if_expression`, `else_clause`,
/// `match_arm`, `match_block`, `for_expression`, `while_expression`,
/// `loop_expression`, and `try_block` (and into the `block` children
/// those constructs own) because their bodies are continuations of the
/// outer function's control flow, not isolated scopes. See the
/// `matches!` arm below for the canonical list.
///
/// Unsafe and async blocks also use `block` under the hood, so they
/// halt with the same logic — exotic edge case worth knowing about.
///
/// Grammar note (tree-sitter-rust 0.23): the kind is `block`, not
/// `block_expression`. The function body itself is a `block`, so this
/// helper expects to be called on the body's children, not on the body
/// node directly — see `extract_function_let_binding_counts_high`.
fn count_outer_scope_let_declarations<F>(
    node: tree_sitter::Node,
    source: &[u8],
    matches: &F,
    count: &mut usize,
) where
    F: Fn(tree_sitter::Node, &[u8]) -> bool,
{
    // Halt at nested fn / closure — they own their own scope.
    if node.kind() == "function_item" || node.kind() == "closure_expression" {
        return;
    }

    if node.kind() == "let_declaration" && matches(node, source) {
        *count += 1;
        // Continue descending — `let x = { let y = ...; }` should still
        // count `x` at the outer scope. The child-block recurse decision
        // below keeps `y` from contributing.
    }

    let mut walker = node.walk();
    for child in node.children(&mut walker) {
        // If the child is a `block`, decide whether to recurse based on
        // *our* kind. A `block` whose parent is a control-flow construct
        // (if/else/match-arm/match-block/for/while/loop/try) is part of
        // the outer flow. A `block` whose parent is anything else
        // (let_declaration value, function arg, assignment RHS, an
        // `unsafe`/`async` block, …) is in expression position — the
        // block-pattern shape — and we halt.
        if child.kind() == "block"
            && !matches!(
                node.kind(),
                "if_expression"
                    | "else_clause"
                    | "match_arm"
                    | "match_block"
                    | "for_expression"
                    | "while_expression"
                    | "loop_expression"
                    | "try_block"
            )
        {
            continue;
        }
        count_outer_scope_let_declarations(child, source, matches, count);
    }
}

/// Functions with 8 or more outer-scope `let` declarations.
/// See `count_outer_scope_let_declarations` for scoping semantics.
/// Test code (`#[test]` fns, anything under `#[cfg(test)]`) is exempt —
/// a test that sets up eight fixtures is not a block-pattern candidate.
pub(super) fn extract_function_let_binding_counts_high(
    parsed: &ParsedFile,
) -> Vec<(String, usize)> {
    const LET_BINDING_THRESHOLD: usize = 8;
    let ParsedFile::Rust { tree, source } = parsed else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut walker = tree.walk();
    walk_function_items(&mut walker, source.as_bytes(), &mut |fn_node, name| {
        if in_test_code(fn_node, source.as_bytes()) {
            return;
        }
        let Some(body) = fn_node.child_by_field_name("body") else {
            return;
        };
        let mut count = 0usize;
        count_outer_scope_let_declarations(body, source.as_bytes(), &|_, _| true, &mut count);
        if count >= LET_BINDING_THRESHOLD {
            out.push((name.to_string(), count));
        }
    });
    out
}

/// True when a `let_declaration` node has a `mutable_specifier` child
/// (i.e., the `mut` keyword is present). Tree-sitter-rust grammar
/// represents `mut` as a sibling-of-pattern child, not a field.
///
/// Pattern-internal `mut` (e.g., `let (mut a, mut b) = ...`) is intentionally
/// not counted; this surfaces only canonical `let mut x` bindings.
fn has_mut_keyword(node: tree_sitter::Node, _source: &[u8]) -> bool {
    let mut walker = node.walk();
    for child in node.children(&mut walker) {
        if child.kind() == "mutable_specifier" {
            return true;
        }
    }
    false
}

/// Functions with 3 or more outer-scope `let mut` declarations.
/// See `count_outer_scope_let_declarations` for scoping semantics.
/// Test code is exempt, as for `extract_function_let_binding_counts_high`.
pub(super) fn extract_function_let_mut_counts_high(parsed: &ParsedFile) -> Vec<(String, usize)> {
    const LET_MUT_THRESHOLD: usize = 3;
    let ParsedFile::Rust { tree, source } = parsed else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut walker = tree.walk();
    walk_function_items(&mut walker, source.as_bytes(), &mut |fn_node, name| {
        if in_test_code(fn_node, source.as_bytes()) {
            return;
        }
        let Some(body) = fn_node.child_by_field_name("body") else {
            return;
        };
        let mut count = 0usize;
        count_outer_scope_let_declarations(body, source.as_bytes(), &has_mut_keyword, &mut count);
        if count >= LET_MUT_THRESHOLD {
            out.push((name.to_string(), count));
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use crate::syntax::rust::extract;

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
    fn let_binding_count_high_fires_on_long_ladder() {
        let code = "fn parse_config(path: &str) -> Result<Config> {
            let raw = fs::read(path)?;
            let s = String::from_utf8(raw)?;
            let stripped = strip_comments(&s);
            let json = unescape(&stripped);
            let parsed = serde_json::from_str(&json)?;
            let validated = validate(parsed)?;
            let normalized = normalize(validated);
            let final_cfg = expand_env(normalized);
            Ok(final_cfg)
        }";
        let facts = extract(code);
        assert_eq!(
            facts.function_let_binding_counts_high,
            vec![("parse_config".to_string(), 8)]
        );
    }

    #[test]
    fn let_binding_count_high_silent_on_block_adopter() {
        // The same logical work as `let_binding_count_high_fires_on_long_ladder`,
        // but scoped into a block expression. The block pattern adopter
        // should NOT fire.
        let code = "fn parse_config(path: &str) -> Result<Config> {
            let final_cfg = {
                let raw = fs::read(path)?;
                let s = String::from_utf8(raw)?;
                let stripped = strip_comments(&s);
                let json = unescape(&stripped);
                let parsed = serde_json::from_str(&json)?;
                let validated = validate(parsed)?;
                let normalized = normalize(validated);
                expand_env(normalized)
            };
            Ok(final_cfg)
        }";
        let facts = extract(code);
        assert!(
            facts.function_let_binding_counts_high.is_empty(),
            "block-pattern adopter must not fire; got {:?}",
            facts.function_let_binding_counts_high
        );
    }

    #[test]
    fn let_binding_count_high_does_not_count_closure_lets() {
        let code = "fn host() {
            let _a = 1; let _b = 2; let _c = 3;
            let _d = 4; let _e = 5; let _f = 6;
            let _g = 7;
            let _result = items.iter().map(|x| {
                let y = x + 1;
                let z = y * 2;
                z
            }).collect::<Vec<_>>();
        }";
        // host has 8 outer-scope lets (_a.._g + _result) → fires.
        // The closure's let y / let z must NOT contribute, otherwise the
        // count would be 10.
        let facts = extract(code);
        assert_eq!(
            facts.function_let_binding_counts_high,
            vec![("host".to_string(), 8)]
        );
    }

    #[test]
    fn let_binding_count_high_nested_fn_counts_independently() {
        // Outer has 2 outer-scope lets (well below threshold);
        // inner has 8 (at threshold). Only inner fires.
        let code = "fn outer() {
            let _x = 1; let _y = 2;
            fn inner() {
                let _a = 1; let _b = 2; let _c = 3;
                let _d = 4; let _e = 5; let _f = 6;
                let _g = 7; let _h = 8;
            }
        }";
        let facts = extract(code);
        assert_eq!(
            facts.function_let_binding_counts_high,
            vec![("inner".to_string(), 8)]
        );
    }

    #[test]
    fn let_binding_count_high_counts_inside_if_and_match_arms() {
        // 4 outer + 2 in if + 2 in match = 8 → fires with count 8.
        // If `if` or `match` halted recursion, the count would be 4.
        let code = "fn flow(x: i32) {
            let _a = 1; let _b = 2; let _c = 3; let _d = 4;
            if x > 0 {
                let _e = 5;
                let _f = 6;
            }
            match x {
                0 => { let _g = 7; }
                _ => { let _h = 8; }
            }
        }";
        let facts = extract(code);
        assert_eq!(
            facts.function_let_binding_counts_high,
            vec![("flow".to_string(), 8)]
        );
    }

    #[test]
    fn let_mut_count_high_fires_at_three() {
        let code = "fn mutable_heavy() {
            let mut a = vec![];
            let mut b = String::new();
            let mut c = 0;
            a.push(1); b.push_str(\"x\"); c += 1;
        }";
        let facts = extract(code);
        assert_eq!(
            facts.function_let_mut_counts_high,
            vec![("mutable_heavy".to_string(), 3)]
        );
    }

    #[test]
    fn let_mut_count_high_silent_on_mut_inside_block() {
        // The `let mut`s are scoped inside a block expression — the
        // outer function should see them as already-scoped and the rule
        // must NOT fire.
        let code = "fn frozen_after() {
            let result = {
                let mut a = vec![];
                let mut b = String::new();
                let mut c = 0;
                a.push(1); b.push_str(\"x\"); c += 1;
                (a, b, c)
            };
            use_result(&result);
        }";
        let facts = extract(code);
        assert!(
            facts.function_let_mut_counts_high.is_empty(),
            "mut-in-block-adopter must not fire; got {:?}",
            facts.function_let_mut_counts_high
        );
    }

    #[test]
    fn let_mut_count_high_below_threshold_silent() {
        // Two `let mut`s — below the threshold of 3.
        let code = "fn two_muts() {
            let mut a = vec![];
            let mut b = String::new();
            a.push(1); b.push_str(\"x\");
        }";
        let facts = extract(code);
        assert!(facts.function_let_mut_counts_high.is_empty());
    }

    #[test]
    fn let_binding_count_high_counts_inside_else_branch() {
        // The else_clause allow-list entry in the walker is what makes
        // this work — without it, lets inside `else { ... }` would be
        // silently skipped because the `block` node's parent is
        // `else_clause`, not `if_expression`.
        // 5 outer + 1 in if + 2 in else = 8 → fires.
        let code = "fn branch_flow(x: i32) {
            let _a = 1; let _b = 2; let _c = 3; let _d = 4; let _e = 5;
            if x > 0 {
                let _f = 6;
            } else {
                let _g = 7;
                let _h = 8;
            }
        }";
        let facts = extract(code);
        assert_eq!(
            facts.function_let_binding_counts_high,
            vec![("branch_flow".to_string(), 8)]
        );
    }

    /// Regression: `phr-mcp audit` reported `#[test]` fns inside
    /// `#[cfg(test)] mod tests` for the let-binding / let-mut counts even
    /// though the rules document test code as exempt. Both the `#[test]`
    /// attribute and the enclosing `#[cfg(test)]` module must exclude
    /// the function (a plain helper in the test module has neither marker
    /// on itself, so the module check is load-bearing).
    #[test]
    fn let_counts_skip_functions_in_cfg_test_module_and_test_fns() {
        let code = "\
#[cfg(test)]
mod tests {
    fn helper() {
        let mut a = 1; let mut b = 2; let mut c = 3;
        let _d = 4; let _e = 5; let _f = 6; let _g = 7; let _h = 8;
        a += b; b += c; c += a;
    }
    #[test]
    fn big() {
        let mut a = 1; let mut b = 2; let mut c = 3;
        let _d = 4; let _e = 5; let _f = 6; let _g = 7; let _h = 8;
        a += b; b += c; c += a;
    }
}
#[tokio::test]
async fn top_level_test() {
    let mut a = 1; let mut b = 2; let mut c = 3;
    let _d = 4; let _e = 5; let _f = 6; let _g = 7; let _h = 8;
    a += b; b += c; c += a;
}
fn production() {
    let mut a = 1; let mut b = 2; let mut c = 3;
    let _d = 4; let _e = 5; let _f = 6; let _g = 7; let _h = 8;
    a += b; b += c; c += a;
}
";
        let facts = extract(code);
        assert_eq!(
            facts.function_let_binding_counts_high,
            vec![("production".to_string(), 8)],
            "only the production fn should be reported"
        );
        assert_eq!(
            facts.function_let_mut_counts_high,
            vec![("production".to_string(), 3)],
            "only the production fn should be reported"
        );
    }
}

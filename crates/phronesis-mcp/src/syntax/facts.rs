//! Structural facts produced by values-aware extractors. `SyntaxFacts` keeps
//! per-predicate named fields (compile-time discoverability) but
//! `all_facts()` aggregates them into a flat `Vec<Fact>` so `hook.rs` can
//! assert the entire batch through one uniform loop.

use phr::Fact;

/// Structural facts extracted from a single source file.
///
/// Field naming convention: language-agnostic predicates (those that mean the
/// same thing in Rust and Swift) use language prefixes only where ambiguity
/// would arise (e.g. `swift_async_functions` vs Rust `async_functions`).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyntaxFacts {
    // ─── Rust ───────────────────────────────────────────────────────
    /// Functions whose return type is `Result<_, String>` (error state).
    pub functions_returning_result_string: Vec<String>,
    /// (fn_name, param_name, type_text) — one entry per parameter. `type_text`
    /// is whitespace-normalized.
    pub function_param_types: Vec<(String, String, String)>,
    /// (fn_name, param_name) for parameters whose type starts with `&Vec<` or
    /// `&mut Vec<`. Derived from `function_param_types`. Used by the
    /// `warn-rust-public-fn-takes-vec-ref` rule, which the existing
    /// `function_param_type` predicate can't express because it requires
    /// exact string match and `&Vec<...>` has variable contents.
    pub vec_ref_params: Vec<(String, String)>,
    /// (fn_name, count) for functions whose parameter count meets or exceeds
    /// a threshold. Derived from `function_param_types` by grouping. Threshold
    /// fixed at 5 by convention (matches `function_clone_counts_high` shape).
    /// `&self` is not counted (the param extractor already skips it).
    pub function_param_counts_high: Vec<(String, usize)>,
    /// (fn_name, count) — number of `.clone()` MethodCall invocations in the
    /// body (including inside closures). UFCS form `Clone::clone(&x)` is not
    /// counted. Only populated when count >= 1.
    pub function_clone_counts: Vec<(String, usize)>,
    /// Functions with 3 or more `.clone()` calls in their body. The threshold is
    /// fixed at 3 — see Tier 1 spec for rationale. Args: (fn_name, count).
    pub function_clone_counts_high: Vec<(String, usize)>,
    /// `pub fn` declarations with no preceding doc comment. Args: (fn_name,).
    /// Trait-impl methods and `#[test]` functions are exempt.
    pub pub_fns_without_doc_comment: Vec<String>,
    /// `#[test]` functions whose body contains no assertion-macro invocation
    /// (assert!/assert_eq!/etc., panic!/unreachable!/todo!) AND no `?` operator.
    /// Args: (fn_name,).
    pub tests_without_assertion: Vec<String>,
    /// Functions declared `pub` (not `pub(crate)`, `pub(super)`, etc.).
    pub public_functions: Vec<String>,
    /// Functions declared `async`.
    pub async_functions: Vec<String>,
    /// (struct_name, trait_name) — one entry per derived trait.
    pub struct_derives: Vec<(String, String)>,
    /// Functions that call `engine.eval(...)` or `engine.eval::<T>(...)` with
    /// a string literal as the first argument. Args: (fn_name,).
    pub engine_eval_string_literals: Vec<String>,
    /// Functions with 8 or more *outer-scope* `let` declarations.
    /// Bindings inside child `block_expression` and `closure_expression`
    /// nodes are NOT counted, so functions that already adopted the
    /// block pattern (`let x = { let raw = ...; let parsed = ...; ... }`)
    /// go silent. Conditional and loop bodies (if/match/for/while/loop)
    /// DO recurse because they're continuations of the outer flow.
    /// Args: (fn_name, count). Threshold fixed at 8.
    pub function_let_binding_counts_high: Vec<(String, usize)>,
    /// Functions with 3 or more *outer-scope* `let mut` declarations.
    /// Same scope semantics as `function_let_binding_counts_high`
    /// (halt at child blocks/closures, recurse into if/match/for/while).
    /// Args: (fn_name, count). Threshold fixed at 3.
    pub function_let_mut_counts_high: Vec<(String, usize)>,

    // ─── Swift ──────────────────────────────────────────────────────
    /// (fn_name, count) — number of force-unwrap (`!`) postfix expressions.
    /// Only populated when count >= 1.
    pub swift_force_unwraps: Vec<(String, usize)>,
    pub swift_throwing_functions: Vec<String>,
    pub swift_async_functions: Vec<String>,
}

impl SyntaxFacts {
    /// Flatten every populated field into a `Vec<Fact>` ready for assertion.
    /// `file_path` is the first arg of every fact (matches existing convention).
    pub fn all_facts(&self, file_path: &str) -> Vec<Fact> {
        let mut out = Vec::new();

        for (i, name) in self.functions_returning_result_string.iter().enumerate() {
            out.push(Fact {
                id: format!("function_returns_result_string_{}_{}", name, i),
                predicate: "function_returns_result_string".to_string(),
                args: vec![file_path.to_string(), name.clone()],
                timestamp: 0,
            });
        }

        for (i, (fn_name, param, ty)) in self.function_param_types.iter().enumerate() {
            out.push(Fact {
                id: format!("function_param_type_{}_{}_{}", fn_name, param, i),
                predicate: "function_param_type".to_string(),
                args: vec![
                    file_path.to_string(),
                    fn_name.clone(),
                    param.clone(),
                    ty.clone(),
                ],
                timestamp: 0,
            });
        }

        for (i, (fn_name, param)) in self.vec_ref_params.iter().enumerate() {
            out.push(Fact {
                id: format!("function_param_is_vec_ref_{}_{}_{}", fn_name, param, i),
                predicate: "function_param_is_vec_ref".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), param.clone()],
                timestamp: 0,
            });
        }

        for (i, (fn_name, count)) in self.function_param_counts_high.iter().enumerate() {
            out.push(Fact {
                id: format!("function_param_count_high_{}_{}", fn_name, i),
                predicate: "function_param_count_high".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), count.to_string()],
                timestamp: 0,
            });
        }

        for (i, (fn_name, count)) in self.function_clone_counts.iter().enumerate() {
            out.push(Fact {
                id: format!("function_clone_count_{}_{}", fn_name, i),
                predicate: "function_clone_count".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), count.to_string()],
                timestamp: 0,
            });
        }

        for (i, (fn_name, count)) in self.function_clone_counts_high.iter().enumerate() {
            out.push(Fact {
                id: format!("function_clone_count_high_{}_{}", fn_name, i),
                predicate: "function_clone_count_high".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), count.to_string()],
                timestamp: 0,
            });
        }

        for (i, name) in self.pub_fns_without_doc_comment.iter().enumerate() {
            out.push(Fact {
                id: format!("pub_fn_without_doc_comment_{}_{}", name, i),
                predicate: "pub_fn_without_doc_comment".to_string(),
                args: vec![file_path.to_string(), name.clone()],
                timestamp: 0,
            });
        }

        for (i, fn_name) in self.tests_without_assertion.iter().enumerate() {
            out.push(Fact {
                id: format!("test_without_assertion_{}_{}", fn_name, i),
                predicate: "test_without_assertion".to_string(),
                args: vec![file_path.to_string(), fn_name.clone()],
                timestamp: 0,
            });
        }

        for (i, name) in self.public_functions.iter().enumerate() {
            out.push(Fact {
                id: format!("function_is_public_{}_{}", name, i),
                predicate: "function_is_public".to_string(),
                args: vec![file_path.to_string(), name.clone()],
                timestamp: 0,
            });
        }

        for (i, name) in self.async_functions.iter().enumerate() {
            out.push(Fact {
                id: format!("function_is_async_{}_{}", name, i),
                predicate: "function_is_async".to_string(),
                args: vec![file_path.to_string(), name.clone()],
                timestamp: 0,
            });
        }

        for (i, (struct_name, trait_name)) in self.struct_derives.iter().enumerate() {
            out.push(Fact {
                id: format!("struct_derives_{}_{}_{}", struct_name, trait_name, i),
                predicate: "struct_derives".to_string(),
                args: vec![
                    file_path.to_string(),
                    struct_name.clone(),
                    trait_name.clone(),
                ],
                timestamp: 0,
            });
        }

        for (i, fn_name) in self.engine_eval_string_literals.iter().enumerate() {
            out.push(Fact {
                id: format!("engine_eval_string_literal_{}_{}", fn_name, i),
                predicate: "engine_eval_string_literal".to_string(),
                args: vec![file_path.to_string(), fn_name.clone()],
                timestamp: 0,
            });
        }

        for (i, (fn_name, count)) in self.swift_force_unwraps.iter().enumerate() {
            out.push(Fact {
                id: format!("function_uses_force_unwrap_{}_{}", fn_name, i),
                predicate: "function_uses_force_unwrap".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), count.to_string()],
                timestamp: 0,
            });
        }

        for (i, name) in self.swift_throwing_functions.iter().enumerate() {
            out.push(Fact {
                id: format!("function_throws_{}_{}", name, i),
                predicate: "function_throws".to_string(),
                args: vec![file_path.to_string(), name.clone()],
                timestamp: 0,
            });
        }

        for (i, name) in self.swift_async_functions.iter().enumerate() {
            out.push(Fact {
                id: format!("function_is_async_{}_{}", name, i),
                predicate: "function_is_async".to_string(),
                args: vec![file_path.to_string(), name.clone()],
                timestamp: 0,
            });
        }

        out
    }
}

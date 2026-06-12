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

    // ─── Python ─────────────────────────────────────────────────────
    /// Enclosing function name (or `<module>`) per bare `except:` clause.
    pub python_bare_excepts: Vec<String>,
    /// (fn_name, param_name) for defaults that are mutable literals or
    /// `list()`/`dict()`/`set()` calls — created once at def time.
    pub python_mutable_default_args: Vec<(String, String)>,
    /// (fn_name, count) for defs with 6+ parameters (`self`/`cls` excluded).
    pub python_function_param_counts_high: Vec<(String, usize)>,
    /// Public `def`s (no leading `_`) whose body lacks a docstring.
    pub python_functions_missing_docstring: Vec<String>,

    // ─── TypeScript ─────────────────────────────────────────────────
    /// (fn_name or `<module>`, count) of explicit `any` type annotations.
    pub ts_explicit_anys: Vec<(String, usize)>,
    /// (fn_name or `<module>`, count) of non-null assertions (`x!`).
    pub ts_non_null_assertions: Vec<(String, usize)>,
    /// File-level count of `@ts-ignore` / `@ts-expect-error` /
    /// `@ts-nocheck` comments.
    pub ts_suppression_comment_count: usize,
    /// (fn_name, count) for functions with 5+ parameters.
    pub ts_function_param_counts_high: Vec<(String, usize)>,
}

impl SyntaxFacts {
    /// Predicate names that `all_facts` can emit. Shared with
    /// `audit::is_ast_predicate` so the audit runner and the fact
    /// emitter can't drift. A test below proves the list matches every
    /// emission block in `all_facts`; if you add a new emission block,
    /// the test fails until you add the predicate name here.
    pub const PREDICATES: &'static [&'static str] = &[
        // Rust
        "function_returns_result_string",
        "function_param_type",
        "function_param_is_vec_ref",
        "function_param_count_high",
        "function_clone_count",
        "function_clone_count_high",
        "function_let_binding_count_high",
        "function_let_mut_count_high",
        "pub_fn_without_doc_comment",
        "test_without_assertion",
        "function_is_public",
        "function_is_async",
        "struct_derives",
        "engine_eval_string_literal",
        // Swift
        "function_uses_force_unwrap",
        "function_throws",
        // `function_is_async` appears for both Rust and Swift but is
        // listed once because it's the same predicate name.
        // Python
        "python_bare_except",
        "python_mutable_default_arg",
        "python_function_param_count_high",
        "python_function_missing_docstring",
        // TypeScript
        "ts_explicit_any",
        "ts_non_null_assertion",
        "ts_suppression_comment",
        "ts_function_param_count_high",
    ];

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

        for (i, (fn_name, count)) in self.function_let_binding_counts_high.iter().enumerate() {
            out.push(Fact {
                id: format!("function_let_binding_count_high_{}_{}", fn_name, i),
                predicate: "function_let_binding_count_high".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), count.to_string()],
                timestamp: 0,
            });
        }

        for (i, (fn_name, count)) in self.function_let_mut_counts_high.iter().enumerate() {
            out.push(Fact {
                id: format!("function_let_mut_count_high_{}_{}", fn_name, i),
                predicate: "function_let_mut_count_high".to_string(),
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

        for (i, fn_name) in self.python_bare_excepts.iter().enumerate() {
            out.push(Fact {
                id: format!("python_bare_except_{}_{}", fn_name, i),
                predicate: "python_bare_except".to_string(),
                args: vec![file_path.to_string(), fn_name.clone()],
                timestamp: 0,
            });
        }

        for (i, (fn_name, param)) in self.python_mutable_default_args.iter().enumerate() {
            out.push(Fact {
                id: format!("python_mutable_default_arg_{}_{}_{}", fn_name, param, i),
                predicate: "python_mutable_default_arg".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), param.clone()],
                timestamp: 0,
            });
        }

        for (i, (fn_name, count)) in self.python_function_param_counts_high.iter().enumerate() {
            out.push(Fact {
                id: format!("python_function_param_count_high_{}_{}", fn_name, i),
                predicate: "python_function_param_count_high".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), count.to_string()],
                timestamp: 0,
            });
        }

        for (i, fn_name) in self.python_functions_missing_docstring.iter().enumerate() {
            out.push(Fact {
                id: format!("python_function_missing_docstring_{}_{}", fn_name, i),
                predicate: "python_function_missing_docstring".to_string(),
                args: vec![file_path.to_string(), fn_name.clone()],
                timestamp: 0,
            });
        }

        for (i, (fn_name, count)) in self.ts_explicit_anys.iter().enumerate() {
            out.push(Fact {
                id: format!("ts_explicit_any_{}_{}", fn_name, i),
                predicate: "ts_explicit_any".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), count.to_string()],
                timestamp: 0,
            });
        }

        for (i, (fn_name, count)) in self.ts_non_null_assertions.iter().enumerate() {
            out.push(Fact {
                id: format!("ts_non_null_assertion_{}_{}", fn_name, i),
                predicate: "ts_non_null_assertion".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), count.to_string()],
                timestamp: 0,
            });
        }

        if self.ts_suppression_comment_count > 0 {
            out.push(Fact {
                id: "ts_suppression_comment_0".to_string(),
                predicate: "ts_suppression_comment".to_string(),
                args: vec![
                    file_path.to_string(),
                    self.ts_suppression_comment_count.to_string(),
                ],
                timestamp: 0,
            });
        }

        for (i, (fn_name, count)) in self.ts_function_param_counts_high.iter().enumerate() {
            out.push(Fact {
                id: format!("ts_function_param_count_high_{}_{}", fn_name, i),
                predicate: "ts_function_param_count_high".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), count.to_string()],
                timestamp: 0,
            });
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_facts_emits_let_binding_count_high() {
        let facts = SyntaxFacts {
            function_let_binding_counts_high: vec![("foo".to_string(), 10)],
            ..Default::default()
        };
        let out = facts.all_facts("/tmp/src.rs");
        let hit = out
            .iter()
            .find(|f| f.predicate == "function_let_binding_count_high");
        assert!(
            hit.is_some(),
            "no function_let_binding_count_high fact emitted"
        );
        let hit = hit.unwrap();
        assert_eq!(
            hit.args,
            vec![
                "/tmp/src.rs".to_string(),
                "foo".to_string(),
                "10".to_string()
            ]
        );
    }

    #[test]
    fn all_facts_emits_let_mut_count_high() {
        let facts = SyntaxFacts {
            function_let_mut_counts_high: vec![("bar".to_string(), 4)],
            ..Default::default()
        };
        let out = facts.all_facts("/tmp/src.rs");
        let hit = out
            .iter()
            .find(|f| f.predicate == "function_let_mut_count_high");
        assert!(hit.is_some(), "no function_let_mut_count_high fact emitted");
        let hit = hit.unwrap();
        assert_eq!(
            hit.args,
            vec![
                "/tmp/src.rs".to_string(),
                "bar".to_string(),
                "4".to_string()
            ]
        );
    }

    /// Drift guard: every predicate name `all_facts` actually emits must
    /// be listed in `SyntaxFacts::PREDICATES`, and vice versa. If you add
    /// a new emission block in `all_facts` without updating `PREDICATES`,
    /// this test will fail — pointing you at the file with both call sites.
    /// Without this guard, `audit::is_ast_predicate` (which reads
    /// `PREDICATES`) would silently fail to recognize the new predicate
    /// and the rule using it would never fire under `phr-mcp audit`.
    #[test]
    fn predicates_const_matches_all_facts_emission_set() {
        // Populate one entry in every Vec so every emission block fires.
        // Exhaustive literal (no `..Default::default()`): adding a field to
        // SyntaxFacts without populating it here is a compile error, which is
        // exactly the drift signal this guard exists to produce.
        let facts = SyntaxFacts {
            functions_returning_result_string: vec!["a".to_string()],
            function_param_types: vec![("a".to_string(), "b".to_string(), "c".to_string())],
            vec_ref_params: vec![("a".to_string(), "b".to_string())],
            function_param_counts_high: vec![("a".to_string(), 5)],
            function_clone_counts: vec![("a".to_string(), 1)],
            function_clone_counts_high: vec![("a".to_string(), 3)],
            function_let_binding_counts_high: vec![("a".to_string(), 8)],
            function_let_mut_counts_high: vec![("a".to_string(), 3)],
            pub_fns_without_doc_comment: vec!["a".to_string()],
            tests_without_assertion: vec!["a".to_string()],
            public_functions: vec!["a".to_string()],
            async_functions: vec!["a".to_string()],
            struct_derives: vec![("S".to_string(), "Debug".to_string())],
            engine_eval_string_literals: vec!["a".to_string()],
            swift_force_unwraps: vec![("a".to_string(), 1)],
            swift_throwing_functions: vec!["a".to_string()],
            swift_async_functions: vec!["a".to_string()],
            python_bare_excepts: vec!["a".to_string()],
            python_mutable_default_args: vec![("a".to_string(), "b".to_string())],
            python_function_param_counts_high: vec![("a".to_string(), 6)],
            python_functions_missing_docstring: vec!["a".to_string()],
            ts_explicit_anys: vec![("a".to_string(), 1)],
            ts_non_null_assertions: vec![("a".to_string(), 1)],
            ts_suppression_comment_count: 1,
            ts_function_param_counts_high: vec![("a".to_string(), 5)],
        };

        let emitted: std::collections::BTreeSet<String> = facts
            .all_facts("/tmp/x.rs")
            .iter()
            .map(|f| f.predicate.clone())
            .collect();
        let listed: std::collections::BTreeSet<String> = SyntaxFacts::PREDICATES
            .iter()
            .map(|p| p.to_string())
            .collect();

        let missing_from_const: Vec<&String> = emitted.difference(&listed).collect();
        let stale_in_const: Vec<&String> = listed.difference(&emitted).collect();

        assert!(
            missing_from_const.is_empty(),
            "all_facts emits predicates not in SyntaxFacts::PREDICATES — add them: {:?}",
            missing_from_const
        );
        assert!(
            stale_in_const.is_empty(),
            "SyntaxFacts::PREDICATES lists predicates all_facts no longer emits — remove them: {:?}",
            stale_in_const
        );
    }
}

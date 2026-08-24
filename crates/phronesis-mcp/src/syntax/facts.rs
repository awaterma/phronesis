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
    /// (fn_name, param_name) for parameters whose normalized type starts with
    /// `&Box<`. Derived from `function_param_types`.
    pub box_ref_params: Vec<(String, String)>,
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
    /// Enclosing function per `unsafe` block without a nearby `SAFETY:` note.
    pub unsafe_blocks_without_safety_comment: Vec<String>,
    /// (fn_name, callee) for known blocking calls made directly by `async fn`.
    pub async_blocking_calls: Vec<(String, String)>,
    /// (fn_name, guard_name) for std synchronization guards whose lexical
    /// scope continues across an await point.
    pub sync_lock_guards_across_await: Vec<(String, String)>,
    /// (fn_name or `<module>`, construct) for parser-identified starter-pack
    /// invocations: unwrap, empty-message expect, todo, panic, unimplemented,
    /// and dbg.
    pub rust_governed_invocations: Vec<(String, String)>,
    /// Stable names for parser-identified crate/item attributes governed by
    /// starter rules.
    pub rust_governed_attributes: Vec<String>,
    /// (implementing_type, trait_name) for parsed Rust trait impl blocks.
    pub rust_trait_impls: Vec<(String, String)>,
    /// (implementing_type, construct) for panicking constructs — unwrap,
    /// empty-message expect, todo, panic, unimplemented — inside the body of
    /// a `Drop::drop` implementation. A panic during unwind inside
    /// `Drop::drop` aborts the whole process rather than failing gracefully.
    pub rust_panic_in_drop: Vec<(String, String)>,
    /// (fn_name or `<module>`, shape) for governed Rust match-arm forms.
    pub rust_governed_match_arms: Vec<(String, String)>,
    /// (field_name, primitive_type) for parsed fields ending in `_id` whose
    /// type is `u64`. The String-ID rule retains line-oriented matching so
    /// its field-level documentation exception remains enforceable.
    pub rust_primitive_id_fields: Vec<(String, String)>,
    /// Count of parsed `Rc<RefCell<_>>` type shapes.
    pub rust_rc_refcell_count: usize,
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
    /// (fn_name or `<module>`, construct) for parser-identified Swift starter
    /// rule shapes.
    pub swift_governed_constructs: Vec<(String, String)>,

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
    /// Enclosing function name (or `<module>`) per `print()` call whose
    /// callee is the bare identifier `print` (not `x.print()`, `sprint()`,
    /// etc.).
    pub python_print_calls: Vec<String>,
    /// (fn_name, param_name, callee_name) for default arguments whose
    /// value is a call expression. Corresponds broadly to Bugbear B008.
    /// Immutable constructors like `list()`, `dict()`, `set()` are
    /// also included here so projects can selectively ignore them.
    pub python_call_in_default_args: Vec<(String, String, String)>,
    /// (fn_name, exception_type) for exception handlers whose body is
    /// only `pass`, comments, or ellipsis (`...`). Typed handlers only;
    /// bare handlers are excluded because the bare-except rule catches them.
    pub python_exception_handler_passes: Vec<(String, String)>,
    /// Callee names for obvious I/O performed at module import time.
    pub python_import_time_io: Vec<String>,
    /// Enclosing function per identity comparison against a value literal.
    pub python_is_literal_comparisons: Vec<String>,
    /// (fn_name, global_name) for mutations of module-level containers.
    pub python_mutated_module_globals: Vec<(String, String)>,
    /// Module names used by `from module import *`.
    pub python_star_imports: Vec<String>,

    // ─── Python: python-patterns.guide derived (opt-in `python-patterns` pack)
    /// (fn_name, global_name) per name declared with `global` inside a def.
    pub python_global_statements: Vec<(String, String)>,
    /// Enclosing function per `globals()[...] = ...` assignment.
    pub python_globals_subscript_assignments: Vec<String>,
    /// Enclosing function per three-argument `type(name, bases, ns)` call.
    pub python_dynamic_class_creations: Vec<String>,
    /// (class_name, shape) per class defining `__new__`; shape is
    /// `singleton` when the body touches a `_instance`-style cache, else
    /// `custom`.
    pub python_new_overrides: Vec<(String, String)>,
    /// (fn_name, count) for defs whose `if`/`elif` conditions call
    /// `isinstance` at least twice.
    pub python_isinstance_chains: Vec<(String, usize)>,
    /// Classes that implement a container protocol method and also make
    /// `__iter__` return `self` alongside `__next__`.
    pub python_containers_own_iterator: Vec<String>,
    /// (class_name, count) for classes with 2+ concrete (non-mixin,
    /// non-ABC/Protocol/Generic) base classes.
    pub python_multiple_inheritance: Vec<(String, usize)>,
    /// (class_name, depth) for classes whose file-local inheritance chain
    /// is 3+ levels deep.
    pub python_inheritance_depths: Vec<(String, usize)>,
    /// Classes named `*Mixin` that define `__init__`.
    pub python_mixins_with_init: Vec<String>,
    /// (class_name, attr, count) for classes with 4+ methods that only
    /// delegate `return self.<attr>.<same_name>(...)` and no `__getattr__`.
    pub python_static_delegation_wrappers: Vec<(String, String, usize)>,
    /// (class_name, attr) for class-body `attr = []` / `{}` / `set()` etc.
    pub python_mutable_class_attributes: Vec<(String, String)>,
    /// Enclosing function per `x == None` / `x != None` comparison.
    pub python_equality_with_none: Vec<String>,

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
    /// (fn_name or `<module>`, count) for direct global `console.log(...)`
    /// calls.
    pub ts_console_log_calls: Vec<(String, usize)>,
}

/// A `tests/` or `benches/` target is test code in its entirety, with no
/// `#[cfg(test)]` or `#[test]` marker for the AST walk to key on — the
/// harness supplies that framing. The hazard predicates describe production
/// latency and soundness defects, so they say nothing useful there.
fn is_test_target(file_path: &str) -> bool {
    let path = file_path.replace('\\', "/");
    path.starts_with("tests/")
        || path.starts_with("benches/")
        || path.contains("/tests/")
        || path.contains("/benches/")
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
        "function_param_is_box_ref",
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
        "rust_unsafe_without_safety_comment",
        "rust_async_blocking_call",
        "rust_sync_lock_guard_across_await",
        "rust_governed_invocation",
        "rust_governed_attribute",
        "rust_trait_impl",
        "rust_panic_in_drop",
        "rust_governed_match_arm",
        "rust_primitive_id_field",
        "rust_rc_refcell_type",
        // Swift
        "function_uses_force_unwrap",
        "function_throws",
        "swift_governed_construct",
        // `function_is_async` appears for both Rust and Swift but is
        // listed once because it's the same predicate name.
        // Python
        "python_bare_except",
        "python_mutable_default_arg",
        "python_function_param_count_high",
        "python_function_missing_docstring",
        "python_print_call",
        "python_call_in_default_arg",
        "python_exception_handler_passes",
        "python_import_time_io",
        "python_is_literal_comparison",
        "python_mutated_module_global",
        "python_star_import",
        "python_global_statement",
        "python_globals_subscript_assignment",
        "python_dynamic_class_creation",
        "python_new_override",
        "python_isinstance_chain",
        "python_container_is_own_iterator",
        "python_multiple_inheritance",
        "python_inheritance_depth",
        "python_mixin_with_init",
        "python_static_delegation_wrapper",
        "python_mutable_class_attribute",
        "python_equality_with_none",
        // TypeScript
        "ts_explicit_any",
        "ts_non_null_assertion",
        "ts_suppression_comment",
        "ts_function_param_count_high",
        "ts_console_log_call",
    ];

    /// Flatten every populated field into a `Vec<Fact>` ready for assertion.
    /// `file_path` is the first arg of every fact (matches existing convention).
    pub fn all_facts(&self, file_path: &str) -> Vec<Fact> {
        let mut out = Vec::new();
        let language = std::path::Path::new(file_path)
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("unknown");
        let source = Some(format!("ast:{language}"));

        for (i, name) in self.functions_returning_result_string.iter().enumerate() {
            out.push(Fact {
                id: format!("function_returns_result_string_{}_{}", name, i),
                predicate: "function_returns_result_string".to_string(),
                args: vec![file_path.to_string(), name.clone()],
                timestamp: 0,
                source: source.clone(),
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
                source: source.clone(),
            });
        }

        for (i, (fn_name, param)) in self.vec_ref_params.iter().enumerate() {
            out.push(Fact {
                id: format!("function_param_is_vec_ref_{}_{}_{}", fn_name, param, i),
                predicate: "function_param_is_vec_ref".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), param.clone()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, (fn_name, param)) in self.box_ref_params.iter().enumerate() {
            out.push(Fact {
                id: format!("function_param_is_box_ref_{}_{}_{}", fn_name, param, i),
                predicate: "function_param_is_box_ref".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), param.clone()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, (fn_name, count)) in self.function_param_counts_high.iter().enumerate() {
            out.push(Fact {
                id: format!("function_param_count_high_{}_{}", fn_name, i),
                predicate: "function_param_count_high".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), count.to_string()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, (fn_name, count)) in self.function_clone_counts.iter().enumerate() {
            out.push(Fact {
                id: format!("function_clone_count_{}_{}", fn_name, i),
                predicate: "function_clone_count".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), count.to_string()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, (fn_name, count)) in self.function_clone_counts_high.iter().enumerate() {
            out.push(Fact {
                id: format!("function_clone_count_high_{}_{}", fn_name, i),
                predicate: "function_clone_count_high".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), count.to_string()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, (fn_name, count)) in self.function_let_binding_counts_high.iter().enumerate() {
            out.push(Fact {
                id: format!("function_let_binding_count_high_{}_{}", fn_name, i),
                predicate: "function_let_binding_count_high".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), count.to_string()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, (fn_name, count)) in self.function_let_mut_counts_high.iter().enumerate() {
            out.push(Fact {
                id: format!("function_let_mut_count_high_{}_{}", fn_name, i),
                predicate: "function_let_mut_count_high".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), count.to_string()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, name) in self.pub_fns_without_doc_comment.iter().enumerate() {
            out.push(Fact {
                id: format!("pub_fn_without_doc_comment_{}_{}", name, i),
                predicate: "pub_fn_without_doc_comment".to_string(),
                args: vec![file_path.to_string(), name.clone()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, fn_name) in self.tests_without_assertion.iter().enumerate() {
            out.push(Fact {
                id: format!("test_without_assertion_{}_{}", fn_name, i),
                predicate: "test_without_assertion".to_string(),
                args: vec![file_path.to_string(), fn_name.clone()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, name) in self.public_functions.iter().enumerate() {
            out.push(Fact {
                id: format!("function_is_public_{}_{}", name, i),
                predicate: "function_is_public".to_string(),
                args: vec![file_path.to_string(), name.clone()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, name) in self.async_functions.iter().enumerate() {
            out.push(Fact {
                id: format!("function_is_async_{}_{}", name, i),
                predicate: "function_is_async".to_string(),
                args: vec![file_path.to_string(), name.clone()],
                timestamp: 0,
                source: source.clone(),
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
                source: source.clone(),
            });
        }

        for (i, fn_name) in self.engine_eval_string_literals.iter().enumerate() {
            out.push(Fact {
                id: format!("engine_eval_string_literal_{}_{}", fn_name, i),
                predicate: "engine_eval_string_literal".to_string(),
                args: vec![file_path.to_string(), fn_name.clone()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, fn_name) in self.unsafe_blocks_without_safety_comment.iter().enumerate() {
            out.push(Fact {
                id: format!("rust_unsafe_without_safety_comment_{}_{}", fn_name, i),
                predicate: "rust_unsafe_without_safety_comment".to_string(),
                args: vec![file_path.to_string(), fn_name.clone()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, (fn_name, callee)) in self
            .async_blocking_calls
            .iter()
            .filter(|_| !is_test_target(file_path))
            .enumerate()
        {
            out.push(Fact {
                id: format!("rust_async_blocking_call_{}_{}_{}", fn_name, callee, i),
                predicate: "rust_async_blocking_call".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), callee.clone()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, (fn_name, guard)) in self
            .sync_lock_guards_across_await
            .iter()
            .filter(|_| !is_test_target(file_path))
            .enumerate()
        {
            out.push(Fact {
                id: format!(
                    "rust_sync_lock_guard_across_await_{}_{}_{}",
                    fn_name, guard, i
                ),
                predicate: "rust_sync_lock_guard_across_await".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), guard.clone()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, (fn_name, construct)) in self.rust_governed_invocations.iter().enumerate() {
            out.push(Fact {
                id: format!("rust_governed_invocation_{}_{}_{}", fn_name, construct, i),
                predicate: "rust_governed_invocation".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), construct.clone()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, attribute) in self.rust_governed_attributes.iter().enumerate() {
            out.push(Fact {
                id: format!("rust_governed_attribute_{}_{}", attribute, i),
                predicate: "rust_governed_attribute".to_string(),
                args: vec![file_path.to_string(), attribute.clone()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, (implementing_type, trait_name)) in self.rust_trait_impls.iter().enumerate() {
            out.push(Fact {
                id: format!("rust_trait_impl_{}_{}_{}", implementing_type, trait_name, i),
                predicate: "rust_trait_impl".to_string(),
                args: vec![
                    file_path.to_string(),
                    implementing_type.clone(),
                    trait_name.clone(),
                ],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, (implementing_type, construct)) in self.rust_panic_in_drop.iter().enumerate() {
            out.push(Fact {
                id: format!(
                    "rust_panic_in_drop_{}_{}_{}",
                    implementing_type, construct, i
                ),
                predicate: "rust_panic_in_drop".to_string(),
                args: vec![
                    file_path.to_string(),
                    implementing_type.clone(),
                    construct.clone(),
                ],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, (fn_name, shape)) in self.rust_governed_match_arms.iter().enumerate() {
            out.push(Fact {
                id: format!("rust_governed_match_arm_{}_{}_{}", fn_name, shape, i),
                predicate: "rust_governed_match_arm".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), shape.clone()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, (field, primitive)) in self.rust_primitive_id_fields.iter().enumerate() {
            out.push(Fact {
                id: format!("rust_primitive_id_field_{}_{}_{}", field, primitive, i),
                predicate: "rust_primitive_id_field".to_string(),
                args: vec![file_path.to_string(), field.clone(), primitive.clone()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        if self.rust_rc_refcell_count > 0 {
            out.push(Fact {
                id: "rust_rc_refcell_type_0".to_string(),
                predicate: "rust_rc_refcell_type".to_string(),
                args: vec![
                    file_path.to_string(),
                    self.rust_rc_refcell_count.to_string(),
                ],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, (fn_name, count)) in self.swift_force_unwraps.iter().enumerate() {
            out.push(Fact {
                id: format!("function_uses_force_unwrap_{}_{}", fn_name, i),
                predicate: "function_uses_force_unwrap".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), count.to_string()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, name) in self.swift_throwing_functions.iter().enumerate() {
            out.push(Fact {
                id: format!("function_throws_{}_{}", name, i),
                predicate: "function_throws".to_string(),
                args: vec![file_path.to_string(), name.clone()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, name) in self.swift_async_functions.iter().enumerate() {
            out.push(Fact {
                id: format!("function_is_async_{}_{}", name, i),
                predicate: "function_is_async".to_string(),
                args: vec![file_path.to_string(), name.clone()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, (fn_name, construct)) in self.swift_governed_constructs.iter().enumerate() {
            out.push(Fact {
                id: format!("swift_governed_construct_{}_{}_{}", fn_name, construct, i),
                predicate: "swift_governed_construct".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), construct.clone()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, fn_name) in self.python_bare_excepts.iter().enumerate() {
            out.push(Fact {
                id: format!("python_bare_except_{}_{}", fn_name, i),
                predicate: "python_bare_except".to_string(),
                args: vec![file_path.to_string(), fn_name.clone()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, (fn_name, param)) in self.python_mutable_default_args.iter().enumerate() {
            out.push(Fact {
                id: format!("python_mutable_default_arg_{}_{}_{}", fn_name, param, i),
                predicate: "python_mutable_default_arg".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), param.clone()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, (fn_name, count)) in self.python_function_param_counts_high.iter().enumerate() {
            out.push(Fact {
                id: format!("python_function_param_count_high_{}_{}", fn_name, i),
                predicate: "python_function_param_count_high".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), count.to_string()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, fn_name) in self.python_functions_missing_docstring.iter().enumerate() {
            out.push(Fact {
                id: format!("python_function_missing_docstring_{}_{}", fn_name, i),
                predicate: "python_function_missing_docstring".to_string(),
                args: vec![file_path.to_string(), fn_name.clone()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, fn_name) in self.python_print_calls.iter().enumerate() {
            out.push(Fact {
                id: format!("python_print_call_{}_{}", fn_name, i),
                predicate: "python_print_call".to_string(),
                args: vec![file_path.to_string(), fn_name.clone()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, (fn_name, param, callee)) in self.python_call_in_default_args.iter().enumerate() {
            out.push(Fact {
                id: format!(
                    "python_call_in_default_arg_{}_{}_{}_{}",
                    fn_name, param, callee, i
                ),
                predicate: "python_call_in_default_arg".to_string(),
                args: vec![
                    file_path.to_string(),
                    fn_name.clone(),
                    param.clone(),
                    callee.clone(),
                ],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, (fn_name, exc)) in self.python_exception_handler_passes.iter().enumerate() {
            out.push(Fact {
                id: format!("python_exception_handler_passes_{}_{}_{}", fn_name, exc, i),
                predicate: "python_exception_handler_passes".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), exc.clone()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, callee) in self.python_import_time_io.iter().enumerate() {
            out.push(Fact {
                id: format!("python_import_time_io_{}_{}", callee, i),
                predicate: "python_import_time_io".to_string(),
                args: vec![file_path.to_string(), callee.clone()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, fn_name) in self.python_is_literal_comparisons.iter().enumerate() {
            out.push(Fact {
                id: format!("python_is_literal_comparison_{}_{}", fn_name, i),
                predicate: "python_is_literal_comparison".to_string(),
                args: vec![file_path.to_string(), fn_name.clone()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, (fn_name, global)) in self.python_mutated_module_globals.iter().enumerate() {
            out.push(Fact {
                id: format!("python_mutated_module_global_{}_{}_{}", fn_name, global, i),
                predicate: "python_mutated_module_global".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), global.clone()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, module) in self.python_star_imports.iter().enumerate() {
            out.push(Fact {
                id: format!("python_star_import_{}_{}", module, i),
                predicate: "python_star_import".to_string(),
                args: vec![file_path.to_string(), module.clone()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        // python-patterns.guide derived facts. Shapes: (file, a) / (file, a, b)
        // / (file, a, b, c) in the field order documented on the struct.
        let mut push2 = |predicate: &str, a: &str, i: usize| {
            out.push(Fact {
                id: format!("{}_{}_{}", predicate, a, i),
                predicate: predicate.to_string(),
                args: vec![file_path.to_string(), a.to_string()],
                timestamp: 0,
                source: source.clone(),
            });
        };
        for (i, f) in self.python_globals_subscript_assignments.iter().enumerate() {
            push2("python_globals_subscript_assignment", f, i);
        }
        for (i, f) in self.python_dynamic_class_creations.iter().enumerate() {
            push2("python_dynamic_class_creation", f, i);
        }
        for (i, c) in self.python_containers_own_iterator.iter().enumerate() {
            push2("python_container_is_own_iterator", c, i);
        }
        for (i, c) in self.python_mixins_with_init.iter().enumerate() {
            push2("python_mixin_with_init", c, i);
        }
        for (i, f) in self.python_equality_with_none.iter().enumerate() {
            push2("python_equality_with_none", f, i);
        }
        let mut push3 = |predicate: &str, a: &str, b: &str, i: usize| {
            out.push(Fact {
                id: format!("{}_{}_{}_{}", predicate, a, b, i),
                predicate: predicate.to_string(),
                args: vec![file_path.to_string(), a.to_string(), b.to_string()],
                timestamp: 0,
                source: source.clone(),
            });
        };
        for (i, (f, g)) in self.python_global_statements.iter().enumerate() {
            push3("python_global_statement", f, g, i);
        }
        for (i, (c, shape)) in self.python_new_overrides.iter().enumerate() {
            push3("python_new_override", c, shape, i);
        }
        for (i, (f, n)) in self.python_isinstance_chains.iter().enumerate() {
            push3("python_isinstance_chain", f, &n.to_string(), i);
        }
        for (i, (c, n)) in self.python_multiple_inheritance.iter().enumerate() {
            push3("python_multiple_inheritance", c, &n.to_string(), i);
        }
        for (i, (c, n)) in self.python_inheritance_depths.iter().enumerate() {
            push3("python_inheritance_depth", c, &n.to_string(), i);
        }
        for (i, (c, a)) in self.python_mutable_class_attributes.iter().enumerate() {
            push3("python_mutable_class_attribute", c, a, i);
        }
        for (i, (c, attr, n)) in self.python_static_delegation_wrappers.iter().enumerate() {
            out.push(Fact {
                id: format!("python_static_delegation_wrapper_{}_{}_{}", c, attr, i),
                predicate: "python_static_delegation_wrapper".to_string(),
                args: vec![
                    file_path.to_string(),
                    c.clone(),
                    attr.clone(),
                    n.to_string(),
                ],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, (fn_name, count)) in self.ts_explicit_anys.iter().enumerate() {
            out.push(Fact {
                id: format!("ts_explicit_any_{}_{}", fn_name, i),
                predicate: "ts_explicit_any".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), count.to_string()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, (fn_name, count)) in self.ts_non_null_assertions.iter().enumerate() {
            out.push(Fact {
                id: format!("ts_non_null_assertion_{}_{}", fn_name, i),
                predicate: "ts_non_null_assertion".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), count.to_string()],
                timestamp: 0,
                source: source.clone(),
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
                source: source.clone(),
            });
        }

        for (i, (fn_name, count)) in self.ts_function_param_counts_high.iter().enumerate() {
            out.push(Fact {
                id: format!("ts_function_param_count_high_{}_{}", fn_name, i),
                predicate: "ts_function_param_count_high".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), count.to_string()],
                timestamp: 0,
                source: source.clone(),
            });
        }

        for (i, (fn_name, count)) in self.ts_console_log_calls.iter().enumerate() {
            out.push(Fact {
                id: format!("ts_console_log_call_{}_{}", fn_name, i),
                predicate: "ts_console_log_call".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), count.to_string()],
                timestamp: 0,
                source: source.clone(),
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
            box_ref_params: vec![("a".to_string(), "b".to_string())],
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
            unsafe_blocks_without_safety_comment: vec!["a".to_string()],
            async_blocking_calls: vec![("a".to_string(), "b".to_string())],
            sync_lock_guards_across_await: vec![("a".to_string(), "b".to_string())],
            rust_governed_invocations: vec![("a".to_string(), "unwrap".to_string())],
            rust_governed_attributes: vec!["deny_warnings".to_string()],
            rust_trait_impls: vec![("S".to_string(), "Deref".to_string())],
            rust_panic_in_drop: vec![("S".to_string(), "unwrap".to_string())],
            rust_governed_match_arms: vec![("a".to_string(), "none_empty".to_string())],
            rust_primitive_id_fields: vec![("user_id".to_string(), "u64".to_string())],
            rust_rc_refcell_count: 1,
            swift_force_unwraps: vec![("a".to_string(), 1)],
            swift_throwing_functions: vec!["a".to_string()],
            swift_async_functions: vec!["a".to_string()],
            swift_governed_constructs: vec![("a".to_string(), "try_force".to_string())],
            python_bare_excepts: vec!["a".to_string()],
            python_mutable_default_args: vec![("a".to_string(), "b".to_string())],
            python_function_param_counts_high: vec![("a".to_string(), 6)],
            python_functions_missing_docstring: vec!["a".to_string()],
            python_print_calls: vec!["a".to_string()],
            python_call_in_default_args: vec![("a".to_string(), "b".to_string(), "c".to_string())],
            python_exception_handler_passes: vec![("a".to_string(), "b".to_string())],
            python_import_time_io: vec!["a".to_string()],
            python_is_literal_comparisons: vec!["a".to_string()],
            python_mutated_module_globals: vec![("a".to_string(), "b".to_string())],
            python_star_imports: vec!["a".to_string()],
            python_global_statements: vec![("a".to_string(), "b".to_string())],
            python_globals_subscript_assignments: vec!["a".to_string()],
            python_dynamic_class_creations: vec!["a".to_string()],
            python_new_overrides: vec![("A".to_string(), "singleton".to_string())],
            python_isinstance_chains: vec![("a".to_string(), 2)],
            python_containers_own_iterator: vec!["A".to_string()],
            python_multiple_inheritance: vec![("A".to_string(), 2)],
            python_inheritance_depths: vec![("A".to_string(), 3)],
            python_mixins_with_init: vec!["AMixin".to_string()],
            python_static_delegation_wrappers: vec![("A".to_string(), "_f".to_string(), 4)],
            python_mutable_class_attributes: vec![("A".to_string(), "b".to_string())],
            python_equality_with_none: vec!["a".to_string()],
            ts_explicit_anys: vec![("a".to_string(), 1)],
            ts_non_null_assertions: vec![("a".to_string(), 1)],
            ts_suppression_comment_count: 1,
            ts_function_param_counts_high: vec![("a".to_string(), 5)],
            ts_console_log_calls: vec![("a".to_string(), 1)],
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

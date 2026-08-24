use super::*;
use crate::graph::unit::TsConfig;
use std::collections::BTreeMap;

fn ctx(files: &[&str]) -> UnitContext {
    UnitContext {
        id: "typescript:myapp".to_string(),
        module_base: "src".to_string(),
        siblings: BTreeMap::new(),
        ts: TsConfig {
            base_url: "src".to_string(),
            paths: BTreeMap::new(),
        },
        files: files.iter().map(|f| (*f).to_string()).collect(),
        lua_files: Vec::new(),
        cue_files: Vec::new(),
        test_target: false,
    }
}

fn edges_of(x: &Extracted, p: &str) -> Vec<Vec<String>> {
    x.edges
        .iter()
        .filter(|e| e.p == p)
        .map(|e| e.a.clone())
        .collect()
}

// ─── file_type ──────────────────────────────────────────────────

#[test]
fn a_plain_module_is_production() {
    assert_eq!(file_type("src/billing.ts"), "production");
}

#[test]
fn a_dot_test_file_is_a_test() {
    assert_eq!(file_type("src/billing.test.ts"), "test");
    assert_eq!(file_type("src/billing.spec.tsx"), "test");
}

#[test]
fn a_file_under_a_tests_directory_is_a_test() {
    assert_eq!(file_type("src/__tests__/billing.ts"), "test");
}

// ─── declares_module ────────────────────────────────────────────

#[test]
fn a_file_declares_its_own_module() {
    let out = extract_typescript(
        "src/billing.ts",
        "export const x = 1\n",
        &ctx(&["src/billing.ts"]),
    );
    assert_eq!(
        edges_of(&out, "declares_module"),
        vec![vec![
            "src/billing.ts".to_string(),
            "typescript:myapp::billing".to_string()
        ]]
    );
}

// ─── defines_fn ─────────────────────────────────────────────────

#[test]
fn a_function_declaration_is_a_defined_function() {
    let out = extract_typescript(
        "src/billing.ts",
        "export function charge() { return 1 }\n",
        &ctx(&["src/billing.ts"]),
    );
    assert_eq!(
        edges_of(&out, "defines_fn")[0][1],
        "typescript:myapp::billing::charge"
    );
}

#[test]
fn an_arrow_function_assigned_to_a_const_is_a_defined_function() {
    // The dominant style in modern TypeScript; missing it would leave
    // most codebases looking empty.
    let out = extract_typescript(
        "src/billing.ts",
        "export const charge = () => 1\n",
        &ctx(&["src/billing.ts"]),
    );
    assert_eq!(
        edges_of(&out, "defines_fn")[0][1],
        "typescript:myapp::billing::charge"
    );
}

#[test]
fn a_class_method_is_qualified_by_its_class() {
    let out = extract_typescript(
        "src/billing.ts",
        "export class Ledger { charge() { return 1 } }\n",
        &ctx(&["src/billing.ts"]),
    );
    assert_eq!(
        edges_of(&out, "defines_fn")[0][1],
        "typescript:myapp::billing::Ledger::charge"
    );
}

#[test]
fn a_non_function_const_is_not_a_defined_function() {
    let out = extract_typescript(
        "src/billing.ts",
        "export const RATE = 0.2\n",
        &ctx(&["src/billing.ts"]),
    );
    assert!(edges_of(&out, "defines_fn").is_empty());
}

#[test]
fn an_abstract_class_method_is_qualified_by_its_class() {
    let out = extract_typescript(
        "src/billing.ts",
        "abstract class A { m() { return 1 } }\n",
        &ctx(&["src/billing.ts"]),
    );
    assert_eq!(
        edges_of(&out, "defines_fn")[0][1],
        "typescript:myapp::billing::A::m"
    );
    assert_eq!(out.skipped, 0, "an abstract class must not inflate skipped");
}

#[test]
fn an_abstract_class_does_not_inflate_skipped() {
    // Regression for the `"class"` keyword token — an unnamed child of
    // `abstract_class_declaration` — matching the class-expression arm
    // and being counted as an anonymous class with no name of its own.
    let out = extract_typescript(
        "src/billing.ts",
        "abstract class A { m() {} }\nabstract class B { n() {} }\n",
        &ctx(&["src/billing.ts"]),
    );
    assert_eq!(out.skipped, 0);
}

// ─── imports (including re-exports) ──────────────────────────────

#[test]
fn a_star_re_export_is_an_import() {
    let out = extract_typescript(
        "src/index.ts",
        "export * from './a'\n",
        &ctx(&["src/index.ts", "src/a.ts"]),
    );
    assert_eq!(
        edges_of(&out, "imports"),
        vec![vec![
            "typescript:myapp::index".to_string(),
            "typescript:myapp::a".to_string()
        ]]
    );
}

#[test]
fn a_named_re_export_is_an_import() {
    let out = extract_typescript(
        "src/index.ts",
        "export { b } from './b'\n",
        &ctx(&["src/index.ts", "src/b.ts"]),
    );
    assert_eq!(
        edges_of(&out, "imports"),
        vec![vec![
            "typescript:myapp::index".to_string(),
            "typescript:myapp::b".to_string()
        ]]
    );
}

#[test]
fn a_type_only_re_export_is_an_import() {
    let out = extract_typescript(
        "src/index.ts",
        "export type { T } from './c'\n",
        &ctx(&["src/index.ts", "src/c.ts"]),
    );
    assert_eq!(
        edges_of(&out, "imports"),
        vec![vec![
            "typescript:myapp::index".to_string(),
            "typescript:myapp::c".to_string()
        ]]
    );
}

#[test]
fn a_barrel_file_of_only_re_exports_is_not_a_clean_leaf() {
    // The regression this whole finding is about: without re-export
    // support, the single most common index.ts shape reported zero
    // dependencies and skipped=0 — a clean leaf rather than a hub.
    let out = extract_typescript(
        "src/index.ts",
        "export * from './a'\nexport { b } from './b'\n",
        &ctx(&["src/index.ts", "src/a.ts", "src/b.ts"]),
    );
    assert_eq!(edges_of(&out, "imports").len(), 2);
}

// ─── default exports ───────────────────────────────────────────────

#[test]
fn an_anonymous_default_exported_function_is_a_defined_function() {
    let out = extract_typescript(
        "src/billing.ts",
        "export default function () { return 1 }\n",
        &ctx(&["src/billing.ts"]),
    );
    assert_eq!(
        edges_of(&out, "defines_fn")[0][1],
        "typescript:myapp::billing::default"
    );
}

#[test]
fn an_anonymous_default_exported_class_method_is_qualified_by_default() {
    let out = extract_typescript(
        "src/billing.ts",
        "export default class { charge() { return 1 } }\n",
        &ctx(&["src/billing.ts"]),
    );
    assert_eq!(
        edges_of(&out, "defines_fn")[0][1],
        "typescript:myapp::billing::default::charge"
    );
}

// ─── object-literal / class-expression method identity ────────────

#[test]
fn object_and_class_expression_methods_do_not_collide_with_a_free_function() {
    let out = extract_typescript(
        "src/billing.ts",
        "function m() {}\nconst obj = { m() {} };\nconst C = class { m() {} };\n",
        &ctx(&["src/billing.ts"]),
    );
    let mut identities: Vec<String> = edges_of(&out, "defines_fn")
        .into_iter()
        .map(|a| a[1].clone())
        .collect();
    identities.sort();
    assert_eq!(
        identities,
        vec![
            "typescript:myapp::billing::C::m".to_string(),
            "typescript:myapp::billing::m".to_string(),
            "typescript:myapp::billing::obj::m".to_string(),
        ]
    );
}

// ─── guards ─────────────────────────────────────────────────────

#[test]
fn a_non_typescript_file_yields_nothing() {
    assert_eq!(
        extract_typescript("src/a.rs", "fn f() {}", &ctx(&[])),
        Extracted::default()
    );
}

#[test]
fn unparseable_source_preserves_existing_evidence() {
    // Not an empty extraction: that would erase the file's edges and
    // report the graph fresh.
    let out = extract_typescript(
        "src/billing.ts",
        "function ((( {",
        &ctx(&["src/billing.ts"]),
    );
    assert!(out.parse_failed, "must signal parse failure");
    assert!(out.edges.is_empty());
}

// ─── tested_by ──────────────────────────────────────────────────

#[test]
fn a_test_callback_with_no_calls_still_has_an_independent_identity() {
    let out = extract_typescript(
        "tests/api.test.ts",
        "it('exists', () => { expect(true); });",
        &ctx(&["tests/api.test.ts"]),
    );
    assert_eq!(edges_of(&out, "defines_test").len(), 1);
}

#[test]
fn a_test_callback_records_what_it_calls() {
    // TS tests are callbacks, not named functions, so the coverage source is
    // identified by its title string — the only stable identity available.
    let out = extract_typescript(
        "src/billing.test.ts",
        "it('charges the order', () => { charge(cart) })\n",
        &ctx(&["src/billing.test.ts"]),
    );
    assert_eq!(
        edges_of(&out, "tested_by"),
        vec![vec![
            "charge".to_string(),
            "typescript:myapp::billing.test::charges the order".to_string()
        ]]
    );
}

#[test]
fn a_test_spelled_with_test_rather_than_it_also_counts() {
    let out = extract_typescript(
        "src/billing.test.ts",
        "test('charges', () => { charge() })\n",
        &ctx(&["src/billing.test.ts"]),
    );
    assert_eq!(edges_of(&out, "tested_by")[0][0], "charge");
}

#[test]
fn a_helper_outside_a_test_callback_is_not_coverage() {
    // A helper's calls are not evidence that anything was verified — the
    // same rule Python applies to non-`test_*` functions.
    let out = extract_typescript(
        "src/billing.test.ts",
        "function buildFixture() { charge() }\n",
        &ctx(&["src/billing.test.ts"]),
    );
    assert!(edges_of(&out, "tested_by").is_empty());
}

#[test]
fn a_method_call_inside_a_test_records_the_method_name() {
    let out = extract_typescript(
        "src/billing.test.ts",
        "it('works', () => { ledger.charge() })\n",
        &ctx(&["src/billing.test.ts"]),
    );
    assert_eq!(edges_of(&out, "tested_by")[0][0], "charge");
}

#[test]
fn a_test_wrapped_in_describe_still_counts() {
    // `describe` is not itself a test invocation, but does not block the
    // ordinary descent into its callback — the `it` inside is walked
    // exactly as if `describe` were not there.
    let out = extract_typescript(
        "src/billing.test.ts",
        "describe('group', () => { it('inner', () => { charge() }) })\n",
        &ctx(&["src/billing.test.ts"]),
    );
    assert_eq!(
        edges_of(&out, "tested_by"),
        vec![vec![
            "charge".to_string(),
            "typescript:myapp::billing.test::inner".to_string()
        ]]
    );
}

#[test]
fn a_test_nested_two_describes_deep_still_counts() {
    let out = extract_typescript(
        "src/billing.test.ts",
        "describe('outer', () => { describe('inner', () => { it('deep', () => { charge() }) }) })\n",
        &ctx(&["src/billing.test.ts"]),
    );
    assert_eq!(edges_of(&out, "tested_by")[0][0], "charge");
}

#[test]
fn an_async_test_callback_still_counts() {
    let out = extract_typescript(
        "src/billing.test.ts",
        "it('async works', async () => { await charge() })\n",
        &ctx(&["src/billing.test.ts"]),
    );
    assert_eq!(edges_of(&out, "tested_by")[0][0], "charge");
}

#[test]
fn a_skipped_test_is_not_coverage() {
    // `it.skip` never runs, so a call inside it is not evidence anything
    // was verified — same principle as the bare-helper case.
    let out = extract_typescript(
        "src/billing.test.ts",
        "it.skip('nope', () => { charge() })\n",
        &ctx(&["src/billing.test.ts"]),
    );
    assert!(edges_of(&out, "tested_by").is_empty());
}

#[test]
fn an_only_test_still_counts() {
    // `test.only` / `it.only` do run (exclusively), so they are coverage.
    let out = extract_typescript(
        "src/billing.test.ts",
        "test.only('yes', () => { charge() })\n",
        &ctx(&["src/billing.test.ts"]),
    );
    assert_eq!(
        edges_of(&out, "tested_by"),
        vec![vec![
            "charge".to_string(),
            "typescript:myapp::billing.test::yes".to_string()
        ]]
    );
}

#[test]
fn a_parameterized_each_test_counts() {
    // `it.each([...])(title, cb)` is a real, executed test — its outer
    // call's `function` field is itself a call expression (`it.each(...)`),
    // one level removed from the plain-identifier and `it.only` shapes.
    let out = extract_typescript(
        "src/billing.test.ts",
        "it.each([1, 2])('works %s', (n) => { charge(n) })\n",
        &ctx(&["src/billing.test.ts"]),
    );
    assert_eq!(
        edges_of(&out, "tested_by"),
        vec![vec![
            "charge".to_string(),
            "typescript:myapp::billing.test::works %s".to_string()
        ]]
    );
}

// ─── calls_api ──────────────────────────────────────────────────

#[test]
fn a_non_null_assertion_is_a_watched_api_call() {
    let out = extract_typescript(
        "src/billing.ts",
        "export function charge(o?: Order) { return o!.total }\n",
        &ctx(&["src/billing.ts"]),
    );
    assert_eq!(
        edges_of(&out, "calls_api"),
        vec![vec![
            "typescript:myapp::billing::charge".to_string(),
            "non_null_assertion".to_string()
        ]]
    );
}

#[test]
fn a_function_without_assertions_calls_no_watched_api() {
    let out = extract_typescript(
        "src/billing.ts",
        "export function charge(o: Order) { return o.total }\n",
        &ctx(&["src/billing.ts"]),
    );
    assert!(edges_of(&out, "calls_api").is_empty());
}

#[test]
fn several_assertions_in_one_function_yield_one_edge() {
    let out = extract_typescript(
        "src/billing.ts",
        "export function charge(o?: Order) { return o!.total + o!.tax }\n",
        &ctx(&["src/billing.ts"]),
    );
    assert_eq!(edges_of(&out, "calls_api").len(), 1);
}

#[test]
fn a_method_assertion_is_a_watched_api_call() {
    let out = extract_typescript(
        "src/billing.ts",
        "class Ledger { charge(o?: Order) { return o!.total } }\n",
        &ctx(&["src/billing.ts"]),
    );
    assert_eq!(
        edges_of(&out, "calls_api"),
        vec![vec![
            "typescript:myapp::billing::Ledger::charge".to_string(),
            "non_null_assertion".to_string()
        ]]
    );
}

#[test]
fn an_arrow_const_assertion_is_a_watched_api_call() {
    let out = extract_typescript(
        "src/billing.ts",
        "const charge = (o?: Order) => o!.total\n",
        &ctx(&["src/billing.ts"]),
    );
    assert_eq!(
        edges_of(&out, "calls_api"),
        vec![vec![
            "typescript:myapp::billing::charge".to_string(),
            "non_null_assertion".to_string()
        ]]
    );
}

#[test]
fn an_assertion_in_a_nested_named_function_does_not_count_toward_the_outer_function() {
    // Task 6 does not descend into a nested `function_declaration` for
    // `defines_fn` — `inner` gets no identity of its own inside `outer`.
    // Attributing `inner`'s assertion to `outer` would blame code that
    // never runs unless `outer` explicitly invokes `inner`, so the same
    // boundary applies to `calls_api`: deliberately not counted.
    let out = extract_typescript(
        "src/billing.ts",
        "export function outer() { function inner(o?: Order) { return o!.total } }\n",
        &ctx(&["src/billing.ts"]),
    );
    assert!(edges_of(&out, "calls_api").is_empty());
}

#[test]
fn an_assertion_inside_an_inline_callback_does_count_toward_the_enclosing_function() {
    // Unlike a nested *named* function, an anonymous arrow callback
    // (`.map(x => x!.y)`, an inline `const f = () => …` immediately
    // invoked, etc.) has no identity of its own and runs as part of the
    // enclosing function's own control flow — so its assertions belong
    // to the enclosing function.
    let out = extract_typescript(
        "src/billing.ts",
        "export function outer(o?: Order) { const f = () => o!.total; return f() }\n",
        &ctx(&["src/billing.ts"]),
    );
    assert_eq!(
        edges_of(&out, "calls_api"),
        vec![vec![
            "typescript:myapp::billing::outer".to_string(),
            "non_null_assertion".to_string()
        ]]
    );
}

// ─── fix round 1: wider tested_by recognition ──────────────────────

#[test]
fn a_concurrent_test_counts() {
    let out = extract_typescript(
        "src/billing.test.ts",
        "test.concurrent('t', () => { f() })\n",
        &ctx(&["src/billing.test.ts"]),
    );
    assert_eq!(edges_of(&out, "tested_by")[0][0], "f");
}

#[test]
fn a_failing_test_counts() {
    // `it.failing` still executes its callback (the test passes iff it
    // throws) — the callback running is what matters for coverage.
    let out = extract_typescript(
        "src/billing.test.ts",
        "it.failing('t', () => { f() })\n",
        &ctx(&["src/billing.test.ts"]),
    );
    assert_eq!(edges_of(&out, "tested_by")[0][0], "f");
}

#[test]
fn a_sequential_test_counts() {
    let out = extract_typescript(
        "src/billing.test.ts",
        "test.sequential('t', () => { f() })\n",
        &ctx(&["src/billing.test.ts"]),
    );
    assert_eq!(edges_of(&out, "tested_by")[0][0], "f");
}

#[test]
fn a_todo_test_has_no_callback_and_records_no_coverage_or_skip() {
    // `it.todo('t')` is a placeholder by convention — no callback is
    // ever expected, so this is not an analysis gap and must not
    // inflate `skipped`.
    let out = extract_typescript(
        "src/billing.test.ts",
        "it.todo('not written yet')\n",
        &ctx(&["src/billing.test.ts"]),
    );
    assert!(edges_of(&out, "tested_by").is_empty());
    assert_eq!(out.skipped, 0);
}

#[test]
fn a_reference_callback_records_no_coverage_but_is_counted_as_skipped() {
    // `it('t', handler)` — a named-reference callback. We cannot
    // follow the reference to know what it calls, so this must not
    // silently look like a clean "no coverage" the way `it.todo` does.
    let out = extract_typescript(
        "src/billing.test.ts",
        "it('t', handler)\n",
        &ctx(&["src/billing.test.ts"]),
    );
    assert!(edges_of(&out, "tested_by").is_empty());
    assert_eq!(out.skipped, 1);
}

#[test]
fn a_quoted_title_containing_the_opposite_quote_char_keeps_its_closing_char() {
    // Regression: `trim_matches` against the whole quote set ate the
    // title's own trailing `"` in `'has "quotes"'`. Only the specific
    // delimiter the literal opened with should be stripped.
    let out = extract_typescript(
        "src/billing.test.ts",
        "it('has \"quotes\"', () => { charge() })\n",
        &ctx(&["src/billing.test.ts"]),
    );
    assert_eq!(
        edges_of(&out, "tested_by"),
        vec![vec![
            "charge".to_string(),
            "typescript:myapp::billing.test::has \"quotes\"".to_string()
        ]]
    );
}

// ─── fix round 1: defines_fn survives inside a test callback ───────

#[test]
fn a_helper_defined_inside_a_test_callback_still_gets_a_defines_fn_edge() {
    // Regression: the `call_expression` arm used to `return` after
    // emitting `tested_by`, skipping the ordinary descent into the
    // callback's own body — silently losing `defines_fn` for anything
    // defined inside a test (Task 6's contract, which this task must
    // not change). Harmless once restored: `file_type` for this file
    // is "test", and `warn-untested-risky-call` gates on
    // `file_type(?file, "production")`.
    let out = extract_typescript(
        "src/billing.test.ts",
        "it('t', () => { const h = () => g(); h() })\n",
        &ctx(&["src/billing.test.ts"]),
    );
    assert_eq!(
        edges_of(&out, "defines_fn")[0][1],
        "typescript:myapp::billing.test::h"
    );
}

#[test]
fn a_class_defined_inside_a_test_callback_still_gets_defines_fn_edges() {
    let out = extract_typescript(
        "src/billing.test.ts",
        "it('t', () => { class C { m() {} } })\n",
        &ctx(&["src/billing.test.ts"]),
    );
    assert_eq!(
        edges_of(&out, "defines_fn")[0][1],
        "typescript:myapp::billing.test::C::m"
    );
}

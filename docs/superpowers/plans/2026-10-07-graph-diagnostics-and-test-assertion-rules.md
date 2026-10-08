# Graph Diagnostics, Rhai Boundary Spec, and Test Assertion Rules Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Address key feedback on Phronesis by strengthening test assertion rules with `#[should_panic]` awareness and broad macro matching with whole-tree audit support, drafting a formal specification for the Rust ↔ Rhai code graph boundary, and enhancing MCP graph query tools with diagnostic triage on empty results and overview discovery.

**Architecture:** 
1. Enhance the Rust syntax tree-sitter AST visitor in `syntax/rust/assertions.rs` to treat `#[should_panic]` attributes as explicit panic expectations and generalize assertion macro detection to match all `assert*` prefixed macros (e.g., `assert_that!`, `assert_ok!`, `insta::assert_snapshot!`). Mark `warn-empty-test` with `"audit": true` in the Rust rule pack and `.phronesis/rules.json`.
2. Author `docs/specs/SPEC-rhai-code-graph-boundary.md` defining the schema, edge contracts, and bridge architecture for bidirectional Rust ↔ Rhai static analysis in the structural code graph.
3. Extend `crates/phronesis-mcp/src/graph/query.rs` and `server.rs` (`query_code_graph`) with diagnostic introspection: when a query matches 0 edges, return structured diagnostic triage (unknown relation detection, closest known relations, sample argument patterns for existing relations, and substring/glob suggestions) plus high-level graph overview support.

**Tech Stack:** Rust 2024 edition, `tree-sitter`, `tree-sitter-rust`, `serde_json`, `phronesis` RETE engine, Model Context Protocol (`rmcp`).

**Spec:** `docs/specs/SPEC-rhai-code-graph-boundary.md` (authored in Task 2) and `crates/phronesis-mcp/CLAUDE.md`.

## Global Constraints

- Never use `.unwrap()` in production paths (use `?`, `unwrap_or`, or pattern matching).
- Follow Rust coding guidelines: `cargo clippy --workspace -- -D warnings` must pass cleanly.
- Keep module sizes under 800 LOC.
- Preserve backward-compatible JSON schema for MCP tools.

## Review Focus

1. `#[should_panic]` test functions with no macros in body must not be flagged as `test_without_assertion`.
2. Tests using third-party assertion macros (like `assert_that!`, `assert_json_eq!`, `claim::assert_ge!`) must not be flagged as `test_without_assertion`.
3. Whole-tree audit (`phr-mcp audit`) must evaluate `warn-empty-test` across all test files when enabled.
4. `query_code_graph` with an unknown relation must return `total: 0` along with structured diagnostics suggesting valid relations.
5. `query_code_graph` with a valid relation but unmatched args must return sample argument shapes and glob suggestions without panicking.

---

### Task 1: Enhance `test_without_assertion` AST extraction and enable audit in `warn-empty-test`

**Files:**
- Modify: `crates/phronesis-mcp/src/syntax/rust/assertions.rs`
- Modify: `crates/phronesis-mcp/src/syntax/rust/walk.rs` (if helper needed for attributes)
- Modify: `crates/phronesis-mcp/src/init/rules_rust.rs`
- Modify: `.phronesis/rules.json`

**Interfaces:**
- Consumes: `tree_sitter::Node`, `ParsedFile`, `super::walk::{function_name, is_test_fn}`
- Produces: `extract_tests_without_assertion(&ParsedFile) -> Vec<String>` (updated with `should_panic` & prefix match)

- [ ] **Step 1: Write failing unit tests for `#[should_panic]` and `assert_*` macros**

Add unit tests to `crates/phronesis-mcp/src/syntax/rust/assertions.rs`:
```rust
#[test]
fn test_with_should_panic_attribute_is_not_flagged() {
    let code = "#[test]\n#[should_panic]\nfn expects_panic() { call_risky(); }";
    let facts = extract(code);
    assert!(facts.tests_without_assertion.is_empty(), "test with #[should_panic] should not be flagged");
}

#[test]
fn test_with_custom_assert_macro_is_not_flagged() {
    let code = "#[test]\nfn custom_macro() { assert_that!(val, is_ok()); }";
    let facts = extract(code);
    assert!(facts.tests_without_assertion.is_empty(), "assert_that! macro should count as assertion");
}

#[test]
fn test_with_scoped_custom_assert_is_not_flagged() {
    let code = "#[test]\nfn scoped() { insta::assert_snapshot!(val); }";
    let facts = extract(code);
    assert!(facts.tests_without_assertion.is_empty(), "insta::assert_snapshot! should count as assertion");
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test -p phronesis-mcp -- test_with_should_panic_attribute_is_not_flagged`
Expected: FAIL

- [ ] **Step 3: Implement `is_should_panic_fn` and generalized assertion macro matching**

In `crates/phronesis-mcp/src/syntax/rust/assertions.rs`:
1. Check if the function item carries `#[should_panic]` (or `#[should_panic(...)]`).
2. Update `has_assertion_or_exception`: in addition to exact `ASSERTION_MACROS`, accept any bare macro identifier that:
   - Starts with `assert` (e.g. `assert_that`, `assert_json_eq`, `assert_err`)
   - Or ends with `_assert` (e.g. `prop_assert`)
   - Or is in `ASSERTION_MACROS` (like `panic`, `unreachable`, `todo`).
3. If `is_should_panic_fn(state, source)`, do not treat the test as missing assertions.

- [ ] **Step 4: Enable `"audit": true` on `warn-empty-test`**

In `crates/phronesis-mcp/src/init/rules_rust.rs` and `.phronesis/rules.json`:
Add `"audit": true` to the `warn-empty-test` rule object.

- [ ] **Step 5: Run tests and clippy**

Run: `cargo test -p phronesis-mcp -- assertions`
Run: `cargo clippy -p phronesis-mcp -- -D warnings`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/phronesis-mcp/src/syntax/rust/assertions.rs crates/phronesis-mcp/src/init/rules_rust.rs .phronesis/rules.json
git commit -m "feat(rules): support should_panic, generalized assertion macros, and audit for empty tests"
```

---

### Task 2: Author Formal Specification for the Rust ↔ Rhai Code Graph Boundary

**Files:**
- Create: `docs/specs/SPEC-rhai-code-graph-boundary.md`

**Interfaces:**
- Documents the schema, extraction rules, graph relations, gap mitigation, and validation strategies for Rust ↔ Rhai interoperability in Phronesis.

- [ ] **Step 1: Draft the specification**

Cover:
1. **Background & Motivation:** Why the Rust ↔ Rhai boundary creates graph blind spots, and the risks of runtime failures from renamed or missing bindings.
2. **Current Graph Representation:** Analysis of existing extractors (`crates/phronesis-mcp/src/graph/rhai.rs` and `extract.rs`): `calls`, `exposes`, `rhai:callable::<name>`, `rhai_callable_backing`, `loads_rhai_script`.
3. **Identified Blind Spots & Gaps:**
   - Single-statement registration limitations (`register_fn` vs `register_static_module`, `register_type_with_name`, `register_custom_operator`).
   - Dynamic closures and helper abstractions.
   - Cross-script Rhai imports (`import ... as ...`).
   - Predicate lifecycle: `emit_fact` to RETE fact ingestion.
4. **Formal Data Model & Graph Edge Contracts:**
   - Base edges: `exposes`, `rhai_callable_backing`, `loads_rhai_script`, `rhai_calls`, `rhai_emits_predicate`.
   - Derived edges: `resolves_to`, `runtime_reachable`, `rhai_implements_predicate`, `rhai_unresolved_callable`, `unused_rhai_export`.
5. **Static Analysis & Tooling Architecture:**
   - AST extraction refinements for Rust module-level registrations.
   - Diagnostic validation during whole-tree audit.
   - MCP tool exposure and query patterns.

- [ ] **Step 2: Review and verify against existing codebase specs**

Confirm alignment with `SPEC-call-graph-and-suppression-rules.md` and `docs/superpowers/specs/2026-06-01-rhai-script-evaluator-design.md`.

- [ ] **Step 3: Commit**

```bash
git add docs/specs/SPEC-rhai-code-graph-boundary.md
git commit -m "docs(specs): formalize Rust-Rhai code graph boundary specification"
```

---

### Task 3: Enhance Code Graph Query Diagnostics and Overview

**Files:**
- Modify: `crates/phronesis-mcp/src/graph/query.rs`
- Modify: `crates/phronesis-mcp/src/server.rs`
- Modify: `crates/phronesis-mcp/src/server_params.rs` (if new parameter like `overview` or `verbose` is added)
- Test: `crates/phronesis-mcp/src/graph/query.rs` (unit tests)
- Test: `crates/phronesis-mcp/tests/save_rules_integration.rs` or new integration test

**Interfaces:**
- Consumes: `&[Edge]`, `&Pattern`
- Produces: `query_diagnostics(&[Edge], &Pattern) -> Option<QueryDiagnostics>`
- Exposes: diagnostic triage object in `query_code_graph` result when `total == 0`.

- [ ] **Step 1: Write failing tests for graph query diagnostics**

In `crates/phronesis-mcp/src/graph/query.rs`:
```rust
#[test]
fn unknown_relation_suggests_existing_relations() {
    let edges = graph();
    let pat = Pattern::parse(&toks(&["callz", "*"]));
    let diag = query_diagnostics(&edges, &pat).expect("diagnostics should be present for unknown relation");
    assert!(diag.unknown_relation);
    assert!(diag.suggested_relations.contains(&"defines_fn".to_string()) || diag.suggested_relations.contains(&"tested_by".to_string()));
}

#[test]
fn unmatched_arguments_suggests_sample_patterns_and_globs() {
    let edges = graph();
    let pat = Pattern::parse(&toks(&["tested_by", "nonexistent_fn"]));
    let diag = query_diagnostics(&edges, &pat).expect("diagnostics should be present when edges exist for relation");
    assert!(!diag.unknown_relation);
    assert_eq!(diag.relation_edge_count, 2);
    assert!(!diag.sample_argument_patterns.is_empty());
}
```

- [ ] **Step 2: Run test to verify failure**

Run: `cargo test -p phronesis-mcp -- unknown_relation_suggests_existing_relations`
Expected: FAIL

- [ ] **Step 3: Implement `query_diagnostics` in `crates/phronesis-mcp/src/graph/query.rs`**

Define:
```rust
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct QueryDiagnostics {
    pub unknown_relation: bool,
    pub relation_edge_count: usize,
    pub suggested_relations: Vec<String>,
    pub sample_argument_patterns: Vec<Vec<String>>,
    pub suggestions: Vec<String>,
}
```
Implement logic:
1. If `pattern.relation` is not in the set of existing relations in `edges`:
   - Set `unknown_relation = true`.
   - Find candidate relations using Levenshtein / substring similarity, plus top relations by count.
   - Suggest: `"Relation '{r}' not found. Available relations: ..."`
2. If `pattern.relation` exists in `edges`:
   - Calculate total edges for that relation (`relation_edge_count`).
   - Extract up to 3 distinct sample argument shapes from existing edges of that relation.
   - For each query arg, check if any edge contains that argument as a substring; if so, suggest: `"Try glob pattern '*{arg}*' to match qualified identifiers"`.

- [ ] **Step 4: Integrate diagnostics into `query_code_graph` in `crates/phronesis-mcp/src/server.rs`**

When `total == 0`:
Compute `q::query_diagnostics(&edges, &pattern)` and include it under `"diagnostics"` in the JSON response:
```json
{
  "total": 0,
  "returned": 0,
  "truncated": false,
  "results": [],
  "diagnostics": { ... }
}
```

- [ ] **Step 5: Run tests and clippy**

Run: `cargo test -p phronesis-mcp -- query`
Run: `cargo clippy -p phronesis-mcp -- -D warnings`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/phronesis-mcp/src/graph/query.rs crates/phronesis-mcp/src/server.rs
git commit -m "feat(graph): add diagnostic triage and pattern suggestions to query_code_graph"
```

---

### Task 4: Whole Workspace Verification and Validation

- [ ] **Step 1: Run whole workspace tests**

Run: `cargo test --workspace`
Expected: PASS

- [ ] **Step 2: Run workspace clippy with `-D warnings`**

Run: `cargo clippy --workspace -- -D warnings`
Expected: PASS

- [ ] **Step 3: Run format check**

Run: `cargo fmt --all -- --check`
Expected: PASS

- [ ] **Step 4: Test `phr-mcp audit` and query tool via integration run**

Verify `audit` evaluates `warn-empty-test` cleanly and `query_code_graph` returns diagnostics on empty matches.

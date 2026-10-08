# SPEC-rhai-code-graph-boundary: Rust ↔ Rhai Interoperability in the Code Graph

**Author:** Phronesis Governance Team  
**Status:** Proposed  
**Date:** 2026-10-07  
**Supersedes:** Informal Rhai notes in `SPEC-call-graph-and-suppression-rules.md`  
**Tracking Issue:** Code Graph Rust-Rhai Boundary Support  

---

## 1. Abstract

Phronesis embeds the Rhai scripting engine for user-defined guard conditions (`__script__`), dynamic LHS predicate providers (`.phronesis/predicates/*.rhai`), and downstream game/simulation mechanics. While the core RETE engine evaluates facts and rules deterministically, the structural code graph (`graph.jsonl`) historically treated Rhai scripts and their host Rust runtime as largely disconnected silos.

This specification formalizes the **Rust ↔ Rhai boundary in the Phronesis code graph**. It defines the static extraction contracts for both Rust host registration and Rhai script consumption, establishes the bidirectional edge ontology, defines derived reachability and dead-code relations, and outlines audit diagnostics to eliminate cross-language blind spots.

---

## 2. Motivation & Problem Statement

### 2.1 The Cross-Language Blind Spot
In hybrid Rust/Rhai applications:
1. **Host-Side Registrations:** Rust code defines functions and types, then registers them with an `Engine` instance (e.g., via `engine.register_fn(...)`, module builders, or macro expansions).
2. **Script-Side Invocations:** Rhai scripts invoke functions by name in a dynamically-typed scripting context and emit facts via `emit_fact("predicate", [...])`.

When the code graph only tracks Rust-to-Rust symbol references (`defines_fn`, `calls`, `imports`), several failure classes emerge:
* **Silent Runtime Breakage:** Renaming or changing the parameter signature of a Rust function causes downstream Rhai scripts to fail at runtime with `Function not found` errors, unnoticed by `cargo check` or static AST linters.
* **Dead Host Exports:** Rust functions developed and registered exclusively for Rhai scripts become orphaned when scripts are refactored or deleted, yet continue accumulating compile-time and binary maintenance cost.
* **Unresolvable Script Callables:** Rhai scripts invoke function names that are neither local Rhai functions nor known host-registered APIs.
* **Orphaned Predicate Providers:** A Rhai provider script emits a predicate (`emit_fact("foo_bar", ...)`) that no active RETE rule condition listens for, wasting evaluation cycles at hook time.

---

## 3. Current Implementation & Limitations

### 3.1 Existing Extractors
Phronesis currently implements conservative structural extraction in:
* `crates/phronesis-mcp/src/graph/rhai.rs`: Extracts literal `calls` from `.rhai` files to synthetic target `rhai:callable::{name}`, plus `rhai_emits_predicate` from `emit_fact(...)`.
* `crates/phronesis-mcp/src/graph/extract.rs`: Searches Rust AST for direct `engine.register_fn("literal_name", backing_fn)` method calls and legacy `register_state_proxy!` macro patterns, emitting `exposes` and `rhai_callable_backing`.
* `crates/phronesis-mcp/src/graph/derive.rs`: Matches `calls` to `exposes` in `rhai_reachability` to emit `resolves_to` and `runtime_reachable`.

### 3.2 Gaps in Current Coverage
1. **Opaque Module Registrations:** Modern Rhai applications register modules hierarchically (`engine.register_static_module(...)`, `combine_with_exported_module`, or `rhai::exported_module!`), which are ignored by the single-call `register_fn` extractor.
2. **Custom Type & Operator Registrations:** Type methods registered via `engine.register_type_with_name::<T>("T")` or getters/setters (`register_get_set`) produce no code-graph entries.
3. **Cross-Script Invocations:** Rhai scripts that import and invoke symbols from other scripts (`import "helpers" as h; h::calc()`) are not connected to the imported script's module entity.
4. **Silent Dropping in Derivation:** If an edge cannot be resolved with certainty, `derive.rs` currently silently ignores it or emits minimal diagnostics (`ambiguous_backing`), leaving the graph queryable surface empty without triage advice.

---

## 4. Formal Data Model & Edge Ontology

### 4.1 Identifiers and Entity Naming

| Entity | Canonical Pattern | Example |
|--------|-------------------|---------|
| **Rhai Script Module** | `rhai:<unit>::<relative_path_without_ext>` | `rhai:phronesis::predicates::change_set` |
| **Rhai Callable Identifier** | `rhai:callable::<name>` | `rhai:callable::emit_fact`, `rhai:callable::parse_patch` |
| **Rhai Script Function** | `rhai:<unit>::<path>::<fn_name>` | `rhai:core::scripts::helpers::calculate_score` |
| **Rust Function** | `rust:<crate>::<module>::<fn_name>` | `rust:phronesis_mcp::syntax::facts::extract` |
| **RETE Rule** | `<rule_id>` | `warn-empty-test` |
| **Predicate** | `<predicate_name>` | `change_set_production_rust` |

### 4.2 Base Extracted Edges (Level 0)

1. `exposes(module, callable)`
   * **Subject:** Rust module registering the symbol (`rust:...`).
   * **Object:** Callable identifier (`rhai:callable::<name>`).
   * **Evidence:** Literal `register_fn`, `register_type_with_name`, or procedural module macro attribute in Rust AST.

2. `rhai_callable_backing(callable, rust_symbol)`
   * **Subject:** Callable identifier (`rhai:callable::<name>`).
   * **Object:** Backing Rust function name or qualified path (`rust:...` or bare name).
   * **Evidence:** The second argument expression passed to `register_fn` or method target in Rust.

3. `loads_rhai_script(rust_module, script_file)`
   * **Subject:** Rust module invoking script compilation or evaluation (`rust:...`).
   * **Object:** Relative path to the `.rhai` script.
   * **Evidence:** `compile_file`, `compile`, or `eval_file` call with literal path in Rust AST.

4. `calls(script_module, target)`
   * **Subject:** Rhai script module (`rhai:...`).
   * **Object:** `rhai:callable::<name>` (for host calls) or `rhai:<module>::<fn_name>` (for local/imported script calls).
   * **Evidence:** Call expression in Rhai script AST outside known language keywords and local definitions.

5. `rhai_emits_predicate(script_module, predicate)`
   * **Subject:** Rhai script module (`rhai:...`).
   * **Object:** Rule predicate string.
   * **Evidence:** `emit_fact("<predicate>", ...)` expression in Rhai script AST.

6. `rhai_imports_module(script_module, target_script)`
   * **Subject:** Rhai script module (`rhai:...`).
   * **Object:** Imported script module (`rhai:...`).
   * **Evidence:** `import "<path>" as <alias>` statement in Rhai script AST.

### 4.3 Derived Edges (Level 1)

1. `resolves_to(callable, rust_symbol)`
   * Emitted when `rhai:callable::<name>` has exactly one unambiguously identified backing definition in Rust.
2. `runtime_reachable(rust_symbol, script_module)`
   * Emitted when `script_module` calls `callable`, and `callable` resolves to `rust_symbol`. Establishes that changing `rust_symbol` has runtime impact on `script_module`.
3. `rhai_implements_predicate(script_module, predicate, rule_id)`
   * Emitted when `script_module` emits `predicate`, and `rule_id` contains a condition matching `predicate`.
4. `rhai_unresolved_callable(script_module, callable)`
   * Emitted when a Rhai script invokes a callable that is neither defined in the script, imported from another script, nor exposed by any registered host module.
5. `unused_rhai_export(rust_module, callable)`
   * Emitted when a Rust module exposes `callable`, but no Rhai script in the codebase calls it, and no test exercises it through the scripting bridge.

---

## 5. Static Extraction Architecture

```
+------------------------------------+       +------------------------------------+
|             Rust Source            |       |            Rhai Source             |
+------------------------------------+       +------------------------------------+
                  |                                             |
                  v                                             v
      Tree-sitter Rust Parser                       Tree-sitter / Regex Lexer
                  |                                             |
                  v                                             v
   - engine.register_fn(...)                     - fn local_func(...) { ... }
   - #[rhai_fn] exports                         - host_func(...)
   - loads_rhai_script(...)                      - emit_fact("pred", ...)
                  |                                             |
                  +----------------------+----------------------+
                                         |
                                         v
                         Code Graph Store (`graph.jsonl`)
                                         |
                                         v
                           Derivation Pass (`derive.rs`)
                                         |
            +----------------------------+----------------------------+
            |                            |                            |
            v                            v                            v
   `runtime_reachable`         `rhai_unresolved_callable`      `unused_rhai_export`
```

### 5.1 Rust Host Extraction Rules
1. **Direct Function Registrations:**
   Detect method calls `.register_fn(name_lit, func_ref)`. Extract string literal `name_lit` and resolve `func_ref` against in-scope function items.
2. **Procedural Module Exports:**
   Recognize `#[rhai::plugin::exported_module]` or `rhai::exported_module!` declarations. Extract each `pub fn` within the module as an exposed symbol with module-prefixed callable names.
3. **Type Registrations:**
   Detect `.register_type_with_name::<T>(name)` and treat methods registered on `T` as scoped callables `rhai:callable::<T>::<method>`.

### 5.2 Rhai Script Extraction Rules
1. **Normalized Local Filtering:**
   Filter out standard Rhai language keywords (`if`, `for`, `while`, `switch`, `return`, `throw`, etc.) and functions defined within the script body (`fn ...(...)`).
2. **Aliased Module Calls:**
   Map `alias::func()` calls back to the imported target if `import "path" as alias;` exists in scope.
3. **Predicate Assertion Tracking:**
   Track both literal strings and simple identifier constants passed as the first parameter to `emit_fact(...)`.

---

## 6. Audit & Governance Rules

The boundary edges enable three automated governance rules:

### 6.1 `warn-rhai-unresolved-callable`
* **Phase:** `post` / `audit`
* **Condition:** `{"rhai_unresolved_callable": ["?script", "?callable"]}`
* **Action:** Warn that `?script` invokes `?callable` which is not provided by any known host registration. Prevents runtime `Function not found` exceptions.

### 6.2 `audit-unused-rhai-export`
* **Phase:** `audit`
* **Condition:** `{"unused_rhai_export": ["?module", "?callable"]}`
* **Action:** Flag Rust functions registered for Rhai consumption that are completely uncalled. Prevents dead API creep.

### 6.3 `warn-rhai-orphaned-predicate`
* **Phase:** `post` / `audit`
* **Condition:** `{"rhai_orphaned_predicate": ["?script", "?pred"]}`
* **Action:** Warn when a Rhai predicate provider emits a fact that is never consumed by any rule in `.phronesis/rules.json`.

### 6.4 Suppression & Dynamic Evaluation Discipline
* **Dynamic Code (`eval`, variable function dispatch):** Calls made through dynamic reflection or runtime strings cannot be statically guaranteed. Rules `warn-rhai-unresolved-callable` and `audit-unused-rhai-export` are diagnostic advisories.
* **Suppression Mechanism:** Intentional framework hooks, test-only script registrations, and dynamic evaluation points can be opted out using standard Phronesis inline rule filters or `.phronesisignore` for script paths.

---

## 7. Migration & Rollout Plan

1. **Phase 1 (Documentation & Schema Definition):** This specification.
2. **Phase 2 (Extractor Refinement):** Extend `crates/phronesis-mcp/src/graph/extract.rs` to support Rhai module exports and type registrations.
3. **Phase 3 (Diagnostic Edge Derivation):** Implement `rhai_unresolved_callable` and `unused_rhai_export` in `derive.rs`.
4. **Phase 4 (Pack Rules):** Add `warn-rhai-unresolved-callable` and `audit-unused-rhai-export` to the `rhai` starter pack.

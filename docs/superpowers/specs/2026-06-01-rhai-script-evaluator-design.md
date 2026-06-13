# Rhai Script Evaluator for Phronesis

**Date**: 2026-06-01
**Status**: Draft
**Branch**: feat/wiki-drift (to be moved to its own branch for implementation)

## Problem

Phronesis's `ScriptEvaluator` claims Rhai support in its docstrings but is actually a hand-rolled parser supporting only two primitives: `facts_contain` and `facts_count`. The `__script__` condition plumbing in the RETE network is correctly wired (conditions are evaluated before agenda insertion, unsupported syntax blocks the rule), but any script beyond these two patterns fails with "Unknown script expression."

Downstream consumers need expressive guard conditions (inventory checks, numeric comparisons on fact arguments, boolean combinators) that can't be expressed with the current DSL. They've worked around this by pre-filtering facts in Rust before asserting them — shifting guard logic out of the rule engine entirely.

## Design Decisions

### 1. Separate crate, not core dependency

Rhai lives in a new `phronesis-rhai` crate. The core `phronesis` crate stays minimal (serde, tracing, uuid only). A `ScriptEval` trait in core defines the contract; `phronesis-rhai` provides the Rhai implementation.

**Rationale**: Core is intentionally dependency-free. The trait boundary was already anticipated in `script_evaluator.rs:12`: "A future richer scripting layer (real rhai, wasm, etc.) would plug in behind a trait."

### 2. Facts and bindings only in Rhai scope

Rhai scripts see two variables:
- `facts` — array of maps, each with `predicate` (string) and `args` (array of strings)
- `bindings` — map of string to string (RETE variable bindings like `?player` -> `"alice"`)

No access to WME metadata, rule list, agenda state, or engine internals.

**Rationale**: Clean, predictable, easy to reason about. Matches the current DSL's scope. Engine internals are implementation details that scripts shouldn't couple to.

### 3. Full sandbox

Rhai engine configured with:
- `Engine::new_raw()` — no standard library by default
- Selectively register: arithmetic, logic, string, array packages
- `set_max_operations(100_000)`
- `set_max_call_stack_depth(16)`
- `set_max_string_size(4096)`
- No file I/O, no network, no closures, no modules

**Rationale**: Script conditions run on every rule evaluation. A malformed or malicious script must not hang the engine or access the filesystem.

### 4. Core DSL and Rhai evaluator are independent

The existing `BuiltinScriptEvaluator` (renamed from `ScriptEvaluator`) stays in core unchanged. `RhaiScriptEvaluator` in the new crate is a completely separate implementation. Consumers choose which to wire in. There is no fast-path fallback chain — if you use Rhai, all `__script__` conditions go through Rhai.

**Rationale**: Simpler mental model. No hidden dispatch logic. If a consumer wants both, they can write a composite evaluator.

## Architecture

### Core crate changes (`phronesis`)

**New trait** in `src/script_evaluator.rs`:

```rust
pub trait ScriptEval: Send + Sync + std::fmt::Debug {
    fn evaluate(
        &self,
        script: &str,
        facts: &[Fact],
        bindings: &HashMap<String, String>,
    ) -> Result<bool, String>;
}
```

**Rename**: `ScriptEvaluator` -> `BuiltinScriptEvaluator`, implements `ScriptEval`.

**`ReteNetwork` changes**:
- Field: `script_evaluator: ScriptEvaluator` -> `script_evaluator: Box<dyn ScriptEval>`
- Existing `new()` defaults to `Box::new(BuiltinScriptEvaluator)`
- New constructor: `with_script_evaluator(evaluator: Box<dyn ScriptEval>)`

**Re-exports** from `lib.rs`: `ScriptEval`, `BuiltinScriptEvaluator`.

**Docstring fixes**: Remove misleading "Rhai" references from core. The module doc and struct doc should describe what the builtin evaluator actually does.

### New crate (`phronesis-rhai`)

```
crates/phronesis-rhai/
  Cargo.toml
  src/
    lib.rs          # RhaiScriptEvaluator + re-exports
```

**`Cargo.toml`**:

```toml
[package]
name = "phronesis-rhai"
version = "0.1.0"
edition = "2024"
rust-version = "1.90"
description = "Rhai script evaluator for phronesis RETE engine."
license = "MIT"

[dependencies]
phronesis = { path = "../phronesis" }
rhai = { version = "1", features = ["no_module", "no_closure"] }
```

**`RhaiScriptEvaluator`**:
- Owns a `rhai::Engine` configured at construction (sandbox limits applied once)
- `evaluate()` creates a fresh `rhai::Scope` per call, pushes `facts` and `bindings`, evaluates the script string
- `facts` is pushed as `rhai::Array` of `rhai::Map` where each map has keys `"predicate"` (Dynamic string) and `"args"` (Dynamic array of strings). Example Rhai access: `facts[0].predicate == "inventory"`, `facts[0].args.contains("sword")`
- `bindings` is pushed as `rhai::Map` with string keys/values. Example: `bindings["?player"] == "alice"`
- Script must return `bool`. Non-bool return -> `Err`. Rhai runtime error -> `Err`.
- The `Engine` is reused across calls (it's just configuration + registered functions)

### MCP crate integration (`phronesis-mcp`)

**Optional dependency**:

```toml
[features]
rhai = ["dep:phronesis-rhai"]

[dependencies]
phronesis-rhai = { path = "../phronesis-rhai", optional = true }
```

**Wiring**: When `rhai` feature is enabled, construct `ReteNetwork::with_script_evaluator(Box::new(RhaiScriptEvaluator::new()))`. Otherwise, default constructor.

**No rules.json format changes**: The `__script__` condition syntax already carries arbitrary strings. Scripts that previously failed silently will now actually evaluate.

**No MCP tool surface changes**: No new tools, no protocol changes.

## Testing Strategy

### Layer 1: Unit tests in `phronesis-rhai`

Test `RhaiScriptEvaluator` in isolation against the `ScriptEval` trait:
- Simple boolean expressions (`true`, `false`, `1 > 0`)
- Fact inspection: `facts.len() > 0`, iterating facts to check predicates and args
- Binding access: `bindings["?player"] == "alice"`
- Compound logic: `facts.len() > 2 && bindings.contains_key("?name")`
- Sandbox enforcement: script exceeding max operations returns `Err`
- Non-bool return produces `Err`
- Empty script / syntax error produces `Err`

### Layer 2: Integration tests in `phronesis-rhai`

Full RETE round-trip — the test that doesn't exist today:
- Create `ReteNetwork::with_script_evaluator(Box::new(RhaiScriptEvaluator::new()))`
- Add a rule with both a normal predicate condition and a `__script__` condition
- Assert facts, call `update_agenda()` + `fire_rules()`
- Verify: script returning `false` -> rule does NOT fire
- Verify: script returning `true` -> rule fires and produces consequences
- Verify: script error -> rule does NOT fire (safe default)

### Layer 3: Existing tests untouched

- `script_evaluator_tests.rs` continues testing `BuiltinScriptEvaluator`
- `phronesis-mcp` rules_file tests continue testing JSON round-trip
- Existing RETE tests in core keep working (they use default constructor)

## Scope

### In scope
- `ScriptEval` trait in core
- `BuiltinScriptEvaluator` rename + trait impl
- `ReteNetwork::with_script_evaluator()` constructor
- `phronesis-rhai` crate with `RhaiScriptEvaluator`
- Feature flag in `phronesis-mcp`
- Fix misleading "Rhai" docstrings in core
- Tests at all three layers

### Not in scope
- Changing any existing rule in `.phronesis/rules.json`
- Migrating `phronesis-mcp`'s own rules to use Rhai scripts
- Rhai helper functions (e.g., `has_fact()` convenience) — future enhancement
- The consumer migration to Rhai conditions — that's the consumer's P5/P6 work, blocked on this landing

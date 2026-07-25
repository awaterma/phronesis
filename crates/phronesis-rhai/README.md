# phronesis-rhai

A [Rhai](https://rhai.rs) implementation of phronesis's `ScriptEval` trait —
expressive `__script__` guard conditions for the
[phronesis](https://crates.io/crates/phronesis) RETE engine.

The core `phronesis` crate ships a small, dependency-free
`BuiltinScriptEvaluator` supporting only `facts_contain` and `facts_count`.
Rules that need numeric comparisons, boolean combinators, or array/string
inspection over fact arguments have to pre-filter facts in Rust before
asserting them. This crate removes that workaround: wire in
`RhaiScriptEvaluator` and write the guard directly in the rule.

## Usage

```rust
use phronesis::ReteNetwork;
use phronesis_rhai::RhaiScriptEvaluator;

let network = ReteNetwork::with_script_evaluator(Box::new(RhaiScriptEvaluator::new()));
```

Every `__script__` condition in the network is then evaluated as a Rhai
expression.

## Script scope

Each script sees exactly two variables and must return a `bool`:

| Variable   | Shape                                                        | Example                                   |
|------------|-------------------------------------------------------------|-------------------------------------------|
| `facts`    | array of `#{ predicate: string, args: [string, ...] }` maps | `facts[0].args[1].parse_int() >= 5`       |
| `bindings` | map of RETE variable name → bound value                     | `bindings["?player"] == "alice"`          |

```rhai
// "some inventory fact has quantity >= 5"
facts.some(|f| f.predicate == "inventory" && f.args[1].parse_int() >= 5)
```

A non-`bool` return, a syntax error, or a sandbox-limit breach yields an
error, which the network treats as a **blocked** condition — a broken guard
never silently passes.

## Sandbox

The engine is built from `Engine::new_raw()` with only Rhai's standard
package registered (arithmetic, logic, strings, arrays, maps — no file,
network, `eval`, modules, or closures) plus hard limits: 100k operations, a
call depth of 16, and a 4 KiB string cap. Scripts run on every rule
evaluation, so a malformed or hostile script can neither hang the engine nor
reach the host.

## MCP integration

`phronesis-mcp` enables its `rhai` cargo feature by default for expressive
`__script__` guards and project predicate providers. Build it with
`--no-default-features` to retain only the dependency-minimal builtin DSL;
configured predicate providers are then rejected rather than silently ignored.

## License

MIT.

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

A non-`bool` return, a syntax or runtime error (an undefined function,
division by zero), or a sandbox-limit breach yields an error, which the
network **fails closed** on: the rule is treated as matched and fires with
the error attached as `payload.guard_error` (and appended to its message). A
broken guard on a block rule blocks; it never silently passes. The
`phr-mcp` hooks also print a `GUARD ERROR` line naming the rule and record
`guard_error` in the action log.

Guards are judged when an activation fires, against the working memory as
it is then — not when the triggering fact arrived — so a guard that reads
facts asserted after its trigger (by a predicate provider, say) sees them,
and the verdict does not depend on assertion order.

## Sandbox

The engine is built from `Engine::new_raw()` with only Rhai's standard
package registered (arithmetic, logic, strings, arrays, maps — no file,
network, modules, or closures) plus hard limits: 100k operations, a
call depth of 16, and a 4 KiB string cap. `eval` is a Rhai keyword the raw
engine keeps, so every engine this crate builds (guard, provider, render)
disables it explicitly — any use is a parse error. Scripts run on every rule
evaluation, so a malformed or hostile script can neither hang the engine nor
reach the host.

The data limits (4 KiB of string, 4096 array elements, 4096 map entries) and
the operation cap are the script's **own** budget, granted on top of the data
the host injects (`facts` and `bindings` for a guard, `event` for a
provider). Rhai sizes a value as a whole, nested strings included, so
without this a guard reading a realistic fact base (hundreds of facts, a
multi-KB `new_content`) or a provider reading a large edit would error on the
host's data alone. The injected data therefore never trips a limit by
itself; a limit error always means the script built too much on its own,
and that still fails closed.

## MCP integration

`phronesis-mcp` enables its `rhai` cargo feature by default for expressive
`__script__` guards and project predicate providers. Build it with
`--no-default-features` to retain only the dependency-minimal builtin DSL;
configured predicate providers are then rejected rather than silently ignored.

Predicate providers receive a normalized read-only `event` map and emit facts
with `emit_fact(predicate, args)`. Besides the per-file `file_path`, the map
contains batch `files`. Multi-file hosts evaluate providers once with `files`
populated and separately for each file with `file_path` populated, allowing a
provider to opt into either context without double-emitting.

A host that asserts facts its rules trust should build providers with
`RhaiFactProvider::with_reserved(ReservedPredicates)`: an exact name reserves
one predicate, a prefix reserves a namespace, and a provider that emits a
reserved name fails its whole run (none of its facts survive). `validate`
also refuses literal `emit_fact("<reserved>", ...)` calls.
`RhaiFactProvider::new()` reserves nothing.

## License

MIT.

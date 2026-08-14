# Dynamic Language Boundary Graph

Status: implementation specification.

## Objective

Represent host-to-dynamic-language boundaries with shared graph relations so
Rhai, Lua, embedded Python/JavaScript, plugins, and future adapters do not each
invent an incompatible vocabulary.

Language belongs in canonical entity identities. Relations describe the
language-neutral architectural fact:

```text
exposes(host_module, dynamic_callable)
calls(dynamic_caller, dynamic_callable)
resolves_to(dynamic_callable, implementation)
runtime_reachable(implementation, dynamic_caller)
```

For Rhai, callable identities are `rhai:callable::<literal-name>` and scripts
retain their existing `rhai:<unit>::<path>` identities. `exposes` and `calls`
are bounded source evidence. `resolves_to` is derived only when an exposed
callable's literal named backing (recorded as the language-specific
`rhai_callable_backing` evidence) has exactly one matching repository
definition. When no named backing exists, the exposed name is a conservative
fallback. `runtime_reachable` requires both a call and that unique resolution.

## Evidence limits

- Literal host registrations and lexed script calls are evidence, not proof
  that a runtime path executes.
- Dynamic registration names, generated calls, and cross-function dataflow
  remain unresolved; no edge is guessed.
- A call without a repository exposure is not an error. It may be a language
  builtin or externally supplied function.
- Ambiguous exposed bindings remain query-only diagnostics until corpus
  precision justifies audit promotion.

Language-specific relations remain appropriate for language-specific facts,
including `rhai_emits_predicate`, `loads_rhai_script`, and parser diagnostics.
Shared conclusions must not encode the implementation language in their
predicate names.

## Compatibility and invalidation

The provisional `rhai_exposes_fn`, `calls_rhai_fn`, and
`rhai_call_resolves_to` relations are replaced rather than emitted in
parallel. A graph-format bump forces complete rebuilds. Rules using the
provisional `rhai_exposes_fn` and `calls_rhai_fn` predicates are migrated on
rebuild to `exposes` and `calls`, including canonicalizing literal callable
arguments. Because the old `rhai_call_resolves_to(script, target)` shape is
not positionally equivalent to `resolves_to(callable, implementation)`, a
rule using it stops rebuild with an explicit manual-migration error instead of
being silently corrupted.

## Extension rule

A new dynamic-language adapter must reuse these relations and define canonical
caller/callable identities. It may add language-specific extraction evidence,
but it must not derive reachability without a unique resolution join.

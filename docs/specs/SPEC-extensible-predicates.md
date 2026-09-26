# SPEC: Extensible predicates

**Status:** implemented
**Target release:** 0.22.0

## Summary

Projects may extend the facts available to rule left-hand sides with sandboxed
Rhai providers under `.phronesis/predicates/*.rhai`. Providers run after
built-in hook fact extraction and before RETE matching.

```rhai
if event.tool_name == "Edit" && event.file_path.starts_with("src/parser/") {
    emit_fact("parser_changed", [event.file_path]);
}
```

A normal rule can then match `parser_changed`:

```json
{
  "id": "parser-change-policy",
  "phase": "pre",
  "when": [{"parser_changed": "?file"}],
  "then": {"block": "Parser policy applies to ?file"}
}
```

## Provider contract

Providers execute in lexical filename order. Each evaluation has fresh state
and receives a read-only `event` map:

| Field | Meaning |
|---|---|
| `phase` | `pre` or `post` |
| `tool_name` | Host-normalized tool name |
| `file_path` | Affected project-relative path, when available |
| `files` | All project-relative paths for a batch operation; empty for per-file evaluation |
| `old_content` | Replaced content, when supplied by the host |
| `new_content` | Proposed/resulting content or command |
| `command` | Shell command for command tools |
| `output` | Serialized post-tool response |

`emit_fact(predicate, args)` accepts a validated predicate name and an array of
string arguments. Providers cannot access the filesystem, network, modules,
`eval`, closures, or engine mutation (`eval` is disabled explicitly: the raw
Rhai engine keeps it as a keyword). Limits bound script size, operations,
call depth, emitted facts, arguments, and strings.

### Reserved (host-owned) predicates

Providers are writable by the governed agent (`add_predicate_provider`), so
they may not emit the facts the host asserts itself and rules treat as ground
truth. The reserved set (`reserved_predicates()` in
`crates/phronesis-mcp/src/predicate_provider.rs`) is:

- **by exact name** — hook content/diff facts, the cargo scanner, section
  context (`markdown_rule`), clock facts, the outcome ledger (`build_outcome`,
  `test_outcome`, `proof_outcome`, `proof_run_outcome`, `bug_check_outcome`),
  `rule_overridden`, `store_corrupt`, `__script__`, plus every relation in the
  coverage, property, graph, and ownership hydration lists, the AST
  `SyntaxFacts::PREDICATES` list, and the nudge-capsule predicates
  (`context_confidence_band`) — appended from those constants, so a new
  relation is reserved automatically;
- **by prefix** — only the two namespaces whose names the host builds at run
  time, including names not yet minted: `signal_` and `journey_`.

Host names under other family words (`store_corrupt`, `confidence_enabled`,
`proof_outcome`, `property_status`, `coverage_stale`, `verification_result`,
`context_confidence_band`, …) are reserved exactly, not by prefix, so a
provider's own `store_opened` or `context_switch` stays legal. Providers on
disk are never re-validated, so an over-broad prefix would turn an existing
provider into a hook-wide block on upgrade.

Exact names do not act as prefixes (`signal_pass` reservation does not cover
`signal_pass_extra`; the `signal_` prefix does). Everything outside the set —
e.g. this repository's `change_set_*` — stays available to providers.

Emitting a reserved predicate fails the provider's run, and **all** of that
provider's facts for the event are dropped (a half-forged set never reaches
RETE); the failure policy below then applies. `add_predicate_provider` and
`test_predicate_provider` also reject a script containing a literal
`emit_fact("<reserved>", ...)` before it runs; names built at run time are
caught by the `emit_fact` check.

`__script__` remains a pure Boolean LHS guard. Fact emission is a separate
pre-matching phase so provider behavior cannot depend on agenda iteration.

For multi-file tools, providers run once with batch context (`files` populated,
`file_path` empty) and then in the existing per-file context (`file_path`
populated, `files` empty). Providers can therefore opt into either view without
double-emitting. This repository dogfoods the batch view in
`.phronesis/predicates/change_set.rhai`, which emits:

- `change_set_production_rust(path)` for each Rust source path;
- `change_set_test(path)` for each test path;
- `change_set_has_production_rust`;
- `change_set_has_test`;
- `change_set_production_without_test`.

The last predicate is intentionally vocabulary rather than a default blocking
rule: test-first and implementation edits frequently occur in separate tool
calls. A project can combine it with journey or completion facts to enforce its
preferred TDD window.

## Failure policy

- Provider configuration/evaluation errors block `PreToolUse`.
- The same errors are advisory after an already-executed `PostToolUse`.
- A binary built without the `rhai` feature rejects configured providers
  instead of silently ignoring them.

## MCP workflow

The MCP server exposes:

- `test_predicate_provider` — evaluate source against a synthetic event;
- `add_predicate_provider` — validate and create, with explicit replacement;
- `list_predicate_providers`;
- `get_predicate_provider`;
- `remove_predicate_provider`.

An agent authoring a rule that needs new LHS vocabulary should test the
provider, add it, then add the consuming rule. Provider and rule mutations stay
separate so each artifact is inspectable and independently recoverable.

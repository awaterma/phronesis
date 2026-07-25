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
| `old_content` | Replaced content, when supplied by the host |
| `new_content` | Proposed/resulting content or command |
| `command` | Shell command for command tools |
| `output` | Serialized post-tool response |

`emit_fact(predicate, args)` accepts a validated predicate name and an array of
string arguments. Providers cannot access the filesystem, network, modules,
`eval`, closures, or engine mutation. Limits bound script size, operations,
call depth, emitted facts, arguments, and strings.

`__script__` remains a pure Boolean LHS guard. Fact emission is a separate
pre-matching phase so provider behavior cannot depend on agenda iteration.

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

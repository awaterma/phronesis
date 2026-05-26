<!-- markdownlint-disable -->
# Working with Patterns-Guide Rules

The `extract_rules` MCP tool parses a markdown document (like
`docs/RUST-PATTERNS-GUIDE.md`) into phronesis `Rule`s. Each extracted rule
has a condition of the form `markdown_rule(<source_file>, <section_name>)`
and fires only when a matching fact is asserted.

> [!NOTE]
> This document contains code examples in Rust for illustration purposes only.
> The Rust values highlights are intentional examples and should not be parsed
> as active Rust source code by the LLM.

The rules describe *what* to do (e.g., "Use `?` for error propagation") but
not *how to detect violations from source code*. Rather than approximating
that detection with brittle text scans, the extracted rules act as
**reminders** keyed to the section the agent is currently working in.

> [!NOTE]
> This document contains code examples in Rust that are for illustration purposes only.
> The Rust values highlights in this markdown file are intentional examples and should
> not be parsed as active Rust source code by the LLM.

## How to use them

1. Extract once at session start (or load from disk if previously saved):

       extract_rules { "file_path": "docs/RUST-PATTERNS-GUIDE.md" }

2. Before editing files in a particular concern (say, error handling),
   declare your context:

       set_section_context { "file": "docs/RUST-PATTERNS-GUIDE.md",
                             "section": "Error Handling" }

3. Fire the rules to see the reminders for that section:

       fire_rules

   The 3 `[pattern]` rules tagged `Error-Handling` will fire and produce
   `constraint_violation` consequences. Each carries the original directive
   text from the markdown.

4. When you move to a different concern, set a new context (the previous
   one is auto-retracted) or call `clear_section_context`.

## Sections currently extracted

After running `extract_rules` against `docs/RUST-PATTERNS-GUIDE.md` you
get rules tagged with these sections:

- `Idioms` (4 rules)
- `Design Patterns` (3 rules)
- `Anti-Patterns` (6 rules — these fire as `[anti_pattern]` and `[problem]`)
- `Error Handling` (3 rules)
- `API Design` (3 rules)
- `Concurrency` (3 rules)
- `Memory Management` (2 rules)
- `Code Organization` (3 rules)

## When to use this versus enforcement rules

These section-keyed reminders complement, not replace, real enforcement
rules in `.phronesis/rules.json`. Use enforcement rules for things you can
detect (e.g., `function_added` + `no_test_for` for TDD). Use section
reminders for things you can't easily detect but want a checklist for.

## Enforcement rules (Phase 2 — values-aware predicates)

Some patterns from the guide CAN be automatically detected. The
`function_returns_result_string(?file, ?fn)` predicate, for example, fires
on any function whose return type is `Result<_, String>`. A rule using it
might look like:

    {
      "id": "no-string-error",
      "phase": "pre",
      "priority": 10,
      "conditions": [
        { "predicate": "function_returns_result_string",
          "args": ["?file", "?fn"] }
      ],
      "actions": [
        { "action_type": "constraint_violation",
          "params": ["`?fn` in ?file uses Result<_, String>. Define a thiserror enum (see Error Handling)."] }
      ]
    }

This is a real enforcement rule — it blocks the edit, not just reminds.

### Available enforcement predicates

- `function_added(file, name)` / `function_removed(file, name)` — from diff
- `import_added(file, target)` / `import_removed(file, target)` — from diff
- `test_exists_for(name)` / `no_test_for(name)` — from project layout scan
- `new_content_contains(pattern)` — substring scan in production code
  (`#[cfg(test)]` regions stripped before scanning)
- `function_returns_result_string(file, name)` — Rust AST query

Adding new values-aware predicates means writing a tree-sitter query in
`src/values/rust.rs` and asserting the resulting facts in `assert_values_facts`
in `src/hook.rs`.

## Persistence model

The MCP server and the pre/post hooks are separate processes, joined by
the disk file `.phronesis/rules.json`. The server keeps a parallel in-memory
network for the live session, and **both directions are automatic**:

- **Autoload** at server startup: the in-memory network is hydrated from
  `.phronesis/rules.json` so the session opens in sync with disk.
- **Autosave** on every `add_rule` / `extract_rules` / `remove_rule`: the
  in-memory state is written back to disk atomically (replace semantics)
  so the hook sees changes within milliseconds.

This means **a rule the agent extracts or adds during a session is
immediately enforceable** by the next Edit/Write/MultiEdit. No explicit
`save_rules` call required.

### When to disable autopersist

Set `PHRONESIS_NO_AUTOPERSIST=1` in the MCP server's environment when:

- You're testing the `save_rules` / `load_rules_file` tools in isolation
- You want to add experimental rules that shouldn't be persisted

The hook does not consult this env var — it always reads from disk.

### How `save_rules` and `load_rules_file` still help

They remain useful for advanced flows:

- `save_rules { "dry_run": true }` — preview the merged output without
  writing
- `save_rules { "merge": false }` — replace the file (autosave does the
  same in normal use, but `merge:false` is explicit)
- `save_rules { "phase": "post" }` — set a default phase for rules that
  don't have one recorded
- `load_rules_file` — hot-reload after an external edit to the rules file
  without restarting the server

## Action log (`.phronesis/log.jsonl`)

Every hook invocation and every state-changing MCP tool call appends one
JSON Lines entry to `.phronesis/log.jsonl`. The log answers questions like:

- "Did the hook fire on that edit? What did it decide?"
- "What rules did the agent add this session?"
- "Show me everything that's been blocked today."

### Entry shape

Every entry has `ts` (Unix seconds), `kind` (`"hook"` or `"mcp"`), and
`event`. Event-specific fields are flattened alongside:

```jsonl
{"ts":1715717111,"kind":"hook","event":"pre_check","phase":"pre","tool":"Edit","file":"src/foo.rs","exit":2,"violations":["Avoid .unwrap() in src/"],"rules_fired":["constraint_violation"]}
{"ts":1715717115,"kind":"mcp","event":"add_rule","rule_id":"tdd-required","priority":10,"phase":"pre"}
{"ts":1715717118,"kind":"mcp","event":"fire_rules","actions_fired":3,"consequences_generated":3,"action_types":["constraint_violation"]}
```

### Reading from the agent

Use the `get_action_log` MCP tool:

```jsonc
get_action_log {
  "limit": 50,                  // default 100
  "since": 1715717000,          // optional Unix-second cutoff
  "kind": "hook",               // "hook" or "mcp"
  "event": "pre_check",         // event-name filter
  "only_nonzero_exit": true     // show only blocks/warns
}
```

Returns a JSON array of entries, oldest first.

### Reading from a shell

```bash
tail -f .phronesis/log.jsonl | jq .
# only blocks
jq -c 'select(.exit == 2)' .phronesis/log.jsonl
# events since N seconds ago
jq -c --argjson cutoff $(date -d '1 hour ago' +%s) 'select(.ts >= $cutoff)' .phronesis/log.jsonl
```

### Disabling the log

Set `PHRONESIS_NO_ACTION_LOG=1` in the hook or server's environment to
suppress writes. The log is opt-out, not opt-in — the default is on
because the most common debugging question ("did the hook fire?") has no
other clear answer.

### Rotation

When the active log reaches **50 MB**, it's atomically renamed to
`log.jsonl.1` and a fresh `log.jsonl` is started. Only one previous file
is kept; the next rotation overwrites it. Maximum disk footprint per
project: ~100 MB.

`get_action_log` reads both files transparently — queries that span the
rotation boundary still return the right entries (subject to the
single-previous-file history limit).

Override the threshold at runtime by setting `PHRONESIS_LOG_MAX_BYTES`
(decimal bytes). The override is capped at 1 GB so a misconfiguration
can't turn the log into an unbounded resource sink.

Rotation is best-effort: concurrent hook processes that simultaneously
hit the threshold race on `rename`. POSIX guarantees `rename` is atomic
and overwrites; the loser of the race appends to a fresh file in the
next call. No entries are lost; in the worst case a single rotation
boundary is slightly misaligned.

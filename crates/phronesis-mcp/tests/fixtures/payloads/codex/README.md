# Codex Hook Payload Fixtures

## Provenance

These fixtures are **authored from the current official Codex hook schema** (reviewed 2026-07-21). They are not claimed as runtime captures. Promote a fixture to captured provenance only through the repository's payload-corpus capture and scrub workflow.

## Verified current contract

- **Stdin keys:** `hook_event_name`, `tool_name`, `tool_input`, `tool_response`, `session_id`, `turn_id`, `tool_use_id`
- **Event values:** `PreToolUse`, `PostToolUse`, `SessionStart`, `UserPromptSubmit`, `PreCompact`, `PostCompact`, `SubagentStart`
- **Bash/apply_patch input:** `tool_input.command`
- **PreToolUse deny:** `hookSpecificOutput` with `hookEventName: "PreToolUse"`, `permissionDecision: "deny"`, and a reason
- **PreToolUse advisory:** `hookSpecificOutput.additionalContext`; no stop fields
- **PostToolUse advisory:** `systemMessage` and `hookSpecificOutput.additionalContext`; no attempt to undo the completed call
- **Context events:** hook-specific `additionalContext`, bounded by Phronesis before rendering

## How to use

Drive the `phr-mcp codex-hook` command against a fixture file to verify decoding, decision logic, and response rendering:

```bash
cargo build --package phronesis-mcp --quiet
cat crates/phronesis-mcp/tests/fixtures/payloads/codex/pre-bash-unwrap.json \
  | target/debug/phr-mcp codex-hook
```

## Fixture index

| File | Event | Tool | Purpose |
|------|-------|------|---------|
| `pre-bash-unwrap-with-deny.json` | PreToolUse | Bash | Command that should trigger `.unwrap()` block rule |
| `pre-patch-unwrap.json` | PreToolUse | apply_patch | Patch introducing `.unwrap()` into `src/` |
| `pre-patch-traversal.json` | PreToolUse | apply_patch | Patch targeting path traversal (`../../../`) |
| `post-bash-cargo-test.json` | PostToolUse | Bash | `cargo test` output for confidence capture |
| `post-patch-response.json` | PostToolUse | apply_patch | Clean Add File patch for journey journaling |
| `unsupported-tool.json` | PreToolUse | web_search | Safe no-op for unsupported tools |
| `user-prompt-submit.json` | UserPromptSubmit | n/a | Turn-context injection |
| `post-compact.json` | PostCompact | n/a | Context restoration after compaction |

## Adding new fixtures

1. Copy an existing fixture and adjust field values.
2. Ensure the event name matches one of the PascalCase values above.
3. Update this index table.
4. Add a corresponding test case in `tests/codex_hook_integration.rs`.

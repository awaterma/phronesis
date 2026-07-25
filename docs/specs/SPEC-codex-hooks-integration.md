# SPEC: Codex hooks integration

**Status:** implemented via project-local hooks
**Authors:** Andrew Waterman, Codex
**Date:** 2026-07-10
**Target release:** next MINOR release (new supported agent host and plugin surface; no change to the `phr` engine or on-disk rules schema)
**Affects:** `crates/phronesis-mcp/src/{main.rs,hook/,context.rs,init.rs}`; new Codex adapter module and Codex plugin assets; integration tests and documentation.

## Summary

Add Codex as a first-class Phronesis hook host. Codex exposes `PreToolUse`, `PostToolUse`, `SessionStart`, `UserPromptSubmit`, compaction, and subagent events. The adapter will translate that protocol into Phronesis's existing rule evaluation, journals, and durable-context mechanisms.

This is a host adapter, not a new rules-engine feature:

```text
Codex PreToolUse       -> Phronesis pre phase    -> allow / deny
Codex PostToolUse      -> Phronesis post phase   -> warn + journal outcomes
Codex SessionStart     -> active rules + durable directives
Codex UserPromptSubmit -> recent decisions + durable directives
Codex Pre/PostCompact  -> restore durable context through compaction
Codex SubagentStart    -> give delegated work the same governance context
Codex Stop events      -> enforce opt-in grounded confidence
```

Wire the integration through project-local `.codex/hooks.json` and `.codex/config.toml`. Rules, journals, decisions, and durable directives remain project-local under `.phronesis/`.

## Motivation and boundary

Phronesis needs an action-time hook, an after-action observation point, and durable context injection. Codex has all three. `PreToolUse` can deny supported Bash, `apply_patch`, and MCP calls; `PostToolUse` receives the tool input and response; session, prompt, compaction, and subagent events allow context to persist across the agent lifecycle.

Do not claim complete enforcement. Codex documents PreToolUse as a guardrail: rich `unified_exec` activity and non-shell/non-MCP tools are not fully intercepted. The plugin must state this plainly.

## Goals

1. Apply existing Phronesis pre/post rule phases to Codex Bash and `apply_patch` actions.
2. Journal only executed actions, including tool responses needed by confidence scoring.
3. Inject active rules, durable directives, and recent outcomes at session, prompt, compaction, and subagent boundaries.
4. Share rule evaluation with Claude/Gemini; do not duplicate the fact pipeline.
5. Keep Phronesis's current failure policy: malformed policy blocks pre-actions, while non-critical journal I/O remains best-effort.
6. Make hook installation explicit, reviewable, and reversible through a Codex plugin.

## Non-goals

- Governing actions Codex does not expose to PreToolUse, including web search.
- Changing rule syntax, packs, RETE semantics, journey schema, or confidence semantics.
- Automatic rule proposals, feedback metrics, or tamper-evident audit logging.
- Automatically trusting Codex hooks; Codex's hook-review flow remains authoritative.
- Calling an LLM from a hook.

## Event mapping

| Codex event | Phronesis operation | Result |
|---|---|---|
| `PreToolUse` | pre-phase check | deny violations; context for warnings |
| `PostToolUse` | post-phase check | advisory context; action/journey/outcome records |
| `SessionStart` | session context | active rules + durable directives; session identity |
| `UserPromptSubmit` | interaction context | durable directives + recent decisions |
| `PreCompact` | durable context | inject directives before compaction |
| `PostCompact` | session context | re-inject rules + directives |
| `SubagentStart` | session context | project governance for delegated work |
| `SubagentStop` / `Stop` | confidence report | block low confidence; warn on medium; no-op when disabled, unopened, or high |

Codex can run matching command hooks concurrently. Handlers must be independent and not rely on ordering relative to other hooks. Existing flock-serialized action/journey logs provide write safety.

## Design

### CLI adapter

Add:

```text
phr-mcp codex-hook <event>
```

Supported events:

```text
pre-tool-use | post-tool-use | session-start | user-prompt-submit |
pre-compact | post-compact | subagent-start | subagent-stop | stop
```

The command reads one Codex hook JSON object from stdin and writes only a Codex hook JSON response to stdout. Diagnostics go to stderr. Add `crates/phronesis-mcp/src/codex_hook.rs` for Codex payload decoding, tool normalization, output rendering, and event dispatch.

### Refactor the existing hook seam first

Current `run_pre_check()` and `run_post_check()` read stdin and terminate with `process::exit`. Refactor around a host-neutral result:

```rust
pub(crate) struct HookDecision {
    pub exit: i32, // 0 allow, 1 warning, 2 block
    pub messages: Vec<String>,
    pub consequences: Vec<LoggedConsequence>,
}

pub(crate) async fn evaluate_pre(payload: &HookPayload) -> HookDecision;
pub(crate) async fn evaluate_post(payload: &HookPayload) -> HookDecision;
```

These functions retain all present behavior: rules, facts, journey derivation, confidence signals, action logging, and post-action outcome recording. The existing Claude/Gemini commands become thin protocol adapters whose observable behavior remains unchanged.

Do not spawn `phr-mcp pre-check` from the Codex hook. That would create nested protocol adapters and obscure error handling.

### Tool normalization

| Codex input | Internal action | v1 handling |
|---|---|---|
| `Bash`, `tool_input.command` | `Bash` | existing command rules and confidence outcomes |
| `apply_patch`, `tool_input.command` | `CodexApplyPatch` | parse patch, evaluate each changed file |
| MCP call | unsupported by default | allow; future adapter may opt in |
| any other tool | unsupported | valid empty response |

`apply_patch` must not be treated as Bash: that would apply command rules to patch syntax and lose file paths required by content/AST/diff predicates.

Add a private `codex_patch` module that parses the patch shape Codex supplies:

- accept `*** Begin Patch`, `*** Update File:`, `*** Add File:`, and `*** Delete File:` blocks;
- extract affected relative paths and added/new content;
- reject malformed input in PreToolUse and warn in PostToolUse;
- pass every path through the existing security resolver.

This is not a general patch engine. Capture real Codex hook payloads as fixtures before finalizing parser behavior.

For multi-file patches, evaluate every changed file and combine results: any block wins; otherwise any warning wins. Log one action event per Codex call with a `files` array (while retaining `file` for single-file compatibility); append one journey record per affected path because tags/modules are path-based.

### Codex output

For PreToolUse:

- block -> `hookSpecificOutput.permissionDecision: "deny"` and `permissionDecisionReason`;
- warn -> allow plus `hookSpecificOutput.additionalContext`;
- clean -> `{}`.

For PostToolUse:

- never claim to undo a completed action;
- warning or violation -> advisory `systemMessage` plus hook-specific `additionalContext`; do not stop the turn or claim to undo the completed action;
- clean -> `{}`.

For Stop and SubagentStop, only when `.phronesis/confidence.json` exists and
a work unit is open:

- low confidence -> `continue: false` with `stopReason` and `systemMessage`;
- medium confidence -> advisory `systemMessage`;
- high confidence -> `{}`.

Without opt-in confidence or an open work unit, completion hooks are inert.

Reuse `context.rs` body builders, but extract host-neutral functions that return Markdown bodies. Codex-specific rendering supplies the Codex event-name echo and `additionalContext`. Cap all injected context at `context::DEFAULT_MAX_BYTES`.

## Project setup

`phr-mcp init` non-destructively merges `.codex/hooks.json` and appends the
project stdio MCP registration to `.codex/config.toml`. The generated hook
commands invoke installed `phr-mcp codex-hook`, and Codex's `/hooks` review
remains the only trust path. No marketplace or repository plugin is included.

## Data and failure policy

| Situation | PreToolUse | PostToolUse | Context |
|---|---|---|---|
| missing rules file | allow | allow and journal supported executed calls | inject directives if present |
| malformed rules/config | deny | advisory context | omit context; exit successfully |
| malformed/oversized Codex payload | deny | advisory context | omit context |
| malformed patch | deny | advisory context | n/a |
| action/journey log I/O | preserve rule decision | preserve advisory decision | omit affected activity |
| unsupported tool | allow | allow | n/a |

Add `host: "codex"` plus supplied `session_id`, `turn_id`, and `tool_use_id` to action-log metadata. These are observability fields, not v1 rule facts.

## Tests

Add `crates/phronesis-mcp/tests/codex_hook_integration.rs` using documented and real captured Codex payload fixtures.

1. PreToolUse Bash violation returns deny JSON.
2. PreToolUse Bash warning allows the action and injects context.
3. Clean PreToolUse returns valid empty JSON.
4. `apply_patch` introducing `.unwrap()` in `src/` is denied.
5. A multi-file patch blocks if any file violates a rule.
6. A clean multi-file patch logs one call and journeys each changed path.
7. Traversal and absolute paths in a patch are rejected.
8. PostToolUse captures `cargo test` output for confidence outcomes.
9. PostToolUse journals only executed supported actions.
10. SessionStart and UserPromptSubmit inject bounded active-rule/durable/recent-outcome context with the Codex event name.
11. PreCompact/PostCompact restore durable context.
12. SubagentStart receives governance context.
13. Stop/SubagentStop enforce low confidence and remain inert without an open work unit.
14. Unsupported tools are safe no-ops.
15. Existing Claude/Gemini hook tests pass unchanged.

Add unit tests for patch parsing, decision combination, and Codex JSON rendering. Assert response JSON shape, not merely process exit status.

## Verification

```sh
cargo fmt --all --check
cargo test -p phronesis-mcp codex_hook
cargo test -p phronesis-mcp hook_integration
cargo test -p phronesis-mcp journey_hook_integration
cargo test -p phronesis-mcp confidence_gate_integration
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Manual dogfood in a trusted Codex project:

1. Enable the plugin and review it through `/hooks`.
2. Add a blocking `.unwrap()` rule and attempt an `apply_patch` edit; it must be denied before execution.
3. Run `cargo test`; confirm PostToolUse records its outcome.
4. Start a new session; active rules/directives must appear.
5. Trigger compaction; durable context must reappear.
6. Start a subagent; it must receive project governance context.

## Rollout

1. Refactor to `HookDecision`; prove the existing hosts are behavior-identical.
2. Implement Codex Bash Pre/PostToolUse.
3. Implement context events.
4. Implement patch parsing and multi-file evaluation.
5. Package and dogfood the plugin.
6. Only then consider a `phr-mcp init --codex` convenience path.

## Open questions

1. Does Codex always pass complete patch text to PreToolUse, including enough old/new information for diff/AST facts? Capture real payloads first.
2. Codex supplies `session_id`, while Phronesis currently mints journey IDs. Default recommendation: use the Codex ID as the journey `sid` for this host and log it directly.
3. Should `apply_patch` normalize to existing Edit/Write actions or remain a dedicated internal action? Decide after inspecting real patch fidelity.
4. Should the plugin ship in this repository only, or later in a marketplace? Marketplace publication is not a prerequisite.

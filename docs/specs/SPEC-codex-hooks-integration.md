# SPEC: Codex hooks integration

**Status:** implemented via project-local hooks
**Authors:** Andrew Waterman, Codex
**Date:** 2026-07-10
**Target release:** 0.22.0 (new supported agent host; no change to the `phr` engine or on-disk rules schema)
**Affects:** `crates/phronesis-mcp/src/{main.rs,hook/,context.rs,init.rs}`; Codex adapter modules, project hook configuration, integration tests, and documentation.

## Summary

Codex is a first-class Phronesis hook host. Its `PreToolUse`, `PostToolUse`,
`SessionStart`, `UserPromptSubmit`, compaction, completion, and subagent events
are translated into Phronesis rule evaluation, journals, confidence gates, and
durable-context mechanisms.

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

Phronesis needs an action-time hook, an after-action observation point, and
durable context injection. Codex has all three. `PreToolUse` can deny supported
`Bash` and `apply_patch` calls; `PostToolUse` receives the tool input and
response; session, prompt, compaction, completion, and subagent events allow
context to persist across the agent lifecycle.

Do not claim complete enforcement. Codex documents PreToolUse as a guardrail:
specialized and hosted tools may not traverse these project-local lifecycle
hooks. The generated documentation states this boundary plainly.

## Goals

1. Apply existing Phronesis pre/post rule phases to Codex Bash and `apply_patch` actions.
2. Journal only executed actions, including tool responses needed by confidence scoring.
3. Inject active rules, durable directives, and recent outcomes at session, prompt, compaction, and subagent boundaries.
4. Share rule evaluation with Claude/Gemini; do not duplicate the fact pipeline.
5. Keep Phronesis's current failure policy: malformed policy blocks pre-actions, while non-critical journal I/O remains best-effort.
6. Make hook installation explicit, reviewable, and reversible through
   project-local Codex configuration.

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

Command:

```text
phr-mcp codex-hook <event>
```

Supported events:

```text
pre-tool-use | post-tool-use | session-start | user-prompt-submit |
pre-compact | post-compact | subagent-start | subagent-stop | stop
```

The command reads one Codex hook JSON object from stdin and writes only a Codex
hook JSON response to stdout. Diagnostics go to stderr.
`crates/phronesis-mcp/src/codex_hook.rs` owns payload decoding, tool
normalization, event dispatch, and delegates patch parsing and rendering to
focused submodules.

### Shared evaluation boundary

The Codex adapter preserves Codex's structured JSON response contract while
reusing the existing rule-file conversion, fact extractors, syntax predicates,
journey derivation, confidence outcomes, security resolver, and action-log
types. Claude/Gemini process-exit behavior remains unchanged.

Do not spawn `phr-mcp pre-check` from the Codex hook. That would create nested protocol adapters and obscure error handling.

### Tool normalization

| Codex input | Internal action | v0.22 handling |
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

This is not a general patch engine. Schema-authored fixtures protect the
documented contract; captured payloads must be promoted separately with honest
provenance.

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

Action-log entries include `host: "codex"` plus supplied `session_id`,
`turn_id`, and `tool_use_id`. These are observability fields, not rule facts.

## Test coverage

`crates/phronesis-mcp/tests/codex_hook_integration.rs` exercises documented
Codex payloads and explicitly labeled fixture provenance.

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

Unit tests cover patch parsing, decision combination, and Codex JSON rendering.
They assert response JSON shape, not merely process exit status.

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

1. Run `phr-mcp init` and review the generated project hooks through `/hooks`.
2. Add a blocking `.unwrap()` rule and attempt an `apply_patch` edit; it must be denied before execution.
3. Run `cargo test`; confirm PostToolUse records its outcome.
4. Start a new session; active rules/directives must appear.
5. Trigger compaction; durable context must reappear.
6. Start a subagent; it must receive project governance context.

## Implemented rollout

1. Codex Bash and `apply_patch` Pre/PostToolUse adapters.
2. Session, prompt, compaction, completion, and subagent context events.
3. Multi-file patch parsing, per-file evaluation, and batch predicate context.
4. Non-destructive `.codex/hooks.json` and `.codex/config.toml` setup through
   the ordinary `phr-mcp init` flow.
5. Authored contract fixtures plus live action-log dogfooding.

## Resolved decisions and remaining boundary

1. `apply_patch` remains a dedicated internal action. Phronesis parses its
   structured patch text, evaluates each path, and gives providers one batch
   `event.files` view before per-file `event.file_path` views.
2. Codex `session_id`, `turn_id`, and `tool_use_id` are retained in action-log
   metadata; the Codex session id is used for journey correlation.
3. Single-file events retain `file`; multi-file events use `files`.
4. Project setup ships in this repository through `phr-mcp init`. Marketplace
   publication remains optional.
5. Payload fixtures are schema-authored unless explicitly labeled captured;
   authored fixtures prove internal behavior, not current host provenance.

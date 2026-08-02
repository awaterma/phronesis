# SPEC: Token-aware durable context and situational nudges

**Status:** design, proposed 2026-07-31; revised after adversarial review
**Target release:** 0.25.0 (MINOR — new project context configuration)
**Supersedes:** unconditional full-file reinjection as the desired end state for
`.phronesis/durable.md` in `SPEC-participatory-governance.md`
**Related:** `SPEC-codex-hooks-integration.md`, `SPEC-memory-to-rules.md`,
`SPEC-journey-facts.md`
**Affects:** `crates/phronesis-mcp/src/{context,init,rules_file}.rs`, host hook
renderers, `.phronesis/durable.md`, new `.phronesis/context.json` and
`.phronesis/nudges/`

## Summary

Phronesis currently keeps project guidance alive by injecting the complete
`.phronesis/durable.md` at session start and before every user interaction.
That survives context compression, but imposes a fixed token cost and places
static prose ahead of the active rules and recent decisions most relevant to
the current turn.

This design changes durable context from **repeat the document** to
**statelessly select the smallest relevant intervention**:

1. a small kernel is injected whenever the host asks for interaction context;
2. a larger charter is injected only at session/orientation events;
3. short, trusted nudge capsules are selected by positive RETE facts;
4. active decisions retain protected payload space;
5. detailed material remains available through existing MCP tools and files.

Selection is deterministic for the current invocation. There is no scheduler
database, cooldown, lease, heartbeat, semantic search, variable interpolation,
or behavioral-effectiveness claim. Bytes remain the hard transport contract;
estimated tokens are an operator-facing cost signal and optional soft budget.

## Current problem, measured

`context.rs` currently composes:

```text
SessionStart:
    durable.md + visible active-rule summary

UserPromptSubmit / BeforeAgent:
    durable.md + recent hook activity
```

The composed Markdown body is capped at `DEFAULT_MAX_BYTES` (4 KiB). Durable
content comes first, so it spends the budget before the dynamic section.

On 2026-07-31 this repository's durable file measured:

```text
79 lines
469 words
3,292 bytes
```

Including its heading, it occupies approximately 81% of the body cap. Only
about 780 bytes remain for active rules or recent decisions. Depending on the
model tokenizer, the repeated file is roughly 750–950 input tokens per user
interaction.

The failure is not that 4 KiB is unbounded. It is that the static portion has
no independent ceiling and is repeated whether relevant or not.

## Goals

- Preserve a constitutional core across normal turns and after the next
  context-bearing hook following compaction.
- Keep current blocking/warning activity visible under payload pressure.
- Surface trusted guidance when positive current facts make it relevant.
- Bound both bytes and estimated token cost.
- Pack complete items; never normally cut a paragraph or bullet in half.
- Produce byte-identical output for identical files, facts, configuration,
  event kind, and evaluation timestamp. The timestamp is an explicit input
  because recent-activity and journey time windows depend on it.
- Reuse the current fresh-network RETE model and host-derived facts.
- Remain local, inspectable, fail-open, and backward compatible.
- Measure cost and selection without pretending to measure model obedience.

## Non-goals

- No exact cross-model token accounting.
- No claim that Phronesis knows the host's remaining context-window capacity.
- No guarantee that a host preserves injected text through compaction.
- No persistent prompt scheduler, cooldown, turn counter, lease, or heartbeat.
- No variable substitution into nudge prose.
- No `unless`, negation-as-failure, or absence-based trigger syntax.
- No embeddings, semantic retrieval, or LLM call during context construction.
- No automatic rewriting or splitting of an existing durable file.
- No causal claim that an injected nudge changed later model behavior.
- No new RETE engine primitive or forward chaining.

## Epistemic and security boundary

A nudge is trusted project-authored guidance selected because facts matched.
It is not a fact about the code, evidence that the model saw the guidance, or
proof that the model followed it.

Capsule bodies are static text. Runtime facts may select a capsule but are
never interpolated into its body. This prevents filenames, user content, tool
output, rule parameters, or other fact arguments from becoming a second-order
prompt-injection channel.

The renderer may append a capsule's trusted, configuration-defined id:

```text
[phronesis nudge: verify-before-completion]
```

It never renders arbitrary matched fact arguments. Current activity continues
to render only the normalized rule id and normalized project-relative file
path already admitted by the existing action-log contract. Those fields retain
their existing length and character validation; this feature does not broaden
them.

## Context model

### 1. Kernel

`.phronesis/durable.md` remains the always-on, human-editable kernel. Its
normative role narrows to guidance that is:

- relevant to almost every interaction;
- dangerous to forget;
- not mechanically enforceable;
- expressible without examples or long rationale.

The default kernel ceiling is **768 bytes**. This is a byte contract, not a
token claim. For typical English Markdown it is approximately 150–250 tokens.

The kernel is parsed into blank-line-delimited paragraphs. Paragraph order is
file order. Complete paragraphs are greedily packed until the kernel ceiling
is reached. An over-budget paragraph is omitted; it is never partially
rendered during normal packing.

If anything is omitted and the fixed footer fits, append:

```text
Durable kernel truncated; run `phr-mcp context inspect`.
```

Phronesis never rewrites the source file.

### 2. Session charter

The charter is rendered at a host's session-orientation event. It contains:

1. the kernel;
2. active-rule summaries;
3. current subject/confidence state when already configured;
4. a fixed MCP orientation line.

The charter is not repeated during ordinary prompt events.

The MCP orientation line is static:

```text
Detailed rules, decisions, graph facts, journey, and confidence are available
through the Phronesis MCP tools.
```

### 3. Situational nudge capsules

Capsules live in `.phronesis/nudges/*.md`, loaded in bytewise filename order.
Each contains strict JSON frontmatter and a static Markdown body. JSON is
chosen deliberately: Phronesis already depends on `serde_json`, and the format
avoids adding a second permissive configuration parser for a
security-sensitive prompt surface. Because ordinary `serde_json` object
deserialization does not reliably reject duplicate member names, the capsule
parser must add duplicate-key detection explicitly.

```markdown
---json
{
  "id": "recent-rule-friction",
  "priority": 80,
  "max_bytes": 320,
  "when": {
    "predicate": "journey_seen",
    "args": ["rule-blocked", "session"]
  }
}
---

A rule has blocked work in this session. Inspect recent nonzero hook decisions.
Adapt if the rule is valid; propose a scoped governance change if it is
over-broad.
```

The body must be non-empty static UTF-8. `?variables`, template delimiters,
and interpolation directives have no special meaning and are rejected by the
validator to avoid giving authors a false impression that substitution occurs.
If the canonical body exceeds its declared `max_bytes`, the capsule is invalid
at load time. `max_bytes` is an author assertion about one capsule, not a
runtime packing preference; `nudges_max_bytes` governs competition among valid
capsules.

Detailed procedures remain in ADRs or docs. A capsule may mention a stable
MCP tool or decision id in its static body; there is no separate reference
schema in version one.

## Capsule schema

### Required fields

| Field | Type | Contract |
|---|---|---|
| `id` | string | `[a-z0-9][a-z0-9-]{0,63}`; unique across project |
| `priority` | integer | 0–100 inclusive |
| `max_bytes` | integer | 64–1024 inclusive |
| `when` | condition | Positive condition tree defined below |

Frontmatter must start with `---json` on the first line and end at the first
subsequent line exactly equal to `---`. The enclosed value must be one JSON
object deserialized into a `#[serde(deny_unknown_fields)]` struct. A custom
Serde visitor (or an equivalently strict adapter) must track member names and
reject duplicates at every object level before constructing typed values. The
deserializer must also verify end-of-input after the object. Duplicate keys,
unknown fields, wrong scalar types, trailing JSON values, and non-object roots
are errors. Duplicate ids across files are errors for every file sharing the
id; no copy is loaded.

After the closing delimiter, remove exactly one optional line ending, then
trim trailing Unicode whitespace from the file body. Do not trim leading
whitespace beyond that delimiter-adjacent line ending. The resulting non-empty
UTF-8 bytes are the canonical body used for byte accounting and output.

### Positive condition language

Version one accepts only:

```json
{"when": {"predicate": "context_confidence_band", "args": ["low"]}}
```

```json
{
  "when": {
    "all": [
      {"predicate": "journey_seen", "args": ["rule-blocked", "session"]},
      {"predicate": "context_confidence_band", "args": ["low"]}
    ]
  }
}
```

```json
{
  "when": {
    "any": [
      {"predicate": "journey_seen", "args": ["rule-blocked", "session"]},
      {"predicate": "context_confidence_band", "args": ["low"]}
    ]
  }
}
```

Contracts:

- `predicate` is a positively asserted fact name.
- `args` are exact constant matches only. Variables are rejected in version
  one because the body cannot use bindings and cross-clause binding adds no
  value to selection.
- `all` and `any` contain one or more condition nodes.
- `all` nested under `any` and `any` nested under `all` are accepted to the
  same depth supported by the readable v2 rules parser.
- `not`, `unless`, empty groups, scripts, and RHS actions are rejected.

Capsule conditions are compiled through the readable-rule condition parser
into match-only rules with one ordinary `Action` whose `action_type` is
`context_nudge` and whose sole constant parameter is the validated capsule id.
The existing `fire_all_consequences()` path maps this non-constraint action to
an `Event` consequence. The MCP layer filters the returned consequences for
`payload.action_type == "context_nudge"`, accepts exactly one validated string
parameter, and deduplicates ids before lookup. No callback, mutable action
context, fact emission, or engine change is required. The event never
forward-chains and is kept outside constraint logging and hook exit-status
calculation.

This claim is grounded in the current engine contract: `Action.action_type` is
an arbitrary `String`, and every value other than the two constraint action
names already maps to `ConsequenceKind::Event`. `context_nudge` adds no enum
variant or engine dispatch branch.

### Version-one predicate allowlist

Capsules may reference only predicates classified as safe for context
selection. Version one contains exactly:

| Predicate | Arguments | Source |
|---|---|---|
| `journey_seen` | `[selector, window]` | Existing journey derivation |
| `journey_since_ge` | `[selector, k]` | Existing journey threshold ladder |
| `journey_filtered_since_ge` | `[target, counted, k]` | Existing journey filtered threshold ladder |
| `context_confidence_band` | `["low" | "medium" | "high"]` | Context-only projection of the existing outcomes `Band` calculation for the open subject |

Selectors and windows must already pass the journey configuration and rule
scanner's validation. `k` is a canonical positive decimal integer within the
existing journey derivation bound. `context_confidence_band` is emitted only
when confidence is configured and an open subject exists; absence produces no
substitute fact.

This is the complete v1 list, not a category description. In particular,
`completion_claim`, `commit_attempt`, and `journey_same_rule_block_count` are
not current facts and are not accepted. Claude and Gemini context hooks do not
currently expose a normalized user-prompt payload to the context builder, so
v1 does not pretend to classify prompt intent. Codex receives richer event
payloads, but host-specific predicates are withheld until a shared contract
exists.

Raw content predicates such as `new_content_contains`, arbitrary tool-output
facts, payload text, clock facts, graph facts, and project-defined Rhai
predicates are denied in version one. This bounds prompt manipulation,
host-specific behavior, and surprise I/O.

The exact allowlist is a single constant in the MCP crate and is exposed by
`phr-mcp context predicates`. Adding a predicate is a reviewed code change.

Historical counts are never accumulated inside RETE. Journey threshold facts
are computed by the existing host derivation before the fresh network is
populated. If a demanded aggregate cannot be produced, the capsule does not
match.

## Configuration

Projects opt in with `.phronesis/context.json`:

```json
{
  "version": 1,
  "hard_max_bytes": 4096,
  "estimated_max_tokens": 900,
  "interaction": {
    "kernel_max_bytes": 768,
    "activity_reserve_bytes": 1024,
    "nudges_max_bytes": 1536
  },
  "session": {
    "kernel_max_bytes": 768,
    "state_reserve_bytes": 384,
    "rules_max_bytes": 2304
  }
}
```

### Authoritative and advisory limits

`hard_max_bytes` is authoritative because every host accepts a byte string and
the existing implementation already enforces this boundary.

`estimated_max_tokens` is a soft admission limit. Version one estimates:

```text
estimated_tokens = ceil(rendered_utf8_bytes / 3)
```

This intentionally overestimates the common English approximation of roughly
one token per four bytes, leaving margin for punctuation and code. It is not
exact for every tokenizer or language and is reported as an estimate.

"Bytes are authoritative" means `hard_max_bytes` is the final transport-safety
contract. It does not mean the byte ceiling must be the first limit reached.
An operator who configures a smaller soft token budget is deliberately asking
the packer to omit otherwise byte-safe items. The default 900-token estimate
therefore intentionally admits about 2,700 rendered bytes under a 4,096-byte
hard guard.

An item is admitted only if both its bytes fit the active byte capacity and
its estimated tokens fit the remaining soft token capacity. The final hard
byte check remains authoritative.

If the soft token limit excludes an item that would fit by bytes, `inspect`
reports that reason. If configuration omits `estimated_max_tokens`, selection
is byte-only.

## Exact packing algorithms

Every renderable unit is an indivisible `ContextItem` with:

```text
kind
stable_id
priority
severity
utf8_bytes
estimated_tokens
body
```

The renderer computes body bytes including headings and separators before
selection. No later formatting step may increase an admitted item's size.

In every normative algorithm below, **pack** means: admit the next complete
item only when it fits (a) the item's kind-specific byte ceiling, when one
exists, (b) the remaining shared byte capacity, and (c) the remaining soft
token capacity when configured. A failed admission omits the item and
continues with the next item; it never makes the final assertion fail.

### Interaction event

Inputs are kernel paragraphs, current/recent activity bullets, and matched
capsules.

1. Reserve `activity_reserve_bytes` from `hard_max_bytes`.
2. Pack activity into the reserve and remaining soft token capacity, ordered by:
   - current-event block;
   - current-event warning;
   - older block newest first;
   - older warning newest first;
   - rule id, then file path as stable tie-breakers.
3. Unused activity reserve returns to the shared capacity.
4. Pack kernel paragraphs in file order up to `kernel_max_bytes`, shared byte
   capacity, and remaining soft token capacity.
5. Sort matched capsules by priority descending, body bytes ascending, id
   ascending. Greedily pack complete capsules up to `nudges_max_bytes`, shared
   byte capacity, and remaining soft token capacity.
6. Activity items that did not fit their reserve are considered again, in the
   same order, against remaining shared byte capacity and remaining soft token
   capacity.
7. Pack at most one fixed omission footer under the same remaining byte and
   soft token capacities.
8. Verify the final body bytes are `<= hard_max_bytes` and estimated tokens
   are `<= estimated_max_tokens` when configured. A violated invariant is an
   internal renderer error handled by the last-resort fail-open guard, never a
   process assertion or panic.

This deliberately gives current activity first claim, then the kernel, then
nudges, then overflow activity. Static kernel content can never borrow beyond
its own ceiling.

### Session event

Inputs are kernel paragraphs, state lines, active-rule summaries, and the MCP
orientation line.

1. Reserve `state_reserve_bytes` from `hard_max_bytes`; shared capacity starts
   at `hard_max_bytes - state_reserve_bytes`. Pack state lines into the reserve,
   subject to remaining soft token capacity, in fixed order: open subject,
   confidence band, freshness diagnostic.
2. Return unused state reserve to shared capacity.
3. Pack kernel paragraphs up to `kernel_max_bytes`, shared byte capacity, and
   remaining soft token capacity.
4. Pack rule summaries by severity descending, priority descending, rule id
   ascending, up to `rules_max_bytes`, shared byte capacity, and remaining
   soft token capacity.
5. Pack the static MCP orientation line if it fits the remaining byte and soft
   token capacities.
6. Reconsider overflow state lines against remaining shared byte capacity and
   remaining soft token capacity.
7. Pack one omission footer under the remaining byte and soft token capacities,
   then verify final limits without a process assertion or panic.

No nudge capsule is emitted at SessionStart unless the host also supplies a
real interaction event with facts. Session construction must not invent an
event merely to trigger capsules.

### Last-resort truncation

Normal packing must never require raw truncation. The existing UTF-8-safe
truncator remains as a final transport guard against renderer bugs or envelope
differences. Every activation of that guard is logged as an internal context
error and covered by a regression test.

## Host lifecycle and compaction

Phronesis promises behavior only for hook events a host actually exposes:

| Capability | Behavior |
|---|---|
| Session orientation event | Render session charter |
| Per-user-interaction event | Render kernel + activity + matched nudges |
| Post-compaction orientation event | Render session charter again |
| Pre-compaction event only | No restoration guarantee |
| No per-interaction event | No always-on kernel guarantee between sessions |

The implementation baseline for this release is:

| Host adapter | Session charter | Interaction context | Post-compaction restoration | Contract source |
|---|---|---|---|---|
| Claude Code | `SessionStart` | `UserPromptSubmit` | No wired post-compaction event | `init.rs` Claude hook map |
| Gemini CLI | `SessionStart` | `BeforeAgent` | No wired post-compaction event | `init.rs` Gemini hook map |
| Codex | `SessionStart` | `UserPromptSubmit` | `PostCompact` renders the session charter | `codex_hook.rs` dispatch and `.codex/hooks.json` |

Codex `PreCompact` currently injects durable text, but it is advisory only and
is not represented by `ContextEvent`; the post-compaction charter is the
normative restoration. `SubagentStart` also renders session context today but
is outside this feature's durability contract. Any host-capability change
requires an adapter test and an update to this table.

A PreCompact injection is not treated as restoration because the host may
discard or summarize it. For Codex/host integrations that expose both pre- and
post-compaction events, only the post-compaction event is normative for
restoration.

For hosts without PostCompact but with a later UserPromptSubmit/BeforeAgent,
the next ordinary interaction restores the kernel. The specification does not
claim the kernel was present during the compaction operation itself.

Host adapters declare capabilities explicitly rather than imitating missing
events. The host-neutral renderer receives an enum:

```rust
enum ContextEvent {
    Session,
    Interaction,
    PostCompact,
}
```

Unsupported events are never synthesized.

## Fact hydration and evaluation

For an interaction event:

1. Load and validate context configuration and capsules.
2. Collect the set of capsule predicates.
3. Produce normalized current-event facts.
4. Demand-compute only allowlisted journey/outcomes facts referenced by a
   loaded capsule. For journey predicates, the context builder passes the
   internal capsule rules through the existing journey rule scanner and
   invokes derivation only for the selector/window/threshold tuples it found;
   with no referenced journey predicate, it performs no journey journal I/O.
   For `context_confidence_band`, the context builder reads the existing
   outcomes report for the open subject, applies the existing `Band`
   calculation, and asserts exactly one context-only band fact before firing.
   It does not modify outcomes storage or engine internals.
5. Populate a fresh RETE network with those facts and internal capsule rules.
6. Fire once and collect `context_nudge` consequences.
7. Discard the network.
8. Pack the matched static bodies with kernel and activity items.

Graph hydration is excluded from version one. It can be added later only with
a measured latency budget and an explicit freshness contract.

Projects with no valid capsules perform no journey/outcomes hydration beyond
what the existing context path already performs. Capsule evaluation errors
produce no candidates and cannot fail the model turn.

## Observability

Context construction records cost and selection, not behavioral causality.
A bounded action-log entry uses the existing rotated `.phronesis/log.jsonl`
with `kind: "context"` and contains no body or user content:

```json
{
  "kind": "context",
  "event": "interaction_context",
  "bytes": 1260,
  "estimated_tokens": 420,
  "kernel_paragraphs": 2,
  "capsules": ["verification-before-completion"],
  "activity_items": 2,
  "omitted": {
    "kernel_paragraphs": 1,
    "capsules": 0,
    "activity_items": 3,
    "rules": 0
  },
  "raw_truncation": false
}
```

The CLI adds:

```text
phr-mcp context inspect --event interaction
phr-mcp context inspect --event session
phr-mcp context inspect --event interaction --json
phr-mcp context predicates
phr-mcp context stats --since 7d
```

`inspect` reports source items, validation failures, selection order, byte and
estimated-token costs, selected items, and omission reasons. Omission reason
codes distinguish `kind_ceiling`, `byte_capacity`, `token_capacity`, and
`displaced_by_nudge`; the last applies when an overflow activity item would
have fit before admitted nudges consumed shared capacity. It uses a normalized
synthetic event unless a supported fixture path is explicitly provided.

`stats` reports only directly observed properties:

- average and p95 bytes per payload;
- average and p95 estimated tokens;
- capsule match and selection counts;
- item omission counts by kind;
- raw-truncation count;
- context build latency.

It does not report compliance, effectiveness, or subsequent-block
correlations. Existing rule statistics and debt-trend readers ignore
`kind: "context"`; context statistics read only that kind. Reusing the bounded
log avoids a second retention and locking design.

## Security and validation

- Configuration and capsules are trusted project files with the same trust
  level as `rules.json` and project hooks.
- Capsule bodies are static and cannot interpolate fact arguments.
- Only a compiled predicate allowlist may trigger capsules.
- Capsule directories and files are resolved inside the canonical project
  root using `security.rs`.
- Symlink escapes and traversal are rejected.
- Per-file capsule size is capped at 8 KiB before parsing; aggregate capsule
  input is capped at 256 KiB.
- Frontmatter is strict JSON parsed through a duplicate-detecting Serde visitor
  into deny-unknown-fields structs. The parser verifies end-of-input; duplicate
  keys, wrong scalar types, unknown fields, trailing values, and non-object
  roots are rejected.
- Markdown is treated as prompt text, not HTML; no rendering or script
  execution occurs.
- The internal `context_nudge` action cannot map to constraint severity.
- Raw payloads, tool output, and matched fact arguments never enter context
  logs.

## Failure behavior

Context injection remains advisory and must not prevent a model turn.

| Failure | Behavior |
|---|---|
| Missing `context.json` | Exact legacy behavior |
| Malformed `context.json` | Warn; render bounded kernel and activity with built-in safe defaults |
| Missing durable file | Continue without kernel |
| Malformed capsule | Skip it; report path-specific warning |
| Capsule body exceeds its declared `max_bytes` | Reject capsule at load time |
| Duplicate capsule id | Skip all duplicates for that id |
| Disallowed predicate | Skip capsule |
| Journey/outcomes unavailable | Dependent capsule cannot match |
| RETE evaluation failure | Emit no capsules; continue with kernel/activity |
| Metrics log failure | Ignore it |
| Last-resort truncation | UTF-8-safe cut, diagnostic, `raw_truncation: true` when logging succeeds |

All diagnostics go to stderr or `context inspect`, never into the prompt body
unless the diagnostic itself is the fixed omission footer.

## Backward compatibility and migration

The feature is opt-in through `.phronesis/context.json`.

Without that file:

- current full `durable.md` reinjection remains byte-for-byte unchanged;
- the 4 KiB cap remains unchanged;
- capsules are not scanned;
- no context metrics are written.

Before the measurement gate passes, newly initialized projects retain legacy
behavior. `phr-mcp init --packs context` explicitly opts in and writes:

- a kernel-oriented `durable.md` under 768 bytes only when no durable file
  exists;
- `context.json` with defaults;
- `.phronesis/nudges/README.md` with the capsule schema.

After the measurement gate passes in a released version, `context` may join
the default init pack set. That default change is a separate reviewed commit;
the implementation does not infer gate success at runtime.

Re-running `init` never overwrites any of these existing files. Existing
projects migrate with `init --packs context` or by manually creating
`context.json` and shortening `durable.md`; no automatic content rewrite is
included in this release.

## Default kernel

```markdown
# Durable project kernel

- Treat Phronesis findings as evidence with stated limits, not proof.
- Obtain current build, test, manual, or traced evidence before claiming completion.
- Surface conflicts between guidance, rules, decisions, and implementation.
- Retrieve detailed rules, decisions, graph facts, and history over MCP when relevant.
```

Projects may add compact privacy or authority constraints. Examples,
procedures, and rationale belong elsewhere.

## Example capsules

### Low grounded confidence

```markdown
---json
{
  "id": "low-grounded-confidence",
  "priority": 95,
  "max_bytes": 240,
  "when": {
    "predicate": "context_confidence_band",
    "args": ["low"]
  }
}
---

Grounded confidence for the open work unit is low. Obtain current build, test,
or known-bug evidence before making an irreversible claim or operation.
```

### Recent rule friction

This example assumes `rule-blocked` is a declared selector in the project's
existing journey configuration:

```markdown
---json
{
  "id": "recent-rule-friction",
  "priority": 80,
  "max_bytes": 320,
  "when": {
    "predicate": "journey_seen",
    "args": ["rule-blocked", "session"]
  }
}
---

A rule has blocked work in this session. Inspect recent nonzero decisions.
Adapt if the rule is valid; propose a scoped governance change only when the
recorded friction shows the rule is over-broad.
```

## Implementation stages

### Stage 1: bounded item packing

1. Add `ContextItem`, byte measurement, estimated-token measurement, and the
   exact interaction/session packers.
2. Parse `context.json` and retain a byte-identical legacy path when absent.
3. Split the durable kernel into paragraphs and enforce its ceiling.
4. Protect activity/state reserves and add complete-item omission reporting.
5. Add `context inspect` and cost-only metrics.

Stage 1 ships independently. It fixes context starvation without RETE capsule
matching.

### Stage 2: static triggered capsules

1. Parse and validate `.phronesis/nudges/*.md` in deterministic order.
2. Add the positive condition subset and predicate allowlist.
3. Compile match-only rules and map the internal `context_nudge` action.
4. Demand-compute existing journey/outcomes facts.
5. Pack matched static capsule bodies under the exact interaction algorithm.

### Stage 3: template and measurement

1. Measure explicitly opted-in use in this repository and one external corpus.
2. Tune defaults from observed cost, omission, latency, and false-relevance
   review.
3. Confirm every measurement-gate acceptance target.
4. In a separate reviewed commit, add `context` to the default init pack set
   and replace the generated durable template for new projects only.
5. Ship default capsules only when their trigger facts are already in the v1
   allowlist and corpus review shows acceptable relevance.

Graph-triggered capsules, exact model tokenizers, and any persistent
scheduling remain separate future proposals.

## Acceptance tests

### Compatibility

- Without `context.json`, session and interaction bodies are byte-identical to
  legacy behavior.
- Existing durable/config/nudge files are never overwritten by `init`.
- Host envelopes wrap the same host-neutral body for the same event kind.

### Packing

- Every admitted item includes all headings and separators in its measured
  size.
- Body bytes never exceed `hard_max_bytes`.
- Estimated tokens never exceed the configured soft limit.
- No normal path cuts inside an item.
- Activity receives its reserve before kernel or capsules.
- Kernel never exceeds `kernel_max_bytes` and never borrows.
- The two-pass activity overflow step is deterministic.
- Session state, kernel, rules, and overflow ordering match the normative
  algorithm.
- Identical inputs and evaluation timestamp produce byte-identical output.
- Kernel-alone-over-budget, item-exactly-at-boundary, footer-does-not-fit, and
  multibyte UTF-8 boundaries are covered.
- The last-resort truncator is unreachable for all generated-property cases
  within schema limits.

### Capsule parsing and security

- Static body and valid positive conditions load.
- Unknown or duplicate JSON keys, wrong scalar types, trailing values,
  duplicate ids, invalid ids, variables, templates, scripts, `not`, `unless`,
  empty groups, and disallowed predicates are rejected.
- A canonical capsule body larger than its declared `max_bytes` is rejected at
  load time rather than silently competing under a false author assertion.
- User-controlled fact arguments containing Markdown instructions, newlines,
  control characters, or oversized strings never appear in output.
- A malicious filename or tool payload cannot create an unconfigured capsule
  body or exhaust more than the configured nudge capacity.
- Symlink and traversal escapes are rejected.

### RETE and hydration

- A capsule matches when its complete positive fact conditions are asserted.
- It does not match when any `all` fact is absent.
- `any` produces one candidate even if several branches match.
- Repeated RETE activations for one capsule deduplicate by capsule id.
- The internal action returns an Event consequence through
  `fire_all_consequences()`, emits no fact, and cannot change verdict severity.
- Journey aggregate facts are host-derived before network population.
- Unavailable demanded facts produce no match, not a positive or negative
  substitute.
- Projects with no valid capsules do no capsule-specific journey/outcomes I/O.

### Host lifecycle

- Session renders charter; interaction renders kernel/activity/nudges.
- PostCompact renders charter only when the host emits that capability.
- PreCompact alone makes no restoration claim.
- A later interaction after compaction restores the kernel.
- Unsupported events are not synthesized.

### Observability and failure

- Metrics contain ids, counts, costs, omissions, and latency but no bodies,
  fact arguments, raw payloads, or user content.
- Context metrics use `kind: "context"`; rule statistics and debt trends ignore
  that kind.
- Metrics make no effectiveness or compliance claim.
- Malformed configuration and capsules fail open with deterministic
  diagnostics.
- RETE, hydration, and metrics failures cannot fail the model turn.
- Every activation of raw truncation is observable.

## Measurement gate

Before enabling `context.json` by default for newly initialized projects,
measure this repository and one external corpus:

- median and p95 payload bytes;
- median and p95 estimated tokens;
- reduction from legacy full-file interaction injection;
- activity/rule/capsule omission counts;
- context construction latency;
- raw-truncation count;
- manual review of every capsule match for false relevance.

Acceptance targets:

- at least 60% reduction in median static interaction-context bytes compared
  with legacy full-file injection;
- zero omitted current-event blocking items in the measured corpora;
- zero raw truncations;
- p95 context construction under 5 ms when no journey/outcomes hydration is
  demanded;
- every false-relevance finding explained and either fixed or documented
  before the default template changes.

## Open questions

1. Whether exact tokenizers justify their model-version and dependency cost in
   a later optional feature.
2. Whether `memory-drift` should distinguish always-on kernel coverage from
   situational capsule coverage.
3. Which shared, normalized prompt-intent facts would justify a future
   allowlist expansion across Claude, Gemini, and Codex without parsing raw
   user text inside context construction.

None blocks Stage 1.

## Decision summary

Phronesis retains a small always-on kernel, protects current enforcement
activity, and introduces static trusted capsules selected only by positive,
allowlisted facts in a fresh RETE network. Packing is stateless and fully
specified. Bytes are authoritative; tokens are conservatively estimated.
Compaction behavior is capability-based and never overstated.

Durability means:

> The model receives the smallest trusted guidance justified by the current
> situation, within explicit context limits, without turning prompt selection
> into another persistent state machine.

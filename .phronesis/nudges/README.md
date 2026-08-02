# Situational context nudges

Each `*.md` file in this directory is a **capsule**: a short piece of trusted
project guidance that is injected into the model's context only when current
facts make it relevant. Files load in bytewise filename order.

A capsule is strict JSON frontmatter delimited by `---json` and a line of
exactly `---`, followed by a static Markdown body:

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

## Frontmatter

| Field | Type | Contract |
|---|---|---|
| `id` | string | `[a-z0-9][a-z0-9-]{0,63}`, unique across the project |
| `priority` | integer | 0–100 inclusive; higher packs first |
| `max_bytes` | integer | 64–1024 inclusive; an assertion about *this* body |
| `when` | condition | positive condition tree, see below |

Parsing is deliberately unforgiving, because this is a prompt surface:
duplicate keys at any nesting level, unknown fields, wrong scalar types,
trailing JSON after the object, and non-object roots are all errors. A
duplicate `id` skips **every** file sharing it — no copy wins. A body larger
than its declared `max_bytes` is rejected at load time rather than silently
competing under a false assertion.

`max_bytes` bounds one capsule. Competition *among* capsules is governed by
`interaction.nudges_max_bytes` in `.phronesis/context.json`.

## Conditions

`when` is either a single leaf or a positive `all` / `any` group:

```json
{"when": {"predicate": "context_confidence_band", "args": ["low"]}}

{"when": {"all": [
  {"predicate": "journey_seen", "args": ["rule-blocked", "session"]},
  {"predicate": "context_confidence_band", "args": ["low"]}
]}}
```

- `args` are exact constant matches. Variables (`?name`) are rejected —
  the body cannot use bindings, so a binding would buy nothing.
- `all` and `any` nest, up to 16 levels and 256 expanded alternatives.
- `not`, `unless`, empty groups, scripts, and actions are rejected. There is
  no absence-based trigger: a capsule fires on facts that are true, never on
  facts that are missing.

Only allowlisted predicates may trigger a capsule. Run
`phr-mcp context predicates` for the current list — adding to it is a
reviewed code change, not a configuration option.

## Bodies are static

A capsule body is literal text. Runtime facts select a capsule but are never
interpolated into it, which is what stops a filename, tool output, or rule
parameter from becoming a second-order prompt-injection channel. `?variable`,
`{{ ... }}`, and `${ ... }` are rejected so no author is misled into thinking
substitution happens.

The renderer appends only the capsule's own trusted id:

```text
[phronesis nudge: low-grounded-confidence]
```

Keep detailed procedures in ADRs or project docs — a capsule may name a stable
MCP tool or decision id in prose, but it is a short situational reminder, not
documentation.

## Checking your work

```sh
phr-mcp context predicates                    # what may trigger a capsule
phr-mcp context inspect --event interaction   # dry run: what would be selected, and why not
phr-mcp context stats --since 7d              # observed cost and selection counts
```

`inspect` is read-only: it writes no observation, so inspecting never
contaminates the data it reports on. It names every capsule that failed to
load and every demanded fact that could not be hydrated — which is how you
tell "the facts were false" apart from "the selector has a typo."

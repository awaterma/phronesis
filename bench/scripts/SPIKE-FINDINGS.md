# SPIKE-FINDINGS — Phase 0 (Tasks 1 & 2)

Environment: macOS (Apple Silicon), phronesis @ `feat/phr-bench`, phr-mcp 0.36.0,
claude CLI 2.1.283, ollama server 0.34.4 (client binary 0.30.11), Docker.app installed.

## Task 1 — router wiring + hooks-in-headless: PASS

- `spike-router.sh`: **PASS** — 358 events, 3 assistant messages, 1 structured
  `tool_use`, `proof.txt` created. Elapsed **5 s** (2 turns; `duration_ms=5029`).
- `spike-hooks.sh`: **PASS** — log.jsonl had 45 entries, **21 pre_checks, 1
  blocked** (the `unwrap` rule, exit 2), 2 lifecycle entries. The agent ran 37
  turns (408 s) after the block; `permission_denials: 1` in the result event.
  Hooks fire in headless `claude -p` — the treatment arm is real.

### Router architecture (operated)

**No translation proxy is needed.** The local ollama daemon (>= 0.34) speaks the
Anthropic Messages API natively at `POST /v1/messages`, including structured
`tool_use` blocks and `stop_reason: "tool_use"` (verified by direct curl).
`ANTHROPIC_BASE_URL` points straight at ollama.

> Do **not** route through LiteLLM 1.79.3 (`anthropic /v1/messages` →
> `ollama_chat/...`): tool-calls come back broken — the model's tool invocation
> arrives as literal *text* (`name": "Write", "arguments": {...` inside a text
> block), no structured tool_use, no file written. Direct ollama is both simpler
> and correct.

Working env (`bench/run-env.sh`, from `bench/run-env.example`):

```bash
export ANTHROPIC_BASE_URL="http://127.0.0.1:11434"   # local ollama, Anthropic Messages API
export ANTHROPIC_API_KEY="ollama-local-dummy"        # satisfies the CLI auth check; ignored by ollama
export ANTHROPIC_MODEL="glm-5.3:cloud"
export ANTHROPIC_SMALL_FAST_MODEL="glm-5.3:cloud"    # aux/background calls map to the same model
```

Cosmetic warnings, ignorable: `[claude-code:unrecognized_model] {"model":"glm-5.3:cloud"}`
(model is not in claude's catalog; requests route fine) and "claude.ai connectors
are disabled because ANTHROPIC_API_KEY … takes precedence".

### stream-json event shapes (contract for Task 5)

Top-level event `type`s observed: `system` (`subtype`: `init`,
`hook_started`, `hook_response`), `assistant`, `user`, `result`.

- **`tool_use` items are nested**: they appear as `{"type":"tool_use", ...}`
  entries inside `assistant` events' `message.content` arrays — there is **no
  top-level `tool_use` event type**. The plan sketch's top-level filter was
  wrong; Task 5's nested-scan approach is correct.
- **glm-5.3 emits `thinking` content items** ("thinking" blocks) — the parser
  must tolerate them alongside `text` and `tool_use`.
- **Token usage is available**: the `result` event carries
  `usage.input_tokens`, `usage.output_tokens`, `usage.cache_read_input_tokens`,
  `num_turns`, `duration_ms`, and `modelUsage` (per-model totals). Task 5
  should take `tokens_in`/`tokens_out` from the `result` event — never null in
  this stack.
- `result.subtype` observed: `success`. `result.is_error` and
  `result.api_error_status` exist for error propagation (Task 8).
- `total_cost_usd` is present but meaningless through the dummy key — cost must
  come from token counts if wanted at all.
- `system` events dominate by count (hook_started/hook_response pairs); the
  parser should not treat their absence/presence as significant.

### Corrections made to the plan sketches (scripts committed for provenance)

1. `spike-router.sh`: `ROOT` is captured **before** `cd` into the temp dir —
   `git rev-parse` in a non-repo dir would fail the script under `set -e`.
2. `spike-router.sh`: `tool_use` counted from nested assistant content (above).
3. `spike-hooks.sh`: phr-mcp >= 0.36 wires claude hooks into
   **`.claude/settings.local.json`** (also `.codex/hooks.json`,
   `.gemini/settings.json`), not `.claude/settings.json`; the assertion accepts
   either.

## Task 2 — SWE-bench instance end-to-end

(Filled in by `spike-eval.sh` — pinned dataset id/revision, real column names,
exact `run_evaluation` flags, Docker x86-on-Apple-Silicon timings.)
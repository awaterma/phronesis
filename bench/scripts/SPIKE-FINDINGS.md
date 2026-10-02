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

## Task 2 — SWE-bench instance end-to-end: PASS

**Everything below is the contract for Tasks 9 and 13.**

### Pinned dataset and instance

- Dataset: `SWE-bench/SWE-bench_Multilingual` @ **846e647b9f33c0b51b739d005d13d85493c9af09**
  (HF dataset card: 300 test instances, 41 repos, 9 languages).
- **Real column names** (differs from the plan sketch): `base_commit, created_at,
  eval_type, image, instance_id, log_parser, repo, version, patch, test_patch,
  eval_script, problem_statement, hints_text, FAIL_TO_PASS, PASS_TO_PASS`.
  - **No `language` column** — language is encoded in the repo/instance_id and
    the parser name (rust ⇒ `log_parser: "parse_log_cargo"`).
  - The issue column is **`problem_statement`** (not `issue_text`).
  - `repo` is `owner/name` — clone `https://github.com/<repo>.git`.
  - `FAIL_TO_PASS`/`PASS_TO_PASS` are **real lists** here (not JSON-encoded strings).
  - Each instance ships a **prebuilt docker image** (`image`,
    `sweb.eval.x86_64.<id>`), an `eval_script`, and a `log_parser`.
- Rust instances: 43 (ruff 7, ripgrep 2, tokio 9, axum 7, bat 8, nushell 5,
  uutils/coreutils 5) — all `parse_log_cargo`.
- Spike instance: **`burntsushi__ripgrep-2209`** (smallest rust repo, one
  regression test), base_commit `4dc6c73c5a9203c5a8a89ce2161feca542329812`.

### Pinned harness

- The PyPI `swebench` package (5.0.2) **lacks** multilingual log parsers.
  The official harness is the **main SWE-bench repo** @
  **02e7a74ffd0b707aab73d203fe87bdc7c76afc8e** (`swebench/harness/log_parsers/rust.py`
  has `parse_log_cargo`).
- Working invocation (exact flags; venv: `bench/.venv`, Python 3.12 via uv —
  py3.14 lacks pyarrow wheels for `datasets`):

```bash
PYTHONPATH=<swe-bench checkout @ 02e7a74f> bench/.venv/bin/python \
  -m swebench.harness.run_evaluation \
  -d SWE-bench/SWE-bench_Multilingual -s test \
  -p <predictions.jsonl> -id <run-id> --max_workers 1
```

- `preds.jsonl` line format: `{"instance_id": ..., "model_name_or_path": ...,
  "model_patch": "<patch.diff contents>"}`. `--predictions_path gold` evaluates
  reference solutions.
- Harness writes reports to `./logs/evaluation/<run-id>/results.json`
  (gitignored).

### Docker x86 on Apple Silicon

- Images are x86_64-only: `docker pull` on arm64 fails with "no matching
  manifest" — **`docker pull --platform linux/amd64`** works and runs under
  emulation. The harness pulls images itself; if it hits the same manifest
  failure, pre-pull with `--platform linux/amd64` (Task 9 must do this).
- **Emulation is fast enough**: full eval of one ripgrep instance ≈ **19-21 s**
  (warm prebuilt image, incremental cargo). Verification will not be the
  bottleneck; the agent runs will be.

### End-to-end results

- **Gold validation**: reference `patch` through the harness —
  **resolved 1/1, 19.0 s, zero infra failures** (`logs/evaluation/spike-gold/`).
- **Control arm**: claude -p (glm-5.3:cloud via local ollama) on the real
  ripgrep-2209 issue — **71 turns, 598 s (~10 min), 5,782-byte patch,
  resolved 1/1 in 20.4 s** (`logs/evaluation/spike-control/`).
- Per-run telemetry from the `result` event: `tokens in 111,614 / out 33,067 /
  cache-read 5,333,184`, `num_turns: 71`, `duration_ms: 598484`.
- A ~10-minute, 71-turn run on a rust task sits well inside the 45-min cap and
  the pilot gate's ≤30-min mean expectation.

### Corrections to the plan sketch (spike-eval.sh committed for provenance)

1. `repo` needs the `https://github.com/<repo>.git` prefix.
2. The patch is extracted as `git add -A && git diff --cached <pre-run HEAD>` —
   the agent may commit, and may leave untracked files; both must be captured.
3. `problem_statement`, not `issue_text`; no `language` column (see above).
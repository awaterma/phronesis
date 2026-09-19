# Gemini CLI lifecycle payload fixtures

These fixtures were captured from a real Gemini CLI session running in a
scratch project with `sh`-based capture hooks (see Plan 4 Task 0). They are
redacted: prompts, `prompt_response`, and `tool_input.prompt` are replaced
with `<redacted:N bytes>`; absolute home and workspace paths are replaced
with `/home/dev` and `/home/dev/scratch`; the real `session_id` is replaced
with the synthetic `gemini-s-001`.

## Capture provenance

- **Gemini CLI version:** `0.46.0` (`gemini --version`)
- **Capture date:** 2026-09-19
- **Capture method:** `gemini --skip-trust -p "use the codebase_investigator sub-agent to read README.md and tell me what it says"` in a scratch git project at `/tmp/phr-gemini-scratch` with the six `sh -c 'mkdir -p /tmp/phr-gemini && cat > /tmp/phr-gemini/<Event>.json; echo {}'` hooks from Plan 4 Task 0 Step 1.
- **Observed `agent_name`:** `codebase_investigator` (snake_case, Gemini built-in). A separate run also produced `generalist` and `cli_help`; the snake_case name is the one the sanitizer test in Task 3 asserts on.

## Field observations

- **`SessionStart` carries a `source` field:** yes, value `"startup"`. (A resume would carry `"resume"`.)
- **`AfterTool` uses `tool_response`, not `tool_output`:** confirmed. The payload key is `tool_response`, and it carries an object with `llmContent` (array of `{text}`) and `returnDisplay`. This pins spec §Adjacent findings 4 — the repo previously misread the field as `tool_output`.
- **`AfterAgent` carries `prompt` and `prompt_response`:** confirmed. `prompt` echoes the turn's prompt; `prompt_response` is the agent's final text. `stop_hook_active` is present and `false` on a normal stop.
- **`SessionEnd` carries a `reason` field:** value `"exit"`.

## Esc / interrupt observation (Task 0 Step 4)

SKIPPED, needs human: the Esc-then-new-prompt sequence requires an
interactive Gemini session and cannot be reproduced in headless (`-p`)
mode. The `open_turn` inference that depends on `AfterAgent` *not* firing
on an abort is covered by the end-to-end test
`gemini_second_prompt_without_after_agent_is_an_interrupt_and_correction`
in `tests/hook_integration.rs` (Task 4), which simulates the missing
`AfterAgent` directly. A human-run interactive capture that confirms
`AfterAgent.json` retains the *previous* turn's `prompt` after an Esc
would strengthen the evidence but is not blocking; the behaviour is pinned
by the test regardless.
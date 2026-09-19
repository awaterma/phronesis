# Claude Code lifecycle payload fixtures (raw, synthetic)

The JSON files in this directory are **synthetic**, assembled from the field
lists in the Claude Code hooks reference. They are **not** captures from a live
Claude Code session. The Claude Code version and the capture date are both
**unknown**. These are evidence, not contract fixtures:
`tests/payload_contract.rs::collect_fixtures` walks exactly one level
(`payloads/<cli>/*.json`), so nothing in `raw/` is replayed by the contract
runner. Task 7 of the Claude adapter plan promotes them into
`payloads/claude/*.json` envelopes.

The human should replace these synthetic files with real captures per Task 1 of
the plan before merging. The steps below are the plan's Task 1 procedure, kept
here so the replacement is repeatable.

## How to capture real payloads (Plan 2 Task 1)

1. Add these hooks to `.claude/settings.local.json` in this repo, merging
   into the existing `hooks` object:

   ```json
   {
     "hooks": {
       "UserPromptSubmit": [{"matcher": "", "hooks": [{"type": "command", "command": "sh -c 'mkdir -p /tmp/phr-capture && cat > /tmp/phr-capture/UserPromptSubmit.json; echo {}'"}]}],
       "SessionStart":     [{"matcher": "", "hooks": [{"type": "command", "command": "sh -c 'mkdir -p /tmp/phr-capture && cat > /tmp/phr-capture/SessionStart.json; echo {}'"}]}],
       "SessionEnd":       [{"matcher": "", "hooks": [{"type": "command", "command": "sh -c 'mkdir -p /tmp/phr-capture && cat > /tmp/phr-capture/SessionEnd.json; echo {}'"}]}],
       "SubagentStart":    [{"matcher": "", "hooks": [{"type": "command", "command": "sh -c 'mkdir -p /tmp/phr-capture && cat > /tmp/phr-capture/SubagentStart.json; echo {}'"}]}],
       "SubagentStop":     [{"matcher": "", "hooks": [{"type": "command", "command": "sh -c 'mkdir -p /tmp/phr-capture && cat > /tmp/phr-capture/SubagentStop.json; echo {}'"}]}],
       "Stop":             [{"matcher": "", "hooks": [{"type": "command", "command": "sh -c 'mkdir -p /tmp/phr-capture && cat > /tmp/phr-capture/Stop.json; echo {}'"}]}],
       "PreToolUse":       [{"matcher": "", "hooks": [{"type": "command", "command": "sh -c 'mkdir -p /tmp/phr-capture && cat >> /tmp/phr-capture/PreToolUse.jsonl; echo {}'"}]}],
       "PostToolUse":      [{"matcher": "", "hooks": [{"type": "command", "command": "sh -c 'mkdir -p /tmp/phr-capture && cat >> /tmp/phr-capture/PostToolUse.jsonl; echo {}'"}]}]
     }
   }
   ```

   The two tool hooks **append** (`>>`); everything else overwrites, so the
   last writer wins.

2. Quit and restart Claude Code in this repo (hooks load at startup).
3. Drive one session: a plain turn, a sub-agent dispatch, and an Esc
   interrupt, per Plan 2 Task 1.
4. Redact every file: `prompt`, `prompt_response` and
   `last_assistant_message` values become `"<redacted:N bytes>"`,
   absolute home paths become `/home/dev`, and real
   session/transcript/prompt/agent/tool ids become obviously-synthetic
   ones. Keep every key, including keys with empty-string values.
5. From `PreToolUse.jsonl`, pick one main-agent and one sub-agent line; save
   them as `PreToolUse.json` and `PreToolUse-in-subagent.json`.
6. Copy the last 64 KiB of the transcript as
   `tests/fixtures/transcripts/claude/interrupted-tail.jsonl`, redacting
   prose but leaving the `[Request interrupted by user]` marker lines
   verbatim.
7. Record the real `claude --version`, the capture date, the observed
   `SessionStart` `source` value, and whether sub-agent `PreToolUse`
   carries `agent_id` in this README.
8. Remove the temporary capture hooks
   (`git checkout -- .claude/settings.local.json`).

## Status

- [ ] **Not yet captured.** The files present are synthetic placeholders
      built from the documented Claude Code hook field lists. The version,
      capture date, `SessionStart` `source` value, and the sub-agent
      `agent_id` question are all unknown until the human performs the
      capture above and replaces them.
# Claude Code lifecycle payload fixtures (raw)

Every `*.json` file in this directory is a **real capture** from a live Claude
Code session, redacted and scrubbed as described below. Nothing here is
synthetic any more.

These are evidence, not contract fixtures:
`tests/payload_contract.rs::collect_fixtures` walks exactly one level
(`payloads/<cli>/*.json`), so nothing in `raw/` is replayed by the contract
runner. `payload_contract.rs` pins the *field sets* of these files;
`payloads/claude/*.json` holds the replayable envelopes.

## Provenance

| item | value |
|---|---|
| Claude Code version | `2.1.270 (Claude Code)` |
| capture date | 2026-09-20 |
| mode | headless, `claude -p --permission-mode bypassPermissions --model sonnet` |
| project | a throwaway git repo in `/tmp` with a README, one source file, `NOTES.md`, initialised with `phr-mcp init --packs none` |
| capture mechanism | `PHRONESIS_CAPTURE_DIR=/tmp/phr-capture-claude`, i.e. `hook/mod.rs::capture_raw_payload` — **not** the ad-hoc `cat >` hooks the previous version of this file described |
| scrubber | `phr-mcp scrub-payload` 0.34.0, `--project-root <the temp project>` |

Two consecutive headless sessions contributed:

- `PreToolUse.json` / `PostToolUse.json` — session A (`prompt_id` →
  `claude-p-001`).
- everything else, including `PreToolUse-in-subagent.json` — session B
  (`prompt_id` → `claude-p-002`).

Both sessions' `session_id` scrubbed to the same `sess-00000000`, so the
corpus reads as one session. That is a scrubber artefact, stated here rather
than hidden.

## What the redaction did

1. `capture_raw_payload` replaced every `prompt`, `prompt_response` and
   `last_assistant_message` value, at any depth, with
   `"<redacted:N bytes>"` **before** the payload ever reached disk. The
   byte counts in these files are the real lengths of real text.
2. `phr-mcp scrub-payload` then rewrote `$HOME` → `/home/dev`, the project
   root → `/home/dev/project`, the username, the session id
   (→ `sess-00000000`) and the transcript path
   (→ `/home/dev/.claude/transcript.jsonl`).
3. One manual substitution afterwards: the real `prompt_id` UUIDs became
   `claude-p-001` / `claude-p-002`, because `scrub-payload` did not touch
   `prompt_id` at the time and it is UUID-shaped. **That gap is now closed**:
   the scrubber treats `prompt_id`, `turn_id` and `agent_id` as identity keys
   and rewrites them to `prompt-00000000` / `turn-00000000` /
   `agent-00000000`, so a fresh capture no longer needs the manual step.

These files keep their hand-substituted values rather than the scrubber's
placeholders, verified to contain no UUID under any identity key: the
placeholder is one value per class, and collapsing `claude-p-001` and
`claude-p-002` into one would erase the two-turn correlation these fixtures
exist to pin. `agent_id` (`a36af5c22bb836f19`) and `tool_use_id`
(`toolu_01…`) are likewise left verbatim: they are opaque, per-session, and
their *shape* is the thing under test.

`git commit` messages and `NOTES.md` content are from the throwaway project
and carry nothing private.

## What the captures settled

- **`prompt_id` is present** on `UserPromptSubmit`, `SubagentStart`,
  `SubagentStop`, `Stop`, `SessionEnd`, `PreToolUse` and `PostToolUse`. It is
  a UUID and is stable for the whole turn — it is the adapter's `turn_id`.
- **`SessionStart` `source` is `"startup"`** for a headless `-p` run.
  `SessionEnd` `reason` is `"other"`.
- **Sub-agent `PreToolUse`/`PostToolUse` do carry `agent_id` *and*
  `agent_type`** (`PreToolUse-in-subagent.json`). `inflight` agent scoping
  therefore works against the real host, and the spurious-`correction` case
  the spec worries about is theoretical rather than real.
- **`agent_type` is `"Explore"`**, non-empty — the empty-`agent_type` case of
  Open question 2 did not reproduce for a first-party sub-agent type.
- **Claude sends no exit code for `Bash`.** `tool_response` is
  `{gitOperation, interrupted, isImage, noOutputExpected, stderr, stdout}`.
  It does, however, carry `gitOperation.commit.{branch,kind,sha}` on a
  commit — ground truth the adapter currently ignores.
- Fields present in the real payloads that the spec's documented list does
  not mention: `effort`, `permission_mode`, `background_tasks`,
  `session_crons`, `duration_ms`, `cwd`, `noOutputExpected`, `gitOperation`.

## Still synthetic / still missing

- **The interrupted transcript tail**
  (`tests/fixtures/transcripts/claude/interrupted-tail.jsonl`) is **not**
  captured. It needs an interactive Esc during a running turn, which a
  headless `-p` run cannot produce. Capture it from an interactive session
  before the marker branch is trusted.
- **No `SubagentStop` with an empty `agent_type`** (Open question 2) and no
  `SessionStart` with `source` `resume` / `clear` / `compact` / `fork`.
- **No `UserPromptSubmit` with `mode: mid_turn`**, and therefore no answer to
  Open question 1 from this corpus: a headless run submits exactly one prompt
  per session, so a system-injected `UserPromptSubmit` could neither be
  provoked nor ruled out. That question still needs an interactive session.

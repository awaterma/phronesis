#!/bin/sh
# Fake claude CLI for runner tests — emits a canned stream-json transcript,
# optionally simulates agent work / failure / slowness, then exits.
# No model calls: behavior is steered entirely by env knobs.
#
#   FAKE_CLAUDE_TRANSCRIPT  emit this file instead of the canned transcript
#   FAKE_CLAUDE_EXIT        exit code (default 0)
#   FAKE_CLAUDE_SLEEP       sleep this many seconds before exiting (default 0)
#   FAKE_CLAUDE_STDERR      write this string to stderr before exiting
#   FAKE_CLAUDE_TOUCH       create this file (the "agent's" worktree edit)
#   FAKE_CLAUDE_DUMP_ENV    dump `env` output into this file
set -u

if [ -n "${FAKE_CLAUDE_DUMP_ENV:-}" ]; then
  env > "$FAKE_CLAUDE_DUMP_ENV"
fi

if [ -n "${FAKE_CLAUDE_TRANSCRIPT:-}" ] && [ -f "$FAKE_CLAUDE_TRANSCRIPT" ]; then
  cat "$FAKE_CLAUDE_TRANSCRIPT"
else
  cat <<'EOF'
{"type":"system","subtype":"init","session_id":"fake"}
{"type":"assistant","message":{"id":"m1","content":[{"type":"text","text":"Working on it."}]}}
{"type":"result","subtype":"success","usage":{"input_tokens":300,"output_tokens":130},"num_turns":2,"duration_ms":1234}
EOF
fi

if [ -n "${FAKE_CLAUDE_TOUCH:-}" ]; then
  printf 'proof\n' > "$FAKE_CLAUDE_TOUCH"
fi

if [ -n "${FAKE_CLAUDE_STDERR:-}" ]; then
  echo "$FAKE_CLAUDE_STDERR" >&2
fi

if [ -n "${FAKE_CLAUDE_SLEEP:-}" ]; then
  sleep "$FAKE_CLAUDE_SLEEP"
fi

exit "${FAKE_CLAUDE_EXIT:-0}"
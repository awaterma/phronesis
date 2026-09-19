# SPEC: agent lifecycle events — sub-agent start/stop, interrupts, mid-turn context, corrections, kalpas

**Status:** draft, revised after adversarial review (2026-09-18)
**Authors:** Claude, Andrew Waterman
**Date:** 2026-09-18
**Target release:** phronesis-mcp 0.35.0 (MINOR — new journal record kind and
              schema version, new `claude-hook` subcommand, new hook
              registrations in all three init writers, new action-log kind,
              new metrics family, new built-in selector namespace, new `kalpa`
              subcommand). No change to the `phr` library crate.
**Affects:** `crates/phronesis-mcp/src/{main.rs, hook/{mod.rs, pre.rs, post.rs,
              seq.rs, journey_record.rs}, codex_hook.rs, codex_hook/renderer.rs,
              context.rs, init.rs, journey/{journal.rs, derive.rs, mod.rs},
              journey_cli.rs, stats.rs, payload_scrub.rs}`, new
              `crates/phronesis-mcp/src/lifecycle/`,
              `crates/phronesis-metrics/src/families.rs`,
              and one amendment paragraph in `docs/specs/SPEC-journey-facts.md`
              (see §Determinism and versioning).

## Premise

Phronesis records what an agent does to files and shells. It records nothing
about the shape of the conversation around those actions: when a sub-agent was
spawned and when it returned, when the human stopped the agent mid-turn, and
when the human spoke while the agent was still working. Those moments are
where governance signal is densest. An interrupt followed by a new prompt is a
correction, and corrections are the raw material for the friction-driven rule
proposals described in `docs/participatory-governance.md`. Without lifecycle
records a rule cannot say "warn when a sub-agent ends and no test ran" or "two
corrections this session, raise a suggestion", and a human cannot read the log
and see where the agent went wrong.

Today (`v0.34.0`):

- `.phronesis/log.jsonl` records `pre_check`, `post_check`, `codex_hook`
  (tool phases only), `context` renders, and MCP calls. Nothing else.
- The Codex adapter (`codex_hook.rs::dispatch`, lines 126–144) already receives
  `SubagentStart`, `SubagentStop`, `Stop`, and `UserPromptSubmit`, but only
  renders context or runs the confidence gate. It writes no log or journal
  entry for any of them, and `CodexPayload` (lines 35–51) drops `agent_id`,
  `agent_type`, `prompt`, `transcript_path`, and `stop_hook_active`.
- The Claude Code path registers no `SubagentStart`, `SubagentStop`, or `Stop`
  hooks (`init.rs::write_settings`, lines 580–623). `HookPayload`
  (`hook/mod.rs:55–65`) keeps only tool name, input, and output; `session_id`,
  `hook_event_name`, and `tool_use_id` are discarded. `session-context` and
  `interaction-context` never read stdin (`main.rs:675–700`).
- The Gemini path registers `BeforeTool`, `AfterTool`, `SessionStart`, and
  `BeforeAgent` (`init.rs:652–712`).
- Nothing pairs a tool's pre-check with its post-check. They are separate
  processes with no shared key.
- Two session-id sources can disagree: the Claude/Gemini path uses
  `journey::current_sid` (the `.phronesis/journey/session` file); the Codex
  journal write uses `payload.session_id` directly (`codex_hook.rs:1058`).

## Goals

1. Record these lifecycle events from Claude Code, Codex CLI, and Gemini CLI
   with one schema: sub-agent start, sub-agent stop, human prompt (fresh turn,
   mid-turn steer, or post-interrupt correction), interrupt, turn stop, and
   commit.
2. Make them visible to rules through the existing `journey_*` fact family
   with no aggregator API changes.
3. Make them visible to humans in `.phronesis/log.jsonl`, `phr-mcp journey`,
   `phr-mcp stats`, and the Prometheus families, including the full scrubbed
   prompt text.
4. Let a human name a **kalpa**, a theme spanning sessions, so lifecycle
   counts can be reported per theme.
5. Never break an existing journey rule: every existing selector and every
   existing window computes the same facts before and after this change, for
   the same tool records.

## Non-goals

- Detecting corrections lexically ("no, do X instead"). False positives in the
  journal cannot be unseen by rules. A correction is a prompt that follows an
  interrupt, nothing more.
- Reading transcripts for anything beyond the host's own interrupt marker.
- Merge detection. A merged PR is not reliably observable from a shell
  command's text and exit code; a local merge produces a commit, which is
  already recorded. Revisit when a GitHub-side signal exists.
- Sub-agent support for Gemini CLI beyond what its hook surface exposes.
- Crush and other hosts without a hook protocol.
- Fixing the Codex `PreCompact`/`PostCompact` response bug (§Adjacent
  findings). It is real and separate.
- Derived ratios other than one: `interventions / commit` is printed because
  it is the autonomy signal the feature exists for, and it is printed beside
  the retention boundary so its window is visible. No other ratio ships; raw
  counts do.

## Event model

Six kinds in the core stream, plus `kalpa_start` / `kalpa_end` described in
§Outcomes and kalpas. Every kind produces one journal record and one
action-log entry.

| kind | when | host source |
|---|---|---|
| `subagent_start` | a sub-agent is spawned | Claude `SubagentStart`; Codex `SubagentStart`; Gemini `BeforeTool` on `invoke_agent` |
| `subagent_stop` | a sub-agent returns | Claude `SubagentStop`; Codex `SubagentStop`; Gemini `AfterTool` on `invoke_agent` |
| `prompt` | the human submits text | Claude `UserPromptSubmit`; Codex `UserPromptSubmit`; Gemini `BeforeAgent` |
| `interrupt` | the human aborts a running turn | Codex `Interrupt` (direct); Claude and Gemini inferred at the next `prompt` (§Classification) |
| `stop` | the main agent finishes a turn | Claude `Stop`; Codex `Stop`; Gemini `AfterAgent` |
| `commit` | `HEAD` moved during a shell tool call | derived in `post-check` (§Outcomes and kalpas) |

A `prompt` record carries a `mode`:

| mode | meaning |
|---|---|
| `fresh` | the previous turn ended with a `stop` (or this is the first prompt of the session) |
| `mid_turn` | the previous turn is still open and no interrupt was detected: the human added context while the agent worked |
| `correction` | an `interrupt` record immediately precedes this prompt in the same session |

**Intervention.** The autonomy signal this feature exists to measure is
*interventions per completed task*, and it only means something if an
intervention is the human **changing the plan**, not merely replying. The
mode classification is what separates the two: a `fresh` prompt arrives after
the agent stopped and is, as far as hooks can tell, a reply; a `mid_turn` or
`correction` prompt arrives while the agent was still executing its plan, or
after the human stopped it, and is a steer. A `prompt` record with mode
`mid_turn` or `correction` therefore also carries the tag
`lifecycle:intervention`. The definition's limits, stated so the number is
read honestly: it undercounts plan changes delivered as a fresh prompt after a
natural stop, and it may count a mid-turn clarification the agent would have
asked for anyway. Neither can be fixed without reading intent, which is a
non-goal. Sub-agent start and stop are the `agent-started` / `agent-finished`
pair; `stop` and `commit` are the completed-task candidates, with `commit` the
one the kalpa report divides by.

Selectors exposed to rules, all under built-in namespaces and carried in the
record's `tags` so `matches_selector` needs no change:

```
lifecycle:subagent_start
lifecycle:subagent_stop
lifecycle:agent:<agent_type>        (on sub-agent records when the host supplied a type)
lifecycle:prompt
lifecycle:prompt:fresh
lifecycle:prompt:mid_turn
lifecycle:prompt:correction
lifecycle:intervention              (on prompt records with mode mid_turn or correction)
lifecycle:interrupt
lifecycle:stop
lifecycle:commit
lifecycle:kalpa_start
lifecycle:kalpa_end
kalpa:<name>                        (on every lifecycle record while a kalpa is open)
```

## The journal record, v2

`JournalRecord` (`journey/journal.rs:59–91`) gains optional fields and bumps
`v` to 2. Field order remains the serialization order. Readers accept v1 and
v2; v1 records simply have `kind: None`.

```jsonc
{
  "v": 2, "ts": 1789095489, "sid": "s-2026-09-18-3a9f1c", "seq": 4412,
  "tool": "__lifecycle", "path": "",
  "tags": ["lifecycle:prompt", "lifecycle:prompt:correction", "kalpa:lifecycle-events"],
  "kind": "prompt",                 // new: subagent_start|subagent_stop|prompt|interrupt|stop|commit|kalpa_start|kalpa_end
  "mode": "correction",             // new: prompt records only
  "host": "claude",                 // new: claude|codex|gemini|cli
  "turn": "t-…",                    // new: host turn id when supplied
  "agent": "a-…",                   // new: agent id (sub-agent records; prompts inside a sub-agent)
  "agent_type": "Explore",          // new: host agent type when supplied
  "kalpa": "lifecycle-events",      // new: open kalpa name when any
  "subject": "unit-1789095489589855000"  // existing: open work unit, when any
}
```

Rules:

- `tool` is the fixed sentinel `__lifecycle` and `path` is `""` so every
  existing reader that indexes by tool or path keeps working and no lifecycle
  record can collide with a real tool name.
- No prompt text, ever. `SPEC-journey-facts.md` §"The journal record" fixes the
  journal to tags plus an optional subject, never full content. That constraint
  stands; the text lives in the action log (§Action log).
- **Tool projection.** `derive::assert_facts` (`derive.rs:497–539`) splits the
  records it read into two views: `tool_records` (`kind.is_none()`) and
  `all_records`. `WindowContext` carries both. The aggregators use them as
  follows, and each line is byte-identical for existing rules because no
  existing tag selector ever matches a lifecycle record:

  | aggregator | window `Nc` (positional) | windows `Ns` / `s` | notes |
  |---|---|---|---|
  | `occurrence`, `count`, `seen` | position among `tool_records` | filter over `all_records` | a `lifecycle:*` selector with an `Nc` window therefore yields no facts; lifecycle selectors use `s` or time windows. Documented in the selector list. |
  | `since_ge` | n/a | n/a | last match searched over `all_records`; distance = number of **tool** records after it. "Tool calls since the last interrupt" works; existing distances unchanged. |
  | `filtered_since_ge` | n/a | n/a | target searched and `counted` counted over `all_records`, so "corrections since the last commit" works with lifecycle records on both sides. |
  | `distinct` | position among `tool_records` | filter over `tool_records` | lifecycle records have `path: ""` and must never add a distinct path. |

  This is what makes Goal 5 true: for the same tool records, every existing
  fact is identical.
- **Read bound.** When any rule asks for a `Calls(n)` window, the read size
  computed at `derive.rs:490–503` is the maximum `n` over *tool* records. Since
  lifecycle records share the file, `read_recent` is asked for
  `min(SUFFIX_HARD_CAP, 2 * n + 64)` lines and the projection then trims to
  the last `n` tool records. If fewer than `n` tool records are present after
  the over-read, the window is whatever was read, which is also today's
  behavior when the journal is short.
- `validate_selectors` (`derive.rs:415–467`) exempts any selector with the
  `lifecycle:` or `kalpa:` prefix from the tagger-tag / module check. The
  exemption applies when `.phronesis/journey.json` is absent too
  (`TaggerConfig::default()`), so a project with no tagger config can still
  write a lifecycle rule. Everything else still fails closed as
  `UndefinedSelector`.
- **Compaction.** `latest_outcome_indices` (`journal.rs:263–283`) retains
  prefix records that carry a subject and a grounded outcome tag. It
  additionally retains records tagged `lifecycle:commit`, `lifecycle:kalpa_start`,
  and `lifecycle:kalpa_end`. Other lifecycle records compact like tool records.
  Sub-agent pairing does not depend on the journal (§Correlation state), so a
  compacted `subagent_start` is harmless.
- The determinism test in `tests/journey_derive.rs` gains a fixture whose last
  20 records are 10 tool records interleaved with 10 lifecycle records, and
  asserts every `journey_*` fact equals the fact set computed from the 10 tool
  records alone under `Calls`, `since_ge`, `filtered_since_ge`, and `distinct`.

## Action log

Each lifecycle event also appends one `LogEntry` with `kind: "lifecycle"` and
`event: <kind>`. Fields, flat as the action log convention requires:

```jsonc
{"ts":1789095489,"kind":"lifecycle","event":"prompt",
 "host":"claude","sid":"s-…","seq":4412,"session_id":"…","turn_id":"…",
 "mode":"correction","kalpa":"lifecycle-events",
 "prompt":"<full scrubbed text>","prompt_bytes":184}
{"ts":…,"kind":"lifecycle","event":"subagent_stop",
 "host":"codex","sid":"…","seq":4420,"session_id":"…","turn_id":"…",
 "agent_id":"…","agent_type":"reviewer",
 "duration_secs":212,"stop_hook_active":false,"matched_start":true}
{"ts":…,"kind":"lifecycle","event":"interrupt",
 "host":"claude","sid":"…","seq":4425,"session_id":"…","turn_id":"…",
 "inferred_from":"inflight"}   // codex: "hook"; claude: "inflight" | "transcript"; gemini: "inflight" | "open_turn"
{"ts":…,"kind":"lifecycle","event":"commit",
 "host":"claude","sid":"…","seq":4430,"tool_use_id":"…",
 "sha":"0f3c…","head_before":"12eb…","confidence_band":"high"}
```

- `prompt` is the full text, scrubbed as §Privacy and scrubbing describes. It
  appears only on `prompt` entries. `.phronesis/log.jsonl` is gitignored at
  both the root and `**/` levels (`.gitignore:3, 32`), so text never lands in
  a published tree.
- `.phronesis/journey.json` gains an optional `lifecycle` block:

  ```json
  { "lifecycle": { "prompt_text": "full" } }
  ```

  Values: `"full"` (default, the decision recorded for this spec) or `"none"`.
  Under `"none"` the `prompt` field is omitted everywhere it would appear
  (action log, payload capture, `--corrections`) and only `prompt_bytes`
  remains.
- `sid` and `seq` on the log entry are the same values written to the journal
  record, so a reader can join the two files.
- `session_id` and `transcript_path` values in these entries are the host's
  raw values, as they are in today's `codex_hook` entries. They are scrubbed by
  `phr-mcp scrub-payload` on the way to any corpus, as today.

`stats::aggregate` (`stats.rs:71–131`) is rule-centric and ignores
`kind`/`event`. It gains a `lifecycle` section: counts per event, counts per
prompt mode, sub-agent count with median duration computed in-process from the
log, and commit count. When a kalpa is open its name is printed in the stats
header.

`families::build` (`phronesis-metrics/src/families.rs:180–265`) gains a
`"lifecycle"` arm emitting `phronesis_lifecycle_events_total{host,event,mode}`
(a `Counter` family; `mode` is `""` for non-prompt events) and
`phronesis_subagent_duration_seconds` as a `Histogram` with
`exponential_buckets(1.0, 2.0, 12)` (1 s to about 68 min). No `kalpa` label in
v1: it is user-typed free text and the existing families cap rule-id series
for exactly this reason (`families.rs:170–172`).

## Correlation state

Five small files under `.phronesis/journey/`. Each has a sibling `<name>.lock`
file; every read-modify-write holds `fs2::FileExt::lock_exclusive` on the lock
file across read, mutate, `set_len(0)`, and write, modelled on
`journal.rs::acquire_lock` (`journal.rs:145–163`). The helper lives in
`lifecycle/state.rs` as `with_locked<T>(path, |contents| -> (new_contents, T))`.
All writes are best-effort: an IO error degrades to "unknown", never to a
failed hook. `hook/seq.rs::bump_seq_file` stays as it is.

| file | written by | read by | contents |
|---|---|---|---|
| `session` (exists) | SessionStart, SessionEnd | everything | the session id. New: SessionStart **overwrites** it with the host's `session_id` when present (today `current_sid` is create-on-miss and never overwrites); SessionEnd truncates it. Codex stops using `payload.session_id` directly and reads this file like the other hosts. Migration note: `s` windows do not span the upgrade; the first post-upgrade SessionStart begins a new sid. |
| `agents` | `subagent_start` push, `subagent_stop` pop | `subagent_stop`, `prompt` | JSON lines `{agent_id, agent_type, ts, seq}` for currently open sub-agents. Pop by `agent_id`; if the stop carries no id, pop LIFO; if nothing is open, the stop record is written with `matched_start: false` and no duration. This file, not the journal, is authoritative for pairing. Truncated at SessionStart. |
| `inflight` | `pre-check` push, `post-check` pop | `prompt` classification, `post-check` commit detection | JSON lines `{key, tool, ts, agent_id?, head_before?}`. `key` is `tool_use_id` when the host supplies one (Claude, Codex) and otherwise the hex of `std::hash::DefaultHasher` over `tool_name` and the canonical (sorted-key) `tool_input` JSON (Gemini; no new dependency, and `BeforeTool`/`AfterTool` carry identical `tool_input`). `head_before` is `git rev-parse HEAD` in the project root at pre time, only for shell tools. A blocked pre-check (exit 2) pops its own entry before exiting: a block is not an interrupt. Entries older than **900 s** are ignored by classification and dropped on the next write. Truncated at SessionStart. |
| `turn` | `prompt` sets open; `stop`, `interrupt`, SessionEnd set closed | `prompt` classification | `{open: bool, turn_id?, last_prompt_ts, last_event: "prompt"|"stop"|"interrupt"}` |
| `kalpa` | `phr-mcp kalpa start/end` | every lifecycle write, `journey`, `stats` | `{name, started_ts}`. Survives SessionStart. |

Sub-agent identity: Claude Code and Codex supply `agent_id` and `agent_type`
on their sub-agent events. When `agent_id` is absent (Gemini, or a Claude
internal fork with empty fields) the start synthesizes `agent_id =
format!("{sid}:{seq}")` and the stop pops LIFO. Nesting depth is not tracked.

**Sub-agent tool calls and `inflight`.** A Claude sub-agent's tool calls fire
the same `PreToolUse`/`PostToolUse` hooks against the same project root. Two
sub-agents dispatched in one message run concurrently. Because `inflight` is a
keyed set, their entries do not clobber each other or the parent's. When the
prompt handler evaluates `inflight`, it considers only entries whose `agent_id`
is absent or equals the prompt's own `agent_id`. A sub-agent's own in-flight
tool never makes the parent's next prompt a `correction`.

**Where the writes happen.** `pre.rs` and `post.rs` today exit early for tools
outside their allowlist (`pre.rs:30–43`, `post.rs:37–50`) and when no rules of
that phase exist (`pre.rs:48`). The `inflight` push/pop and the Gemini
`invoke_agent` sub-agent derivation run immediately after `read_payload`,
before the tool-name match and before rule loading, in both runners.
`invoke_agent` is added to both allowlists.

## Classification at prompt time

Runs inside the `prompt` handler on every host, before the record is written.

1. Read `turn`. If absent or `open: false`, mode is `fresh`. Write the record,
   set `turn` open. Done.
2. Turn is open. Check for an interrupt, in this order, stopping at the first
   hit:
   - **Codex:** the last lifecycle record for this `sid` is an `interrupt`
     (the `Interrupt` hook fired before the prompt). Mode is `correction`.
   - **Inflight:** `inflight` has a live entry (age under 900 s) visible to
     this prompt's agent scope. Write an `interrupt` record with
     `inferred_from: "inflight"`, drop those entries, then the prompt with
     mode `correction`.
   - **Transcript marker (Claude only):** `transcript_path` is present and
     readable. Read at most the last 64 KiB and look for a `user` entry whose
     text is exactly `[Request interrupted by user]` or begins with
     `[Request interrupted by user for tool use]` with a timestamp after
     `turn.last_prompt_ts`. Hit → `interrupt` with `inferred_from:
     "transcript"`, then `correction`.
   - **Open turn (Gemini only):** Gemini never delivers a prompt while a turn
     is running, so an open turn here means `AfterAgent` was skipped, which
     only happens on abort. Write `interrupt` with `inferred_from:
     "open_turn"`, then `correction`.
3. No interrupt evidence. Mode is `mid_turn`. Reachable on Claude and Codex
   only. On Codex a `turn_id` equal to the open turn's id is a second,
   sufficient signal for `mid_turn`.

Known limits, stated where they apply:

- Claude Code's `UserPromptSubmit` is reported (community write-up, not docs)
  to fire for some system-injected messages such as sub-agent completion
  notices. Those would classify as `mid_turn`. This is Open question 1 and is
  resolved by the manual evidence step before any mitigation is designed; no
  speculative field ships.
- Claude Code fires no hook on interrupt. If the interrupt landed between tool
  calls (no `inflight`) and the transcript is unreadable, the next prompt is
  classified `mid_turn`. `inferred_from` makes confidence visible.
- A stale `inflight` entry from a killed hook, a denied permission, or a
  cancelled tool is bounded by the 900 s TTL and the SessionStart truncation.
  Within that window one spurious `correction` is possible.

## Host adapters

### Shared module: `crates/phronesis-mcp/src/lifecycle/`

- `event.rs`: `LifecycleEvent { kind, mode, host, sid, seq, session_id,
  turn_id, agent_id, agent_type, kalpa, prompt, ts, extra }` plus
  `to_journal_record(&self) -> JournalRecord` and `to_log_entry(&self) ->
  LogEntry`. One place decides both on-disk shapes.
- `state.rs`: the five correlation files, `with_locked`, and
  `classify_prompt(root, host, agent_id, transcript_path) -> (Mode,
  Option<Interrupt>)`.
- `record.rs`: `record(root, event)` = bump seq, append journal, append log,
  update state. Failures are swallowed and reported on stderr with the
  `phronesis:` prefix, matching `metrics::record`.
- `outcome.rs`: `detect_commit(root, inflight_entry, command_exit) ->
  Option<Commit>` (§Outcomes and kalpas).
- `scrub.rs`: `scrub_prompt(root, text) -> String` (§Privacy and scrubbing).

All three adapters build a `LifecycleEvent` and call `record`. No adapter
writes the journal or log directly.

### Claude Code

- New subcommand `phr-mcp claude-hook <Event>` in `main.rs`, mirroring
  `codex-hook`. It reads stdin (capped by `security::read_stdin_capped`) and
  parses a `ClaudePayload` with `hook_event_name`, `session_id`, `prompt_id`,
  `transcript_path`, `agent_id`, `agent_type`, `agent_transcript_path`,
  `stop_hook_active`, `prompt`, `tool_name`, `tool_input`, `tool_response`, all
  `#[serde(default)]`. Tool phases delegate to the existing pre/post runners.
- **Failure policy.** On any non-tool event, a stdin read or parse failure
  prints `{}`, exits 0, and logs to stderr. A `UserPromptSubmit` hook that
  exits 2 would discard the human's prompt; that must never happen because of
  a Phronesis bug. Tool events keep today's exit-2-on-parse-failure behavior.
- **Response shapes.** `UserPromptSubmit` and `SessionStart` print the
  existing context JSON, or `{}` when the render is empty (today
  `handle_interaction_context` prints nothing on empty; `claude-hook` always
  prints valid JSON). `SubagentStart` prints `{}`. `Stop` and `SubagentStop`
  print `{"decision":"block","reason":"<gate text>"}` when the confidence gate
  blocks, otherwise `{}`. When the payload has `stop_hook_active: true`, both
  print `{}` without evaluating the gate, as the Claude docs require, so a
  blocking gate cannot loop.
- `UserPromptSubmit` → render interaction context, then record `prompt`.
- `SessionStart` → overwrite `session` with `session_id` when present,
  truncate `agents` and `inflight`, then render session context as today.
- `SessionEnd` → if `turn` is open, record `stop`; truncate `session`; set
  `turn` closed.
- `SubagentStart` / `SubagentStop` / `Stop` → record. `Stop` and
  `SubagentStop` run `make_completion_decision` as the Codex adapter does.
- `pre-check` pushes `inflight` (with `head_before` for shell tools);
  `post-check` pops it and runs `detect_commit`. `HookPayload` gains
  `#[serde(default)] session_id`, `tool_use_id`, `hook_event_name`,
  `agent_id`.
- **Payload capture.** `capture_raw_payload` (`hook/mod.rs:88–112`) tees stdin
  verbatim to `PHRONESIS_CAPTURE_DIR`, and `docs/payload-corpus-promotion.md`
  promotes that file into the committed corpus. `claude-hook` and `codex-hook`
  redact `prompt` and `last_assistant_message` to `"<redacted:N bytes>"` before
  the tee for every event. A test asserts no prompt text reaches
  `payloads.jsonl`.
- `init.rs::write_settings` registers `SubagentStart`, `SubagentStop`, `Stop`,
  and `SessionEnd` with an empty matcher pointing at `phr-mcp claude-hook
  <Event>`, and switches `UserPromptSubmit` and `SessionStart` to
  `claude-hook`. **Replacement is command-keyed**, like `upsert_codex_hook`
  (`init.rs:1564–1583`): only entries whose command starts with `phr-mcp ` are
  replaced. Today's `upsert_hook` is matcher-keyed (`init.rs:1544–1559`) and
  would delete a user's own empty-matcher `Stop` hook; a test pins that a
  foreign hook survives `phr-mcp init`.
- `session-context` and `interaction-context` remain as aliases that ignore
  stdin and behave exactly as today, so settings files written by older
  versions keep working.
- **Payload fixtures are a precondition.** `prompt_id`, `agent_id`, and
  `agent_type` are asserted by the Claude docs but appear in no fixture in
  this repo. Before the Claude adapter is implemented, one real payload per
  event (`SubagentStart`, `SubagentStop`, `Stop`, `UserPromptSubmit`,
  `SessionEnd`) is captured with `PHRONESIS_CAPTURE_DIR`, redacted, and
  committed under `tests/fixtures/payloads/claude/`. The `agent_id` fallback
  above covers a missing field, but the fixtures decide what "missing" means.

### Codex CLI

Facts from `openai/codex` (`codex-rs/hooks/schema/generated/*.schema.json`,
`codex-rs/core/src/hook_runtime.rs`, `codex-rs/core/src/tasks/mod.rs`):

- Twelve events exist: `PreToolUse`, `PermissionRequest`, `PostToolUse`,
  `PreCompact`, `PostCompact`, `SessionStart`, `SessionEnd`,
  `UserPromptSubmit`, `SubagentStart`, `SubagentStop`, `Stop`, `Interrupt`.
- `Stop` does not fire on abort. `Interrupt` does, before `TurnAborted`, with
  the transcript already flushed. Its payload is `cwd`, `hook_event_name`,
  `model`, `permission_mode`, `session_id`, `transcript_path`, `turn_id`. It
  ignores `matcher`.
- `SubagentStart` carries `agent_id`, `agent_type`, `turn_id`, `session_id`.
  `SubagentStop` adds `agent_transcript_path`, `last_assistant_message`,
  `stop_hook_active`. Only thread-spawned sub-agents fire them.
- `UserPromptSubmit` carries `prompt` verbatim and `turn_id`. A message queued
  mid-turn fires it with the *running* turn's id.
- Output schemas are `deny_unknown_fields`; an extra key fails the whole hook.
  Permitted stdout keys per event:

  | event | permitted keys |
  |---|---|
  | `SessionStart`, `SubagentStart`, `UserPromptSubmit` | `continue`, `stopReason`, `suppressOutput`, `systemMessage`, `hookSpecificOutput{hookEventName, additionalContext}`; `UserPromptSubmit` also `decision: block`, `reason` |
  | `PreToolUse`, `PostToolUse` | as today (`decision`, `reason`, `hookSpecificOutput{…}`) |
  | `Stop`, `SubagentStop` | `continue`, `decision: block`, `reason`, `stopReason`, `suppressOutput`, `systemMessage`. **No `hookSpecificOutput`.** |
  | `PreCompact`, `PostCompact` | `continue`, `stopReason`, `suppressOutput`, `systemMessage`. **No `hookSpecificOutput`** (today's renderer violates this; §Adjacent findings) |
  | `Interrupt` | `systemMessage` only |
  | `SessionEnd`, `PermissionRequest` | advisory / not used by Phronesis |

Changes:

- `CodexPayload` gains `agent_id`, `agent_type`, `transcript_path`,
  `agent_transcript_path`, `last_assistant_message`, `stop_hook_active`,
  `prompt`, all optional.
- `dispatch` adds `"Interrupt"` → record `interrupt` with `inferred_from:
  "hook"`, set `turn` closed, respond `{}`. Adds `"SessionEnd"` → record
  `stop` if `turn` is open, truncate `session`, respond `{}`.
- `SubagentStart`, `SubagentStop`, `Stop`, `UserPromptSubmit` record their
  events in addition to what they do today. `Stop` and `SubagentStop` continue
  through `make_completion_decision`, whose response already fits the table.
- `renderer.rs` emits `{}` for `Interrupt` and `SessionEnd`. The
  `SubagentStart` context render stays: its schema permits
  `hookSpecificOutput.additionalContext`.
- `init.rs::write_codex_hooks` adds `Interrupt` and `SessionEnd` to the
  registration loop and changes the `SessionStart` matcher from
  `"startup|resume|clear"` to `""`. The current matcher is exact alternation
  in Codex's matcher grammar, so `compact` and `fork` sessions get no context
  today.
- The Codex journal write (`codex_hook.rs:1052–1074`) reads `sid` from the
  shared `session` file like every other host.

### Gemini CLI

Facts from `google-gemini/gemini-cli` at `main`
(`packages/core/src/hooks/types.ts`, `hookEventHandler.ts`, `hookRunner.ts`,
`packages/core/src/core/client.ts`, `packages/cli/src/ui/hooks/useMessageQueue.ts`):

- Eleven events: `BeforeTool`, `AfterTool`, `BeforeAgent`, `AfterAgent`,
  `Notification`, `SessionStart`, `SessionEnd`, `PreCompress`, `BeforeModel`,
  `AfterModel`, `BeforeToolSelection`. No sub-agent event exists.
- Sub-agents exist and are invoked through a single tool named
  `invoke_agent` with `tool_input: {agent_name, prompt}`. `BeforeTool` and
  `AfterTool` fire for it like any tool. Tools run *inside* a sub-agent also
  fire `BeforeTool`/`AfterTool` with no field marking them as nested.
- `BeforeAgent` carries `prompt`. `AfterAgent` carries `prompt`,
  `prompt_response`, `stop_hook_active`. Neither fires for sub-agents.
- **`AfterAgent` does not fire on interrupt.** The abort path yields
  `UserCancelled` and returns before the hook call. No event or field
  signals cancellation.
- Text typed while the agent is streaming is queued and submitted only when
  the agent is idle, joined with `\n\n`, as an ordinary new turn. There is no
  true mid-turn message on Gemini.
- Exit 0 with non-JSON stdout becomes a user-visible `systemMessage`; always
  print `{}` or valid JSON. Exit 2 and any other non-zero blocks.
- Gemini's `AfterTool` payload field is `tool_response`, the same as Claude.
  The `hook/mod.rs:57–64` comment saying Gemini sends `tool_output` is wrong
  and is corrected when `HookPayload` is touched.

Changes:

- `write_gemini_settings` registers `AfterAgent` and `SessionEnd` (empty
  matcher) → `phr-mcp claude-hook <Event>`, and repoints `SessionStart` and
  `BeforeAgent` at `claude-hook`. The adapter maps Gemini names inside:
  `BeforeAgent` → `prompt`, `AfterAgent` → `stop`. Every response is `{}` or
  the existing context JSON; never empty stdout.
- `subagent_start` / `subagent_stop` on Gemini are derived in `pre-check` /
  `post-check` when `tool_name == "invoke_agent"`, before the allowlist match:
  `agent_type` is `tool_input.agent_name`, `agent_id` is the synthesized
  `{sid}:{seq}`, stored in `agents` so the matching `AfterTool` pops LIFO. The
  `BeforeTool` matcher is widened and anchored:
  `^(replace|write_file|run_shell_command|invoke_agent)$` (the current matcher
  is an unanchored regex and over-matches).
- Interrupt classification on Gemini gets the `open_turn` branch described in
  §Classification. `mid_turn` is never emitted for Gemini.
- Nested tool records between an `invoke_agent` start and stop are not tagged
  as belonging to the sub-agent in v1.

## Outcomes and kalpas

A **kalpa** is a named theme that groups sessions: "rule-sync", "lifecycle
events", "java corpus". It sits above the existing hierarchy. Work units
(`outcomes::subject`, ids `unit-<nanos>`) live inside sessions; sessions live
inside a kalpa. A kalpa is how the question "did this line of work produce
anything" gets a denominator.

### Naming the kalpa

- `phr-mcp kalpa start <name>` writes `{name, started_ts}` to
  `.phronesis/journey/kalpa` and records a `kalpa_start` lifecycle event with
  `host: "cli"`. `phr-mcp kalpa end` removes the file and records
  `kalpa_end`. `phr-mcp kalpa` prints the current name and age.
- `<name>` is `[a-z0-9][a-z0-9-]{0,63}`; anything else is rejected with a
  message. This bounds label length and stops typos from creating unbounded
  distinct names by accident.
- A kalpa outlives sessions. It is not cleared by SessionStart, SessionEnd, or
  compaction. Only `kalpa end` or `kalpa start <other>` (which ends the current
  one first) changes it.
- **Forgotten kalpas are made visible, not expired.** `phr-mcp journey`,
  `phr-mcp stats`, and the session-context render print the active kalpa name
  and age in their header. Past 30 days the header adds `(stale? run phr-mcp
  kalpa end)`. The file is never auto-cleared, because a wrong auto-clear is
  as unfixable retroactively as a wrong stamp.
- Every lifecycle record written while a kalpa is open carries
  `kalpa: "<name>"` in the journal and the action log, and the tag
  `kalpa:<name>`. Tool records do not carry it in v1.

### Success signal: `commit`

Detected in `post-check` from ground truth, not command text:

1. `pre-check` for a shell tool (`Bash`, `run_shell_command`) runs
   `git rev-parse HEAD` in the project root with a 2 s timeout and stores the
   result as `head_before` on the `inflight` entry. Failure (not a repo, git
   unavailable) stores nothing and disables detection for that call.
2. `post-check` pops the entry. If `head_before` is present and
   `command_exit == 0`, it runs `git rev-parse HEAD` again. If `HEAD` differs,
   it records `commit` with `sha`, `head_before`, and `confidence_band` (from
   `outcomes::report` when confidence scoring is enabled and a work unit is
   open).
3. A cheap text pre-filter (`git commit` or `git cherry-pick` or `git revert`
   or `git merge` or `git rebase` present in the command) decides whether to
   run step 2's `rev-parse` at all, so most shell calls pay nothing at post.

This is immune to heredocs, `git -C other-repo`, `&& … || true` chains, and
aliases, because it observes the repository rather than the string. It misses
a commit made by a tool other than the shell (none exist among the hosted
tools today) and a commit followed by a reset within the same command (which
is not a landed commit). Amends and rebases move `HEAD` and are recorded;
`sha` distinguishes them from a new commit for any consumer that cares.

Fixture tests in `tests/lifecycle_outcome.rs` run in a temp git repo: a real
commit is detected; a `--dry-run` is not; a commit in a sibling repo via
`git -C` is not; a heredoc containing the words is not; a `git commit &&
false` chain is not (exit ≠ 0); `confidence_band` is present only when
`.phronesis/confidence.json` exists.

### Reporting

`phr-mcp kalpa show <name>` (and `phr-mcp stats --kalpa <name>`) reads the
lifecycle entries in `.phronesis/log.jsonl` and its one rotated predecessor
tagged with the kalpa and prints raw counts with the retention boundary in the
header, because the log rotates at 50 MiB keeping one predecessor
(`action_log.rs:86, 140, 248–275`) and a long kalpa's early events fall off:

```
kalpa: lifecycle-events      started 2026-09-18 (3d)      counts since log entry 2026-09-17 14:02
sessions        4
prompts        61   fresh 44   mid_turn 9   correction 8
interventions  17   (mid_turn + correction)
interrupts      8
sub-agents     12   median 3m40s
commits         7   confidence at commit: high 5  medium 2  low 0
interventions / commit   2.43
```

`interventions / commit` is omitted when commits are zero. No other ratio
ships in v1 (§Non-goals).

Rule selectors added: `lifecycle:commit`, `lifecycle:kalpa_start`,
`lifecycle:kalpa_end`, and `kalpa:<name>`. A kalpa window (`k`) for
`journey_*` is deliberately not added in v1.

## CLI and MCP surface

- `phr-mcp journey` renders lifecycle records inline with a `⟂` marker and the
  kind/mode instead of a path, and prints the active kalpa in its header.
  `phr-mcp journey --lifecycle` shows only lifecycle records.
- `phr-mcp journey --corrections` reads `correction` entries from the action
  log alone (which carry the text) and prints `ts`, `sid`, and the scrubbed
  prompt, oldest first. This is the input to a human or to `extract_rules`
  when turning friction into a proposal.
- `get_journey` (MCP) includes lifecycle records in its existing output with
  the same fields as the CLI. No new MCP tool.

## Rule examples

```json
{ "id": "warn-subagent-ended-without-tests",
  "conditions": [
    { "journey_seen": ["lifecycle:subagent_stop", "s"] },
    { "__script__": "facts_count('journey_filtered_since_ge', ['lifecycle:subagent_stop','tests',1]) == 0" }
  ],
  "action": { "type": "warning", "message": "A sub-agent finished this session and no test ran since." } }

{ "id": "warn-many-interventions-since-last-commit",
  "conditions": [
    { "__script__": "facts_count('journey_filtered_since_ge', ['lifecycle:commit','lifecycle:intervention',3]) >= 1" }
  ],
  "action": { "type": "warning",
              "message": "Three interventions since the last commit. Stop and re-plan before continuing." } }

{ "id": "suggest-rule-after-two-corrections",
  "conditions": [
    { "__script__": "facts_count('journey_count', ['lifecycle:prompt:correction','s']) >= 2" }
  ],
  "action": { "type": "suggestion",
              "message": "Two corrections this session. `phr-mcp journey --corrections` lists them; consider a rule." } }
```

## Privacy and scrubbing

- Prompt text goes only to `.phronesis/log.jsonl`, which is gitignored, and
  only through `lifecycle::scrub::scrub_prompt`. The journal never receives
  it; no fact, context render, or stats line carries it.
- `scrub_prompt(root, text)` wraps the text as `json!({"prompt": text})` and
  runs `payload_scrub::Scrubber::scrub_value` on it, then unwraps. This is
  necessary because the session-id and transcript-path replacements in
  `scrub_value` (`payload_scrub.rs:104–108`) are keyed on JSON object keys and
  `scrub_str` (`payload_scrub.rs:119`, currently private) only handles the
  project-root prefix, `$HOME` paths, and the bare username. Free text can
  contain a session id or transcript path as a substring, so `scrub_prompt`
  additionally applies two regexes before `scrub_value`: a UUID-shaped token
  following `session` (case-insensitive) within 20 characters, and any path
  ending in `.jsonl` under a directory named `.claude`, `.codex`, or
  `.gemini`. Both are replaced with the same placeholders `scrub_value` uses.
- `Scrubber::new` is fed `security::project_root()` and `$HOME`. When `$HOME`
  is unset or empty (`Scrubber::new` errors on empty, `payload_scrub.rs:690`)
  the hook falls back to project-root-only scrubbing and logs one stderr
  warning; it never writes unscrubbed text and never fails the hook.
- `scrub_str` becomes `pub(crate)` so `scrub_prompt` can reuse it directly
  for the unwrapped result.
- Payload capture redaction: §Host adapters / Claude.
- `tests/scrub_payload_integration.rs` gains: a prompt containing a session id
  as bare text, a transcript path as bare text, and an absolute home-directory
  path is scrubbed; a prompt under `prompt_text: "none"` is absent from the
  action log, the capture file, and `--corrections`.
- `tests/journey_journal.rs` gains a negative test: a `prompt` event with text
  produces a journal record with no field containing the text.

## Determinism and versioning

- Facts remain a pure function of (journal bytes, `now_ts`, `current_sid`).
  Classification happens at write time and is persisted in the record; derive
  never re-classifies. The tool projection (§The journal record) is a pure
  filter.
- `v` bumps to 2. Per the paragraph "don't reserve fields, version the
  schema" in `SPEC-journey-facts.md` (lines 181–187), fields are added with
  the bump rather than reserved. v1 readers ignore unknown fields
  (`serde(default)` throughout), so a downgrade reads lifecycle records as odd
  `__lifecycle` tool records with tags it does not match on. Acceptable.
- **Amendment to `SPEC-journey-facts.md`.** Its §"The journal record" states
  "One line per executed tool call. Written at post-check only … only actions
  that actually happened are journaled." This spec adds one paragraph there:
  "From v2, one line per executed tool call **or lifecycle event**. Lifecycle
  records are written by the event's own hook, carry `tool: "__lifecycle"`,
  and are excluded from every record-position and record-count computation
  by the tool projection described in `SPEC-agent-lifecycle-events.md`."
- `SUFFIX_HARD_CAP` is unchanged. Lifecycle records are roughly one per human
  turn plus two per sub-agent.

## Testing

Unit tests live beside the code; integration tests follow AGENTS.md
§"Testing Approach".

| test file | adds |
|---|---|
| `tests/journey_journal.rs` | v2 round trip; v1 record read under v2; compaction with mixed records retains `commit`/`kalpa_*`; no-text negative test |
| `tests/journey_derive.rs` | `lifecycle:*` and `kalpa:*` selector match; validation exemption with a real config and with `TaggerConfig::default()`; tool-projection determinism fixture (10 tool + 10 lifecycle interleaved) for `Calls`, `since_ge`, `filtered_since_ge`, `distinct`; over-read bound |
| `tests/lifecycle_state.rs` (new) | `with_locked` under 16 concurrent writers; `agents` push/pop by id, LIFO fallback, unmatched stop; `inflight` push/pop by key, blocked-pre pop, TTL expiry, SessionStart truncation, agent-scoped visibility; `turn` transitions; `kalpa` file round trip |
| `tests/lifecycle_classify.rs` (new) | every branch of §Classification driven by fixture state files and a fake transcript tail; Codex branch driven by a pre-written `interrupt` journal record |
| `tests/lifecycle_concurrency.rs` (new) | **gate for the Claude adapter step**: spawn N concurrent `pre-check`/`post-check` process pairs (with distinct `tool_use_id`s and mixed `agent_id`s) plus one `claude-hook UserPromptSubmit` for the parent; assert no `interrupt` and a `fresh`/`mid_turn` prompt, never `correction` |
| `tests/hook_integration.rs` | `claude-hook` for each event with the committed fixtures; failure policy (`{}` exit 0 on bad stdin for non-tool events); `Stop` block/allow shapes and `stop_hook_active` short-circuit; pre pushes inflight, post pops it; `invoke_agent` derivation before the allowlist |
| `tests/codex_hook_integration.rs` | `Interrupt` records and responds `{}`; `SubagentStart`/`Stop` record with agent fields; `UserPromptSubmit` records text; every response validated against a per-event allowed-key set mirroring the table |
| `tests/init_integration.rs` | all three writers register the new events idempotently; Codex `SessionStart` matcher is `""`; Claude `UserPromptSubmit` migrates from `interaction-context` to `claude-hook` in place; a foreign `""`-matcher `Stop` hook survives init; Gemini matcher is anchored |
| `tests/scrub_payload_integration.rs` | prompt scrubbing cases above; `prompt_text: none`; no prompt text in `payloads.jsonl` under `PHRONESIS_CAPTURE_DIR` |
| `tests/journey_cli_integration.rs` | `--lifecycle` and `--corrections` rendering; kalpa header |
| `tests/lifecycle_outcome.rs` (new) | the commit fixture list in a temp git repo |
| `tests/kalpa_integration.rs` (new) | `kalpa start`/`end`/`show`; name validation; stamping on lifecycle records; survival across a simulated SessionStart; `stats --kalpa` counts and retention boundary against a fixture log |
| `phronesis-metrics/tests/derivation.rs` | lifecycle counter family and duration histogram buckets |
| `tests/payload_contract.rs` | the committed Claude fixtures and a Codex `Interrupt` fixture pinned to the documented field sets |

Manual evidence before the branch is called done: one Claude Code session and
one Codex session in this repo, each with a sub-agent spawn, a mid-turn
message, an Esc interrupt followed by a prompt, and a commit, then `phr-mcp
journey --lifecycle`, `--corrections`, and `kalpa show` showing the expected
sequence. Gemini as available. Open question 1 is answered from the Claude
session's log.

## Rollout

Dependency graph, not a chain:

```
1a shared type + journal v2 + derive projection + selectors + compaction
1b HookPayload widening + lifecycle/state.rs + inflight/agents/turn/session/kalpa files + scrub_prompt + capture redaction
        │
        ├── 2 Claude adapter (claude-hook, pre/post inflight + commit detection, init writer)   ─┐
        ├── 3 Codex adapter (payload fields, Interrupt/SessionEnd, renderer, init writer)        ├── 5 stats, metrics, journey/kalpa CLI rendering
        └── 4 Gemini registrations + invoke_agent derivation                                     ─┘
```

- 1a and 1b are independent of each other and land first. Nothing emits yet;
  existing rules are unaffected by construction.
- 2, 3, and 4 are parallel once 1a and 1b have landed. They touch disjoint
  files except `hook/pre.rs` and `hook/post.rs`, which only step 2 edits
  (Gemini's `invoke_agent` derivation is part of the same edit and step 4 is
  registrations only).
- 5 depends on the action-log field names fixed in 1a's `to_log_entry`, so it
  can start after 1a and finish after 2–4 for its integration tests.
- The `kalpa` subcommand is part of 1b (it only writes a file and records an
  event through the shared module).
- CHANGELOG `## [Unreleased] → ### Added`, hand-written. `phr-mcp catalogue`
  is unaffected (no pack rules change).

Each step is a PR against `main` with a conventional-commit title.

## Adjacent findings (out of scope, tracked here so they are not lost)

1. **Codex `PreCompact`/`PostCompact` responses are rejected.** `renderer.rs`
   emits `hookSpecificOutput` for both; the Codex output schemas for those
   events forbid it under `deny_unknown_fields`, so every non-empty response
   Phronesis sends is discarded as a failed hook.
   `SPEC-codex-hooks-integration.md` lines 68–69 describe an injection channel
   that does not exist. Fix in a separate PR: emit `{}` and move any directive
   to `systemMessage` if wanted.
2. **Codex `PermissionRequest`** is an enforcement point the Codex spec does not
   mention. Not needed for lifecycle; noted for the boundary section.
3. **Codex `hook_event_name` fallback vocabulary.** The kebab-case arms in
   `dispatch` only fire on malformed stdin. Harmless; the Codex spec's CLI
   section overstates their role.
4. **`hook/mod.rs:57–64` comment is wrong** about Gemini and `tool_output`.
   Corrected in step 1b since `HookPayload` is edited there.
5. **Gemini `BeforeTool` matcher over-matches.** Anchored in step 4.
6. **Gemini HTML-escapes `additionalContext`.** `<` and `>` in rule text reach
   the model as entities. Not a bug in Phronesis; worth a note in the Gemini
   install output.
7. **`upsert_hook` is matcher-keyed** and can delete a user's own hook with the
   same matcher. Step 2 introduces the command-keyed variant for the new
   events; migrating the existing four registrations to it is a follow-up.

## Open questions

1. **Claude `UserPromptSubmit` for system-injected messages.** Reported by a
   community write-up, not by the docs. Answered by the manual-evidence
   session: if system-injected prompts appear as `mid_turn`, a follow-up
   designs the filter with real payloads in hand.
2. **Claude `SubagentStop` with empty `agent_type`.** Tracked upstream
   (anthropics/claude-code#87065): internal forks fire it with `agent_type:
   ""`. Records are written as-is with the empty type; a rule scoping to
   `lifecycle:agent:<type>` naturally excludes them.

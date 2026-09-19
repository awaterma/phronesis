# SPEC: agent lifecycle events — sub-agent start/stop, interrupts, mid-turn context, corrections, kalpas

**Status:** draft, revised after two rounds of adversarial review
              (2026-09-18, 2026-09-19)
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
              journey_cli.rs, stats.rs, payload_scrub.rs, action_log.rs}`, new
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
   the same tool records. One carve-out, stated in §Correlation state: the
   session id now rotates per session, so `s` windows scope to a session rather
   than to the `session` file's lifetime.

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
- Derived ratios other than two (`interventions / commit` here and
  `interventions / work item` in §Work items): `interventions / commit` is printed because
  it is the autonomy signal the feature exists for, and it is printed beside
  the retention boundary so its window is visible. No other ratio ships; raw
  counts do.
- Classifying `HEAD` movement by shape (new commit vs amend vs rebase vs
  reset). `sha` and `head_before` are recorded so a later version can; v1
  counts movements and says so in the report.
- A kalpa history file. `kalpa end` deletes the kalpa file, so `kalpa show` for
  an ended kalpa reads its boundaries from the log and loses them to rotation.
- Per-session correlation state. The state files are per-project; two live
  sessions in one project interfere (§Correlation state).

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
| `unit_start` / `unit_end` | a human names a work item, optionally with its spec | `phr-mcp unit start` / `unit end` (§Work items and governed throughput) |

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
`lifecycle:intervention`, **but only when the prompt is top-level** — a prompt
record carrying an `agent_id` (a prompt delivered inside a sub-agent) is never
tagged `lifecycle:intervention` and never counted as a correction, because the
human did not speak. The definition's limits, stated so the number is
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
lifecycle:unit_start
lifecycle:unit_end
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
- **No tagger, no modules.** A lifecycle record carries exactly the selectors
  enumerated in §Event model plus `kalpa:<name>`. The tagger and module
  resolution are not invoked for lifecycle records, so no user-defined tag can
  ever land on one and no existing tag selector can match one. A determinism
  test with a catch-all tagger config pins this.
- `subject` is stamped for joining only. Lifecycle records carry no outcome
  tag, so `latest_outcome_indices` (`journal.rs:263–283`) never treats one as a
  retention anchor, and `outcomes::report` ignores records with `kind:
  Some(_)`. Lifecycle records never contribute to outcome grounding or to a
  confidence band.
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
  | `occurrence`, `count`, `seen` | position among `tool_records` | filter over `all_records` | a `lifecycle:*` selector with an `Nc` window therefore yields no facts; lifecycle selectors use `s` or time windows. Documented in the selector list, and `validate_selectors` prints one stderr warning naming the rule when it sees the pairing, so a rule that can never fire says so instead of sitting silent. |
  | `since_ge` | n/a | n/a | last match searched over `all_records`; distance = number of **tool** records after it. "Tool calls since the last interrupt" works; existing distances unchanged. |
  | `filtered_since_ge` | n/a | n/a | target searched and `counted` counted over `all_records`, so "corrections since the last commit" works with lifecycle records on both sides. |
  | `distinct` | position among `tool_records` | filter over `tool_records` | lifecycle records have `path: ""` and must never add a distinct path. |

  This is what makes Goal 5 true: for the same tool records, every existing
  fact is identical.
- **Read bound.** Today (`derive.rs:490–503`) the read size is
  `SUFFIX_HARD_CAP` whenever any rule uses a session window, a time window,
  `since_ge`, or `filtered_since_ge`, and `max(max_calls, 1)` otherwise. Only
  the second branch needs changing: a `Calls(n)` window means *n tool records*,
  and lifecycle records now share the file. The read becomes **iterative**:
  read `n` lines, count tool records; while fewer than `n` tool records have
  been read and the file start has not been reached, double the read and read
  again, stopping at `SUFFIX_HARD_CAP`. The projection then trims to the last
  `n` tool records. This preserves the window exactly rather than approximating
  it; the only case where fewer than `n` tool records are returned is a journal
  that genuinely holds fewer, or a tail so lifecycle-dense that
  `SUFFIX_HARD_CAP` binds first — that second case is an accepted deviation and
  is stated here so it is a decision. Reading more lines never changes a fact:
  the `Nc` branch is positional and trims, and every other branch already reads
  the hard cap.
- `validate_selectors` (`derive.rs:415–467`) exempts the **closed set** of
  lifecycle selectors listed in §Event model, plus the two open-ended patterns
  `lifecycle:agent:<agent_type>` and `kalpa:<name>`, from the tagger-tag /
  module check. The set is closed on purpose: a typo like
  `lifecycle:prompt:corection` must still fail as `UndefinedSelector` rather
  than validate and silently match nothing. A tagger tag that begins with
  `lifecycle:` or `kalpa:` is rejected at config load — the namespaces are
  reserved. The exemption applies when `.phronesis/journey.json` is absent too
  (`TaggerConfig::default()`), so a project with no tagger config can still
  write a lifecycle rule. Everything else still fails closed as
  `UndefinedSelector`.
- **`seq` is a join key, not a fact.** Lifecycle records share
  `hook/seq.rs::bump_seq_file`, so a tool record's `seq` advances faster than
  it did in v1. No `journey_*` aggregator reads `seq`: call windows use record
  position among `tool_records`, not the counter. `seq` exists to join the
  journal to the action log and as a debug aid.
- **Compaction.** `latest_outcome_indices` (`journal.rs:263–283`) retains
  prefix records that carry a subject and a grounded outcome tag. It
  additionally retains records tagged `lifecycle:commit`, `lifecycle:interrupt`,
  `lifecycle:prompt:correction`, `lifecycle:kalpa_start`, and
  `lifecycle:kalpa_end` — the friction record is the point of the feature, and
  a rule like "two corrections this session" must not stop firing because the
  journal compacted. Other lifecycle records compact like tool records. The
  retained set is bounded by human turns and commits, not by tool calls, so the
  growth it adds is an order of magnitude below the tail it lives beside; no
  further cap ships in v1.
  Sub-agent pairing does not depend on the journal (§Correlation state), so a
  compacted `subagent_start` is harmless.
- The determinism test in `tests/journey_derive.rs` gains a fixture whose last
  20 records are 10 tool records interleaved with 10 lifecycle records, and
  asserts every `journey_*` fact equals the fact set computed from the 10 tool
  records alone under `Calls`, `since_ge`, `filtered_since_ge`, and `distinct`.
  Three further fixtures exercise what a 20-line file cannot: 5 tool records
  followed by 200 lifecycle records under a `Calls(5)` rule (the iterative
  re-read must still find all five), a `Seconds` window whose range contains
  lifecycle records, and a `since_ge` whose last match sits behind more
  lifecycle records than a single read would cover.

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
  appears only on `prompt` entries. `prompt_bytes` is the byte length of the
  **scrubbed** text; it, and the `<redacted:N bytes>` capture placeholder,
  deliberately reveal the original length. That is a decision, not an
  oversight.
- **The gitignore guarantee covers the rotated file too.** The action log
  rotates to `<path>.1`, i.e. `.phronesis/log.jsonl.1`
  (`action_log.rs::rotated_path`). This repo ignores both names at the root and
  `**/` levels (`.gitignore` lines 3–4 and 31–32). Because this spec is what
  first puts human prompt text in that file, `phr-mcp init` gains one step: if
  the project's `.gitignore` does not already ignore `.phronesis/log.jsonl` and
  `.phronesis/log.jsonl.1`, init appends both and says so in its output. A test
  runs `git check-ignore` on both names in a temp repo after `phr-mcp init`.
- `.phronesis/journey.json` gains an optional `lifecycle` block:

  ```json
  { "lifecycle": { "prompt_text": "full" } }
  ```

  Values: `"full"` (default, the decision recorded for this spec) or `"none"`.
  A missing file or a missing `lifecycle` block means `"full"`. An unreadable
  `journey.json`, a malformed `lifecycle` block, or any other value is treated
  as `"none"` with one stderr warning: the switch fails **closed**, never open.
  Under `"none"` the `prompt` field is omitted from the action log and from
  `--corrections`, and only `prompt_bytes` remains. Capture redaction is
  unconditional and independent of the switch, so there is nothing for the
  switch to change there.
- **The switch is enforced at read time as well.** Flipping it to `"none"`
  must also hide prompts already written under `"full"`. Every consumer of
  correction text — `phr-mcp journey --corrections`, the `extract_rules`
  hand-off, any future MCP surface — goes through one accessor,
  `lifecycle::correction_text(root, &entry)`, which consults the current
  `prompt_text` value and yields `None` under `"none"`. No consumer greps the
  log directly.
- `sid` and `seq` on the log entry are the same values written to the journal
  record, so a reader can join the two files.
- `session_id` is the host's raw value, as it is in today's `codex_hook`
  entries; it is scrubbed by `phr-mcp scrub-payload` on the way to any corpus,
  as today. `transcript_path` and `agent_transcript_path` are **not
  persisted**: they are read for classification and dropped at the adapter
  boundary. No lifecycle log entry carries them.

`stats::aggregate` (`stats.rs:71–131`) is rule-centric and ignores
`kind`/`event`. It gains a `lifecycle` section: counts per event, counts per
prompt mode, sub-agent count with median duration computed in-process from the
log and its one rotated predecessor (a pair whose start rotated away
contributes no duration), and commit count. When a kalpa is open its name is printed in the stats
header.

`families::build` (`phronesis-metrics/src/families.rs:180–265`) gains a
`"lifecycle"` arm emitting `phronesis_lifecycle_events_total{host,event,mode}`
(a `Counter` family; `mode` is `""` for non-prompt events) and
`phronesis_subagent_duration_seconds{host}` as a `Histogram` with
`exponential_buckets(1.0, 2.0, 13)` — 13 buckets, so the last finite bucket is
4096 s, about 68 min. The label set is fixed to `{host, event, mode}`. Neither
`kalpa` nor `agent_type` is ever a label: both are free text (user-typed and
model-supplied respectively) and the existing families cap rule-id series for
exactly this reason (`families.rs:170–172`).

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
| `session` (exists) | SessionStart, SessionEnd | everything | the session id. New: a **session-begin** SessionStart (source `startup`, `resume`, or `clear`) **overwrites** it with the host's `session_id` when present (today `current_sid` is create-on-miss and never overwrites); a `compact` or `fork` SessionStart leaves it alone. The write is an atomic replace (temp file + rename) so a lock-free `current_sid` reader never observes an empty file and never mints a phantom sid. SessionEnd does **not** truncate it — truncation would let any stray hook between sessions mint a throwaway sid; the next session-begin overwrites instead. Codex stops using `payload.session_id` directly and reads this file like the other hosts. |
| `agents` | `subagent_start` push, `subagent_stop` pop | `subagent_stop`, `prompt` | JSON lines `{agent_id, agent_type, ts, seq}` for currently open sub-agents. Pop by `agent_id`; if the stop carries no id, pop LIFO; if nothing is open, the stop record is written with `matched_start: false` and no duration. This file, not the journal, is authoritative for pairing. `agent_type` is sanitized before it is written anywhere (below). Truncated at a session-begin SessionStart only. |
| `inflight` | `pre-check` push, `post-check` pop | `prompt` classification, `post-check` commit detection | JSON lines `{key, tool, ts, agent_id?, head_before?}`, a **multiset**: push appends a line, pop removes the *last* line with a matching key. Two concurrent calls with the same key therefore push two lines and pop two lines instead of clobbering each other. `key` is `tool_use_id` when the host supplies one (Claude, Codex) and otherwise the hex of an FNV-1a hash over `tool_name` and the canonical (sorted-key) `tool_input` JSON (Gemini; `BeforeTool`/`AfterTool` carry identical `tool_input`). The hash is pinned rather than `std::hash::DefaultHasher`, whose algorithm is explicitly unspecified across Rust releases, so a pre/post pair split across a rebuild still matches. `head_before` is `git rev-parse HEAD` in the project root at pre time, only for shell tools whose command passes the commit pre-filter (§Success signal). A blocked pre-check (exit 2) pops its own entry before exiting: a block is not an interrupt. **The TTL applies to classification only:** entries older than **900 s** are ignored when classifying a prompt and are dropped when a *classification* pass rewrites the file, but `post-check` pops by key regardless of age, so a twenty-minute build still gets its commit detected. Truncated at a session-begin SessionStart only. |
| `turn` | top-level `prompt` sets open; an *unblocked* `stop`, an `interrupt`, and SessionEnd set closed | `prompt` classification | `{sid, open: bool, turn_id?, last_prompt_ts, last_event: "prompt"\|"stop"\|"interrupt"}`. `sid` is carried so a turn left open by a crashed session cannot leak into the next one: a reader whose `current_sid` differs treats the file as absent. A session-begin SessionStart resets it to closed for the same reason. A prompt carrying an `agent_id` never writes this file — a sub-agent's prompt must not move the parent's turn state or its `last_prompt_ts`. If the write that would close the turn fails, the classifier treats the turn as **closed** (the next prompt is `fresh`), so a best-effort failure degrades to the conservative answer rather than to a false intervention. |
| `kalpa` | `phr-mcp kalpa start/end` | every lifecycle write, `journey`, `stats` | `{name, started_ts}`. Survives SessionStart. |

Sub-agent identity: Claude Code and Codex supply `agent_id` and `agent_type`
on their sub-agent events. When `agent_id` is absent (Gemini, or a Claude
internal fork with empty fields) the start synthesizes `agent_id =
format!("{sid}:{seq}")` and the stop pops LIFO. Nesting depth is not tracked.
LIFO is an approximation: when two sub-agents without ids finish out of start
order their durations are swapped. `matched_start` and the ids keep it
auditable; no better pairing is available from the hook surface.

**`agent_type` is sanitized at the adapter boundary.** On Gemini it is
`tool_input.agent_name`, model-generated free text, and it reaches a journal
tag (`lifecycle:agent:<agent_type>`, hence a RETE fact), the action log, and
the `agents` file. It is lowercased, then kept only if it matches
`[a-z0-9][a-z0-9_.:-]{0,63}`. The set is wider than the kalpa pattern on
purpose: Gemini's built-ins are snake_case (`codebase_investigator`) and
Claude's plugin agents are colon-qualified (`code-simplifier:code-simplifier`),
and both must survive as tags. Colons are safe because `matches_selector`
compares tags by exact string. Anything else is stored as absent and the
`lifecycle:agent:*` tag is dropped.

**Migration: `s` windows become per-session.** Today the `session` file is
create-on-miss and never overwritten, so in practice a project's sid — and
therefore every `s` window — spans the file's lifetime. After this change each
session-begin SessionStart mints a new sid, so `s` windows scope to a session,
which is what the name always claimed. This is a permanent semantic change,
not a one-time boundary, and it is the one carve-out from Goal 5: existing
`s`-window rules see shorter windows. Rules using `Nc` or time windows are
unaffected. The release notes say so and tell users to review `s`-window rules.

**One project, two live sessions.** The correlation files are per-project. Two
host sessions open in the same project share them: the later SessionStart wins
the sid, and one session's `stop` can close the other's turn. Classification
then misattributes some prompts. Accepted for v1 rather than mitigated —
per-sid state files would multiply the file count by the number of sessions and
leave the reaping problem unsolved — and recorded here so the misattribution is
a known limit rather than a surprise.

**Sub-agent tool calls and `inflight`.** A Claude sub-agent's tool calls fire
the same `PreToolUse`/`PostToolUse` hooks against the same project root. Two
sub-agents dispatched in one message run concurrently. Because `inflight` is a
keyed multiset (§Correlation state), their entries do not clobber each other or
the parent's even when two calls share a key. When the
prompt handler evaluates `inflight`, it considers only entries whose `agent_id`
is absent or equals the prompt's own `agent_id`. A sub-agent's own in-flight
tool never makes the parent's next prompt a `correction`. This rests on
`PreToolUse` inside a sub-agent carrying `agent_id`, which step 0's fixture
settles. If it does not, sub-agent entries are parent-scoped and a mid-turn
prompt while a sub-agent's tool runs can read as a `correction`; the fallback
is not a code path but a documented miss, and the fixture decides which world
we are in before the adapter ships.

**Where the writes happen.** `pre.rs` and `post.rs` today exit early for tools
outside their allowlist (`pre.rs:30–43`, `post.rs:37–50`) and when no rules of
that phase exist (`pre.rs:48`). The `inflight` push/pop and the Gemini
`invoke_agent` sub-agent derivation run immediately after `read_payload`,
before the tool-name match and before rule loading, in both runners.
`invoke_agent` is added to both allowlists. A blocked `invoke_agent` pre-check
(exit 2) pops the `agents` entry it just pushed and writes no `subagent_start`
record, mirroring the `inflight` pop: a sub-agent that never ran must not
leave a dangling entry for the next real stop to pop LIFO. An `invoke_agent`
tool record uses the synthetic path `<invoke_agent>` — never the sub-agent
prompt, which would put content in the journal — and that value is the one
`journey_distinct` on `path` will see.

## Classification at prompt time

Runs inside the `prompt` handler on every host, before the record is written.

A prompt carrying an `agent_id` is a sub-agent's prompt, not the human's. It is
recorded with mode `fresh`, is never tagged `lifecycle:intervention`, and skips
everything below: it reads no state and writes none.

1. Read `turn`. Treat it as absent when it is missing, unparseable, or carries
   a `sid` other than the current one.
   - Absent → mode is `fresh`.
   - `last_event == "interrupt"` → mode is **`correction`**, on every host.
     This is the single interrupt-already-recorded path; the Codex `Interrupt`
     hook, and every inferred interrupt below, sets it. It is read from `turn`
     and not from the journal tail, so a concurrent `SubagentStop` record
     cannot hide it and compaction cannot erase it.
   - Otherwise `open: false` → mode is `fresh`.
   - Otherwise the turn is open → step 2.

   In the `fresh` and `correction` cases, write the record and set `turn` open
   with `last_event: "prompt"`. Done.
2. Turn is open and no interrupt has been recorded yet. Check for one, in this
   order, stopping at the first hit. Every branch that infers an interrupt
   writes the `interrupt` record, drops the session's `inflight` entries (so
   one Esc cannot yield two interrupts), sets `turn.last_event` to
   `"interrupt"`, and then writes the prompt with mode `correction`.
   - **Transcript marker (Claude only):** `transcript_path` is present and
     readable. Read at most the last 64 KiB and look for an entry with
     `type == "user"` whose text — `message.content` when it is a string, or
     the `text` of a single text block when it is an array — is exactly
     `[Request interrupted by user]` or begins with `[Request interrupted by
     user for tool use]`, with a `timestamp` after `turn.last_prompt_ts`.
     `inferred_from: "transcript"`. The transcript is checked **before**
     `inflight` because it is direct evidence: a queued mid-turn message also
     leaves a live `inflight` entry, and only the marker distinguishes the two.
   - **Inflight:** `inflight` has a live entry (age under 900 s) visible to
     this prompt's agent scope, and the transcript was absent or unreadable.
     `inferred_from: "inflight"`. Not used on Codex, where the `Interrupt` hook
     is authoritative and step 1 has already spoken.
   - **Open turn (Gemini only):** Gemini never delivers a prompt while a turn
     is running, so an open turn here means `AfterAgent` was skipped, which
     only happens on abort. `inferred_from: "open_turn"`.
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
- The transcript tail is bounded at 64 KiB. A single very large intervening
  entry — a big paste — can push the interrupt marker out of the read, and the
  prompt falls through to the `inflight` branch or to `mid_turn`.
- The `inflight` branch cannot distinguish an Esc from a message typed while a
  tool runs; only the transcript marker can. On Claude with an unreadable
  transcript, a queued mid-turn message is therefore reported as a
  `correction`. Both modes are interventions, so the headline number is
  unaffected; the split between `mid_turn` and `correction` is not.
- A prompt hook that runs concurrently with a still-running `post-check` for
  the previous tool can see that tool's `inflight` entry and infer an
  interrupt. Hosts appear to serialize hook phases, and `post-check` pops the
  entry as its first action after reading stdin, so the window is
  milliseconds, but it is not zero. Recorded as a limit rather than mitigated:
  any age threshold that closed it would also miss real interrupts of
  long-running commands.
- A sub-agent that crashes without firing its stop leaves an entry in
  `agents` until SessionStart truncates it. A later stop with no `agent_id`
  would pop that stale entry LIFO and report a wrong duration. Sub-agents can
  legitimately run for hours, so no TTL is applied; `matched_start` and the
  ids make the case auditable after the fact.

## Host adapters

### Shared module: `crates/phronesis-mcp/src/lifecycle/`

- `event.rs`: `LifecycleEvent { kind, mode, host, sid, seq, session_id,
  turn_id, agent_id, agent_type, kalpa, prompt, ts, extra }` plus
  `to_journal_record(&self) -> JournalRecord` and `to_log_entry(&self) ->
  LogEntry`. One place decides both on-disk shapes. `extra` is a closed
  vocabulary — `inferred_from`, `stop_hook_active`, `matched_start`,
  `duration_secs`, `sha`, `head_before`, `confidence_band`, `tool_use_id` — and
  never carries message or transcript content. `last_assistant_message`,
  `prompt_response`, and `agent_transcript_path` are read for decisions and
  dropped at the adapter boundary; the hook integration tests assert no log
  entry contains them.
- `state.rs`: the five correlation files, `with_locked`, and
  `classify_prompt(root, host, agent_id, transcript_path) -> (Mode,
  Option<Interrupt>)`.
- `record.rs`: `record(root, event)` = bump seq, append journal, append log,
  update state. Failures are swallowed and reported on stderr with the
  `phronesis:` prefix, matching `metrics::record`. **stderr diagnostics carry
  event names, paths, and error kinds only** — never payload fields, prompt
  text, or transcript content; error formatting uses `Display` on the error,
  not on the input.
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
  blocking gate cannot loop. `SessionEnd` prints `{}`.
- **A blocked stop is not a stop.** When the gate blocks, Claude continues the
  same turn. So `Stop` records the `stop` event and closes `turn` **only when
  its response does not block**; a blocking `Stop` leaves `turn` open and
  writes no record, and the `stop` is recorded when the turn really ends (the
  `stop_hook_active: true` re-fire, which prints `{}`, records exactly one
  `stop`). The same rule applies to `SubagentStop` and its `agents` pop.
  Without this, a steer during the continuation would classify `fresh` and the
  intervention would be lost.
- `UserPromptSubmit` → render interaction context, then record `prompt`.
- `SessionStart` → on a session-begin source (`startup`, `resume`, `clear`)
  overwrite `session` with `session_id` when present and truncate `agents`,
  `inflight`, and `turn`; on `compact` or `fork` leave all four alone, because
  the session is continuing and its open sub-agents and in-flight tools are
  real. Then render session context as today.
- `SessionEnd` → if `turn` is open, run the same interrupt detection step 2
  runs and record `interrupt` when there is evidence, otherwise record `stop`;
  set `turn` closed. `session` is left in place (§Correlation state). Quitting
  out of an aborted turn must not be recorded as a completed turn.
- `SubagentStart` / `SubagentStop` / `Stop` → record. `Stop` and
  `SubagentStop` run `make_completion_decision` as the Codex adapter does.
- `pre-check` pushes `inflight` (with `head_before` for shell tools);
  `post-check` pops it and runs `detect_commit`. `HookPayload` gains
  `#[serde(default)] session_id`, `tool_use_id`, `hook_event_name`,
  `agent_id`.
- **Payload capture.** `capture_raw_payload` (`hook/mod.rs:88–112`) writes
  stdin to `PHRONESIS_CAPTURE_DIR`, and `docs/payload-corpus-promotion.md`
  promotes that file into the committed corpus. The redaction moves **into
  `capture_raw_payload` itself**, so every caller inherits it — `pre-check` and
  `post-check` included, which is what covers Gemini's `invoke_agent`, whose
  `tool_input.prompt` is a full sub-agent task. It already parses stdin into a
  `Value`; it now walks that value and replaces the value of any key named
  `prompt`, `prompt_response`, or `last_assistant_message`, **at any depth**,
  with `"<redacted:N bytes>"`, then serializes the redacted value. Stdin that
  is not valid JSON is not written at all (one stderr line naming the event),
  since it cannot be redacted. The capture is therefore a redacted
  re-serialization rather than a verbatim tee, and
  `docs/payload-corpus-promotion.md` is amended to say so. Redaction applies to
  the copy written to the capture dir only: the hook parses the original stdin
  and the action log still receives the full scrubbed prompt. Tests assert both
  halves — no prompt text in `payloads.jsonl`, real text in the log.
- `init.rs::write_settings` registers `SubagentStart`, `SubagentStop`, `Stop`,
  and `SessionEnd` with an empty matcher pointing at `phr-mcp claude-hook
  <Event>`, and switches `UserPromptSubmit` and `SessionStart` to
  `claude-hook`. **Replacement is command-keyed**, like `upsert_codex_hook`
  (`init.rs:1564–1583`): an entry is replaced when its command contains the
  token `phr-mcp` and ends with one of our subcommands (`session-context`,
  `interaction-context`, `claude-hook <Event>`), so a user who invokes us
  through an absolute path or a wrapper gets an in-place upgrade rather than a
  duplicate registration and double context injection. Today's `upsert_hook` is matcher-keyed (`init.rs:1544–1559`) and
  would delete a user's own empty-matcher `Stop` hook; a test pins that a
  foreign hook survives `phr-mcp init`.
- `session-context` and `interaction-context` remain as aliases that ignore
  stdin and behave exactly as today, so settings files written by older
  versions keep working.
- **Payload fixtures are a precondition.** `prompt_id`, `agent_id`, and
  `agent_type` are asserted by the Claude docs but appear in no fixture in
  this repo. Before the Claude adapter is implemented, one real payload per
  event (`SubagentStart`, `SubagentStop`, `Stop`, `UserPromptSubmit`,
  `SessionStart`, `SessionEnd`, `PreToolUse`, `PostToolUse`, and one
  `PreToolUse` fired *inside* a sub-agent) is captured with
  `PHRONESIS_CAPTURE_DIR` and committed under
  `tests/fixtures/payloads/claude/`. The `agent_id` fallback above covers a
  missing field, but the fixtures decide what "missing" means — and the
  sub-agent `PreToolUse` is what decides whether `inflight` agent scoping
  (§Correlation state) works at all; if `agent_id` is absent there, sub-agent
  tool entries are parent-scoped and the spurious-`correction` case is real
  rather than theoretical. The tool fixtures also pin `tool_use_id`, which the
  `inflight` key depends on. "Redacted" means the full pipeline: capture →
  the recursive redaction above → `phr-mcp scrub-payload` → human review.
  `tests/payload_contract.rs` asserts no committed fixture contains a
  UUID-shaped session id, a `.claude`/`.codex`/`.gemini` path, an absolute home
  path, or the capturing machine's username.
- A real transcript tail is a fixture too: the last 64 KiB after a real Esc
  interrupt, committed under `tests/fixtures/transcripts/claude/`, so the
  marker branch is tested against the format Claude actually writes rather than
  against a shape this spec guessed.

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
  "hook"`, set `turn` to `{open: false, last_event: "interrupt"}`, drop the
  session's `inflight` entries (the aborted tools' `PostToolUse` never fires,
  and a lingering entry would fake an interrupt for 900 s), respond `{}`. The
  next prompt reads `last_event` and classifies `correction` (§Classification
  step 1). Adds `"SessionEnd"` → record `stop` if `turn` is open, respond `{}`;
  `session` is left in place.
- `SubagentStart`, `SubagentStop`, `Stop`, `UserPromptSubmit` record their
  events in addition to what they do today. `Stop` and `SubagentStop` continue
  through `make_completion_decision` and, as on Claude, skip the gate entirely
  when `stop_hook_active: true` and record nothing when the response blocks.
  **`SubagentStop` never closes `turn`; only the main-agent `Stop` does** —
  the two share a `dispatch` arm, which makes the distinction easy to lose.
- `make_completion_decision`'s render path for `Stop`/`SubagentStop` must emit
  no `hookSpecificOutput`: the same `deny_unknown_fields` rule that breaks
  `PreCompact` (§Adjacent findings 1) would discard every blocking stop. If the
  renderer emits it today, it is changed here rather than in that follow-up,
  and the per-event allowed-key test is the pin.
- `renderer.rs` emits `{}` for `Interrupt` and `SessionEnd`. The
  `SubagentStart` context render stays: its schema permits
  `hookSpecificOutput.additionalContext`.
- `init.rs::write_codex_hooks` adds `Interrupt` and `SessionEnd` to the
  registration loop, both with the empty matcher `""` as the other new events
  use, and changes the `SessionStart` matcher from `"startup|resume|clear"` to
  `""`. The current matcher is exact alternation in Codex's matcher grammar, so
  `compact` and `fork` sessions get no context today. Widening it is safe only
  because the SessionStart handler is now gated on the source
  (§Correlation state): `compact` and `fork` render context but touch no
  correlation state, so a mid-session compaction cannot orphan an open
  sub-agent or discard an in-flight tool.
- The Codex journal write (`codex_hook.rs:1052–1074`) reads `sid` from the
  shared `session` file like every other host.
- **Codex tool phases own their own `inflight` and commit detection.** Codex
  routes `PreToolUse`/`PostToolUse` to `codex-hook`, not to `pre-check` /
  `post-check`, so the push/pop and `detect_commit` wiring the Claude adapter
  adds to those runners never executes for Codex. The Codex adapter's
  `handle_pre` / `handle_post` do the same work through the shared module:
  push `inflight` (with `head_before` for `Bash`) immediately after parsing
  the payload, pop it in `handle_post` and run `detect_commit`, and pop on a
  blocking pre decision. Without this, Codex records no `commit` events and
  interrupt inference on Codex relies on the `Interrupt` hook alone (which is
  sufficient for interrupts, not for commits). Rollout: a follow-up task on
  the Codex adapter plan, after its first merge.

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
  `BeforeAgent` → `prompt`, `AfterAgent` → `stop`. Every `claude-hook`
  response is `{}` or the existing context JSON; never empty stdout. The
  existing `pre-check` / `post-check` runners print nothing on allow today;
  Gemini tolerates empty stdout (only non-JSON text becomes a user-visible
  message), so they are unchanged by this spec. The confidence gate does not
  run on Gemini-mapped events: Gemini has no documented `decision` semantics
  for `AfterAgent`, so `stop` is recorded and `{}` is printed. The gate stays
  Claude- and Codex-only until Gemini's response schema for that event is
  established.
- Fixtures are a precondition for this adapter too, on the same terms as the
  Claude ones and for the same reason — this repo has already misread a Gemini
  field name once (§Adjacent findings 4). Before step 4: `BeforeAgent`,
  `AfterAgent`, `SessionStart`, `SessionEnd`, and `BeforeTool`/`AfterTool` for
  `invoke_agent`, committed under `tests/fixtures/payloads/gemini/`.
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
  distinct names by accident. **The pattern is a property of the value, not of
  the CLI:** every reader treats a `kalpa` file whose `name` fails it as absent
  (one stderr warning), so a stale or hand-edited file cannot reach a journal
  tag, a log field, or the model-visible context header.
- `kalpa start <other>` writes in this order: record `kalpa_end` tagged with
  the **old** name, replace the file, record `kalpa_start` tagged with the
  **new** name. Each boundary record therefore carries the kalpa it is about.
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

1. `pre-check` for a shell tool (`Bash`, `run_shell_command`) whose command
   passes the text pre-filter in step 3 runs `git rev-parse HEAD` in the
   project root with a 2 s timeout and stores the result as `head_before` on
   the `inflight` entry. Applying the filter at pre as well as at post means a
   shell call that cannot be a commit spawns no git process at all. Failure
   (not a repo, git unavailable) stores nothing; a timeout stores
   `detection: "timeout"` on the entry so the miss is auditable rather than
   silent. Either way detection is disabled for that call.
2. `post-check` pops the entry. If `head_before` is present and
   `command_exit == 0`, it runs `git rev-parse HEAD` again. If `HEAD` differs,
   it records `commit` with `sha`, `head_before`, and `confidence_band` (from
   `outcomes::report` when confidence scoring is enabled and a work unit is
   open).
3. A cheap text pre-filter (`git commit` or `git cherry-pick` or `git revert`
   or `git merge` or `git rebase` present in the command) decides whether to
   run step 2's `rev-parse` at all, so most shell calls pay nothing at post.

Once the pre-filter has matched, the decision is immune to heredocs,
`git -C other-repo`, and `&& … || true` chains, because it observes the
repository rather than the string. The pre-filter itself is a string scan, so
the claim stops there: an alias, an abbreviation (`git com`), a wrapper script
(`./release.sh`), `git pull`, and `git am` all move `HEAD` without matching,
and are missed. So is a commit made outside a shell tool call entirely — the
human committing in another terminal or through a host's commit UI. Commits
are therefore **undercounted**, never overcounted, and `kalpa show` prints
`commits counted from shell tool calls only` under the commit line so the
denominator is read honestly. A commit followed by a reset within the same
command is also missed, which is correct: it did not land. Amends and rebases
move `HEAD` and are recorded; `sha` distinguishes them from a new commit for
any consumer that cares. `command_exit != 0` suppresses the check, which is
what makes `git commit && false` a non-commit; on a host that sends no
`command_exit` the check is skipped and the entry records
`detection: "no_exit_code"`.

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
sub-agents     12   starts, 11 matched   median 3m40s
commits         7   (shell tool calls only)   confidence at commit: high 5  medium 2  low 0
interventions / commit   2.43   (retained window)
```

`sub-agents` counts `subagent_start` records; the matched count is the subset
that paired with a stop, and the median is over those durations alone. The
`confidence at commit` segment is omitted entirely when no commit in the window
carries a band, rather than printing zeros. `interventions / commit` is
computed over the retained window shown in the header, not the kalpa's full
span; when the `kalpa_start` entry has itself rotated off, the header prints
`start not retained` in place of the start date. `interventions / commit` is
omitted when commits are zero. No other ratio
ships in v1 (§Non-goals).

Rule selectors added: `lifecycle:commit`, `lifecycle:kalpa_start`,
`lifecycle:kalpa_end`, and `kalpa:<name>`. A kalpa window (`k`) for
`journey_*` is deliberately not added in v1.

### Work items and governed throughput

The kalpa report answers "did this theme produce anything". The work-item
view answers the question underneath it: for one piece of work, which spec
was it built to, which rules fired, what evidence was recorded, and where did
a human step in. With that, an agent can run freely inside a work item while
the governance stays at the work-item boundary, and "software factory"
becomes a measurable claim about governed throughput rather than a metaphor.

**The work item is the existing work unit.** `outcomes::subject` already
mints an implicit unit id (`unit-<nanos>`) on demand and settles it on a
build/test cycle; journal records, outcome signals, and every lifecycle record
in this spec already carry it as `subject`. This section adds three things.

1. **Explicit units with a spec pointer.** `phr-mcp unit start [<id>] [--spec
   <path>]` sets the open subject (a caller-chosen id or a fresh mint) and
   records a `unit_start` lifecycle event with `extra.spec` (repo-relative;
   the file must exist) and `extra.unit_id`. `phr-mcp unit end` records
   `unit_end` and clears the open subject. Starting a unit while one is open
   ends the open one first. Implicit units keep working exactly as today and
   get no `unit_start` record; the report says which units were implicit.
   `extra.implicit: true` is stamped on `unit_end` for a unit that was never
   started explicitly, when the report has to synthesize one.

   **Where the name comes from.** Three sources, all landing in the same
   `unit_start` record:

   - *The human, on the CLI.* `phr-mcp unit start <id> --spec <path>` as above.
   - *The known-bug registry.* `phr-mcp unit start --bug <bug_id>` looks the
     id up in `.phronesis/bugs.json`, names the unit `bug-<bug_id>`, records
     `extra.test` (the registry's cargo test name) and `extra.spec` when the
     entry has one. `KnownBug` gains two optional fields, `spec` and `title`,
     both ignored by the confidence scorer. An unknown id is an error, not a
     fresh unit: the registry is the source of truth for bug-shaped work.
   - *The agent, from inside the conversation.* The existing MCP tool
     `submit_suggestion` already sets the explicit subject. It gains optional
     `spec` and `bug_id` parameters and, on success, records the same
     `unit_start` event the CLI does (through `lifecycle::unit_cli::start`, so
     there is one code path). This is how the human gets asked in the LLM
     window rather than at a shell: a rule can nudge the agent to name the
     work item when a turn begins with none open, and the agent asks and
     calls the tool. No new MCP tool.

   Rule that does the nudging, shipped as an example, not a packaged rule:

   ```json
   { "id": "suggest-name-the-work-item",
     "conditions": [
       { "journey_seen": ["lifecycle:prompt:fresh", "s"] },
       { "__script__": "facts_count('journey_seen', ['lifecycle:unit_start','s']) == 0" }
     ],
     "action": { "type": "suggestion",
                 "message": "No work item is named this session. Ask which bug or spec this is for, then call submit_suggestion with `bug_id` or `spec`." } }
   ```
2. **`subject` on rule evaluations.** `pre_check` / `post_check` action-log
   entries (`hook/mod.rs::log_hook_event`) gain `subject` when a unit is open.
   Today only journal records carry it, so "which rules fired for this work
   item" is not answerable from the log. Consequences already list `rule_id`
   and `action_type`.
3. **`phr-mcp unit show [<id>]`** joins the journal and the action log on
   `subject` and prints, oldest first:

   ```
   unit: unit-1789095489589855000   explicit   spec: docs/specs/SPEC-agent-lifecycle-events.md
   window: 2026-09-18 14:02 → 16:40   kalpa: lifecycle-events
   rules evaluated  41   fired 6   blocked 1   warned 5
     block-await-on-sync-execute-all-agenda-items  1   warn-piped-verification-masks-exit-status  3   …
   evidence         compile pass   tests pass (12/12)   band: high
   interventions     2   mid_turn 1   correction 1
     16:12  correction  "no, keep the journal free of text …"
   commits           1   0f3c9a1e  band high
   ```

   The intervention lines print the scrubbed prompt text from the action log,
   subject to the `prompt_text` switch. `--json` emits the same as one object.

**Definitions, stated so the numbers mean one thing.**

- A work item is **completed** when at least one `commit` record carries its
  `subject`.
- It is **governed** when it is completed and, in addition, at least one
  `pre_check` or `post_check` entry carries its `subject` (a rule was actually
  evaluated against its edits) and the confidence band at its last commit is
  not `low`.
- **Governed throughput** of a kalpa is the count of governed work items whose
  records fall inside the retention window. It is printed with the boundary,
  like every other kalpa number.
- **Interventions per work item** is interventions carrying a `subject` in the
  kalpa divided by completed work items. This is the second and last ratio
  the spec ships; §Non-goals is amended accordingly.

`kalpa show` gains three lines:

```
work items      9   explicit 6   implicit 3
governed        7   (commit + rules evaluated + band ≥ medium)
interventions / work item   1.89
```

**Limits.** Implicit units split on every build/test cycle, so a session that
never runs `unit start` will report many small units and a flattering
interventions-per-item number; the `explicit` / `implicit` split makes that
visible rather than hidden. A spec pointer is a path, not a hash; if the spec
changes after the unit starts, the report shows the path only. Rules that
fired inside a sub-agent carry the parent's `subject` because the subject file
is per project, which is the desired accounting (the sub-agent worked on the
same item) but is stated here so nobody expects per-agent attribution.

**Storage.** No new file. `unit_start` / `unit_end` are lifecycle records with
`subject` set. `extra` gains `spec`, `unit_id`, and `implicit` to its closed
vocabulary. Rollout node 6 owns this section (§Rollout).

## CLI and MCP surface

- `phr-mcp journey` renders lifecycle records inline with a `⟂` marker and the
  kind/mode instead of a path, and prints the active kalpa in its header.
  `phr-mcp journey --lifecycle` shows only lifecycle records.
- `phr-mcp journey --corrections` reads `correction` entries from the action
  log and its one rotated predecessor (which carry the text, and which
  `kalpa show` already reads as a pair) through
  `lifecycle::correction_text`, and prints `ts`, `sid`, and the scrubbed
  prompt, oldest first, under the same retention-boundary header. The list the
  feature exists to surface must not silently lose its oldest half to
  rotation. This is the input to a human or to `extract_rules`
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
  additionally applies three regexes before `scrub_value`, all unconditionally:
  any UUID-shaped token (no `session` context required — a bare id in free text
  is still an id), the phronesis sid shape `\bs-\d{4}-\d{2}-\d{2}-[0-9a-f]{1,8}\b`,
  and any path ending in `.jsonl` under a directory named `.claude`, `.codex`,
  or `.gemini`, accepting both `/` and `\` separators and relative as well as
  absolute forms. All three are replaced with the same placeholders
  `scrub_value` uses.
- `Scrubber::new` is fed `security::project_root()` and `$HOME`. When `$HOME`
  is unset or empty (`Scrubber::new` errors on empty, `payload_scrub.rs:690`)
  the hook falls back to project-root-only scrubbing and logs one stderr
  warning; the three regexes above still run, so ids and transcript paths are
  removed even then. It never writes unscrubbed text and never fails the hook.
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
  `__lifecycle` tool records — but with no tool projection, so they shift
  positional windows, inflate `since_ge` distances, and add `""` to
  `journey_distinct` on `path`. A downgraded binary also fails closed with
  `UndefinedSelector` on the first lifecycle rule, taking every journey fact
  with it. So: **the hooks and the MCP server upgrade in lockstep, and a
  project that has written a lifecycle rule requires ≥ 0.35.** That is the
  rollout gate, stated here rather than discovered; `tests/journey_journal.rs`
  pins the v1-reader behavior so the hazard stays visible.
- **Amendment to `SPEC-journey-facts.md`.** Its §"The journal record" states
  "One line per executed tool call. Written at post-check only … only actions
  that actually happened are journaled." This spec adds one paragraph there:
  "From v2, one line per executed tool call **or lifecycle event**. Lifecycle
  records are written by the event's own hook, carry `tool: "__lifecycle"`,
  and are excluded from positional (`Nc`) windows and from `journey_distinct`
  by the tool projection described in `SPEC-agent-lifecycle-events.md`.
  Selector-filtered time and session windows enumerate every record, and
  lifecycle selectors match lifecycle records there."
- `SUFFIX_HARD_CAP` is unchanged. Lifecycle records are roughly one per human
  turn plus two per sub-agent.

## Testing

Unit tests live beside the code; integration tests follow AGENTS.md
§"Testing Approach".

| test file | adds |
|---|---|
| `tests/journey_journal.rs` | v2 round trip; v1 record read under v2; compaction with mixed records retains `commit`/`kalpa_*`; no-text negative test |
| `tests/journey_derive.rs` | `lifecycle:*` and `kalpa:*` selector match; validation exemption with a real config and with `TaggerConfig::default()`; a misspelled lifecycle selector still fails as `UndefinedSelector`; a tagger tag in a reserved namespace is rejected at load; a catch-all tagger config stamps nothing on a lifecycle record; tool-projection determinism fixture (10 tool + 10 lifecycle interleaved) for `Calls`, `since_ge`, `filtered_since_ge`, `distinct`; over-read bound |
| `tests/lifecycle_state.rs` (new) | `with_locked` under 16 concurrent writers; `agents` push/pop by id, LIFO fallback, unmatched stop; `inflight` push/pop by key, blocked-pre pop, TTL expiry, SessionStart truncation, agent-scoped visibility; `turn` transitions; `kalpa` file round trip; `session` overwrite on a session-begin source, no overwrite and no truncation on `compact`/`fork`, atomic replace never exposing an empty file to a concurrent `current_sid`; every state file corrupted (invalid JSON, trailing partial line) still exits 0 and records the event; a read-only `.phronesis/journey` exits 0 |
| `tests/lifecycle_classify.rs` (new) | every branch of §Classification driven by fixture state files and the committed real transcript tail (both marker strings, the 64 KiB boundary, the `last_prompt_ts` comparison); `last_event: "interrupt"` yields `correction` on every host and survives a simulated compaction; a sub-agent-scoped prompt is `fresh` and untagged; a corrupt or foreign-`sid` `turn` file yields `fresh` |
| `tests/lifecycle_concurrency.rs` (new) | **gate for the Claude adapter step**: spawn N concurrent `pre-check`/`post-check` pairs all carrying *sub-agent* `agent_id`s, plus one parent pair that completes before the prompt, plus one `claude-hook UserPromptSubmit` for the parent; assert no `interrupt` and a `fresh`/`mid_turn` prompt, never `correction`. A deterministic companion asserts the inverse — one live *parent*-scoped entry with no readable transcript **does** yield `interrupt` + `correction` — so the gate pins agent scoping, not timing. After the storm: `inflight` empty, every journal and log line well-formed |
| `tests/hook_integration.rs` | `claude-hook` for each event with the committed fixtures; failure policy (`{}` exit 0 on bad stdin for non-tool events); `Stop` block/allow shapes and `stop_hook_active` short-circuit; a blocked `Stop` records nothing and leaves `turn` open, the later re-fire records exactly one `stop`; pre pushes inflight, post pops it regardless of age; `invoke_agent` derivation before the allowlist, its blocked-pre `agents` pop, its `<invoke_agent>` path, and a hostile `agent_name` sanitized away; Gemini `BeforeAgent`/`AfterAgent`/`SessionEnd` against the committed fixtures; a sweep over every event × fixture asserting exit 0 and parseable JSON on stdout; one journal record and one log entry per event with equal `sid`/`seq`; no entry contains `last_assistant_message`, `prompt_response`, or a transcript path |
| `tests/codex_hook_integration.rs` | `Interrupt` records and responds `{}`; `SubagentStart`/`Stop` record with agent fields; `UserPromptSubmit` records text; every response validated against a per-event allowed-key set mirroring the table |
| `tests/init_integration.rs` | all three writers register the new events idempotently; Codex `SessionStart` matcher is `""`; Claude `UserPromptSubmit` migrates from `interaction-context` to `claude-hook` in place; a foreign hook survives init per host (Claude `Stop`, Codex `Interrupt`/`SessionEnd`, Gemini `AfterAgent`/`SessionEnd` — the command-keyed upsert applies to all three writers); a `phr-mcp` entry invoked by absolute path is replaced in place, not duplicated; Codex `Interrupt`/`SessionEnd` register with matcher `""`; Gemini matcher is anchored |
| `tests/scrub_payload_integration.rs` | prompt scrubbing cases above, including a bare UUID and a phronesis sid; `prompt_text: none` at write **and** read time; `HOME` unset and empty (exit 0, entry present, regexes still applied, one warning); no prompt text in `payloads.jsonl` for a Gemini `AfterAgent` (`prompt_response`) or an `invoke_agent` `BeforeTool` (`tool_input.prompt`), while the action log still holds the full scrubbed text; `git check-ignore` covers `log.jsonl` and `log.jsonl.1` after `phr-mcp init` |
| `tests/journey_cli_integration.rs` | `--lifecycle` and `--corrections` rendering, including the rotated predecessor; kalpa header in `journey`, `stats`, and the session-context render; `stats` per-event and per-mode counts, median sub-agent duration, commit count; MCP `get_journey` returns lifecycle records with `kind`/`mode` and leaves existing consumers' fields unchanged |
| `tests/lifecycle_outcome.rs` (new) | the commit fixture list in a temp git repo, plus `git -C . commit` in the same repo and a commit made by a wrapper script (both documented misses, pinned so the undercount is known), an amend, and a rebase |
| `tests/kalpa_integration.rs` (new) | `kalpa start`/`end`/`show`; name validation; stamping on lifecycle records; survival across a simulated SessionStart; `stats --kalpa` counts and retention boundary against a fixture log |
| `phronesis-metrics/tests/derivation.rs` | lifecycle counter family and duration histogram buckets |
| `tests/payload_contract.rs` | the committed Claude fixtures and a Codex `Interrupt` fixture pinned to the documented field sets |

Manual evidence, in step 0 before the Claude adapter merges and again before
the branch is called done: one Claude Code session and
one Codex session in this repo, each with a sub-agent spawn, a mid-turn
message, an Esc interrupt followed by a prompt, and a commit, then `phr-mcp
journey --lifecycle`, `--corrections`, and `kalpa show` showing the expected
sequence. A Gemini session is the gate for step 4 specifically: without one,
step 4 does not ship in this release rather than shipping untried. Open
question 1 is answered from the step-0 Claude session's log.

## Rollout

Dependency graph, not a chain:

```
0  capture and commit host fixtures + the Claude transcript tail (blocks 2 and 4)
1a shared type + journal v2 + derive projection + selectors + compaction
1b HookPayload widening + lifecycle/state.rs + inflight/agents/turn/session/kalpa files + scrub_prompt + capture redaction
        │
        ├── 2 Claude adapter (claude-hook, pre/post inflight + commit detection, init writer)   ─┐
        ├── 3 Codex adapter (payload fields, Interrupt/SessionEnd, renderer, init writer)        ├── 5 stats, metrics, journey/kalpa CLI rendering
        └── 4 Gemini registrations                                                               ─┘
                                                                                                  │
                                                        6 work items: unit start/end/show, subject on rule evaluations, governed throughput in kalpa show
```

- 6 runs after 5 and touches `hook/mod.rs::log_hook_event` (Plan 1's file,
  already merged by then), `main.rs` (a `Unit` subcommand), a new
  `lifecycle/unit_cli.rs`, and the report code 5 introduced.

- 0 needs a live host, not a code change, and it gates 2 and 4:
  `tests/hook_integration.rs` and `tests/payload_contract.rs` cannot be written
  against invented payloads. It runs alongside 1a/1b. The Claude session in
  step 0 is also where Open question 1 is answered, so any system-injected
  `UserPromptSubmit` is captured as a fixture before the intervention metric
  ships rather than after.
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
- **Upgrade note for users**, in the release notes as well as the CHANGELOG:
  upgrading the binary registers nothing. Run `phr-mcp init` in each project to
  get the new hook registrations; Codex users then re-trust hooks via `/hooks`.
  Until then `session-context` and `interaction-context` keep behaving exactly
  as today and no lifecycle event is recorded. Two further consequences to
  name there: `s`-window rules now scope to a session (§Correlation state), and
  Claude users with outcomes enabled get the confidence gate on turn stop for
  the first time — disabled the same way it is disabled for Codex today.

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
   community write-up, not by the docs. Answered in step 0, before the Claude
   adapter merges: if system-injected prompts appear as `mid_turn`, the
   captured payloads are the input to a filter designed then, because each such
   prompt is a false `lifecycle:intervention` in the one number this feature
   exists to produce.
2. **Claude `SubagentStop` with empty `agent_type`.** Tracked upstream
   (anthropics/claude-code#87065): internal forks fire it with `agent_type:
   ""`. Records are written as-is with the empty type; a rule scoping to
   `lifecycle:agent:<type>` naturally excludes them.

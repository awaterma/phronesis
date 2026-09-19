# Lifecycle Events — Plan 3: Codex CLI adapter (spec step 3)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Codex adapter emit every lifecycle event the host can observe — `Interrupt`, `SessionEnd`, `SessionStart` session identity, `UserPromptSubmit` with mode classification and scrubbed text, `SubagentStart`/`SubagentStop` pairing, and `Stop` — while keeping every response inside the per-event permitted-key sets.

**Architecture:** `codex_hook::dispatch` gains lifecycle side effects around the handlers it already has. Every write goes through `lifecycle::record::record` and `lifecycle::state`; the adapter never touches the journal or action log for lifecycle. `renderer.rs` keeps responses inside the Codex output schemas (`deny_unknown_fields`), so `Interrupt` and `SessionEnd` render `{}` and completion events stay free of `hookSpecificOutput`.

**Tech Stack:** Rust 2024 edition (rust-version 1.90), serde/serde_json, clap, tempfile. No new dependencies.

**Spec:** `docs/specs/SPEC-agent-lifecycle-events.md` (revised 2026-09-18), §"Host adapters / Codex CLI", §Classification, §"Action log". Read it first; this plan argues from it.

**Depends on:** `docs/superpowers/plans/2026-09-18-lifecycle-events-1-foundation.md` (Plan 1) must be merged. This plan uses **only** these Plan 1 names: `lifecycle::event::{LifecycleEvent, Kind, Mode, Host, Stamped, PromptText}`, `lifecycle::record::record`, `lifecycle::state::{push_agent, pop_agent, OpenAgent, read_turn, open_turn, close_turn, set_session, is_session_begin, reset_for_session_start, clear_inflight, classify_prompt, PromptContext, Classification, InterruptSource}`, `lifecycle::scrub::scrub_prompt`, `hook::redact_for_capture`, plus `hook::capture_raw_payload`, which Plan 1 Task 10 already makes `pub(crate)` *and* already wires through `redact_for_capture`. **This plan makes no edit to `hook/mod.rs`.**

**Three Plan 1 names this plan deliberately does not use**, because the revised
spec removed them: `state::clear_session` (SessionEnd no longer truncates the
session file), `state::last_lifecycle_kind` (classification reads
`turn.last_event`, never the journal tail), and `InterruptSource::Hook`. If any
of the three resolves when you build, you are on an older Plan 1.

**Runs in parallel with:** Plan 2 (Claude) and Plan 4 (Gemini). See §Merge notes at the end of this plan for every shared file and the exact region each plan owns. This plan needs nothing from Plan 2 or Plan 4 and can merge before or after either.

**Files this plan owns exclusively:**

- `crates/phronesis-mcp/src/codex_hook.rs`
- `crates/phronesis-mcp/src/codex_hook/renderer.rs`
- `crates/phronesis-mcp/tests/codex_hook_integration.rs`
- `crates/phronesis-mcp/tests/fixtures/payloads/codex/interrupt.json`
- `docs/specs/SPEC-codex-hooks-integration.md`

**Files shared with Plans 2 and 4** (regions are disjoint; see §Merge notes): `src/init.rs` (only `write_codex_hooks`), `tests/payload_contract.rs`, `tests/fixtures/hook_events.json` (only the `"codex"` array), `CHANGELOG.md`.

## Global Constraints

- No new crate dependencies.
- Prompt text never enters `JournalRecord`, a `phr::Fact`, a context render, or stdout. Only the action log, via `scrub_prompt`.
- Every lifecycle write is fail-open: failures print `phronesis: …` on stderr; the hook still responds with valid JSON and exit 0.
- Codex output schemas are `deny_unknown_fields`. An extra key fails the whole hook. Permitted keys per event are the table in the spec §"Host adapters / Codex CLI".
- `Stop` does not fire on abort; `Interrupt` does. A missing `Stop` never means the turn completed.
- `Interrupt` sets `turn` to `{open: false, last_event: "interrupt"}` **and drops
  the session's `inflight` entries**: the aborted tools' `PostToolUse` never
  fires, and a lingering entry would fake an interrupt for the next 900 s.
- **`SubagentStop` never closes `turn`; only the main-agent `Stop` does.** The two
  share a `dispatch` arm in the current code, which makes the distinction easy to
  lose — Task 5 splits them and pins it.
- `Stop` and `SubagentStop` skip the confidence gate entirely when
  `stop_hook_active: true`, and **record nothing when the response blocks**: a
  blocked stop means the turn continues.
- `SessionStart` is **source-gated**. The matcher widens to `""` in Task 7, so
  `compact` and `fork` now reach the handler; they render context and touch no
  correlation state.
- Conventional-commit messages. Run `cargo fmt` and `cargo clippy --all-targets -p phronesis-mcp -- -D warnings` before every commit.
- Machine note: if `cargo` fails with "You have not agreed to the Xcode license", stop and report; the human must run `sudo xcodebuild -license accept`.

## File structure

| path | responsibility |
|---|---|
| `crates/phronesis-mcp/src/codex_hook.rs` (modify) | `CodexPayload` fields, capture tee, lifecycle side effects in `dispatch`, journal `sid` source |
| `crates/phronesis-mcp/src/codex_hook/renderer.rs` (modify) | explicit `{}` for `Interrupt`/`SessionEnd`; permitted-key tests |
| `crates/phronesis-mcp/src/init.rs` (modify) | register `Interrupt`/`SessionEnd`; `SessionStart` matcher `""` |
| `crates/phronesis-mcp/tests/codex_hook_integration.rs` (modify) | per-event behaviour and init assertions |
| `crates/phronesis-mcp/tests/fixtures/payloads/codex/interrupt.json` (create) | contract fixture |
| `docs/specs/SPEC-codex-hooks-integration.md`, `CHANGELOG.md` (modify) | docs |

---

### Task 1: `CodexPayload` fields and the redacted capture tee

**Files:**
- Modify: `crates/phronesis-mcp/src/codex_hook.rs:34-51` (struct), `:74-100` (`run`, `parse_payload`)
- Test: `crates/phronesis-mcp/tests/codex_hook_integration.rs`

**Interfaces:**
- Consumes: `crate::hook::capture_raw_payload(phase: &str, raw: &str)`, which already applies `crate::hook::redact_for_capture` (Plan 1 Task 10).
- Produces: on `CodexPayload` — `agent_id`, `agent_type`, `transcript_path`, `agent_transcript_path`, `last_assistant_message`, `prompt: Option<String>`, `stop_hook_active: Option<bool>`, all `#[serde(default)]`; `fn parse_payload(event: &str) -> anyhow::Result<CodexPayload>`.

- [ ] **Step 1: Write the failing test** (append to `tests/codex_hook_integration.rs`)

```rust
#[test]
fn codex_hook_captures_payloads_with_prompt_text_redacted() {
    let project = tempfile::tempdir().expect("temp project");
    let capture = tempfile::tempdir().expect("capture dir");
    let payload = json!({
        "hook_event_name": "UserPromptSubmit", "session_id": "codex-s-1",
        "turn_id": "codex-t-1", "prompt": "zzz-secret-prompt-text"
    });
    let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(["codex-hook", "UserPromptSubmit"])
        .env("PHRONESIS_PROJECT_ROOT", project.path())
        .env("PHRONESIS_CAPTURE_DIR", capture.path())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().expect("spawn codex hook");
    child.stdin.take().expect("stdin")
        .write_all(payload.to_string().as_bytes()).expect("write payload");
    assert!(child.wait_with_output().expect("wait").status.success());
    let captured =
        fs::read_to_string(capture.path().join("payloads.jsonl")).expect("captured payloads");
    assert!(!captured.contains("zzz-secret-prompt-text"), "{captured}");
    assert!(captured.contains("<redacted:"), "{captured}");
    assert!(captured.contains("UserPromptSubmit"), "{captured}");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --test codex_hook_integration codex_hook_captures_payloads 2>&1 | tail -20`
Expected: FAIL — `payloads.jsonl` does not exist; `codex_hook` never tees stdin.

- [ ] **Step 3: Implement**

Append to `CodexPayload`, after `tool_response`:

```rust
    /// Sub-agent identity on `SubagentStart` / `SubagentStop`.
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    agent_type: Option<String>,
    /// Main-session transcript; carried by `Interrupt` and most events.
    #[serde(default)]
    transcript_path: Option<String>,
    /// `SubagentStop` only.
    #[serde(default)]
    agent_transcript_path: Option<String>,
    /// `SubagentStop` only. Redacted before capture; never journaled.
    #[serde(default)]
    last_assistant_message: Option<String>,
    /// True when the host re-runs a stop hook after a block. `Stop` and
    /// `SubagentStop`.
    #[serde(default)]
    stop_hook_active: Option<bool>,
    /// `UserPromptSubmit` only, verbatim. Scrubbed before it reaches the log.
    #[serde(default)]
    prompt: Option<String>,
    /// `SessionStart` only: `startup` | `resume` | `clear` | `compact` | `fork`.
    /// Task 7 widens the registration matcher to `""`, so `compact` and `fork`
    /// reach the handler for the first time and the gate is what keeps them
    /// from orphaning an open sub-agent.
    #[serde(default)]
    source: Option<String>,
```

Tee stdin through the shared capture path, which redacts `prompt` and `last_assistant_message`:

```rust
fn parse_payload(event: &str) -> anyhow::Result<CodexPayload> {
    let raw = security::read_stdin_capped()?;
    // Tees to PHRONESIS_CAPTURE_DIR when set, after redacting free-text
    // fields. Best-effort: never changes the response or the exit code.
    crate::hook::capture_raw_payload(event, &raw);
    Ok(serde_json::from_str(&raw)?)
}
```

In `run` (`codex_hook.rs:77-79`), change the call site from `let parsed = parse_payload();` to `let parsed = parse_payload(event);`.

`capture_raw_payload` is already `pub(crate)` and already redacts `prompt` and `last_assistant_message` before writing — Plan 1 Task 10 made both changes and owns that file. **Do not edit `src/hook/mod.rs` in this plan.** If the build says `capture_raw_payload` is private, Plan 1 has not landed; stop and merge Plan 1 first.

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --test codex_hook_integration --test payload_capture 2>&1 | tail -20`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/codex_hook.rs crates/phronesis-mcp/tests/codex_hook_integration.rs
git commit -m "feat(codex): widen CodexPayload and tee redacted payloads to the capture dir"
```

---

### Task 2: `Interrupt` and `SessionEnd`

**Files:**
- Modify: `crates/phronesis-mcp/src/codex_hook.rs` (`dispatch` at `:126-144`, new helpers), `crates/phronesis-mcp/src/codex_hook/renderer.rs:29-40`
- Test: `crates/phronesis-mcp/tests/codex_hook_integration.rs`

**Interfaces:**
- Consumes: `lifecycle::record::record`, `lifecycle::state::{close_turn, clear_inflight, read_turn}`, `lifecycle::event::{LifecycleEvent, Kind, Host}`.
- Produces: `fn lifecycle_event(payload: &CodexPayload, kind: Kind) -> LifecycleEvent` and the test helpers `journal_records` / `lifecycle_records` / `lifecycle_kinds` / `lifecycle_log` / `log_event` / `turn_file` / `prompt_payload` (used by Tasks 3–5). `fn unix_secs_now() -> u64` is added by Task 4, its first user.

- [ ] **Step 1: Write the failing tests**

```rust
fn journal_records(root: &std::path::Path) -> Vec<Value> {
    fs::read_to_string(root.join(".phronesis/journey/events.jsonl"))
        .unwrap_or_default().lines()
        .filter_map(|l| serde_json::from_str(l).ok()).collect()
}

fn lifecycle_records(root: &std::path::Path) -> Vec<Value> {
    journal_records(root).into_iter().filter(|r| r.get("kind").is_some()).collect()
}

fn lifecycle_kinds(root: &std::path::Path) -> Vec<String> {
    lifecycle_records(root).iter()
        .map(|r| r["kind"].as_str().unwrap_or_default().to_string()).collect()
}

fn lifecycle_log(root: &std::path::Path) -> Vec<Value> {
    fs::read_to_string(root.join(".phronesis/log.jsonl"))
        .unwrap_or_default().lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|e| e["kind"] == "lifecycle").collect()
}

fn log_event(root: &std::path::Path, event: &str) -> Value {
    lifecycle_log(root).into_iter().find(|e| e["event"] == event)
        .unwrap_or_else(|| panic!("no {event} log entry"))
}

fn turn_file(root: &std::path::Path) -> Value {
    serde_json::from_str(
        &fs::read_to_string(root.join(".phronesis/journey/turn")).expect("turn file"),
    ).expect("turn JSON")
}

fn prompt_payload(session: &str, turn: &str, text: &str) -> Value {
    json!({"hook_event_name": "UserPromptSubmit",
           "session_id": session, "turn_id": turn, "prompt": text})
}

At this task `UserPromptSubmit` still records nothing and opens no turn — Task 4 adds
that. Both tests below therefore drive the turn state through the library API
(`phronesis_mcp::lifecycle::state` is `pub`) rather than through a prompt hook, and
expect only the records this task's arms actually write. The prompt-driven versions
of these scenarios live in Task 4.

```rust
use phronesis_mcp::lifecycle::state;
```

```rust
#[test]
fn interrupt_records_and_closes_the_turn() {
    let project = tempfile::tempdir().expect("temp project");
    state::open_turn(project.path(), Some("codex-t-2"), 10);
    let interrupt = json!({
        "hook_event_name": "Interrupt", "cwd": "/tmp/p", "model": "gpt-5",
        "permission_mode": "on-request", "session_id": "codex-s-2", "turn_id": "codex-t-2",
        "transcript_path": "/tmp/p/.codex/sessions/codex-s-2.jsonl"
    });
    assert_eq!(response(&run_hook(project.path(), &interrupt)), json!({}));

    // Exactly one record, and it is the interrupt: an abort must never also
    // manufacture a `stop`, which would mean "the turn completed".
    assert_eq!(lifecycle_kinds(project.path()), vec!["interrupt"]);
    let last = lifecycle_records(project.path()).pop().expect("an interrupt record");
    assert_eq!(last["host"], "codex");
    assert_eq!(last["turn"], "codex-t-2");
    assert!(last["tags"].as_array().expect("tags").contains(&json!("lifecycle:interrupt")));
    let entry = log_event(project.path(), "interrupt");
    assert_eq!(entry["inferred_from"], "hook");
    assert_eq!(entry["session_id"], "codex-s-2");
    assert_eq!(turn_file(project.path())["open"], false);
    assert_eq!(turn_file(project.path())["last_event"], "interrupt");
}

#[test]
fn session_end_stops_an_open_turn_and_leaves_the_session_file() {
    let project = tempfile::tempdir().expect("temp project");
    state::set_session(project.path(), "codex-s-3");
    state::open_turn(project.path(), Some("codex-t-3"), 10);
    let end = json!({"hook_event_name": "SessionEnd", "session_id": "codex-s-3"});
    assert_eq!(response(&run_hook(project.path(), &end)), json!({}));
    assert_eq!(lifecycle_kinds(project.path()), vec!["stop"]);
    // SessionEnd does NOT truncate the session file: truncation would let any
    // stray hook between sessions mint a throwaway sid, and the next
    // session-begin SessionStart overwrites it anyway (spec §Correlation state).
    assert_eq!(
        fs::read_to_string(project.path().join(".phronesis/journey/session"))
            .expect("session file").trim(),
        "codex-s-3"
    );
    // A second SessionEnd with no open turn records nothing further.
    assert_eq!(response(&run_hook(project.path(), &end)), json!({}));
    assert_eq!(lifecycle_records(project.path()).len(), 1);
}

/// The aborted tools' `PostToolUse` never fires, so their `inflight` entries
/// would sit there faking an interrupt for the next 900 s. `Interrupt` drops
/// them.
#[test]
fn interrupt_drops_the_sessions_inflight_entries() {
    let project = tempfile::tempdir().expect("temp project");
    state::open_turn(project.path(), Some("codex-t-7"), 10);
    state::push_inflight(project.path(), state::Inflight {
        key: "u-aborted".into(), tool: "Bash".into(), ts: 10,
        agent_id: None, head_before: None, detection: None,
    });
    let interrupt =
        json!({"hook_event_name": "Interrupt", "session_id": "codex-s-7", "turn_id": "codex-t-7"});
    assert_eq!(response(&run_hook(project.path(), &interrupt)), json!({}));
    assert!(
        state::pop_inflight(project.path(), "u-aborted").is_none(),
        "an aborted tool's entry must not survive its own turn"
    );
    // And the classifier's evidence is `last_event`, so the next prompt is a
    // correction with no second interrupt record.
    assert_eq!(turn_file(project.path())["last_event"], "interrupt");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --test codex_hook_integration interrupt_records session_end_stops 2>&1 | tail -30`
Expected: FAIL — no lifecycle records are written; `.phronesis/journey/turn` does not exist.

- [ ] **Step 3: Implement**

Add to the `use` block at the top of `codex_hook.rs` — **only these three**. `Mode` and `crate::lifecycle::scrub` have no use until Task 4 and would fail `-D warnings` (`unused_imports`) at this task's commit; Task 4 adds them:

```rust
use crate::lifecycle::event::{Host, Kind, LifecycleEvent};
use crate::lifecycle::record::record;
use crate::lifecycle::state;
```

Add one helper near `empty_decision` (`unix_secs_now` belongs to Task 4, its first user, for the same `-D warnings` reason — `dead_code` fires on a never-called private fn):

```rust
/// A `LifecycleEvent` pre-filled from the identity fields any Codex payload
/// may carry. Callers add the kind-specific mode, prompt, and extras.
fn lifecycle_event(payload: &CodexPayload, kind: Kind) -> LifecycleEvent {
    let mut event = LifecycleEvent::new(kind, Host::Codex);
    if let Some(sid) = payload.session_id.as_deref().filter(|s| !s.is_empty()) {
        event = event.with_session(sid);
    }
    if let Some(tid) = payload.turn_id.as_deref().filter(|s| !s.is_empty()) {
        event = event.with_turn(tid);
    }
    if let Some(aid) = payload.agent_id.as_deref().filter(|s| !s.is_empty()) {
        event = event.with_agent(aid, payload.agent_type.clone());
    }
    event
}
```

Add two `dispatch` arms before the `_ => empty_decision()` fallback:

```rust
        // Codex fires Interrupt on abort, before TurnAborted, with the
        // transcript flushed. Stop does not fire, so this is the only end of
        // an aborted turn. Its schema permits `systemMessage` only.
        "Interrupt" | "interrupt" => {
            record(
                root,
                lifecycle_event(payload, Kind::Interrupt).with_extra("inferred_from", "hook"),
            );
            // `{open: false, last_event: "interrupt"}`. The next prompt reads
            // `last_event` in classification step 1 and comes out `correction`
            // with no second interrupt record — the single
            // interrupt-already-recorded path, on every host.
            state::close_turn(root, "interrupt");
            // The aborted tools' PostToolUse never fires; a lingering entry
            // would fake an interrupt for 900 s.
            state::clear_inflight(root);
            empty_decision()
        }
        // A session ending with a turn still open ended without a Stop:
        // record the stop so the turn is closed in the journal too. `session` is
        // left in place — the next session-begin SessionStart overwrites it, and
        // truncating would let any stray hook between sessions mint a throwaway
        // sid (spec §"Host adapters / Codex CLI").
        "SessionEnd" | "session-end" => {
            if state::read_turn(root).open {
                record(root, lifecycle_event(payload, Kind::Stop));
            }
            state::close_turn(root, "stop");
            empty_decision()
        }
```

In `renderer.rs`, make the empty response explicit rather than relying on the catch-all:

```rust
        // Interrupt permits `systemMessage` only and SessionEnd is advisory;
        // Phronesis has nothing to say on either, and any extra key would fail
        // the whole hook under deny_unknown_fields.
        "Interrupt" | "interrupt" | "SessionEnd" | "session-end" => "{}".to_string(),
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --test codex_hook_integration 2>&1 | tail -30`
Expected: the two new tests pass; every pre-existing test still passes.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/codex_hook.rs crates/phronesis-mcp/src/codex_hook/renderer.rs crates/phronesis-mcp/tests/codex_hook_integration.rs
git commit -m "feat(codex): record Interrupt and SessionEnd lifecycle events"
```

---

### Task 3: `SessionStart` session identity and the journal `sid` source

**Files:**
- Modify: `crates/phronesis-mcp/src/codex_hook.rs` (`dispatch` SessionStart arm; `journal_post` at `:1052-1074`)
- Test: `crates/phronesis-mcp/tests/codex_hook_integration.rs`

**Interfaces:**
- Consumes: `lifecycle::state::{set_session, reset_for_session_start}`, `journey::current_sid`, and Task 2's test helpers.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn session_start_adopts_the_host_session_id_and_resets_correlation_state() {
    let project = tempfile::tempdir().expect("temp project");
    assert!(run_hook(project.path(), &prompt_payload("codex-s-old", "codex-t-old", "before"))
        .status.success());
    let start = json!({
        "hook_event_name": "SessionStart", "session_id": "codex-s-new", "source": "startup"
    });
    assert!(run_hook(project.path(), &start).status.success());
    assert_eq!(
        fs::read_to_string(project.path().join(".phronesis/journey/session"))
            .expect("session file").trim(),
        "codex-s-new"
    );
    assert_eq!(turn_file(project.path())["open"], false);

    // Tool records now take their sid from the shared session file, not the
    // payload, so every host agrees on session identity.
    let post = json!({
        "hook_event_name": "PostToolUse", "session_id": "codex-s-stale",
        "turn_id": "codex-t-1", "tool_use_id": "u1", "tool_name": "Bash",
        "tool_input": {"command": "cargo test"},
        "tool_response": {"output": "ok", "exit_code": 0}
    });
    assert!(run_hook(project.path(), &post).status.success());
    let tool_record = journal_records(project.path())
        .into_iter().filter(|r| r.get("kind").is_none()).next_back()
        .expect("a tool record");
    assert_eq!(tool_record["sid"], "codex-s-new");
}

/// Task 7 widens the SessionStart matcher to `""`, so compact and fork sessions
/// reach this handler for the first time. They must render context and touch no
/// correlation state: their open sub-agents and in-flight tools are real.
#[test]
fn session_start_on_compact_or_fork_touches_no_correlation_state() {
    for source in ["compact", "fork"] {
        let project = tempfile::tempdir().expect("temp project");
        let begin =
            json!({"hook_event_name": "SessionStart", "session_id": "codex-s-a", "source": "startup"});
        assert!(run_hook(project.path(), &begin).status.success());
        assert!(
            run_hook(project.path(), &prompt_payload("codex-s-a", "codex-t-a", "go"))
                .status.success()
        );
        let sub = json!({
            "hook_event_name": "SubagentStart", "session_id": "codex-s-a",
            "turn_id": "codex-t-a", "agent_id": "codex-a-live", "agent_type": "reviewer"
        });
        assert!(run_hook(project.path(), &sub).status.success());

        let continued = json!({
            "hook_event_name": "SessionStart", "session_id": "codex-s-b", "source": source
        });
        assert!(run_hook(project.path(), &continued).status.success());

        assert_eq!(
            fs::read_to_string(project.path().join(".phronesis/journey/session"))
                .expect("session file").trim(),
            "codex-s-a",
            "{source} must not mint a new sid"
        );
        assert_eq!(turn_file(project.path())["open"], true, "{source}: the turn is still running");
        assert!(
            !fs::read_to_string(project.path().join(".phronesis/journey/agents"))
                .unwrap_or_default().trim().is_empty(),
            "{source}: the open sub-agent must survive"
        );
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --test codex_hook_integration session_start_adopts 2>&1 | tail -20`
Expected: FAIL — the session file is absent and the tool record's `sid` is `codex-s-stale`.

- [ ] **Step 3: Implement**

Replace the `SessionStart` arm in `dispatch`:

```rust
        "SessionStart" | "session-start" => {
            // Source-gated. The shared `session` file is the single source of
            // session identity for every host (spec §Correlation state), and a
            // session-begin source (`startup` / `resume` / `clear`, or an absent
            // source) overwrites it and truncates agents/inflight, closing any
            // turn left open by a crashed or aborted previous session.
            //
            // `compact` and `fork` continue the current session, so its open
            // sub-agents and in-flight tools are real. Task 7 widens the
            // registration matcher from `"startup|resume|clear"` to `""`, which
            // is safe ONLY because of this gate: without it, a mid-session
            // compaction would orphan every open sub-agent and discard every
            // in-flight tool, on a path that did not fire at all before.
            if state::is_session_begin(payload.source.as_deref()) {
                if let Some(sid) = payload.session_id.as_deref().filter(|s| !s.is_empty()) {
                    state::set_session(root, sid);
                }
                state::reset_for_session_start(root);
            }
            make_ctx_decision(root, ContextKind::SessionStart).await
        }
```

In `journal_post` (`codex_hook.rs:1037-1074`), replace the `sid` field expression and bump the record version. Codex writes its own `JournalRecord` literal rather than going through `hook::journey_record::build_journal_record`, so the `v: JOURNAL_V` change Plan 2 makes there does **not** reach this call site — without this line Codex tool records would keep writing `v: 1` while every other host writes `v: 2`:

```rust
    let record = journey::journal::JournalRecord {
        // Tool records and lifecycle records share one schema version.
        v: journey::journal::JOURNAL_V,
        ts: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        // Session identity comes from the shared `session` file, as on every
        // other host; `payload.session_id` can disagree after a fork or resume
        // (spec §Premise, "two session-id sources can disagree").
        sid: journey::current_sid(&root),
        // …every remaining field unchanged, plus the seven lifecycle fields
        // Plan 1 Task 1 added, all `None` on a tool record:
        kind: None,
        mode: None,
        host: None,
        turn: None,
        agent: None,
        agent_type: None,
        kalpa: None,
    };
```

(Plan 1 Task 1 already added those seven `None`s to this literal so the crate compiles; this task only changes `v` and `sid`.)

Add the matching assertion to the test above, after the `sid` check:

```rust
    assert_eq!(tool_record["v"], 2, "Codex tool records share the v2 schema");
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --test codex_hook_integration --test payload_contract 2>&1 | tail -30`
Expected: pass. If a `payload_contract` fixture asserted a payload-supplied `sid`, update the fixture's expectation, not the code.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/codex_hook.rs crates/phronesis-mcp/tests/codex_hook_integration.rs
git commit -m "feat(codex): adopt host session id at SessionStart and journal from the shared session file"
```

---

### Task 4: `UserPromptSubmit` — classification, mode, scrubbed text

**Files:**
- Modify: `crates/phronesis-mcp/src/codex_hook.rs` (`dispatch` UserPromptSubmit arm; new `record_prompt`, `unix_secs_now`)
- Test: `crates/phronesis-mcp/tests/codex_hook_integration.rs`

**Interfaces:**
- Consumes: `lifecycle::state::{classify_prompt, PromptContext, Classification, InterruptSource, read_turn, open_turn}`, `lifecycle::scrub::scrub_prompt`.
- Produces: `fn record_prompt(payload: &CodexPayload, root: &Path)` and `fn unix_secs_now() -> u64` (also used by Task 5).

**Classification rules on Codex (spec §Classification):**
- Step 1 reads `turn.last_event == "interrupt"` and returns
  `Classification { mode: Correction, interrupt: None }`. `None` means **the
  record already exists** — the `Interrupt` arm wrote it in Task 2. The handler
  must not write a second one, and the contract that stops it is exactly this:
  write an `interrupt` record only for `Some(source)`.
- On Codex `classify_prompt` never returns `Some(_)`: the transcript branch is
  Claude-only, the `open_turn` branch is Gemini-only, and the `inflight` branch
  is skipped on Codex because the `Interrupt` hook is authoritative. The
  `Some(source)` arm below is therefore defensive rather than reachable today —
  it is written generically so that if Plan 1 ever enables a branch for Codex,
  this adapter records it instead of silently dropping it.
- An open turn with no interrupt evidence is `mid_turn`. Spec step 3 notes that
  on Codex a `turn_id` equal to the open turn's id is "a second, sufficient
  signal for `mid_turn`" — Codex fires `UserPromptSubmit` with the *running*
  turn's id for a queued message. With the `inflight` branch off on Codex the two
  signals agree by construction, so no override code is needed; the test below
  asserts the agreement rather than the plan re-deriving it.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn prompt_modes_are_fresh_mid_turn_and_never_double_interrupt() {
    let project = tempfile::tempdir().expect("temp project");
    // 1. No open turn → fresh.
    assert!(run_hook(project.path(), &prompt_payload("codex-s-4", "codex-t-1", "first"))
        .status.success());
    // 2. Same turn id while the turn is open → a queued mid-turn message.
    assert!(run_hook(project.path(), &prompt_payload("codex-s-4", "codex-t-1", "also this"))
        .status.success());
    // 3. Interrupt closes the turn and writes the only interrupt record.
    let interrupt =
        json!({"hook_event_name": "Interrupt", "session_id": "codex-s-4", "turn_id": "codex-t-1"});
    assert!(run_hook(project.path(), &interrupt).status.success());
    assert!(run_hook(project.path(), &prompt_payload("codex-s-4", "codex-t-2", "instead do")).
        status.success());

    assert_eq!(
        lifecycle_kinds(project.path()),
        vec!["prompt", "prompt", "interrupt", "prompt"],
        "the Hook branch must not write a second interrupt record"
    );
    let recs = lifecycle_records(project.path());
    let modes: Vec<String> = recs.iter()
        .filter(|r| r["kind"] == "prompt")
        .map(|r| r["mode"].as_str().unwrap_or_default().to_string()).collect();
    // The third prompt follows an `interrupt` in the same session, which is the
    // spec's definition of `correction`. The Interrupt arm closed the turn, and
    // this holds because classification step 1 checks
    // `turn.last_event == "interrupt"` BEFORE declaring `fresh` (Plan 1 Task 8).
    assert_eq!(modes, vec!["fresh", "mid_turn", "correction"]);
    let correction = recs.iter().rfind(|r| r["kind"] == "prompt").expect("a prompt");
    let tags = correction["tags"].as_array().expect("tags");
    assert!(tags.contains(&json!("lifecycle:prompt:correction")), "{correction}");
    assert!(tags.contains(&json!("lifecycle:intervention")), "{correction}");
}

/// A tool record journaled between the interrupt and the prompt must not hide
/// the interrupt — it cannot, because the evidence is `turn.last_event` and not
/// a journal scan — and another session's interrupt must not answer for this
/// one, because the turn file carries its own `sid`.
#[test]
fn correction_survives_an_intervening_tool_record_and_is_session_scoped() {
    let project = tempfile::tempdir().expect("temp project");
    assert!(run_hook(project.path(), &prompt_payload("codex-s-9", "codex-t-9", "go"))
        .status.success());
    let interrupt =
        json!({"hook_event_name": "Interrupt", "session_id": "codex-s-9", "turn_id": "codex-t-9"});
    assert!(run_hook(project.path(), &interrupt).status.success());
    let post = json!({
        "hook_event_name": "PostToolUse", "session_id": "codex-s-9", "tool_use_id": "u9",
        "tool_name": "Bash", "tool_input": {"command": "echo hi"},
        "tool_response": {"output": "hi", "exit_code": 0}
    });
    assert!(run_hook(project.path(), &post).status.success());
    assert!(run_hook(project.path(), &prompt_payload("codex-s-9", "codex-t-10", "instead"))
        .status.success());
    let modes: Vec<String> = lifecycle_records(project.path()).iter()
        .filter(|r| r["kind"] == "prompt")
        .map(|r| r["mode"].as_str().unwrap_or_default().to_string()).collect();
    assert_eq!(modes, vec!["fresh", "correction"]);

    // A new session starts clean: the previous session's interrupt is not its
    // evidence.
    let start = json!({"hook_event_name": "SessionStart", "session_id": "codex-s-10"});
    assert!(run_hook(project.path(), &start).status.success());
    assert!(run_hook(project.path(), &prompt_payload("codex-s-10", "codex-t-11", "new"))
        .status.success());
    let modes: Vec<String> = lifecycle_records(project.path()).iter()
        .filter(|r| r["kind"] == "prompt")
        .map(|r| r["mode"].as_str().unwrap_or_default().to_string()).collect();
    assert_eq!(modes, vec!["fresh", "correction", "fresh"]);
}

#[test]
fn prompt_text_is_scrubbed_into_the_log_and_never_the_journal() {
    let project = tempfile::tempdir().expect("temp project");
    // A temp `$HOME` set on the *child* process only: the parent's environment
    // is never mutated, so this test cannot race the rest of the binary, and it
    // does not require the ambient HOME to exist (CI sandboxes sometimes unset
    // it). `run_hook` does not take env overrides, so spawn directly.
    let home = tempfile::tempdir().expect("fake home");
    let home_str = home.path().display().to_string();
    let payload = prompt_payload(
        "codex-s-5", "codex-t-5", &format!("fix {home_str}/work/notes.txt then run tests"),
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(["codex-hook", "UserPromptSubmit"])
        .env("PHRONESIS_PROJECT_ROOT", project.path())
        .env("HOME", home.path())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().expect("spawn codex hook");
    child.stdin.take().expect("stdin")
        .write_all(payload.to_string().as_bytes()).expect("write payload");
    let out = child.wait_with_output().expect("wait");
    assert!(out.status.success());

    let journal =
        fs::read_to_string(project.path().join(".phronesis/journey/events.jsonl")).expect("journal");
    // Both halves: the raw path, and the part of the text that *survives*
    // scrubbing. Asserting only the path would pass even if the whole scrubbed
    // prompt were journaled.
    assert!(!journal.contains("notes.txt"), "{journal}");
    assert!(!journal.contains("then run tests"), "{journal}");
    // Nor may it reach stdout, where it would become injected context.
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("then run tests"), "{stdout}");

    let entry = log_event(project.path(), "prompt");
    let text = entry["prompt"].as_str().expect("prompt text");
    assert!(text.contains("then run tests"), "{text}");
    assert!(!text.contains(&home_str), "{text}");
    assert!(entry["prompt_bytes"].as_u64().expect("bytes") > 0);
    assert_eq!(entry["mode"], "fresh");
    assert_eq!(entry["turn_id"], "codex-t-5");

    // The two files join on (sid, seq) — spec §"Action log".
    let record = lifecycle_records(project.path()).into_iter()
        .rfind(|r| r["kind"] == "prompt").expect("a prompt record");
    assert_eq!(record["sid"], entry["sid"]);
    assert_eq!(record["seq"], entry["seq"]);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --test codex_hook_integration prompt_modes prompt_text_is_scrubbed 2>&1 | tail -30`
Expected: FAIL — no `prompt` lifecycle records exist.

- [ ] **Step 3: Implement**

Replace the `UserPromptSubmit` arm in `dispatch`:

```rust
        "UserPromptSubmit" | "user-prompt-submit" => {
            record_prompt(payload, root);
            make_ctx_decision(root, ContextKind::InteractionContext).await
        }
```

and add these imports, whose first use is here (Task 2 deliberately left them out so its commit passed `-D warnings`):

```rust
use crate::lifecycle::event::Mode;
use crate::lifecycle::scrub;
```

```rust
fn unix_secs_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Classify the prompt, record any inferred interrupt, record the prompt with
/// its scrubbed text, and open the turn. Spec §Classification.
fn record_prompt(payload: &CodexPayload, root: &Path) {
    let now = unix_secs_now();
    // Read before `classify_prompt`, which may rewrite the file.
    let turn = state::read_turn(root);
    let context = state::PromptContext {
        host: Host::Codex,
        now,
        agent_id: payload.agent_id.as_deref(),
        turn_id: payload.turn_id.as_deref(),
        // Codex needs no transcript scan: its Interrupt hook is ground truth,
        // and step 1 reads the result from `turn.last_event`.
        transcript_path: None,
    };
    let classification = state::classify_prompt(root, &context);

    // Codex fires UserPromptSubmit with the *running* turn's id for a message
    // queued mid-turn, which is spec step 3's second sufficient signal for
    // `mid_turn`. The `inflight` branch is off on Codex, so an open turn with no
    // interrupt evidence already classifies `mid_turn` and the two signals
    // agree; the assertion is here rather than an override, so a future
    // divergence is loud instead of silently papered over.
    debug_assert!(
        !(turn.open
            && payload.turn_id.is_some()
            && turn.turn_id.as_deref() == payload.turn_id.as_deref()
            && classification.mode != Mode::MidTurn
            && classification.interrupt.is_none()),
        "a message queued in the running turn classified as {:?}",
        classification.mode
    );

    // `Some(source)` means this handler must write the record. `None` with mode
    // `Correction` means the `Interrupt` arm already wrote it, and a second
    // record would double-count the friction. On Codex today only the second
    // case occurs.
    if let Some(source) = classification.interrupt {
        record(
            root,
            lifecycle_event(payload, Kind::Interrupt)
                .with_extra("inferred_from", source.as_str()),
        );
    }

    let mut event = lifecycle_event(payload, Kind::Prompt).with_mode(classification.mode);
    if let Some(text) = payload.prompt.as_deref().filter(|t| !t.is_empty()) {
        event = event.with_prompt(scrub::scrub_prompt(root, text));
    }
    record(root, event);
    // A prompt carrying an `agent_id` never writes `turn` — a sub-agent's prompt
    // must not move the parent's turn state (spec §Correlation state). Codex
    // does not deliver prompts inside sub-agents today; the guard costs one line
    // and means this adapter does not have to be revisited if it starts.
    if payload.agent_id.as_deref().filter(|s| !s.is_empty()).is_none() {
        state::open_turn(root, payload.turn_id.as_deref(), now);
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --test codex_hook_integration 2>&1 | tail -30`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/codex_hook.rs crates/phronesis-mcp/tests/codex_hook_integration.rs
git commit -m "feat(codex): classify and record UserPromptSubmit with scrubbed prompt text"
```

---

### Task 5: `SubagentStart`, `SubagentStop`, and `Stop`

**Files:**
- Modify: `crates/phronesis-mcp/src/codex_hook.rs` (`dispatch` subagent and stop arms)
- Test: `crates/phronesis-mcp/tests/codex_hook_integration.rs`

**Interfaces:**
- Consumes: `lifecycle::state::{push_agent, pop_agent, OpenAgent, close_turn}`, `record(root, event) -> Option<Stamped>` with `Stamped { ts, sid, seq, kalpa, subject }`, and the existing `make_completion_decision(root)`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn subagent_start_and_stop_pair_with_duration_and_unmatched_stop_does_not() {
    let project = tempfile::tempdir().expect("temp project");
    let start = json!({
        "hook_event_name": "SubagentStart", "session_id": "codex-s-6",
        "turn_id": "codex-t-6", "agent_id": "codex-a-1", "agent_type": "reviewer"
    });
    assert_eq!(response(&run_hook(project.path(), &start)), json!({}));
    let stop = json!({
        "hook_event_name": "SubagentStop", "session_id": "codex-s-6",
        "turn_id": "codex-t-6", "agent_id": "codex-a-1", "agent_type": "reviewer",
        "agent_transcript_path": "/tmp/p/.codex/agents/codex-a-1.jsonl",
        "last_assistant_message": "done reviewing", "stop_hook_active": false
    });
    assert_eq!(response(&run_hook(project.path(), &stop)), json!({}));

    let recs = lifecycle_records(project.path());
    assert_eq!(recs[0]["kind"], "subagent_start");
    assert_eq!(recs[0]["agent"], "codex-a-1");
    assert_eq!(recs[0]["agent_type"], "reviewer");
    assert!(recs[0]["tags"].as_array().expect("tags")
        .contains(&json!("lifecycle:agent:reviewer")));
    assert_eq!(recs[1]["kind"], "subagent_stop");

    let entry = log_event(project.path(), "subagent_stop");
    assert_eq!(entry["matched_start"], true);
    assert_eq!(entry["stop_hook_active"], false);
    assert!(entry["duration_secs"].is_u64());
    assert_eq!(entry["agent_id"], "codex-a-1");
    // The journal never carries the sub-agent's last assistant message.
    let journal =
        fs::read_to_string(project.path().join(".phronesis/journey/events.jsonl")).expect("journal");
    assert!(!journal.contains("done reviewing"), "{journal}");

    // A stop with no matching start is still recorded, without a duration.
    let ghost = json!({
        "hook_event_name": "SubagentStop", "session_id": "codex-s-6",
        "agent_id": "codex-a-ghost", "stop_hook_active": false
    });
    assert_eq!(response(&run_hook(project.path(), &ghost)), json!({}));
    let unmatched = lifecycle_log(project.path()).into_iter()
        .filter(|e| e["event"] == "subagent_stop").next_back().expect("second stop");
    assert_eq!(unmatched["matched_start"], false);
    assert!(unmatched.get("duration_secs").is_none());
}

/// A bare project has no confidence scoring, so `make_completion_decision`
/// never blocks and both invocations record. The blocking case is the test
/// below this one.
#[test]
fn stop_closes_the_turn_and_records_stop_hook_active() {
    let project = tempfile::tempdir().expect("temp project");
    let prompt = prompt_payload("codex-s-8", "codex-t-8", "go");
    assert!(run_hook(project.path(), &prompt).status.success());
    let stop = json!({
        "hook_event_name": "Stop", "session_id": "codex-s-8",
        "turn_id": "codex-t-8", "stop_hook_active": false
    });
    assert_eq!(response(&run_hook(project.path(), &stop)), json!({}));
    assert_eq!(turn_file(project.path())["open"], false);
    assert_eq!(turn_file(project.path())["last_event"], "stop");

    let mut reentrant = stop.clone();
    reentrant["stop_hook_active"] = json!(true);
    assert_eq!(response(&run_hook(project.path(), &reentrant)), json!({}));
    let flags: Vec<Value> = lifecycle_log(project.path()).into_iter()
        .filter(|e| e["event"] == "stop").map(|e| e["stop_hook_active"].clone()).collect();
    assert_eq!(flags, vec![json!(false), json!(true)]);

    // A prompt after a Stop starts a fresh turn.
    assert!(run_hook(project.path(), &prompt).status.success());
    let modes: Vec<String> = lifecycle_records(project.path()).iter()
        .filter(|r| r["kind"] == "prompt")
        .map(|r| r["mode"].as_str().unwrap_or_default().to_string()).collect();
    assert_eq!(modes, vec!["fresh", "fresh"]);
}

/// A sub-agent finishing does not end the human's turn. The two events share a
/// dispatch arm in the code this task rewrites, so the distinction is one merge
/// away from being lost — and losing it means every prompt after a sub-agent
/// returns classifies `fresh`, erasing the intervention count.
#[test]
fn subagent_stop_never_closes_the_turn() {
    let project = tempfile::tempdir().expect("temp project");
    assert!(run_hook(project.path(), &prompt_payload("codex-s-11", "codex-t-11", "go"))
        .status.success());
    let start = json!({
        "hook_event_name": "SubagentStart", "session_id": "codex-s-11",
        "turn_id": "codex-t-11", "agent_id": "codex-a-2", "agent_type": "reviewer"
    });
    assert!(run_hook(project.path(), &start).status.success());
    let stop = json!({
        "hook_event_name": "SubagentStop", "session_id": "codex-s-11",
        "turn_id": "codex-t-11", "agent_id": "codex-a-2", "stop_hook_active": false
    });
    assert_eq!(response(&run_hook(project.path(), &stop)), json!({}));

    assert_eq!(turn_file(project.path())["open"], true, "the human's turn is still running");
    assert_eq!(turn_file(project.path())["last_event"], "prompt");
    // Which is what makes the next prompt an intervention rather than a reply.
    assert!(run_hook(project.path(), &prompt_payload("codex-s-11", "codex-t-11", "also"))
        .status.success());
    let modes: Vec<String> = lifecycle_records(project.path()).iter()
        .filter(|r| r["kind"] == "prompt")
        .map(|r| r["mode"].as_str().unwrap_or_default().to_string()).collect();
    assert_eq!(modes, vec!["fresh", "mid_turn"]);
}

/// A blocked stop is not a stop: it records nothing, leaves the turn open, and
/// leaves the `agents` entry for the real stop to pop. The re-fire, which skips
/// the gate because `stop_hook_active` is true, records exactly one.
#[test]
fn a_blocked_stop_records_nothing_and_the_refire_records_one() {
    let project = tempfile::tempdir().expect("temp project");
    fs::create_dir_all(project.path().join(".phronesis/outcomes")).expect("outcomes dir");
    fs::write(project.path().join(".phronesis/confidence.json"), "{}").expect("confidence");
    fs::write(project.path().join(".phronesis/outcomes/current"), "unit-1").expect("current");
    assert!(run_hook(project.path(), &prompt_payload("codex-s-12", "codex-t-12", "go"))
        .status.success());

    let stop = json!({
        "hook_event_name": "Stop", "session_id": "codex-s-12",
        "turn_id": "codex-t-12", "stop_hook_active": false
    });
    let blocked = response(&run_hook(project.path(), &stop));
    assert_eq!(blocked["decision"], "block", "{blocked}");
    assert!(
        !lifecycle_kinds(project.path()).contains(&"stop".to_string()),
        "a blocked stop records nothing"
    );
    assert_eq!(turn_file(project.path())["open"], true, "the turn continues");

    let mut refire = stop.clone();
    refire["stop_hook_active"] = json!(true);
    assert_eq!(response(&run_hook(project.path(), &refire)), json!({}), "the gate is skipped");
    assert_eq!(
        lifecycle_kinds(project.path()).iter().filter(|k| *k == "stop").count(),
        1
    );
    assert_eq!(turn_file(project.path())["open"], false);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --test codex_hook_integration subagent_start_and_stop stop_closes 2>&1 | tail -30`
Expected: FAIL — no sub-agent or stop lifecycle records exist.

- [ ] **Step 3: Implement**

Replace the `SubagentStart` arm and split the combined `SubagentStop`/`Stop` arm:

```rust
        "SubagentStart" | "subagent-start" => {
            // Record first so the Stamped seq is available for the synthesized
            // id fallback (spec §Correlation state). Codex always supplies
            // agent_id today; the fallback keeps `agents` poppable if it stops.
            let stamped = record(root, lifecycle_event(payload, Kind::SubagentStart));
            let agent_id = payload.agent_id.clone().unwrap_or_else(|| match &stamped {
                Some(s) => format!("{}:{}", s.sid, s.seq),
                None => format!("codex:{}", unix_secs_now()),
            });
            state::push_agent(
                root,
                state::OpenAgent {
                    agent_id,
                    agent_type: payload.agent_type.clone(),
                    ts: stamped.as_ref().map_or_else(unix_secs_now, |s| s.ts),
                    seq: stamped.as_ref().map_or(0, |s| s.seq),
                },
            );
            make_ctx_decision(root, ContextKind::SubagentStart).await
        }
        // NOTE the shape of both arms: the decision is made FIRST, and the
        // record is written only when it does not block. A blocked stop means
        // the host continues the same turn, so it is not a stop — recording one
        // would close a turn that is still running and the next steer would
        // classify `fresh`, losing the intervention (spec §"Host adapters /
        // Codex CLI", and the same rule as on Claude).
        //
        // `stop_hook_active: true` skips the gate entirely, as the docs require,
        // so a blocking gate cannot loop; the re-fire is the invocation that
        // records.
        "SubagentStop" | "subagent-stop" => {
            let decision = completion_decision_for_stop(root, payload);
            if !blocks(&decision) {
                let opened = state::pop_agent(root, payload.agent_id.as_deref());
                let now = unix_secs_now();
                let mut event = lifecycle_event(payload, Kind::SubagentStop)
                    .with_extra("matched_start", opened.is_some())
                    .with_extra("stop_hook_active", payload.stop_hook_active.unwrap_or(false));
                if let Some(open) = &opened {
                    event = event.with_extra("duration_secs", now.saturating_sub(open.ts));
                    // Backfill identity the stop payload omitted.
                    if event.agent_id.is_none() {
                        event.agent_id = Some(open.agent_id.clone());
                    }
                    if event.agent_type.is_none() {
                        event.agent_type = open.agent_type.clone();
                    }
                }
                record(root, event);
            }
            // **`SubagentStop` never closes `turn`.** A sub-agent finishing does
            // not end the human's turn; only the main-agent `Stop` does. The two
            // arms used to share a body, which is exactly how this gets lost.
            decision
        }
        "Stop" | "stop" => {
            let decision = completion_decision_for_stop(root, payload);
            if !blocks(&decision) {
                // Close before recording so a concurrent prompt hook cannot read
                // the turn as still open.
                state::close_turn(root, "stop");
                record(
                    root,
                    lifecycle_event(payload, Kind::Stop)
                        .with_extra("stop_hook_active", payload.stop_hook_active.unwrap_or(false)),
                );
            }
            decision
        }
```

and add the two helpers beside `lifecycle_event`:

```rust
/// The completion decision for a stop event. `stop_hook_active: true` skips the
/// gate entirely and responds `{}`, as the host docs require: without it a
/// blocking gate re-fires forever.
fn completion_decision_for_stop(root: &Path, payload: &CodexPayload) -> String {
    if payload.stop_hook_active.unwrap_or(false) {
        return empty_decision();
    }
    make_completion_decision(root)
}

/// Did the rendered decision block? Structural, not a substring test: a `reason`
/// string that merely contains the word would false-positive.
fn blocks(response: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(response)
        .ok()
        .and_then(|v| v.get("decision").and_then(|d| d.as_str()).map(str::to_string))
        .as_deref()
        == Some("block")
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --test codex_hook_integration 2>&1 | tail -30`
Expected: pass, including the pre-existing confidence-gate test that drives `Stop` and `SubagentStop`.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/codex_hook.rs crates/phronesis-mcp/tests/codex_hook_integration.rs
git commit -m "feat(codex): record sub-agent start/stop pairing and turn stops"
```

---

### Task 6: Response permitted-key contract

**Files:**
- Modify: `crates/phronesis-mcp/src/codex_hook/renderer.rs` (`mod tests`)

**Interfaces:**
- Consumes: `renderer::render_codex_response(event: &str, decision: &CodexDecision) -> String`.

Codex output schemas are `deny_unknown_fields`; an extra key fails the whole hook. This task pins the spec's table. `Stop`/`SubagentStop` must have **no** `hookSpecificOutput` — `render_completion` already satisfies that, and this test is what keeps it true.

- [ ] **Step 1: Write the failing test** (append to `renderer.rs`'s `mod tests`)

```rust
    /// Permitted stdout keys per event, from SPEC-agent-lifecycle-events
    /// §"Host adapters / Codex CLI". Codex rejects the whole response on any
    /// unlisted key. PreCompact/PostCompact are deliberately absent: today's
    /// renderer violates their schemas (spec Adjacent finding 1, separate PR).
    fn permitted_keys(event: &str) -> &'static [&'static str] {
        const CTX: &[&str] = &[
            "continue", "stopReason", "suppressOutput", "systemMessage", "hookSpecificOutput",
        ];
        match event {
            "SessionStart" | "SubagentStart" => CTX,
            "UserPromptSubmit" => &[
                "continue", "stopReason", "suppressOutput", "systemMessage",
                "hookSpecificOutput", "decision", "reason",
            ],
            // The spec's table says "as today (decision, reason,
            // hookSpecificOutput)" for these two. `systemMessage` is added
            // deliberately, not copied: `render_post` already emits it and the
            // Codex PostToolUse schema permits it. Keep the deliberate widening
            // documented here rather than silently inside the table.
            "PreToolUse" | "PostToolUse" => {
                &["decision", "reason", "systemMessage", "hookSpecificOutput"]
            }
            "Stop" | "SubagentStop" => &[
                "continue", "decision", "reason", "stopReason", "suppressOutput", "systemMessage",
            ],
            "Interrupt" => &["systemMessage"],
            "SessionEnd" => &[],
            other => panic!("no permitted-key set for {other}"),
        }
    }

    fn decision(block: &[&str], warn: &[&str], context: &str) -> CodexDecision {
        CodexDecision {
            block_messages: block.iter().map(|s| s.to_string()).collect(),
            warn_messages: warn.iter().map(|s| s.to_string()).collect(),
            additional_context: context.to_string(),
            files: Vec::new(),
        }
    }

    #[test]
    fn every_event_response_stays_within_its_permitted_keys() {
        let cases = [
            decision(&[], &[], ""),
            decision(&["blocked"], &[], ""),
            decision(&[], &["warned"], ""),
            decision(&[], &[], "## Rules\n- rule-a"),
            decision(&["blocked"], &["warned"], "## Rules\n- rule-a"),
        ];
        for event in [
            "PreToolUse", "PostToolUse", "SessionStart", "UserPromptSubmit", "SubagentStart",
            "SubagentStop", "Stop", "Interrupt", "SessionEnd",
        ] {
            for d in &cases {
                let json = render_codex_response(event, d);
                let value: serde_json::Value =
                    serde_json::from_str(&json).unwrap_or_else(|e| panic!("{event}: {json}: {e}"));
                let obj = value.as_object().unwrap_or_else(|| panic!("{event}: {json}"));
                for key in obj.keys() {
                    assert!(
                        permitted_keys(event).contains(&key.as_str()),
                        "{event} response key `{key}` is not permitted: {json}"
                    );
                }
            }
        }
    }

    #[test]
    fn completion_events_never_emit_hook_specific_output() {
        for event in ["Stop", "SubagentStop", "stop", "subagent-stop"] {
            for d in [
                decision(&["Low confidence for unit-1"], &[], ""),
                decision(&[], &["Medium confidence for unit-1"], ""),
                decision(&[], &[], "context that must be dropped"),
            ] {
                let json = render_codex_response(event, &d);
                // Structural, not substring: a `reason` string that merely
                // mentioned the word would false-fail a containment check.
                let v: serde_json::Value = serde_json::from_str(&json)
                    .unwrap_or_else(|e| panic!("{event}: {json}: {e}"));
                assert!(v.get("hookSpecificOutput").is_none(), "{event}: {json}");
            }
        }
    }

    /// The permitted-key test alone would still pass if the block vanished
    /// entirely (`{}` is within every key set). This pins that a blocking
    /// decision actually blocks and carries its reason.
    #[test]
    fn a_blocking_completion_decision_still_reaches_the_host() {
        let d = decision(&["Low confidence for unit-1"], &[], "");
        for event in ["Stop", "SubagentStop"] {
            let json = render_codex_response(event, &d);
            assert_ne!(json, "{}", "{event} dropped the block");
            let v: serde_json::Value = serde_json::from_str(&json).expect("JSON");
            let obj = v.as_object().expect("object");
            for key in obj.keys() {
                assert!(permitted_keys(event).contains(&key.as_str()), "{event}: {key}: {json}");
            }
            assert!(
                ["reason", "stopReason", "systemMessage"].iter().any(|k| {
                    obj.get(*k).and_then(|x| x.as_str()).is_some_and(|s| s.contains("unit-1"))
                }),
                "{event}: the gate text must survive somewhere: {json}"
            );
        }
    }

    #[test]
    fn interrupt_and_session_end_render_empty() {
        let d = decision(&["ignored"], &["ignored"], "ignored");
        for event in ["Interrupt", "interrupt", "SessionEnd", "session-end"] {
            assert_eq!(render_codex_response(event, &d), "{}");
        }
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp codex_hook::renderer 2>&1 | tail -30`
Expected: **PASS.** Step 1 adds the helpers and the tests together, so there is no red
state to observe. This task is a characterization test by design: the renderer already
conforms once Task 2's explicit `Interrupt`/`SessionEnd` arm exists, and these tests
are what keep it conforming. To satisfy yourself they can fail, temporarily add
`"decision"` to the `Interrupt` row of `permitted_keys`, invert one assertion, watch
it fail, and revert — do not commit that.

- [ ] **Step 3: Confirm (no production change expected)**

The renderer already produces conforming output for every event in the table once Task 2 added the explicit `Interrupt`/`SessionEnd` arm; this step confirms it. Conforming shapes today: `render_completion` emits only `continue`/`stopReason`/`systemMessage`; `render_context` emits only `hookSpecificOutput`; `render_post` emits `systemMessage` and `hookSpecificOutput`; `render_pre` emits only `hookSpecificOutput`. If the test reports a violation, fix `renderer.rs` — never widen `permitted_keys`, which is copied verbatim from the spec.

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp codex_hook::renderer 2>&1 | tail -20`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/codex_hook/renderer.rs
git commit -m "test(codex): pin per-event permitted response keys"
```

---

### Task 7: `init` registers `Interrupt` and `SessionEnd`; `SessionStart` matcher `""`

**Files:**
- Modify: `crates/phronesis-mcp/src/init.rs:735-757` (the `write_codex_hooks` registration loop)
- Test: `crates/phronesis-mcp/tests/codex_hook_integration.rs:545-648` (`init_merges_codex_hooks_and_mcp_idempotently_and_dry_run_is_read_only`)

**Interfaces:**
- Consumes: `upsert_codex_hook(settings: &mut Value, event: &str, new_entry: Value)` — command-keyed, so a user's own hook with the same matcher survives.

- [ ] **Step 1: Write the failing test**

In that test, replace the `SessionStart` matcher assertion:

```rust
    assert!(session.iter().any(|entry| {
        // Codex matchers are exact alternations, so "startup|resume|clear"
        // silently skipped compact and fork sessions. Empty matches every source.
        entry["matcher"] == ""
            && entry["hooks"][0]["command"] == "phr-mcp codex-hook SessionStart"
    }));
```

and append, after the existing `["Stop", "SubagentStop"]` loop:

```rust
    for event in ["Interrupt", "SessionEnd"] {
        let entries = hooks["hooks"][event]
            .as_array()
            .unwrap_or_else(|| panic!("{event} hooks"));
        assert!(
            entries.iter().any(|entry| {
                entry["matcher"] == ""
                    && entry["hooks"][0]["command"] == format!("phr-mcp codex-hook {event}")
            }),
            "{event} must be registered with an empty matcher"
        );
        assert_eq!(
            entries.iter()
                .filter(|entry| {
                    entry["hooks"][0]["command"] == format!("phr-mcp codex-hook {event}")
                })
                .count(),
            1,
            "{event} must be registered exactly once"
        );
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --test codex_hook_integration init_merges_codex_hooks 2>&1 | tail -20`
Expected: FAIL — `SessionStart` still uses `startup|resume|clear`, and `hooks.Interrupt` is not an array.

- [ ] **Step 3: Implement**

In `write_codex_hooks`, change the loop's table:

```rust
    for (event, matcher) in [
        // Codex matchers are exact alternations; "startup|resume|clear" gave
        // compact and fork sessions no context. Empty matches every source.
        ("SessionStart", ""),
        ("SessionEnd", ""),
        ("UserPromptSubmit", ""),
        ("PreCompact", "manual|auto"),
        ("PostCompact", "manual|auto"),
        ("SubagentStart", ""),
        ("SubagentStop", ""),
        ("Stop", ""),
        // Interrupt ignores `matcher` entirely; the empty value is documentation.
        ("Interrupt", ""),
    ] {
```

Then extend **only the `"codex"` array** in `crates/phronesis-mcp/tests/fixtures/hook_events.json`, or `payload_contract.rs::init_wires_hooks_only_under_event_names_that_exist` fails with `init wired unknown Codex hook event "Interrupt"`. Plan 2 edits the `"claude-code"` line and Plan 4 the `"gemini"` line, so the three-way merge is line-disjoint:

```json
  "codex": ["PreToolUse", "PostToolUse", "SessionStart", "SessionEnd", "UserPromptSubmit", "PreCompact", "PostCompact", "SubagentStart", "SubagentStop", "Stop", "Interrupt"]
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --test codex_hook_integration --test payload_contract --test init_integration 2>&1 | tail -30`
Expected: pass. `payload_contract.rs:407` reads `.codex/hooks.json`; if it asserts the old matcher, update that assertion too.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/init.rs crates/phronesis-mcp/tests/codex_hook_integration.rs crates/phronesis-mcp/tests/payload_contract.rs crates/phronesis-mcp/tests/fixtures/hook_events.json
git commit -m "feat(init): register Codex Interrupt and SessionEnd; widen the SessionStart matcher"
```

---

### Task 8: `Interrupt` payload contract fixture

**Files:**
- Create: `crates/phronesis-mcp/tests/fixtures/payloads/codex/interrupt.json`
- Test: `crates/phronesis-mcp/tests/payload_contract.rs` (data-driven; discovers fixtures under `tests/fixtures/payloads/<cli>/`)

**Interfaces:**
- Consumes: the `Fixture`/`Expect` envelope in `tests/payload_contract.rs:11-43` — `schema`, `source`, `subcommand`, `packs`, `payload`, `expect{exit, stdout_json, log_rule_fired, journal_tag_new, journal_tag_from_output, stderr_contains}`.

- [ ] **Step 1: Write the fixture**

```json
{
  "schema": 1,
  "source": {
    "cli": "codex",
    "event": "Interrupt",
    "provenance": "authored",
    "captured": "2026-09-18",
    "description": "Authored from the Codex Interrupt hook schema (codex-rs/hooks/schema/generated). Field set pinned by SPEC-agent-lifecycle-events: cwd, hook_event_name, model, permission_mode, session_id, transcript_path, turn_id."
  },
  "subcommand": "codex-hook",
  "packs": "rust",
  "payload": {
    "hook_event_name": "Interrupt",
    "cwd": "/home/dev/project",
    "model": "gpt-5-codex",
    "permission_mode": "on-request",
    "session_id": "codex-s-030",
    "transcript_path": "/home/dev/.codex/sessions/codex-s-030.jsonl",
    "turn_id": "codex-t-030"
  },
  "expect": {
    "exit": 0,
    "stdout_json": true,
    "log_rule_fired": null,
    "journal_tag_new": ["lifecycle:interrupt"],
    "journal_tag_from_output": [],
    "stderr_contains": []
  }
}
```

- [ ] **Step 2: Run to verify it is exercised**

Run: `cargo test -p phronesis-mcp --test payload_contract 2>&1 | tail -30`
Expected: the runner picks up `codex/interrupt.json` and passes. If `journal_tag_new` is asserted over tool records only, read the runner's `journal_tag_new` implementation (search `journal_tag_new` in `tests/payload_contract.rs`) and, if it filters to `kind: None` records, widen it to all fresh records with a one-line comment: lifecycle records are journal records too.

- [ ] **Step 3: Confirm the field set matches what the adapter reads**

Run: `cargo test -p phronesis-mcp --test codex_hook_integration interrupt_records 2>&1 | tail -10`
Expected: pass — the fixture's field names are exactly the ones `CodexPayload` deserializes.

- [ ] **Step 4: Commit**

```bash
git add crates/phronesis-mcp/tests/fixtures/payloads/codex/interrupt.json crates/phronesis-mcp/tests/payload_contract.rs
git commit -m "test(codex): payload contract fixture for the Interrupt hook"
```

---

### Task 9: Codex hooks spec and changelog

**Files:**
- Modify: `docs/specs/SPEC-codex-hooks-integration.md:60-73` (event mapping table), `:85-90` (supported events)
- Modify: `CHANGELOG.md` (`## [Unreleased]` → `### Added`)

- [ ] **Step 1: Update the event mapping table**

Add after the `SubagentStop / Stop` row:

```markdown
| `Interrupt` | lifecycle record | record the abort, close the turn, respond `{}`; its schema permits `systemMessage` only |
| `SessionEnd` | lifecycle record | record a `stop` when a turn is still open, clear the session file, respond `{}` |
```

and this paragraph below the table:

```markdown
`Stop` does not fire when a turn is aborted; `Interrupt` does, before
`TurnAborted` and with the transcript flushed. A missing `Stop` therefore never
means "the turn completed". `SessionStart`, `SessionEnd`, `UserPromptSubmit`,
`SubagentStart`, `SubagentStop`, `Stop`, and `Interrupt` also write lifecycle
records; see `docs/specs/SPEC-agent-lifecycle-events.md`.
```

- [ ] **Step 2: Update the supported-events list**

```text
pre-tool-use | post-tool-use | session-start | session-end |
user-prompt-submit | pre-compact | post-compact | subagent-start |
subagent-stop | stop | interrupt
```

- [ ] **Step 3: Add the changelog entry** under `## [Unreleased]` / `### Added`

```markdown
- **Lifecycle events, Codex adapter.** `phr-mcp codex-hook` now records
  sub-agent start/stop (with pairing and duration), prompts (with `fresh` /
  `mid_turn` / `correction` mode and the scrubbed text in
  `.phronesis/log.jsonl`), interrupts, and turn stops. `Interrupt` and
  `SessionEnd` are handled and registered by `phr-mcp init`; the `SessionStart`
  matcher is now empty, so compact and fork sessions also get context. Codex
  tool records take their session id from the shared
  `.phronesis/journey/session` file, agreeing with the other hosts. Payloads
  teed to `PHRONESIS_CAPTURE_DIR` have their prompt text redacted.
```

- [ ] **Step 4: Full verification**

Run: `cargo fmt --all -- --check && cargo clippy --all-targets -p phronesis-mcp -- -D warnings && cargo test -p phronesis-mcp 2>&1 | tail -30`
Expected: clean, all green.

- [ ] **Step 5: Commit**

```bash
git add docs/specs/SPEC-codex-hooks-integration.md CHANGELOG.md
git commit -m "docs(codex): document Interrupt and SessionEnd handling and the abort contract"
```

---

## Self-review

**1. Spec coverage (§"Host adapters / Codex CLI", rollout step 3).** All eight `CodexPayload` fields, `source` included (T1). Capture tee with the recursive redaction (T1). `Interrupt` → record with `inferred_from: "hook"`, set `turn` to `{open: false, last_event: "interrupt"}`, **drop the session's `inflight` entries**, `{}` (T2). `SessionEnd` → conditional `stop`, close turn, **`session` left in place**, `{}` (T2). `SessionStart` is **source-gated** and resets correlation state only on a begin, which is what makes T7's matcher widening safe (T3). `UserPromptSubmit` classifies from `turn.last_event`, records an interrupt only for `Some(source)` (never reachable on Codex today, written generically), records the prompt with mode and scrubbed text, and opens the turn only for a top-level prompt (T4). `SubagentStart`/`SubagentStop` push/pop with `duration_secs`, `matched_start`, `stop_hook_active`; **`SubagentStop` never closes the turn**; both stop events skip the gate on `stop_hook_active` and **record nothing when the response blocks** (T5). `Stop` closes the turn and records, then keeps `make_completion_decision` (T5). Renderer emits `{}` for both new events and the permitted-key table is pinned, including "no `hookSpecificOutput` on completion events" (T2, T6). Journal `sid` from `journey::current_sid`, records at `v: JOURNAL_V` (T3). Init registrations and matcher (T7). Contract fixture (T8). Docs (T9).

Out of scope by design: the `PreCompact`/`PostCompact` response bug (spec Adjacent finding 1, separate PR); Claude and Gemini adapters (Plans 2 and 4); stats, metrics, `journey --lifecycle`/`--corrections` (Plan 5); `inflight` push/pop in `pre.rs`/`post.rs`, which Plan 2 owns. This plan never reads `inflight` for classification — the spec disables that branch on Codex, where the `Interrupt` hook is authoritative — and only *clears* it, in the `Interrupt` arm. On a Codex-only install an open turn with no `Interrupt` classifies `mid_turn`, exactly as the spec's known-limits section allows.

**2. Placeholder scan.** No TBDs. Every code step carries compilable Rust or literal JSON. T6's implement step is a verification step by construction — the renderer already conforms after T2 — and states exactly what to do on a violation (fix the renderer, never the table). T8 names the exact function to read if the contract runner needs widening.

**3. Type consistency.** Only Plan 1 names, with Plan 1's **revised** signatures: the `LifecycleEvent` builder (`new`/`with_mode`/`with_session`/`with_turn`/`with_agent`/`with_prompt`/`with_extra`, by value; `with_agent` sanitizes `agent_type`) plus its public `agent_id`/`agent_type` fields for T5's backfill; `record(root, event) -> Option<Stamped>` with `Stamped { ts, sid, seq, kalpa, subject }`; `state::OpenAgent { agent_id, agent_type, ts, seq }`; `state::Inflight { key, tool, ts, agent_id, head_before, detection }`; `state::PromptContext { host, now, agent_id, turn_id, transcript_path }` — **five fields**; `state::Classification { mode, interrupt }`; `state::InterruptSource::{Inflight, Transcript, OpenTurn}` — **no `Hook`**; `state::{read_turn, open_turn, close_turn, set_session, is_session_begin, reset_for_session_start, clear_inflight, push_agent, pop_agent}`; `scrub::scrub_prompt(root, text)`; `hook::{capture_raw_payload, redact_for_capture}`. `close_turn` returns `bool`; this adapter ignores it, because every call site here is already on the conservative side. `lifecycle_event` is introduced in T2 and used unchanged in T3–T5; `unix_secs_now` and `record_prompt` are T4's, `completion_decision_for_stop` and `blocks` are T5's. Test helpers `journal_records`, `lifecycle_records`, `lifecycle_kinds`, `lifecycle_log`, `log_event`, `turn_file`, `prompt_payload` are defined once in T2 and reused by T3–T5, all inside `tests/codex_hook_integration.rs`, which no other plan touches. (Plan 4 defines same-named helpers in `tests/hook_integration.rs`; different file, no collision.)

**4. Shared ownership.** This plan defines nothing that another plan defines. It does not touch `hook/mod.rs`, does not define `upsert_hook_by_command`, and does not add a `main.rs` `Command` variant.

---

## Merge notes

Plans 2, 3, and 4 are executed in separate worktrees off the same Plan 1 base. Every file more than one of them touches is listed here with the exact region each owns.

| file | Plan 2 (Claude) owns | Plan 3 (this plan) owns | Plan 4 (Gemini) owns |
|---|---|---|---|
| `src/init.rs` | `write_settings` (`:580-622`) and the new `upsert_hook_by_command` fn + unit tests added after `upsert_hook` (ends `:1559`) | **`write_codex_hooks`'s `for (event, matcher)` table (`:735-757`) only.** No other line of `init.rs`. | `write_gemini_settings`'s hook block (`:676-706`) and the `report.steps.push` note after `write_json` (`:714`) |
| `src/hook/mod.rs` | one line: `mod lifecycle_wiring;` | **nothing** | nothing |
| `src/main.rs`, `src/lib.rs` | `ClaudeHook` variant + dispatch arm; `pub mod claude_hook;` | nothing | nothing |
| `tests/fixtures/hook_events.json` | the `"claude-code"` array | **the `"codex"` array only** (Task 7) | the `"gemini"` array |
| `tests/payload_contract.rs` | appends `captured_claude_payloads_carry_the_documented_fields` at the end of the file | the `.codex/hooks.json` matcher assertion around `:407`, and — only if the runner filters `journal_tag_new` to `kind: None` records — a one-line widening there (Task 8) | nothing |
| `tests/hook_integration.rs`, `tests/init_integration.rs` | appends its helpers and tests | **nothing** | appends its helpers and tests at the end of the file |
| `CHANGELOG.md` | two bullets under `## [Unreleased]` → `### Added` | one bullet, appended after Plan 2's | one bullet, appended after this plan's |

Ordering: this plan is independent of Plans 2 and 4 and may merge in any position among the three. Plan 4 must merge **after** Plan 2, because it consumes `upsert_hook_by_command` and the `claude-hook` subcommand.

---

### Task 10 (follow-up, after the first merge): `inflight` and commit detection in the Codex tool phases

**Why:** `phr-mcp init` routes Codex `PreToolUse`/`PostToolUse` to `codex-hook`, not to
`pre-check`/`post-check`, so Plan 2's `inflight` push/pop and `detect_commit` never run for
Codex (spec §"Host adapters / Codex CLI", last bullet). Without this Codex records no `commit`
events.

**Files:**
- Modify: `crates/phronesis-mcp/src/codex_hook.rs` (`handle_pre`, `handle_post`, and the blocking path in `handle_pre`)
- Test: `crates/phronesis-mcp/tests/codex_hook_integration.rs`

**Interfaces:**
- Consumes (Plan 1): `lifecycle::state::{push_inflight, pop_inflight, inflight_key_for, Inflight}`, `lifecycle::outcome::{is_shell_tool, command_may_move_head, git_head_probe, HeadProbe, detect_commit, DETECTION_TIMEOUT}`, `lifecycle::record::record`, `lifecycle::{LifecycleEvent, Kind, Host}`.
- Produces: nothing new.

- [ ] **Step 1: Write the failing tests** (append to `tests/codex_hook_integration.rs`, reusing its `run_codex_hook(root, event, payload)` helper and `lifecycle_records(root)` / `log_entries(root)` readers)

```rust
#[test]
fn codex_pre_tool_use_pushes_inflight_and_post_pops_it() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let pre = r#"{"hook_event_name":"PreToolUse","session_id":"c1","turn_id":"t1","tool_use_id":"tu-1","tool_name":"Bash","tool_input":{"command":"echo hi"}}"#;
    run_codex_hook(root, "PreToolUse", pre);
    let inflight = std::fs::read_to_string(root.join(".phronesis/journey/inflight")).unwrap_or_default();
    assert!(inflight.contains(r#""key":"tu-1""#), "{inflight}");
    let post = r#"{"hook_event_name":"PostToolUse","session_id":"c1","turn_id":"t1","tool_use_id":"tu-1","tool_name":"Bash","tool_input":{"command":"echo hi"},"tool_response":{"exit_code":0,"output":"hi"}}"#;
    run_codex_hook(root, "PostToolUse", post);
    let inflight = std::fs::read_to_string(root.join(".phronesis/journey/inflight")).unwrap_or_default();
    assert!(!inflight.contains("tu-1"), "popped: {inflight}");
}

#[test]
fn codex_bash_commit_is_detected_from_head_movement() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_git_repo_with_one_commit(root); // same helper shape as tests/lifecycle_outcome.rs
    let pre = r#"{"hook_event_name":"PreToolUse","session_id":"c1","turn_id":"t1","tool_use_id":"tu-2","tool_name":"Bash","tool_input":{"command":"git commit -am second"}}"#;
    run_codex_hook(root, "PreToolUse", pre);
    std::fs::write(root.join("a"), "2").unwrap();
    git(root, &["commit", "-q", "-am", "second"]);
    let post = r#"{"hook_event_name":"PostToolUse","session_id":"c1","turn_id":"t1","tool_use_id":"tu-2","tool_name":"Bash","tool_input":{"command":"git commit -am second"},"tool_response":{"exit_code":0,"output":""}}"#;
    run_codex_hook(root, "PostToolUse", post);
    let commit = lifecycle_records(root).into_iter().find(|r| r["kind"] == "commit").expect("commit record");
    assert_eq!(commit["host"], "codex");
    let entry = log_entries(root).into_iter().find(|e| e["event"] == "commit").unwrap();
    assert_eq!(entry["sha"].as_str().unwrap().len(), 40);
    assert!(entry["head_before"].is_string());
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p phronesis-mcp --test codex_hook_integration codex_pre_tool_use_pushes codex_bash_commit 2>&1 | tail -20`. Expected: FAIL (inflight file absent; no commit record).

- [ ] **Step 3: Implement** in `codex_hook.rs`. At the top of `handle_pre`, after the payload is parsed and before rule loading:

```rust
    let key = state::inflight_key_for(payload.tool_use_id.as_deref(), tool_name, payload.tool_input.as_ref().unwrap_or(&serde_json::Value::Null));
    let head_before = if outcome::is_shell_tool(tool_name) && outcome::command_may_move_head(&command) {
        match outcome::git_head_probe(root) { HeadProbe::Head(h) => Some(h), _ => None }
    } else { None };
    state::push_inflight(root, Inflight { key: key.clone(), tool: tool_name.to_string(), ts: unix_secs_now(), agent_id: payload.agent_id.clone(), head_before, detection: None });
```

and on the blocking return path of `handle_pre`: `state::pop_inflight(root, &key);`. In `handle_post`, right after parsing:

```rust
    let popped = state::pop_inflight(root, &key);
    if let Some(entry) = popped
        && outcome::is_shell_tool(tool_name)
        && let Some(c) = outcome::detect_commit(root, entry.head_before.as_deref(), &command, extract_command_exit(payload))
    {
        let mut ev = LifecycleEvent::new(Kind::Commit, Host::Codex)
            .with_extra("sha", c.sha).with_extra("head_before", c.head_before).with_extra("tool_use_id", key.clone());
        if let Some(sid) = &payload.session_id { ev = ev.with_session(sid.clone()); }
        if let Some(t) = &payload.turn_id { ev = ev.with_turn(t.clone()); }
        if let Some(b) = crate::outcomes::report(root, None).map(|r| format!("{:?}", r.band).to_lowercase()) { ev = ev.with_extra("confidence_band", b); }
        record(root, ev);
    }
```

`command` is the string `extract_bash_command(payload)` already computes; `extract_command_exit` exists in this file. Mirror Plan 2 Task 4's Claude wiring for any detail not shown here.

- [ ] **Step 4: Run** the two tests plus the whole `codex_hook_integration` suite. Expected: PASS.

- [ ] **Step 5: Commit** — `git commit -m "feat(codex): inflight tracking and commit detection in codex-hook tool phases"`.

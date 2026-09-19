# Lifecycle Events — Plan 2: Claude Code adapter (spec step 2)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Emit lifecycle events from Claude Code (and, for the two agent events Plan 4 registers, Gemini CLI) through a new `phr-mcp claude-hook <Event>` adapter, and wire `pre-check`/`post-check` to the `inflight` correlation file and commit detection.

**Architecture:** One new file, `src/claude_hook.rs`, mirrors `src/codex_hook.rs`: read stdin once, tee it to the capture dir, parse a wholly-optional `ClaudePayload`, dispatch on the event name, print exactly one JSON object, exit 0. Tool phases never reach it — they delegate to the existing `run_pre_check`/`run_post_check` before stdin is touched. The pre/post runners gain a small sibling module, `src/hook/lifecycle_wiring.rs`, that pushes and pops `inflight` and derives Gemini's `invoke_agent` sub-agent pair, so `pre.rs`/`post.rs` each grow only a few lines. Everything routes through Plan 1's `lifecycle::record::record`; no file here writes the journal or action log directly.

**Tech Stack:** Rust 2024 edition (rust-version 1.90), clap, serde/serde_json, tempfile for tests. No new dependencies.

**Spec:** `docs/specs/SPEC-agent-lifecycle-events.md` (revised 2026-09-18), §"Host adapters / Claude Code", §"Classification at prompt time", §"Outcomes and kalpas / Success signal: commit", and the Gemini `invoke_agent` derivation in §"Host adapters / Gemini CLI". Read it first; this plan argues from it.

**Depends on:** `docs/superpowers/plans/2026-09-18-lifecycle-events-1-foundation.md` (Plan 1) must be merged. Every shared name used below comes from it:
`lifecycle::{Host, Kind, LifecycleEvent, Mode, PromptText, Stamped}`, `lifecycle::record::{record, prompt_text_setting}`, `lifecycle::state::{push_inflight, pop_inflight, live_inflight, take_inflight_for_scope, inflight_key_for, Inflight, push_agent, pop_agent, OpenAgent, read_turn, open_turn, close_turn, set_session, clear_session, reset_for_session_start, classify_prompt, PromptContext, Classification, InterruptSource}`, `lifecycle::outcome::{is_shell_tool, git_head, detect_commit, Commit}`, `lifecycle::scrub::scrub_prompt`, `hook::{redact_for_capture, capture_raw_payload}` (Plan 1 Task 10 makes `capture_raw_payload` `pub(crate)` and `redact_for_capture` `pub`; **do not re-make those changes here**), and `HookPayload`'s `session_id` / `tool_use_id` / `hook_event_name` / `agent_id` fields.

**Runs in parallel with:** Plan 3 (Codex) and Plan 4 (Gemini). See §Merge notes at the end of this plan for every file the three share and the exact region each owns.

**Plan 4 depends on this plan** for `init.rs::upsert_hook_by_command` (Task 5 here is its only definition) and for the `claude-hook` adapter its Gemini registrations point at. Land Plan 2 before Plan 4 when the two are merged.

**Files this plan owns exclusively:**

- `crates/phronesis-mcp/src/claude_hook.rs` (create)
- `crates/phronesis-mcp/src/hook/lifecycle_wiring.rs` (create)
- `crates/phronesis-mcp/src/hook/pre.rs`, `crates/phronesis-mcp/src/hook/post.rs`, `crates/phronesis-mcp/src/hook/journey_record.rs`
- `crates/phronesis-mcp/tests/lifecycle_concurrency.rs` (create)
- `crates/phronesis-mcp/tests/fixtures/payloads/claude/**`

**Files shared with Plans 3 and 4** (regions are disjoint; see §Merge notes): `src/init.rs`, `src/main.rs`, `src/lib.rs`, `src/hook/mod.rs`, `tests/hook_integration.rs`, `tests/init_integration.rs`, `tests/payload_contract.rs`, `tests/fixtures/hook_events.json`, `CHANGELOG.md`.

## Global Constraints

- No new crate dependencies.
- Prompt text never enters `JournalRecord`, a `phr::Fact`, a context render, or stdout. Only the action log, via `LifecycleEvent::prompt`, already scrubbed by `scrub_prompt`.
- Every lifecycle write is fail-open: swallow the error, `eprintln!("phronesis: ...")`, continue. A lifecycle failure never changes a hook's exit code.
- `claude-hook` prints exactly one JSON object on stdout and exits 0 for every non-tool event, including on a stdin read or parse failure. A `UserPromptSubmit` hook that exits non-zero discards the human's prompt; that must never happen because of a Phronesis bug.
- `stop_hook_active: true` short-circuits `Stop` and `SubagentStop` to `{}` without evaluating the confidence gate.
- Tool events (`PreToolUse`/`PostToolUse`, and their Gemini aliases `BeforeTool`/`AfterTool`) keep today's exit codes exactly: pre 0/1/2, post 0/1.
- Kalpa names, `inflight` TTL (900 s), and the `__lifecycle` sentinel are Plan 1's; do not restate or re-derive them.
- Conventional-commit messages. Run `cargo fmt` and `cargo clippy --all-targets -p phronesis-mcp -- -D warnings` before every commit.
- Machine note: if `cargo` fails with "You have not agreed to the Xcode license", stop and report; the human must run `sudo xcodebuild -license accept`.

## File structure

| path | responsibility |
|---|---|
| `crates/phronesis-mcp/src/claude_hook.rs` (create) | the whole adapter: `ClaudePayload`, `run`, `dispatch`, per-event handlers, response shapes |
| `crates/phronesis-mcp/src/hook/lifecycle_wiring.rs` (create) | `inflight` push/pop, commit detection, Gemini `invoke_agent` derivation — everything `pre.rs`/`post.rs` need in one place |
| `crates/phronesis-mcp/src/hook/pre.rs` (modify) | push `inflight`, pop on block, `invoke_agent` in the allowlist |
| `crates/phronesis-mcp/src/hook/post.rs` (modify) | pop `inflight` + `detect_commit`, `invoke_agent` in the allowlist |
| `crates/phronesis-mcp/src/hook/mod.rs` (modify) | `mod lifecycle_wiring;` |
| `crates/phronesis-mcp/src/hook/journey_record.rs` (modify) | tool records write `v: JOURNAL_V` |
| `crates/phronesis-mcp/src/main.rs` (modify) | `ClaudeHook { event }` subcommand + dispatch arm |
| `crates/phronesis-mcp/src/lib.rs` (modify) | `pub mod claude_hook;` |
| `crates/phronesis-mcp/src/init.rs` (modify) | `upsert_hook_by_command`, four new Claude registrations, two repointed |
| `crates/phronesis-mcp/tests/fixtures/hook_events.json` (modify) | the four new Claude event names |
| `crates/phronesis-mcp/tests/fixtures/payloads/claude/` (create) | captured payloads: `raw/` (evidence, not collected by the runner) and fixture envelopes |
| `crates/phronesis-mcp/tests/hook_integration.rs` (modify) | adapter shapes, failure policy, inflight, commit, `invoke_agent` |
| `crates/phronesis-mcp/tests/init_integration.rs` (modify) | registrations, migration, foreign-hook survival |
| `crates/phronesis-mcp/tests/lifecycle_concurrency.rs` (create) | N concurrent pre/post pairs + one prompt → never `correction` |
| `crates/phronesis-mcp/tests/payload_contract.rs` (modify) | pin the captured Claude field sets |
| `CHANGELOG.md` (modify) | `[Unreleased] → Added` |

---

### Task 1: Capture real Claude payloads (REQUIRES THE HUMAN)

A worker cannot do this alone: it needs a live Claude Code session driving a real sub-agent and a real interrupt. Everything downstream is written against the documented field names, but the fixtures decide what "missing" means for `prompt_id`, `agent_id`, and `agent_type` (spec §"Payload fixtures are a precondition", Open question 2).

**Files:**
- Create: `crates/phronesis-mcp/tests/fixtures/payloads/claude/raw/{UserPromptSubmit,SessionStart,SessionEnd,SubagentStart,SubagentStop,Stop}.json`
- Create: `crates/phronesis-mcp/tests/fixtures/payloads/claude/raw/README.md`

`tests/payload_contract.rs::collect_fixtures` walks exactly one level (`payloads/<cli>/*.json`), so files under `claude/raw/` are evidence, not fixtures, and do not have to satisfy the fixture envelope. Task 7 promotes them.

- [ ] **Step 1: Ask the human to add the capture hooks**

Give them this verbatim. It uses `sh`, not `phr-mcp`, precisely so the capture does not depend on the subcommand this plan is about to build.

Add to `.claude/settings.local.json` in this repo, merging into the existing `hooks` object:

```json
{
  "hooks": {
    "UserPromptSubmit": [{"matcher": "", "hooks": [{"type": "command", "command": "sh -c 'mkdir -p /tmp/phr-capture && cat > /tmp/phr-capture/UserPromptSubmit.json; echo {}'"}]}],
    "SessionStart":     [{"matcher": "", "hooks": [{"type": "command", "command": "sh -c 'mkdir -p /tmp/phr-capture && cat > /tmp/phr-capture/SessionStart.json; echo {}'"}]}],
    "SessionEnd":       [{"matcher": "", "hooks": [{"type": "command", "command": "sh -c 'mkdir -p /tmp/phr-capture && cat > /tmp/phr-capture/SessionEnd.json; echo {}'"}]}],
    "SubagentStart":    [{"matcher": "", "hooks": [{"type": "command", "command": "sh -c 'mkdir -p /tmp/phr-capture && cat > /tmp/phr-capture/SubagentStart.json; echo {}'"}]}],
    "SubagentStop":     [{"matcher": "", "hooks": [{"type": "command", "command": "sh -c 'mkdir -p /tmp/phr-capture && cat > /tmp/phr-capture/SubagentStop.json; echo {}'"}]}],
    "Stop":             [{"matcher": "", "hooks": [{"type": "command", "command": "sh -c 'mkdir -p /tmp/phr-capture && cat > /tmp/phr-capture/Stop.json; echo {}'"}]}]
  }
}
```

- [ ] **Step 2: Ask the human to drive one session**

1. Quit and restart Claude Code in this repo (hooks load at startup) — that writes `SessionStart.json`.
2. Type: `list the files in crates/phronesis-mcp/src` — that writes `UserPromptSubmit.json`, and `Stop.json` when the turn ends.
3. Type: `use the Explore subagent to find where inflight state is written` — that writes `SubagentStart.json` and `SubagentStop.json`. `agent_type` should read `Explore`; if it is `""`, that is Open question 2 confirmed and the fixture records it as-is.
4. Type a long-running request (`run cargo build --workspace`), press Esc while it runs, then type `never mind, just say hi`. This produces no hook payload (Claude fires none on interrupt) but is the manual evidence the spec asks for later.
5. `/exit` — that writes `SessionEnd.json`.

- [ ] **Step 3: Redact and commit**

For every file in `/tmp/phr-capture`: replace the value of `prompt` and `last_assistant_message` with `"<redacted:N bytes>"` (N = the original byte length), replace absolute home paths with `/home/dev`, and replace real session/transcript ids with obviously-synthetic ones (`claude-s-001`, `/home/dev/.claude/projects/p/claude-s-001.jsonl`). Keep every key, including keys with empty-string values — the absence-vs-empty distinction is the whole point of this task.

```bash
mkdir -p crates/phronesis-mcp/tests/fixtures/payloads/claude/raw
# after redacting each file by hand:
cp /tmp/phr-capture/*.json crates/phronesis-mcp/tests/fixtures/payloads/claude/raw/
```

Write `raw/README.md` recording: the Claude Code version (`claude --version`), the capture date, that values were hand-redacted, and Steps 1–2 above so the capture is repeatable.

- [ ] **Step 4: Ask the human to remove the temporary capture hooks**

Revert `.claude/settings.local.json` (`git checkout -- .claude/settings.local.json`, or delete the six blocks added in Step 1). Confirm `git status` shows only the new fixture files.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/tests/fixtures/payloads/claude/raw
git commit -m "test(fixtures): capture real Claude Code lifecycle payloads"
```

---

### Task 2: `claude-hook` subcommand and adapter skeleton

Response shapes and the failure policy only. No lifecycle record is written yet — Task 3 adds them, so this task's tests stay about the protocol.

**Files:**
- Create: `crates/phronesis-mcp/src/claude_hook.rs`
- Modify: `crates/phronesis-mcp/src/lib.rs`, `crates/phronesis-mcp/src/main.rs:416-430` (enum), `:651` (dispatch)
- Test: `crates/phronesis-mcp/tests/hook_integration.rs`

**Interfaces:**
- Consumes: `context::{run_session_context_configured, run_interaction_context_configured, DEFAULT_MAX_BYTES}`, `hook::{run_pre_check, run_post_check, capture_raw_payload}`, `security::{project_root, read_stdin_capped}`, `outcomes::{enabled, report, Band}`.
- Produces:
```rust
pub struct ClaudePayload { /* every field #[serde(default)] — see Step 3 */ }
pub async fn run(event: &str) -> !;
pub(crate) fn canonical_event(event: &str) -> &str;   // Gemini names → Claude names
pub(crate) fn host_for(event: &str) -> Host;          // Gemini-only names → Host::Gemini
```

- [ ] **Step 1: Write the failing tests**

Append to `crates/phronesis-mcp/tests/hook_integration.rs`:

```rust
/// Like `run_hook_in`, but for the two-argument `claude-hook` form and
/// returning stdout — the adapter's contract is its stdout JSON.
fn run_claude_hook(dir: &Path, event: &str, payload: &str) -> (i32, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .arg("claude-hook")
        .arg(event)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn claude-hook");
    let mut stdin = child.stdin.take().unwrap();
    let _ = stdin.write_all(payload.as_bytes());
    drop(stdin);
    let out = child.wait_with_output().expect("wait");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn claude_hook_always_prints_one_json_object() {
    let dir = tempfile::tempdir().unwrap();
    for (event, payload) in [
        ("SessionStart", r#"{"hook_event_name":"SessionStart","session_id":"s1"}"#),
        ("SessionEnd", r#"{"hook_event_name":"SessionEnd","session_id":"s1"}"#),
        ("SubagentStart", r#"{"hook_event_name":"SubagentStart","agent_id":"a1","agent_type":"Explore"}"#),
        ("SubagentStop", r#"{"hook_event_name":"SubagentStop","agent_id":"a1"}"#),
        ("Stop", r#"{"hook_event_name":"Stop","session_id":"s1"}"#),
        ("UserPromptSubmit", r#"{"hook_event_name":"UserPromptSubmit","prompt":"hi"}"#),
    ] {
        let (code, stdout, stderr) = run_claude_hook(dir.path(), event, payload);
        assert_eq!(code, 0, "{event}: {stderr}");
        let v: serde_json::Value =
            serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("{event}: {e}: {stdout:?}"));
        assert!(v.is_object(), "{event}: {stdout:?}");
    }
}

#[test]
fn claude_hook_bad_stdin_is_empty_json_exit_zero() {
    let dir = tempfile::tempdir().unwrap();
    for event in ["UserPromptSubmit", "SessionStart", "Stop", "SubagentStop"] {
        let (code, stdout, _) = run_claude_hook(dir.path(), event, "not json at all");
        assert_eq!(code, 0, "{event} must never fail the host");
        assert_eq!(stdout.trim(), "{}", "{event}");
    }
}

#[test]
fn claude_hook_stop_blocks_on_low_confidence_and_honors_stop_hook_active() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".phronesis/outcomes")).unwrap();
    std::fs::write(dir.path().join(".phronesis/confidence.json"), "{}").unwrap();
    std::fs::write(dir.path().join(".phronesis/outcomes/current"), "unit-1").unwrap();

    let (code, stdout, _) = run_claude_hook(dir.path(), "Stop", r#"{"hook_event_name":"Stop"}"#);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(v["decision"], "block", "{stdout}");
    assert!(v["reason"].as_str().unwrap().contains("unit-1"), "{stdout}");

    let (_, stdout, _) = run_claude_hook(
        dir.path(),
        "Stop",
        r#"{"hook_event_name":"Stop","stop_hook_active":true}"#,
    );
    assert_eq!(stdout.trim(), "{}", "stop_hook_active must short-circuit the gate");
}

#[test]
fn claude_hook_delegates_tool_events_to_the_existing_runners() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{"id":"no-src","phase":"pre","priority":1,
            "when":[{"file_path_matches":"src/"}],
            "then":{"block":"no edits under src"}}]}"#,
    );
    let payload = r#"{"hook_event_name":"PreToolUse","tool_name":"Edit",
        "tool_input":{"file_path":"src/lib.rs","old_string":"a","new_string":"b"}}"#;
    let (code, _, stderr) = run_claude_hook(dir.path(), "PreToolUse", payload);
    assert_eq!(code, 2, "tool events keep the pre-check exit contract: {stderr}");
    assert!(stderr.contains("no edits under src"), "{stderr}");
}

#[test]
fn claude_hook_accepts_gemini_event_names() {
    let dir = tempfile::tempdir().unwrap();
    for event in ["BeforeAgent", "AfterAgent"] {
        let (code, stdout, stderr) = run_claude_hook(
            dir.path(),
            event,
            &format!(r#"{{"hook_event_name":"{event}","prompt":"hi"}}"#),
        );
        assert_eq!(code, 0, "{event}: {stderr}");
        assert!(
            serde_json::from_str::<serde_json::Value>(stdout.trim()).is_ok(),
            "{event}: {stdout:?}"
        );
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --test hook_integration claude_hook 2>&1 | tail -20`
Expected: every test fails — clap rejects the unknown subcommand `claude-hook` (exit 2, stdout empty).

- [ ] **Step 3: Implement**

Create `crates/phronesis-mcp/src/claude_hook.rs`:

```rust
//! Claude Code lifecycle hook adapter.
//!
//! Reads one Claude Code hook payload from stdin, records the lifecycle event
//! it represents, and prints exactly one JSON object. Tool phases never reach
//! the body of this module: they delegate to the existing `pre-check` /
//! `post-check` runners before stdin is touched, keeping their exit-code
//! contract intact.
//!
//! Gemini CLI's `BeforeAgent` / `AfterAgent` (registered by the Gemini init
//! writer) are accepted here too and mapped onto the Claude vocabulary; the
//! host is inferred from the incoming event name.
//!
//! See `docs/specs/SPEC-agent-lifecycle-events.md` §"Host adapters".

use std::path::Path;
use std::process;

use serde::Deserialize;

use crate::context;
use crate::hook;
use crate::lifecycle::{Host, LifecycleEvent};
use crate::outcomes;
use crate::security;

/// Every field is optional: Claude Code, Gemini CLI and our own fixtures each
/// send a different subset, and a missing field must never fail a hook.
#[derive(Debug, Default, Deserialize)]
pub struct ClaudePayload {
    #[serde(default)]
    pub hook_event_name: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub prompt_id: Option<String>,
    #[serde(default)]
    pub transcript_path: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub agent_type: Option<String>,
    #[serde(default)]
    pub agent_transcript_path: Option<String>,
    #[serde(default)]
    pub stop_hook_active: bool,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_input: Option<serde_json::Value>,
    #[serde(default)]
    pub tool_response: Option<serde_json::Value>,
}

const EMPTY: &str = "{}";

/// Map a host's event name onto the Claude vocabulary this module dispatches
/// on. Gemini's `BeforeAgent` is a prompt and `AfterAgent` is a turn stop
/// (spec §"Host adapters / Gemini CLI").
pub(crate) fn canonical_event(event: &str) -> &str {
    match event {
        "BeforeAgent" => "UserPromptSubmit",
        "AfterAgent" => "Stop",
        "BeforeTool" => "PreToolUse",
        "AfterTool" => "PostToolUse",
        other => other,
    }
}

/// Which host sent this. Only names unique to Gemini identify it; the shared
/// names (`SessionStart`, `SessionEnd`) default to Claude, which is what the
/// Claude init writer registers.
pub(crate) fn host_for(event: &str) -> Host {
    match event {
        "BeforeAgent" | "AfterAgent" | "BeforeTool" | "AfterTool" => Host::Gemini,
        _ => Host::Claude,
    }
}

pub(crate) fn unix_secs_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub async fn run(event: &str) -> ! {
    // Tool phases delegate before stdin is read: the runners read and parse it
    // themselves and own their exit codes (pre 0/1/2, post 0/1).
    match canonical_event(event) {
        "PreToolUse" => {
            let _ = hook::run_pre_check().await;
            process::exit(0);
        }
        "PostToolUse" => {
            let _ = hook::run_post_check().await;
            process::exit(0);
        }
        _ => {}
    }

    let root = security::project_root();
    let raw = match security::read_stdin_capped() {
        Ok(raw) => raw,
        Err(e) => {
            eprintln!("phronesis: claude-hook {event}: stdin read failed: {e}");
            println!("{EMPTY}");
            process::exit(0);
        }
    };
    // Tees to PHRONESIS_CAPTURE_DIR; `redact_for_capture` (Plan 1) strips
    // `prompt` and `last_assistant_message` inside it, so no prompt text ever
    // reaches `payloads.jsonl`.
    hook::capture_raw_payload(event, &raw);

    let payload: ClaudePayload = match serde_json::from_str(&raw) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("phronesis: claude-hook {event}: payload parse failed: {e}");
            println!("{EMPTY}");
            process::exit(0);
        }
    };

    // The payload's own name wins: a settings file may name the event
    // differently from what the host actually fired.
    let named = payload
        .hook_event_name
        .clone()
        .unwrap_or_else(|| event.to_string());
    let host = host_for(&named);
    let response = dispatch(&root, canonical_event(&named), host, &payload).await;
    println!("{response}");
    process::exit(0);
}

async fn dispatch(root: &Path, event: &str, host: Host, p: &ClaudePayload) -> String {
    match event {
        "UserPromptSubmit" => {
            context_or_empty(
                context::run_interaction_context_configured(root, 5, context::DEFAULT_MAX_BYTES)
                    .await,
            )
        }
        "SessionStart" => context_or_empty(
            context::run_session_context_configured(root, context::DEFAULT_MAX_BYTES).await,
        ),
        "Stop" | "SubagentStop" => completion_response(root, p),
        _ => {
            let _ = (host, p);
            EMPTY.to_string()
        }
    }
}

/// `handle_*` renders may legitimately be empty; the Claude and Gemini hook
/// protocols both require parseable JSON on stdout, so empty becomes `{}`.
fn context_or_empty(rendered: String) -> String {
    if rendered.trim().is_empty() {
        EMPTY.to_string()
    } else {
        rendered
    }
}

/// `{"decision":"block","reason":…}` when the confidence gate blocks, else
/// `{}`. `stop_hook_active` short-circuits without evaluating the gate, as the
/// Claude docs require, so a blocking gate cannot loop.
fn completion_response(root: &Path, p: &ClaudePayload) -> String {
    if p.stop_hook_active {
        return EMPTY.to_string();
    }
    match gate_block_reason(root) {
        Some(reason) => serde_json::json!({"decision": "block", "reason": reason}).to_string(),
        None => EMPTY.to_string(),
    }
}

/// Mirrors `codex_hook::make_completion_decision`: low confidence blocks,
/// medium warns on stderr, high is silent.
fn gate_block_reason(root: &Path) -> Option<String> {
    if !outcomes::enabled(root) {
        return None;
    }
    let report = outcomes::report(root, None)?;
    match report.band {
        outcomes::Band::Low => Some(format!(
            "Low confidence for {} — resolve failing or missing grounded signals before completing.",
            report.subject
        )),
        outcomes::Band::Medium => {
            eprintln!(
                "phronesis: Medium confidence for {} — one grounded signal is still missing.",
                report.subject
            );
            None
        }
        outcomes::Band::High => None,
    }
}

/// Placeholder so `LifecycleEvent` is referenced from the skeleton; Task 3
/// replaces this with the real handlers.
#[allow(dead_code)]
fn _typecheck(host: Host) -> LifecycleEvent {
    LifecycleEvent::new(crate::lifecycle::Kind::Stop, host)
}
```

In `src/lib.rs`, add `pub mod claude_hook;` in alphabetical position (immediately before `pub mod codex_hook;`).

In `src/main.rs`, add to the `Command` enum. **Placement convention (shared across this plan set):** the lifecycle variants form a group immediately after the existing `CodexHook` variant (`main.rs:424`), ordered alphabetically among themselves. The set adds `ClaudeHook` (here) and `Kalpa` (Plan 1, already landed), so `ClaudeHook` goes between `CodexHook` and `Kalpa`. Apply the same order in the `match cli.command` dispatch block. No other plan in the set adds a `Command` variant, so this is the only insertion at this point.

```rust
    /// Claude Code lifecycle hook adapter — reads a Claude Code hook JSON
    /// payload from stdin and writes one JSON object to stdout.
    ///
    /// Handles `UserPromptSubmit`, `SessionStart`, `SessionEnd`,
    /// `SubagentStart`, `SubagentStop`, and `Stop`, plus Gemini CLI's
    /// `BeforeAgent` / `AfterAgent`. `PreToolUse` / `PostToolUse` delegate to
    /// `pre-check` / `post-check` unchanged.
    ClaudeHook {
        /// The hook event name. The event from stdin takes precedence when
        /// available.
        #[arg(default_value = "UserPromptSubmit")]
        event: String,
    },
```

and to the `match cli.command` block, after the `CodexHook` arm:

```rust
        Command::ClaudeHook { event } => phronesis_mcp::claude_hook::run(&event).await,
```

`hook::capture_raw_payload` is already `pub(crate)` and already redacts: Plan 1 Task 10 made both changes. Do not edit `src/hook/mod.rs` in this task — the only line this plan adds to that file is `mod lifecycle_wiring;` in Task 4.

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --test hook_integration 2>&1 | tail -20`
Expected: all pass, including the five new tests and every pre-existing one.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/claude_hook.rs crates/phronesis-mcp/src/lib.rs crates/phronesis-mcp/src/main.rs crates/phronesis-mcp/tests/hook_integration.rs
git commit -m "feat(claude-hook): adapter subcommand with response shapes and fail-open policy"
```

---

### Task 3: Record lifecycle events in the adapter

**Files:**
- Modify: `crates/phronesis-mcp/src/claude_hook.rs` (replace `dispatch` and delete `_typecheck`)
- Test: `crates/phronesis-mcp/tests/hook_integration.rs`

**Interfaces:**
- Consumes: `lifecycle::record::record`, `lifecycle::scrub::scrub_prompt`, `lifecycle::state::{classify_prompt, PromptContext, open_turn, close_turn, read_turn, set_session, clear_session, reset_for_session_start, push_agent, pop_agent, OpenAgent}`, `journey::journal::read_recent`, `hook::seq::next_seq`.
- Produces: `pub(crate) fn synth_agent_id(root: &Path) -> String` — the `{sid}:{seq}` fallback, reused by the Gemini derivation in Task 4.

- [ ] **Step 1: Write the failing tests**

Append to `crates/phronesis-mcp/tests/hook_integration.rs`:

```rust
fn journal_records(dir: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(dir.join(".phronesis/journey/events.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn log_entries(dir: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(dir.join(".phronesis/log.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[test]
fn claude_hook_prompt_records_fresh_and_opens_turn() {
    let dir = tempfile::tempdir().unwrap();
    run_claude_hook(
        dir.path(),
        "UserPromptSubmit",
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt_id":"p1","prompt":"add a test"}"#,
    );
    let recs = journal_records(dir.path());
    let prompt = recs.iter().find(|r| r["kind"] == "prompt").expect("prompt record");
    assert_eq!(prompt["tool"], "__lifecycle");
    assert_eq!(prompt["mode"], "fresh");
    assert_eq!(prompt["host"], "claude");
    assert_eq!(prompt["turn"], "p1");
    assert!(
        !std::fs::read_to_string(dir.path().join(".phronesis/journey/events.jsonl"))
            .unwrap()
            .contains("add a test"),
        "prompt text must never reach the journal"
    );
    let log = log_entries(dir.path());
    let entry = log.iter().find(|e| e["event"] == "prompt").expect("log entry");
    assert_eq!(entry["prompt"], "add a test");
    assert_eq!(entry["kind"], "lifecycle");
    // the turn is now open
    let turn: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".phronesis/journey/turn")).unwrap(),
    )
    .unwrap();
    assert_eq!(turn["open"], true);
}

#[test]
fn claude_hook_prompt_after_inflight_records_interrupt_and_correction() {
    let dir = tempfile::tempdir().unwrap();
    run_claude_hook(
        dir.path(),
        "UserPromptSubmit",
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"first"}"#,
    );
    // A tool call is still in flight when the human speaks again.
    run_hook_in(
        "pre-check",
        r#"{"tool_name":"Bash","tool_use_id":"tu-1","tool_input":{"command":"sleep 100"}}"#,
        Some(dir.path()),
    );
    run_claude_hook(
        dir.path(),
        "UserPromptSubmit",
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"stop, do this instead"}"#,
    );
    let recs = journal_records(dir.path());
    let interrupt = recs.iter().find(|r| r["kind"] == "interrupt").expect("interrupt record");
    assert!(
        interrupt["tags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t == "lifecycle:interrupt")
    );
    let modes: Vec<&str> = recs
        .iter()
        .filter(|r| r["kind"] == "prompt")
        .map(|r| r["mode"].as_str().unwrap())
        .collect();
    assert_eq!(modes, vec!["fresh", "correction"]);
    let log = log_entries(dir.path());
    let entry = log.iter().find(|e| e["event"] == "interrupt").unwrap();
    assert_eq!(entry["inferred_from"], "inflight");
}

#[test]
fn claude_hook_stop_records_and_closes_the_turn() {
    let dir = tempfile::tempdir().unwrap();
    run_claude_hook(
        dir.path(),
        "UserPromptSubmit",
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"go"}"#,
    );
    run_claude_hook(dir.path(), "Stop", r#"{"hook_event_name":"Stop","session_id":"s1"}"#);
    assert!(journal_records(dir.path()).iter().any(|r| r["kind"] == "stop"));
    let turn: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".phronesis/journey/turn")).unwrap(),
    )
    .unwrap();
    assert_eq!(turn["open"], false);
    assert_eq!(turn["last_event"], "stop");
    // …and the next prompt is fresh again.
    run_claude_hook(
        dir.path(),
        "UserPromptSubmit",
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"again"}"#,
    );
    let modes: Vec<&str> = journal_records(dir.path())
        .iter()
        .filter(|r| r["kind"] == "prompt")
        .map(|r| r["mode"].as_str().unwrap())
        .collect();
    assert_eq!(modes, vec!["fresh", "fresh"]);
}

#[test]
fn claude_hook_subagent_pair_records_duration_and_match() {
    let dir = tempfile::tempdir().unwrap();
    run_claude_hook(
        dir.path(),
        "SubagentStart",
        r#"{"hook_event_name":"SubagentStart","session_id":"s1","agent_id":"a1","agent_type":"Explore"}"#,
    );
    run_claude_hook(
        dir.path(),
        "SubagentStop",
        r#"{"hook_event_name":"SubagentStop","session_id":"s1","agent_id":"a1","agent_type":"Explore"}"#,
    );
    let recs = journal_records(dir.path());
    let start = recs.iter().find(|r| r["kind"] == "subagent_start").unwrap();
    assert_eq!(start["agent"], "a1");
    assert!(
        start["tags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t == "lifecycle:agent:Explore")
    );
    let stop = log_entries(dir.path())
        .into_iter()
        .find(|e| e["event"] == "subagent_stop")
        .unwrap();
    assert_eq!(stop["matched_start"], true);
    assert!(stop["duration_secs"].is_u64());

    // An unmatched stop is recorded honestly.
    run_claude_hook(
        dir.path(),
        "SubagentStop",
        r#"{"hook_event_name":"SubagentStop","agent_id":"ghost"}"#,
    );
    let unmatched = log_entries(dir.path())
        .into_iter()
        .filter(|e| e["event"] == "subagent_stop")
        .next_back()
        .unwrap();
    assert_eq!(unmatched["matched_start"], false);
    assert!(unmatched.get("duration_secs").is_none());
}

#[test]
fn claude_hook_session_start_overwrites_sid_and_truncates_state() {
    let dir = tempfile::tempdir().unwrap();
    run_hook_in(
        "pre-check",
        r#"{"tool_name":"Bash","tool_use_id":"tu-9","tool_input":{"command":"ls"}}"#,
        Some(dir.path()),
    );
    run_claude_hook(
        dir.path(),
        "SessionStart",
        r#"{"hook_event_name":"SessionStart","session_id":"claude-s-42"}"#,
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join(".phronesis/journey/session"))
            .unwrap()
            .trim(),
        "claude-s-42"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join(".phronesis/journey/inflight"))
            .unwrap()
            .trim(),
        ""
    );
}

#[test]
fn claude_hook_session_end_stops_an_open_turn_and_clears_the_session() {
    let dir = tempfile::tempdir().unwrap();
    run_claude_hook(
        dir.path(),
        "SessionStart",
        r#"{"hook_event_name":"SessionStart","session_id":"claude-s-7"}"#,
    );
    run_claude_hook(
        dir.path(),
        "UserPromptSubmit",
        r#"{"hook_event_name":"UserPromptSubmit","prompt":"go"}"#,
    );
    run_claude_hook(dir.path(), "SessionEnd", r#"{"hook_event_name":"SessionEnd"}"#);
    assert!(journal_records(dir.path()).iter().any(|r| r["kind"] == "stop"));
    assert_eq!(
        std::fs::read_to_string(dir.path().join(".phronesis/journey/session"))
            .unwrap()
            .trim(),
        ""
    );
}

#[test]
fn gemini_before_agent_records_a_gemini_prompt() {
    let dir = tempfile::tempdir().unwrap();
    run_claude_hook(
        dir.path(),
        "BeforeAgent",
        r#"{"hook_event_name":"BeforeAgent","session_id":"g1","prompt":"hello"}"#,
    );
    let rec = journal_records(dir.path())
        .into_iter()
        .find(|r| r["kind"] == "prompt")
        .unwrap();
    assert_eq!(rec["host"], "gemini");
    assert_eq!(rec["mode"], "fresh");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --test hook_integration claude_hook_prompt gemini_before_agent 2>&1 | tail -30`
Expected: FAIL — `.phronesis/journey/events.jsonl` does not exist, so `journal_records` is empty and `expect("prompt record")` panics.

- [ ] **Step 3: Implement**

In `claude_hook.rs`, delete `_typecheck`, extend the imports, and replace `dispatch` with the real one plus the handlers:

```rust
use std::path::{Path, PathBuf};

use crate::journey;
use crate::lifecycle::record::record;
use crate::lifecycle::state;
use crate::lifecycle::{Host, Kind, LifecycleEvent};

async fn dispatch(root: &Path, event: &str, host: Host, p: &ClaudePayload) -> String {
    match event {
        "UserPromptSubmit" => handle_prompt(root, host, p).await,
        "SessionStart" => handle_session_start(root, p).await,
        "SessionEnd" => {
            handle_session_end(root, host, p);
            EMPTY.to_string()
        }
        "SubagentStart" => {
            handle_subagent_start(root, host, p);
            EMPTY.to_string()
        }
        "SubagentStop" => {
            handle_subagent_stop(root, host, p);
            completion_response(root, p)
        }
        "Stop" => {
            handle_stop(root, host, p);
            completion_response(root, p)
        }
        _ => EMPTY.to_string(),
    }
}

fn nonempty(v: &Option<String>) -> Option<&str> {
    v.as_deref().filter(|s| !s.is_empty())
}

/// Stamp session and turn ids when the host supplied them.
fn with_session_turn(mut ev: LifecycleEvent, p: &ClaudePayload) -> LifecycleEvent {
    if let Some(s) = nonempty(&p.session_id) {
        ev = ev.with_session(s);
    }
    if let Some(t) = nonempty(&p.prompt_id) {
        ev = ev.with_turn(t);
    }
    ev
}

/// The `{sid}:{seq}` fallback for hosts that supply no agent id (Gemini, and
/// Claude internal forks with empty fields). See spec §"Correlation state".
pub(crate) fn synth_agent_id(root: &Path) -> String {
    format!(
        "{}:{}",
        journey::current_sid(root),
        crate::hook::seq::next_seq(root)
    )
}

/// The `kind` of the most recent lifecycle journal record, which is how the
/// Codex branch of `classify_prompt` sees its own `Interrupt` hook. Claude
/// never produces one, but the context is built uniformly.
fn last_lifecycle_kind(root: &Path) -> Option<String> {
    journey::journal::read_recent(root, 1)
        .ok()?
        .into_iter()
        .next_back()
        .filter(|r| r.is_lifecycle())
        .and_then(|r| r.kind)
}

async fn handle_prompt(root: &Path, host: Host, p: &ClaudePayload) -> String {
    let now = unix_secs_now();
    let transcript = p.transcript_path.as_deref().map(PathBuf::from);
    let last_kind = last_lifecycle_kind(root);
    let classification = state::classify_prompt(
        root,
        &state::PromptContext {
            host,
            now,
            agent_id: nonempty(&p.agent_id),
            turn_id: nonempty(&p.prompt_id),
            transcript_path: transcript.as_deref(),
            last_journal_kind: last_kind.as_deref(),
        },
    );

    if let Some(source) = classification.interrupt {
        let mut ev = LifecycleEvent::new(Kind::Interrupt, host).with_extra("inferred_from", source.as_str());
        ev = with_session_turn(ev, p);
        if let Some(a) = nonempty(&p.agent_id) {
            ev = ev.with_agent(a, p.agent_type.clone());
        }
        record(root, ev);
        state::close_turn(root, "interrupt");
    }

    let mut ev = LifecycleEvent::new(Kind::Prompt, host).with_mode(classification.mode);
    ev = with_session_turn(ev, p);
    if let Some(a) = nonempty(&p.agent_id) {
        ev = ev.with_agent(a, p.agent_type.clone());
    }
    if let Some(text) = p.prompt.as_deref() {
        ev = ev.with_prompt(crate::lifecycle::scrub::scrub_prompt(root, text));
    }
    record(root, ev);
    state::open_turn(root, nonempty(&p.prompt_id), now);

    context_or_empty(
        context::run_interaction_context_configured(root, 5, context::DEFAULT_MAX_BYTES).await,
    )
}

async fn handle_session_start(root: &Path, p: &ClaudePayload) -> String {
    // The host's id wins over the create-on-miss id `current_sid` would mint.
    if let Some(sid) = nonempty(&p.session_id) {
        state::set_session(root, sid);
    }
    state::reset_for_session_start(root);
    context_or_empty(context::run_session_context_configured(root, context::DEFAULT_MAX_BYTES).await)
}

fn handle_session_end(root: &Path, host: Host, p: &ClaudePayload) {
    if state::read_turn(root).open {
        record(root, with_session_turn(LifecycleEvent::new(Kind::Stop, host), p));
    }
    state::close_turn(root, "stop");
    state::clear_session(root);
}

fn handle_stop(root: &Path, host: Host, p: &ClaudePayload) {
    record(root, with_session_turn(LifecycleEvent::new(Kind::Stop, host), p));
    state::close_turn(root, "stop");
}

fn handle_subagent_start(root: &Path, host: Host, p: &ClaudePayload) {
    let agent_id = nonempty(&p.agent_id)
        .map(str::to_string)
        .unwrap_or_else(|| synth_agent_id(root));
    let agent_type = p.agent_type.clone();
    let ev = with_session_turn(
        LifecycleEvent::new(Kind::SubagentStart, host).with_agent(agent_id.clone(), agent_type.clone()),
        p,
    );
    let stamped = record(root, ev);
    state::push_agent(
        root,
        state::OpenAgent {
            agent_id,
            agent_type,
            ts: stamped.as_ref().map(|s| s.ts).unwrap_or_else(unix_secs_now),
            seq: stamped.as_ref().map(|s| s.seq).unwrap_or(0),
        },
    );
}

fn handle_subagent_stop(root: &Path, host: Host, p: &ClaudePayload) {
    // `agents`, not the journal, is authoritative for pairing.
    let open = state::pop_agent(root, nonempty(&p.agent_id));
    let now = unix_secs_now();
    let mut ev = LifecycleEvent::new(Kind::SubagentStop, host)
        .with_extra("matched_start", open.is_some())
        .with_extra("stop_hook_active", p.stop_hook_active);
    if let Some(o) = &open {
        ev = ev.with_extra("duration_secs", now.saturating_sub(o.ts));
    }
    let agent_id = nonempty(&p.agent_id)
        .map(str::to_string)
        .or_else(|| open.as_ref().map(|o| o.agent_id.clone()));
    if let Some(id) = agent_id {
        let agent_type = p
            .agent_type
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| open.as_ref().and_then(|o| o.agent_type.clone()));
        ev = ev.with_agent(id, agent_type);
    }
    record(root, with_session_turn(ev, p));
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --test hook_integration 2>&1 | tail -20`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/claude_hook.rs crates/phronesis-mcp/tests/hook_integration.rs
git commit -m "feat(claude-hook): record prompt, interrupt, stop, session and sub-agent events"
```

---

### Task 4: `inflight`, commit detection, and `invoke_agent` in pre/post

**Files:**
- Create: `crates/phronesis-mcp/src/hook/lifecycle_wiring.rs`
- Modify: `crates/phronesis-mcp/src/hook/mod.rs` (add `mod lifecycle_wiring;`), `crates/phronesis-mcp/src/hook/pre.rs:22-43`, `crates/phronesis-mcp/src/hook/post.rs:29-50`, `crates/phronesis-mcp/src/hook/journey_record.rs:120`
- Test: `crates/phronesis-mcp/tests/hook_integration.rs`

**Interfaces:**
- Consumes: `lifecycle::state::{push_inflight, pop_inflight, inflight_key_for, Inflight, push_agent, pop_agent, OpenAgent}`, `lifecycle::outcome::{is_shell_tool, git_head, detect_commit}`, `lifecycle::record::record`, `claude_hook::{synth_agent_id, unix_secs_now}`, `hook::journey_record::payload_command_exit`, `hook::extract_new_content`.
- Produces (all `pub(super)`, i.e. visible to `pre.rs` and `post.rs`):
```rust
pub(super) fn pre_push_inflight(root: &Path, payload: &HookPayload) -> String;  // returns the key
pub(super) fn pop_inflight_key(root: &Path, key: &str);
pub(super) fn post_pop_and_detect(root: &Path, payload: &HookPayload, tool_name: &str);
pub(super) fn gemini_subagent_start(root: &Path, payload: &HookPayload);
pub(super) fn gemini_subagent_stop(root: &Path);
```

- [ ] **Step 1: Write the failing tests**

Append to `crates/phronesis-mcp/tests/hook_integration.rs`:

```rust
fn inflight_keys(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join(".phronesis/journey/inflight"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| v["key"].as_str().map(String::from))
        .collect()
}

#[test]
fn pre_pushes_inflight_and_post_pops_it() {
    let dir = tempfile::tempdir().unwrap();
    let pre = r#"{"tool_name":"Bash","tool_use_id":"tu-42","tool_input":{"command":"ls"}}"#;
    run_hook_in("pre-check", pre, Some(dir.path()));
    assert_eq!(inflight_keys(dir.path()), vec!["tu-42".to_string()]);
    let post = r#"{"tool_name":"Bash","tool_use_id":"tu-42","tool_input":{"command":"ls"},
        "tool_response":{"exit_code":0,"stdout":""}}"#;
    run_hook_in("post-check", post, Some(dir.path()));
    assert!(inflight_keys(dir.path()).is_empty());
}

#[test]
fn blocked_pre_check_pops_its_own_inflight_entry() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{"id":"no-rm","phase":"pre","priority":1,
            "when":[{"bash_command_matches":"rm -rf"}],
            "then":{"block":"never"}}]}"#,
    );
    let payload = r#"{"tool_name":"Bash","tool_use_id":"tu-block","tool_input":{"command":"rm -rf /"}}"#;
    let (code, stderr) = run_hook_in("pre-check", payload, Some(dir.path()));
    assert_eq!(code, 2, "{stderr}");
    assert!(
        inflight_keys(dir.path()).is_empty(),
        "a block is not an interrupt: {:?}",
        inflight_keys(dir.path())
    );
}

#[test]
fn post_check_records_a_commit_when_head_moved() {
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .output()
            .expect("git");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "t"]);
    std::fs::write(dir.path().join("a.txt"), "one").unwrap();
    git(&["add", "a.txt"]);
    git(&["commit", "-qm", "first"]);

    let pre = r#"{"tool_name":"Bash","tool_use_id":"tu-c","tool_input":{"command":"git commit -am second"}}"#;
    run_hook_in("pre-check", pre, Some(dir.path()));
    // the command actually runs between pre and post
    std::fs::write(dir.path().join("a.txt"), "two").unwrap();
    git(&["commit", "-qam", "second"]);
    let post = r#"{"tool_name":"Bash","tool_use_id":"tu-c","tool_input":{"command":"git commit -am second"},
        "tool_response":{"exit_code":0,"stdout":""}}"#;
    run_hook_in("post-check", post, Some(dir.path()));

    let commit = journal_records(dir.path())
        .into_iter()
        .find(|r| r["kind"] == "commit")
        .expect("commit record");
    assert_eq!(commit["tool"], "__lifecycle");
    let entry = log_entries(dir.path())
        .into_iter()
        .find(|e| e["event"] == "commit")
        .unwrap();
    assert_eq!(entry["tool_use_id"], "tu-c");
    assert_eq!(entry["sha"].as_str().unwrap().len(), 40);
    assert_ne!(entry["sha"], entry["head_before"]);
}

#[test]
fn non_commit_shell_call_records_nothing() {
    let dir = tempfile::tempdir().unwrap();
    run_hook_in(
        "pre-check",
        r#"{"tool_name":"Bash","tool_use_id":"tu-n","tool_input":{"command":"echo hi"}}"#,
        Some(dir.path()),
    );
    run_hook_in(
        "post-check",
        r#"{"tool_name":"Bash","tool_use_id":"tu-n","tool_input":{"command":"echo hi"},
            "tool_response":{"exit_code":0}}"#,
        Some(dir.path()),
    );
    assert!(!journal_records(dir.path()).iter().any(|r| r["kind"] == "commit"));
}

#[test]
fn invoke_agent_derives_a_subagent_pair_before_the_allowlist() {
    let dir = tempfile::tempdir().unwrap();
    let pre = r#"{"tool_name":"invoke_agent","tool_input":{"agent_name":"reviewer","prompt":"look"}}"#;
    run_hook_in("pre-check", pre, Some(dir.path()));
    let start = journal_records(dir.path())
        .into_iter()
        .find(|r| r["kind"] == "subagent_start")
        .expect("subagent_start");
    assert_eq!(start["host"], "gemini");
    assert_eq!(start["agent_type"], "reviewer");
    assert!(start["agent"].as_str().unwrap().contains(':'));

    let post = r#"{"tool_name":"invoke_agent","tool_input":{"agent_name":"reviewer","prompt":"look"},
        "tool_response":{"output":"done"}}"#;
    run_hook_in("post-check", post, Some(dir.path()));
    let stop = log_entries(dir.path())
        .into_iter()
        .find(|e| e["event"] == "subagent_stop")
        .expect("subagent_stop");
    assert_eq!(stop["matched_start"], true);
    assert_eq!(stop["agent_type"], "reviewer");
}

#[test]
fn tool_records_are_written_at_journal_v2() {
    let dir = tempfile::tempdir().unwrap();
    run_hook_in(
        "post-check",
        r#"{"tool_name":"Edit","tool_input":{"file_path":"src/a.rs","old_string":"a","new_string":"b"}}"#,
        Some(dir.path()),
    );
    let rec = journal_records(dir.path())
        .into_iter()
        .find(|r| r["tool"] == "Edit")
        .expect("tool record");
    assert_eq!(rec["v"], 2);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --test hook_integration pre_pushes blocked_pre post_check_records invoke_agent tool_records_are 2>&1 | tail -30`
Expected: `pre_pushes_inflight_and_post_pops_it` fails (no `inflight` file), `post_check_records_a_commit_when_head_moved` fails on `expect("commit record")`, `invoke_agent_…` fails on `expect("subagent_start")`, `tool_records_are_written_at_journal_v2` fails with `1 != 2`.

- [ ] **Step 3: Implement**

Create `crates/phronesis-mcp/src/hook/lifecycle_wiring.rs`:

```rust
//! Lifecycle side effects of the tool hooks: the `inflight` correlation
//! entry, commit detection from HEAD movement, and Gemini's `invoke_agent`
//! sub-agent derivation. Kept out of `pre.rs`/`post.rs` so those stay about
//! rule evaluation. Everything here is best-effort and never changes an exit
//! code (spec §"Where the writes happen").

use std::path::Path;

use serde_json::Value;

use crate::claude_hook::{synth_agent_id, unix_secs_now};
use crate::lifecycle::record::record;
use crate::lifecycle::state;
use crate::lifecycle::{Host, Kind, LifecycleEvent, outcome};
use crate::outcomes;

use super::HookPayload;

fn tool_of(payload: &HookPayload) -> String {
    payload.tool_name.clone().unwrap_or_default()
}

fn input_of(payload: &HookPayload) -> Value {
    payload.tool_input.clone().unwrap_or(Value::Null)
}

/// Push the in-flight entry. Runs immediately after `read_payload`, before the
/// allowlist and before rules load, so a tool Phronesis does not govern still
/// makes the next prompt a `correction`.
pub(super) fn pre_push_inflight(root: &Path, payload: &HookPayload) -> String {
    let tool = tool_of(payload);
    let input = input_of(payload);
    let key = state::inflight_key_for(payload.tool_use_id.as_deref(), &tool, &input);
    // HEAD is read only for shell tools: this is the one git call on the pre
    // path, and every other tool would pay for nothing.
    let head_before = if outcome::is_shell_tool(&tool) {
        outcome::git_head(root)
    } else {
        None
    };
    state::push_inflight(
        root,
        state::Inflight {
            key: key.clone(),
            tool,
            ts: unix_secs_now(),
            agent_id: payload.agent_id.clone().filter(|s| !s.is_empty()),
            head_before,
        },
    );
    key
}

/// A blocked pre-check pops its own entry before exiting: a block is not an
/// interrupt (spec §"Correlation state").
pub(super) fn pop_inflight_key(root: &Path, key: &str) {
    state::pop_inflight(root, key);
}

/// Pop the entry this call pushed and, for a shell call that may have moved
/// HEAD, record a `commit`.
pub(super) fn post_pop_and_detect(root: &Path, payload: &HookPayload, tool_name: &str) {
    let key = state::inflight_key_for(payload.tool_use_id.as_deref(), tool_name, &input_of(payload));
    let entry = state::pop_inflight(root, &key);
    if !outcome::is_shell_tool(tool_name) {
        return;
    }
    let Some(entry) = entry else { return };
    let command = super::extract_new_content(payload, tool_name).unwrap_or_default();
    let exit = super::journey_record::payload_command_exit(payload);
    let Some(commit) = outcome::detect_commit(root, entry.head_before.as_deref(), &command, exit)
    else {
        return;
    };
    let mut ev = LifecycleEvent::new(Kind::Commit, host_for_tool(tool_name))
        .with_extra("sha", commit.sha)
        .with_extra("head_before", commit.head_before);
    if let Some(id) = payload.tool_use_id.as_deref().filter(|s| !s.is_empty()) {
        ev = ev.with_extra("tool_use_id", id);
    }
    if let Some(band) = confidence_band(root) {
        ev = ev.with_extra("confidence_band", band);
    }
    if let Some(agent) = entry.agent_id {
        ev = ev.with_agent(agent, None);
    }
    record(root, ev);
}

/// `Bash` is Claude's shell tool; `run_shell_command` is Gemini's.
fn host_for_tool(tool_name: &str) -> Host {
    if tool_name == "Bash" { Host::Claude } else { Host::Gemini }
}

/// The band at commit time, when confidence scoring is enabled and a work
/// unit is open. Absent otherwise — no band is better than a fabricated one.
fn confidence_band(root: &Path) -> Option<&'static str> {
    if !outcomes::enabled(root) {
        return None;
    }
    Some(match outcomes::report(root, None)?.band {
        outcomes::Band::Low => "low",
        outcomes::Band::Medium => "medium",
        outcomes::Band::High => "high",
    })
}

/// Gemini has no sub-agent event: it invokes one through the `invoke_agent`
/// tool, so the pair is derived from `BeforeTool`/`AfterTool` (spec §"Host
/// adapters / Gemini CLI"). The id is synthesized and the stop pops LIFO.
pub(super) fn gemini_subagent_start(root: &Path, payload: &HookPayload) {
    let agent_type = payload
        .tool_input
        .as_ref()
        .and_then(|v| v.get("agent_name"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let agent_id = synth_agent_id(root);
    let ev = LifecycleEvent::new(Kind::SubagentStart, Host::Gemini)
        .with_agent(agent_id.clone(), agent_type.clone());
    let stamped = record(root, ev);
    state::push_agent(
        root,
        state::OpenAgent {
            agent_id,
            agent_type,
            ts: stamped.as_ref().map(|s| s.ts).unwrap_or_else(unix_secs_now),
            seq: stamped.as_ref().map(|s| s.seq).unwrap_or(0),
        },
    );
}

pub(super) fn gemini_subagent_stop(root: &Path) {
    let open = state::pop_agent(root, None);
    let now = unix_secs_now();
    let mut ev =
        LifecycleEvent::new(Kind::SubagentStop, Host::Gemini).with_extra("matched_start", open.is_some());
    if let Some(o) = &open {
        ev = ev
            .with_agent(o.agent_id.clone(), o.agent_type.clone())
            .with_extra("duration_secs", now.saturating_sub(o.ts));
    }
    record(root, ev);
}
```

In `src/hook/mod.rs`, add `mod lifecycle_wiring;` next to `pub(crate) mod seq;`.

In `src/hook/pre.rs::run_pre_check`, immediately after the `read_payload` match and **before** the `tool_name` allowlist match, insert:

```rust
    let root = security::project_root();
    let inflight_key = super::lifecycle_wiring::pre_push_inflight(&root, &payload);
    if payload.tool_name.as_deref() == Some("invoke_agent") {
        super::lifecycle_wiring::gemini_subagent_start(&root, &payload);
    }
```

Add `"invoke_agent"` to the allowlist match in the same function (after `|| name == "run_shell_command"`), and add this helper below `run_pre_check`:

```rust
/// Exit 2 after dropping this call's `inflight` entry. A block means the tool
/// never ran, so the entry must not survive to make the next prompt look like
/// a correction.
fn blocked_exit(root: &std::path::Path, key: &str) -> ! {
    super::lifecycle_wiring::pop_inflight_key(root, key);
    process::exit(2)
}
```

Then replace **every** `process::exit(2)` in `run_pre_check` that appears after the `pre_push_inflight` call with `blocked_exit(&root, &inflight_key)`. (The `process::exit(2)` inside the `read_payload` error arm is before the push and stays as it is; the ones in `assert_pre_content_facts` are in a different function and are reached through `?`, whose caller in `run_pre_check` is covered by this rule.)

In `src/hook/post.rs::run_post_check`, immediately after the `read_payload` match and before the allowlist match, insert:

```rust
    let root = security::project_root();
    let raw_tool = payload.tool_name.clone().unwrap_or_default();
    super::lifecycle_wiring::post_pop_and_detect(&root, &payload, &raw_tool);
    if raw_tool == "invoke_agent" {
        super::lifecycle_wiring::gemini_subagent_stop(&root);
    }
```

Add `"invoke_agent"` to post's allowlist match too, and replace the later `let file_path = …` block's use of a freshly computed root with `&root` if clippy flags the shadowing; otherwise leave the existing `security::project_root()` calls alone.

In `src/hook/journey_record.rs::build_journal_record`, change `v: 1,` to `v: journey::journal::JOURNAL_V,` — lifecycle records and tool records now share one schema version.

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --test hook_integration --test payload_contract --test journey_journal 2>&1 | tail -30`
Expected: all pass. If a `payload_contract` fixture asserted `"v":1`, update that fixture's expectation to 2 in the same commit.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/hook crates/phronesis-mcp/tests/hook_integration.rs crates/phronesis-mcp/tests/fixtures
git commit -m "feat(hook): inflight correlation, commit detection and invoke_agent sub-agent derivation"
```

---

### Task 5: Command-keyed Claude hook registrations in `init`

**Files:**
- Modify: `crates/phronesis-mcp/src/init.rs:580-622` (`write_settings`), `:1544-1559` (add `upsert_hook_by_command` beside `upsert_hook`)
- Modify: `crates/phronesis-mcp/tests/fixtures/hook_events.json`
- Test: `crates/phronesis-mcp/tests/init_integration.rs`

**Interfaces:**
- Produces: `fn upsert_hook_by_command(settings: &mut Value, event: &str, new_entry: Value)` — replaces only entries whose command starts with `phr-mcp `, leaving every foreign hook in place.

> **Shared ownership.** This plan is the **sole owner** of `upsert_hook_by_command`. Plan 4 (Gemini) consumes it and defines nothing; it depends on this plan landing first. Do not change the name, signature, or body without updating Plan 4's §Merge notes, which assume this exact definition is already present in `init.rs` when Plan 4 merges.

- [ ] **Step 1: Write the failing tests**

Append to `crates/phronesis-mcp/tests/init_integration.rs`:

```rust
fn claude_settings(dir: &Path) -> serde_json::Value {
    serde_json::from_str(
        &std::fs::read_to_string(dir.join(".claude/settings.local.json")).unwrap(),
    )
    .unwrap()
}

fn commands_for(settings: &serde_json::Value, event: &str) -> Vec<String> {
    settings["hooks"][event]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .flat_map(|entry| {
            entry["hooks"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|h| h["command"].as_str().map(String::from))
        })
        .collect()
}

#[test]
fn init_registers_the_claude_lifecycle_events() {
    let dir = tempfile::tempdir().unwrap();
    assert!(run_init(&[], dir.path()).status.success());
    let s = claude_settings(dir.path());
    for event in ["SubagentStart", "SubagentStop", "Stop", "SessionEnd"] {
        assert_eq!(
            commands_for(&s, event),
            vec![format!("phr-mcp claude-hook {event}")],
            "{event}"
        );
        assert_eq!(s["hooks"][event][0]["matcher"], "");
    }
    assert_eq!(
        commands_for(&s, "UserPromptSubmit"),
        vec!["phr-mcp claude-hook UserPromptSubmit".to_string()]
    );
    assert_eq!(
        commands_for(&s, "SessionStart"),
        vec!["phr-mcp claude-hook SessionStart".to_string()]
    );
}

#[test]
fn init_is_idempotent_for_the_new_registrations() {
    let dir = tempfile::tempdir().unwrap();
    assert!(run_init(&[], dir.path()).status.success());
    assert!(run_init(&["--force"], dir.path()).status.success());
    let s = claude_settings(dir.path());
    for event in ["SubagentStart", "SubagentStop", "Stop", "SessionEnd", "UserPromptSubmit"] {
        assert_eq!(s["hooks"][event].as_array().unwrap().len(), 1, "{event}");
    }
}

#[test]
fn init_migrates_interaction_context_in_place_and_keeps_foreign_hooks() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
    std::fs::write(
        dir.path().join(".claude/settings.local.json"),
        r#"{"hooks":{
            "UserPromptSubmit":[{"matcher":"","hooks":[{"type":"command","command":"phr-mcp interaction-context"}]}],
            "Stop":[{"matcher":"","hooks":[{"type":"command","command":"my-own-notifier"}]}]
        }}"#,
    )
    .unwrap();
    assert!(run_init(&["--force"], dir.path()).status.success());
    let s = claude_settings(dir.path());
    assert_eq!(
        commands_for(&s, "UserPromptSubmit"),
        vec!["phr-mcp claude-hook UserPromptSubmit".to_string()],
        "the old command must be replaced, not appended"
    );
    let stop = commands_for(&s, "Stop");
    assert!(
        stop.contains(&"my-own-notifier".to_string()),
        "a foreign empty-matcher Stop hook must survive init: {stop:?}"
    );
    assert!(stop.contains(&"phr-mcp claude-hook Stop".to_string()), "{stop:?}");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --test init_integration init_registers init_is_idempotent init_migrates 2>&1 | tail -30`
Expected: `init_registers_the_claude_lifecycle_events` fails (`SubagentStart` absent), `init_migrates_…` fails (`UserPromptSubmit` still `phr-mcp interaction-context`).

- [ ] **Step 3: Implement**

In `src/init.rs`, add beside `upsert_hook`:

```rust
/// Replace Phronesis's own entry for a hook event regardless of its former
/// matcher or command, and leave every other hook alone. Matcher-keyed
/// `upsert_hook` both deletes a user's hook that happens to share our matcher
/// (spec §"Adjacent findings" 7) and leaves a stale entry behind whenever we
/// change our own matcher or command; keying on the `phr-mcp ` command prefix
/// does neither. Phronesis registers at most one entry per event, so dropping
/// every `phr-mcp ` entry and pushing one back is exact. Migrating the four
/// pre-existing matcher-keyed registrations to this is a follow-up.
fn upsert_hook_by_command(settings: &mut Value, event: &str, new_entry: Value) {
    let hooks = settings.as_object_mut().and_then(|o| {
        o.entry("hooks".to_string())
            .or_insert_with(|| json!({}))
            .as_object_mut()
    });
    let Some(hooks) = hooks else { return };
    let arr = hooks.entry(event.to_string()).or_insert_with(|| json!([]));
    if !arr.is_array() {
        *arr = json!([]);
    }
    let arr = arr.as_array_mut().unwrap();
    arr.retain(|entry| {
        !entry["hooks"].as_array().is_some_and(|handlers| {
            handlers.iter().any(|hook| {
                hook["command"]
                    .as_str()
                    .is_some_and(|c| c.starts_with("phr-mcp "))
            })
        })
    });
    arr.push(new_entry);
}
```

Also add its two unit tests to `init.rs`'s `#[cfg(test)] mod tests`, beside `upsert_hook_replaces_matching_matcher` (`init.rs:3429`). Plan 4 asserts the same behaviour through the Gemini writer and relies on these:

```rust
#[test]
fn upsert_hook_by_command_replaces_ours_and_keeps_foreign() {
    let mut settings = json!({"hooks": {"SessionStart": [
        {"matcher": "", "hooks": [{"type": "command", "command": "phr-mcp session-context"}]},
        {"matcher": "", "hooks": [{"type": "command", "command": "my-own-tool --flag"}]}
    ]}});
    upsert_hook_by_command(
        &mut settings,
        "SessionStart",
        json!({"matcher": "", "hooks": [{"type": "command", "command": "phr-mcp claude-hook SessionStart"}]}),
    );
    let arr = settings["hooks"]["SessionStart"].as_array().unwrap();
    assert_eq!(arr.len(), 2, "one foreign hook plus exactly one of ours: {arr:?}");
    assert_eq!(arr[0]["hooks"][0]["command"], "my-own-tool --flag");
    assert_eq!(arr[1]["hooks"][0]["command"], "phr-mcp claude-hook SessionStart");
}

#[test]
fn upsert_hook_by_command_creates_missing_event_array() {
    let mut settings = json!({});
    upsert_hook_by_command(
        &mut settings,
        "AfterAgent",
        json!({"matcher": "", "hooks": [{"type": "command", "command": "phr-mcp claude-hook AfterAgent"}]}),
    );
    assert_eq!(settings["hooks"]["AfterAgent"].as_array().unwrap().len(), 1);
}
```

In `write_settings`, replace the two `upsert_hook(… context_entry …)` calls and add the four new events:

```rust
    // Lifecycle + context hooks, all empty-matcher (fire on every event) and
    // all served by one adapter. Command-keyed replacement so a user's own
    // empty-matcher hook on the same event survives `phr-mcp init`.
    for event in [
        "SessionStart",
        "SessionEnd",
        "UserPromptSubmit",
        "SubagentStart",
        "SubagentStop",
        "Stop",
    ] {
        upsert_hook_by_command(
            &mut settings,
            event,
            context_entry(&format!("phr-mcp claude-hook {event}")),
        );
    }
```

In `crates/phronesis-mcp/tests/fixtures/hook_events.json`, extend **only the `claude-code` list** so `payload_contract.rs::init_wires_hooks_only_under_event_names_that_exist` accepts the new keys. Plan 3 edits the `codex` line and Plan 4 the `gemini` line; one key per plan keeps the three-way merge line-disjoint.

```json
  "claude-code": ["PreToolUse", "PostToolUse", "SessionStart", "SessionEnd", "UserPromptSubmit", "SubagentStart", "SubagentStop", "Stop"],
```

`session-context` and `interaction-context` remain as subcommands in `main.rs` and keep behaving exactly as today, so settings files written by older versions keep working; nothing about them changes here.

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --test init_integration --test payload_contract 2>&1 | tail -30`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/init.rs crates/phronesis-mcp/tests/init_integration.rs crates/phronesis-mcp/tests/fixtures/hook_events.json
git commit -m "feat(init): register Claude lifecycle hooks with command-keyed replacement"
```

---

### Task 6: Concurrency gate

The spec makes this the gate for the whole Claude adapter step: two sub-agents dispatched in one message run tool calls concurrently against the same project root, and none of them may make the parent's next prompt look like a `correction`.

**Files:**
- Create: `crates/phronesis-mcp/tests/lifecycle_concurrency.rs`

- [ ] **Step 1: Write the failing test**

```rust
//! Gate for the Claude adapter (spec §Testing, `tests/lifecycle_concurrency.rs`):
//! concurrent tool calls from several agents must leave the `inflight` file
//! consistent, and a prompt arriving after they have all completed must never
//! be classified as a `correction`.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

fn run(dir: &Path, args: &[&str], payload: &str) -> i32 {
    let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");
    let mut stdin = child.stdin.take().unwrap();
    let _ = stdin.write_all(payload.as_bytes());
    drop(stdin);
    child.wait().expect("wait").code().unwrap_or(-1)
}

fn journal(dir: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(dir.join(".phronesis/journey/events.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[test]
fn concurrent_tool_pairs_never_produce_a_correction() {
    const N: usize = 12;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();

    // Open a turn, so a spurious interrupt would be observable.
    run(
        &root,
        &["claude-hook", "UserPromptSubmit"],
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"go"}"#,
    );

    let handles: Vec<_> = (0..N)
        .map(|i| {
            let root = root.clone();
            std::thread::spawn(move || {
                // Half the calls belong to sub-agents, half to the parent.
                let agent = if i % 2 == 0 {
                    format!(r#","agent_id":"sub-{i}""#)
                } else {
                    String::new()
                };
                let pre = format!(
                    r#"{{"tool_name":"Bash","tool_use_id":"tu-{i}","tool_input":{{"command":"echo {i}"}}{agent}}}"#
                );
                assert_eq!(run(&root, &["pre-check"], &pre), 0);
                let post = format!(
                    r#"{{"tool_name":"Bash","tool_use_id":"tu-{i}","tool_input":{{"command":"echo {i}"}},"tool_response":{{"exit_code":0}}{agent}}}"#
                );
                assert_eq!(run(&root, &["post-check"], &post), 0);
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }

    // Every entry was popped by its own post-check.
    let inflight = std::fs::read_to_string(root.join(".phronesis/journey/inflight")).unwrap();
    assert_eq!(inflight.trim(), "", "leftover inflight entries: {inflight}");

    run(
        &root,
        &["claude-hook", "UserPromptSubmit"],
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"and now this"}"#,
    );

    let records = journal(&root);
    assert!(
        !records.iter().any(|r| r["kind"] == "interrupt"),
        "no interrupt may be inferred from completed tool calls"
    );
    let modes: Vec<&str> = records
        .iter()
        .filter(|r| r["kind"] == "prompt")
        .map(|r| r["mode"].as_str().unwrap())
        .collect();
    assert_eq!(modes.len(), 2, "{modes:?}");
    assert_eq!(modes[1], "mid_turn", "{modes:?}");
    assert!(!modes.contains(&"correction"), "{modes:?}");

    // Every tool record survived: the journal append is serialized too.
    assert_eq!(
        records.iter().filter(|r| r["tool"] == "Bash").count(),
        N,
        "lost tool records under concurrency"
    );
}

#[test]
fn a_subagents_live_tool_call_does_not_make_the_parent_prompt_a_correction() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    run(
        &root,
        &["claude-hook", "UserPromptSubmit"],
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"go"}"#,
    );
    // A sub-agent's call is still in flight; the parent speaks.
    run(
        &root,
        &["pre-check"],
        r#"{"tool_name":"Bash","tool_use_id":"tu-x","agent_id":"sub-1","tool_input":{"command":"sleep 100"}}"#,
    );
    run(
        &root,
        &["claude-hook", "UserPromptSubmit"],
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"meanwhile"}"#,
    );
    let records = journal(&root);
    assert!(!records.iter().any(|r| r["kind"] == "interrupt"), "scope leak");
    let modes: Vec<&str> = records
        .iter()
        .filter(|r| r["kind"] == "prompt")
        .map(|r| r["mode"].as_str().unwrap())
        .collect();
    assert_eq!(modes, vec!["fresh", "mid_turn"]);
}
```

- [ ] **Step 2: Run it**

Run: `cargo test -p phronesis-mcp --test lifecycle_concurrency 2>&1 | tail -30`
Expected: PASS given Tasks 3 and 4. If `concurrent_tool_pairs_never_produce_a_correction` fails on leftover `inflight` entries, the bug is a non-atomic read-modify-write in `state::with_locked` (Plan 1 Task 5) — fix it there, not by weakening this test. If it fails on lost tool records, the same applies to `journal::append`.

- [ ] **Step 3: Commit**

```bash
git add crates/phronesis-mcp/tests/lifecycle_concurrency.rs
git commit -m "test(lifecycle): concurrency gate for the Claude adapter"
```

---

### Task 7: Promote the captured payloads into the contract corpus

**Files:**
- Create: `crates/phronesis-mcp/tests/fixtures/payloads/claude/{user-prompt-submit,session-start,session-end,subagent-start,subagent-stop,stop}.json`
- Modify: `crates/phronesis-mcp/tests/payload_contract.rs`

`collect_fixtures` walks `tests/fixtures/payloads/<cli>/*.json`, `run_subcommand` passes the `subcommand` field as one argument, and `claude-hook`'s event argument defaults to `UserPromptSubmit` while the payload's own `hook_event_name` wins — so every envelope below uses `"subcommand": "claude-hook"` and carries `hook_event_name` in its payload.

- [ ] **Step 1: Write the failing test**

Append to `crates/phronesis-mcp/tests/payload_contract.rs`:

```rust
/// Pin the field sets the Claude adapter reads. These are the keys the
/// captured payloads actually carried (Task 1 of the Claude adapter plan); a
/// host that stops sending one must fail here rather than silently degrade
/// classification. `agent_type` may be the empty string
/// (anthropics/claude-code#87065) but the key must exist.
#[test]
fn captured_claude_payloads_carry_the_documented_fields() {
    let raw_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/payloads/claude/raw");
    let expected: &[(&str, &[&str])] = &[
        ("UserPromptSubmit", &["hook_event_name", "session_id", "transcript_path", "prompt"]),
        ("SessionStart", &["hook_event_name", "session_id", "transcript_path"]),
        ("SessionEnd", &["hook_event_name", "session_id"]),
        ("SubagentStart", &["hook_event_name", "session_id", "agent_id", "agent_type"]),
        ("SubagentStop", &["hook_event_name", "session_id", "agent_id", "stop_hook_active"]),
        ("Stop", &["hook_event_name", "session_id", "stop_hook_active"]),
    ];
    for (event, keys) in expected {
        let path = raw_dir.join(format!("{event}.json"));
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{}: {e} — capture it per the plan's Task 1", path.display()));
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        for key in *keys {
            assert!(v.get(key).is_some(), "{event}.json is missing {key}");
        }
        assert!(
            !raw.contains("/Users/"),
            "{event}.json still holds an unredacted home path"
        );
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --test payload_contract captured_claude 2>&1 | tail -20`
Expected: FAIL naming the first key the captures do not carry — or PASS immediately if the captures happen to carry every key, which is also a valid outcome for a pin test.

If a key is genuinely absent from the capture, delete it from the `expected` list and add a one-line comment saying which event did not send it. Do not edit the capture.

- [ ] **Step 3: Write the fixture envelopes**

One file per event, using the captured payload verbatim. `crates/phronesis-mcp/tests/fixtures/payloads/claude/subagent-stop.json`:

```json
{
  "schema": 1,
  "source": {
    "cli": "claude-code",
    "event": "SubagentStop",
    "provenance": "captured-and-scrubbed",
    "capture": {
      "host": "claude-code",
      "host_version": "<paste `claude --version` from raw/README.md>",
      "capture_date": "2026-09-18",
      "scrubber_version": "manual-redaction-1"
    },
    "description": "Real SubagentStop payload; prompt-bearing fields hand-redacted."
  },
  "subcommand": "claude-hook",
  "packs": "rust",
  "payload": { "...": "paste the object from raw/SubagentStop.json here" },
  "expect": {
    "exit": 0,
    "stdout_json": true,
    "log_rule_fired": null,
    "journal_tag_new": ["lifecycle:subagent_stop"],
    "journal_tag_from_output": [],
    "stderr_contains": []
  }
}
```

Repeat for the other five, changing `source.event`, the payload, and `journal_tag_new`:

| file | `journal_tag_new` |
|---|---|
| `user-prompt-submit.json` | `["lifecycle:prompt"]` |
| `session-start.json` | `[]` (SessionStart writes state, not a record) |
| `session-end.json` | `[]` (no open turn in a replayed fixture) |
| `subagent-start.json` | `["lifecycle:subagent_start"]` |
| `subagent-stop.json` | `["lifecycle:subagent_stop"]` |
| `stop.json` | `["lifecycle:stop"]` |

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --test payload_contract 2>&1 | tail -30`
Expected: `captured_claude_payloads_carry_the_documented_fields`, `corpus_replays_green`, and `provenance_coverage_report` all pass; the coverage report now shows `claude-code` with six captured fixtures.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/tests/fixtures/payloads/claude crates/phronesis-mcp/tests/payload_contract.rs
git commit -m "test(contract): pin the captured Claude lifecycle payloads"
```

---

### Task 8: Changelog

**Files:**
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Add the entry**

Under `## [Unreleased]` → `### Added`, above the existing bullets:

```markdown
- **Agent lifecycle events from Claude Code.** A new `phr-mcp claude-hook
  <Event>` adapter records prompts (with `fresh` / `mid_turn` / `correction`
  mode), inferred interrupts, turn stops, and sub-agent start/stop into the
  journey journal and the action log. `phr-mcp init` registers `SubagentStart`,
  `SubagentStop`, `Stop`, and `SessionEnd`, and repoints `UserPromptSubmit` and
  `SessionStart` at the adapter; replacement is now keyed on the command, so a
  hook you wrote yourself on the same event survives `init`. `session-context`
  and `interaction-context` keep working for settings files written by older
  versions.

- **Commit detection from ground truth.** `pre-check` records `HEAD` before a
  shell call and `post-check` compares it after, so a commit is recorded by
  observing the repository rather than by matching command text. Gemini CLI's
  `invoke_agent` tool is derived into the same sub-agent start/stop pair.
```

- [ ] **Step 2: Run the full suite**

Run: `cargo test -p phronesis-mcp 2>&1 | tail -30`
Expected: all green.

- [ ] **Step 3: Commit**

```bash
git add CHANGELOG.md
git commit -m "docs(changelog): Claude lifecycle adapter and commit detection"
```

---

## Self-review

**1. Spec coverage** (§"Host adapters / Claude Code", plus the pre/post wiring in §"Where the writes happen", §"Classification", §"Success signal: commit", and the Gemini `invoke_agent` half of §"Host adapters / Gemini CLI"):

| spec requirement | task |
|---|---|
| `claude-hook <Event>` subcommand, `ClaudePayload`, all `#[serde(default)]` | 2 |
| stdin via `security::read_stdin_capped`, tool phases delegate to the existing runners | 2 |
| failure policy: `{}` exit 0 on non-tool events; tool events keep today's codes | 2 |
| response shapes: context JSON or `{}`; `SubagentStart` `{}`; `Stop`/`SubagentStop` block shape; `stop_hook_active` short-circuit | 2 |
| Gemini `BeforeAgent` → prompt, `AfterAgent` → stop in the same adapter | 2 (dispatch), 3 (host tagging test) |
| `UserPromptSubmit` → render interaction context, then record `prompt` | 3 |
| classification: scrub → classify → interrupt record with `inferred_from` → prompt with mode → `open_turn` | 3 |
| `SessionStart` overwrites `session`, truncates `agents`/`inflight`, renders session context | 3 |
| `SessionEnd` records `stop` when open, truncates `session`, closes `turn` | 3 |
| `SubagentStart` push + record; `SubagentStop` pop, `duration_secs`, `matched_start` | 3 |
| payload capture redaction (`redact_for_capture` inside `capture_raw_payload`) | 2 (call site; the redaction itself is Plan 1 Task 10) |
| `inflight` push after `read_payload`, before the allowlist; `head_before` for shell tools | 4 |
| blocked pre-check pops its own entry | 4 |
| `detect_commit` in post; `commit` record with `sha`, `head_before`, `confidence_band`, `tool_use_id` | 4 |
| Gemini `invoke_agent` derivation; `invoke_agent` in both allowlists | 4 |
| tool records write `v: JOURNAL_V` | 4 |
| command-keyed `upsert_hook_by_command`; four new events; two repointed; aliases kept | 5 |
| foreign `""`-matcher `Stop` hook survives init | 5 |
| `tests/lifecycle_concurrency.rs` gate | 6 |
| captured fixtures under `tests/fixtures/payloads/claude/`, pinned in `payload_contract.rs` | 1, 7 |
| CHANGELOG | 8 |

Out of scope by the spec's own rollout graph and not covered here: the Codex adapter (step 3), the Gemini *registrations* in `write_gemini_settings` and the anchored `BeforeTool` matcher (step 4), and stats/metrics/`journey`/`kalpa` rendering (step 5). The `journey_derive` determinism fixture and every `lifecycle::*` unit test belong to Plan 1.

**2. Placeholder scan:** the only intentional fill-in-the-blank is Task 7's fixture envelopes, which paste a *captured* payload that cannot exist before Task 1 runs on a real host; the surrounding envelope, the table of expected tags, and the pin test are fully written. Task 1 is explicitly marked as requiring the human and gives the exact settings JSON and the exact sentences to type. No "TBD", no "add error handling", no "similar to Task N".

**3. Shared ownership:** `upsert_hook_by_command` is defined here, in Task 5, and nowhere else. Plan 4 consumes it. `hook/mod.rs`'s `HookPayload`, `redact_for_capture` and `capture_raw_payload` visibility belong to Plan 1 Task 10; this plan adds exactly one line (`mod lifecycle_wiring;`) to that file.

---

## Merge notes

Plans 2, 3, and 4 are executed in separate worktrees off the same Plan 1 base and merged in the order 1 → 2 → 3 → 4 → 5. Every file more than one of them touches is listed here with the exact region each plan owns, so the merge is mechanical.

| file | Plan 2 (this plan) owns | Plan 3 (Codex) owns | Plan 4 (Gemini) owns |
|---|---|---|---|
| `src/init.rs` | `write_settings` (`:580-622`) — the two `upsert_hook(context_entry(…))` calls become one `upsert_hook_by_command` loop over six events; **and** the new `upsert_hook_by_command` fn + its two unit tests, added immediately after `upsert_hook` (which ends at `:1559`) | `write_codex_hooks`'s `for (event, matcher)` table (`:735-757`) only | `write_gemini_settings`'s hook block (`:676-706`) and the `report.steps.push` note after `write_json` (`:714`) only. **Plan 4 adds no `upsert_hook_by_command`** — it is already present from this plan. |
| `src/main.rs` | `Command::ClaudeHook` variant (immediately after `CodexHook`, before Plan 1's `Kalpa`) + its dispatch arm in the same relative position | — | — |
| `src/lib.rs` | `pub mod claude_hook;` in alphabetical position, immediately before `pub mod codex_hook;` | — | — |
| `src/hook/mod.rs` | one line: `mod lifecycle_wiring;` next to `pub(crate) mod seq;` (`:10`) | — (Plan 3 must not edit this file) | — |
| `tests/fixtures/hook_events.json` | the `"claude-code"` array only | the `"codex"` array only | the `"gemini"` array only |
| `tests/hook_integration.rs` | appends `run_claude_hook`, `journal_records`, `log_entries`, `inflight_keys`, and Tasks 2/3/4's tests | — | appends `run_hook_at`, `lifecycle_records`, `lifecycle_log`, and Tasks 4/5's `gemini_*` tests. Helper names are disjoint from this plan's on purpose; append at the **end of the file** so the two blocks never interleave. |
| `tests/init_integration.rs` | appends `claude_settings`, `commands_for`, and the three `init_*_claude*` tests | — | appends `gemini_settings`, `only_command`, and the four `init_*_gemini*` tests, at the end of the file |
| `tests/payload_contract.rs` | appends `captured_claude_payloads_carry_the_documented_fields` (Task 7) | may need a one-line widening of `journal_tag_new` and updating the `.codex/hooks.json` matcher assertion (Task 7/8) | — |
| `CHANGELOG.md` | two bullets under `## [Unreleased]` → `### Added` | one bullet, appended after this plan's | one bullet, appended after Plan 3's |

Known duplicate coverage, deliberate and harmless: this plan's `invoke_agent_derives_a_subagent_pair_before_the_allowlist` (Task 4) and Plan 4's `gemini_invoke_agent_pairs_a_subagent_start_and_stop` (its Task 4) both exercise the `invoke_agent` derivation. Plan 4's is the acceptance test (it also pins stdout shape and start/stop id equality); keep both.

**3. Type consistency:** `record(root, ev) -> Option<Stamped>` is used as an `Option` everywhere (Tasks 3 and 4 both `.as_ref().map(|s| s.ts)`). `state::pop_agent(root, Option<&str>)` and `state::pop_inflight(root, &str)` match Plan 1's signatures; `classify_prompt(root, &PromptContext)` is built with exactly Plan 1's six fields. `LifecycleEvent::with_extra` is only ever passed values with `Into<serde_json::Value>` (`&str`, `String`, `bool`, `u64`). `outcome::detect_commit(root, Option<&str>, &str, Option<i32>)` matches Plan 1 Task 9, and `Commit { sha, head_before }` is destructured by those field names. `synth_agent_id` is defined once, in `claude_hook.rs`, and used by `hook/lifecycle_wiring.rs`; `unix_secs_now` likewise, so the two modules cannot drift on the clock. `host_for_tool` in the wiring module and `host_for` in the adapter are deliberately different functions (tool name vs event name) and are named differently.

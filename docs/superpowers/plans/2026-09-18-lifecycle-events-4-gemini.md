# Lifecycle Events — Plan 4: Gemini CLI registrations (spec step 4)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `phr-mcp init` wire Gemini CLI to the lifecycle hooks — `BeforeAgent`, `AfterAgent`, `SessionStart`, `SessionEnd` → `phr-mcp claude-hook <Event>`, and an anchored `BeforeTool`/`AfterTool` matcher that includes `invoke_agent` — so Gemini emits prompts, stops, interrupts, and sub-agent pairs end to end.

**Architecture:** This plan is registrations plus one install-output line. It edits only `init.rs::write_gemini_settings` (and one array in a test fixture), calling the command-keyed `upsert_hook_by_command` that **Plan 2 Task 5** adds beside the existing matcher-keyed `upsert_hook`. The behaviour behind the registrations — the `claude-hook` adapter's Gemini name mapping (`BeforeAgent` → `prompt`, `AfterAgent` → `stop`) and the `invoke_agent` sub-agent derivation in `pre-check`/`post-check` — is owned by Plan 2. This plan's end-to-end tests exercise that behaviour through the CLI and therefore only pass once Plan 2 has landed; they are written here because this plan owns the Gemini surface.

**Tech Stack:** Rust 2024 edition (rust-version 1.90), serde_json, tempfile, `assert`-style integration tests driving `CARGO_BIN_EXE_phr-mcp`. No new dependencies.

**Spec:** `docs/specs/SPEC-agent-lifecycle-events.md` (revised 2026-09-18), §"Host adapters / Gemini CLI", §Classification (`open_turn` branch), §Adjacent findings 5 and 6, Rollout step 4.

## Global Constraints

- No new crate dependencies.
- Gemini exit 0 with non-JSON stdout becomes a user-visible `systemMessage`. **Every hook invocation this plan registers must print `{}` or valid context JSON on stdout, never empty.**
- Gemini's `BeforeTool` matcher is a regex it does not anchor. The generated matcher is the exact string `^(replace|write_file|run_shell_command|invoke_agent)$`.
- Registration replacement is **command-keyed**: only entries whose command starts with `phr-mcp ` are replaced. A user's own hook with the same matcher survives `phr-mcp init`.
- Lifecycle writes are best-effort: a failure prints `phronesis: …` on stderr and never fails the hook.
- Journal lifecycle records carry `tool: "__lifecycle"`, `path: ""`, and `host: "gemini"`.

**Depends on:** **Plan 1 and Plan 2, both merged, before any task after Task 0 runs.** Task 0 (fixture capture) is the spec's rollout node 0 for Gemini and needs a live host rather than any code, so it runs in parallel with Plan 1.

- Plan 1 (`docs/superpowers/plans/2026-09-18-lifecycle-events-1-foundation.md`) supplies `lifecycle::{event, state, record}` and `classify_prompt`'s `open_turn` branch.
- Plan 2 (`docs/superpowers/plans/2026-09-18-lifecycle-events-2-claude-adapter.md`) supplies **`init.rs::upsert_hook_by_command`**, the `phr-mcp claude-hook <Event>` subcommand, and the `invoke_agent` derivation in `pre-check`/`post-check`. This plan defines none of them. Task 1 of this plan used to define `upsert_hook_by_command`; it has been removed so there is exactly one definition, in Plan 2 Task 5.

Consequently this plan is **not** independently mergeable: merge Plan 2 first, then this plan. Every task here is red without Plan 2. Plan 3 (Codex) is independent and may merge in any position among the three.

**Interfaces consumed from Plan 2** (do not re-implement; if a name differs when Plan 2 lands, adapt the test, not the behaviour):
- `phr-mcp claude-hook <Event>` reads a payload on stdin and dispatches on `hook_event_name`, mapping `BeforeAgent` → `Kind::Prompt`, `AfterAgent` → `Kind::Stop`, both with `Host::Gemini`.
- `pre-check`/`post-check` derive `subagent_start`/`subagent_stop` when `tool_name == "invoke_agent"`, with `agent_type = tool_input.agent_name` and a synthesized `agent_id`, before the tool allowlist match.
- `fn upsert_hook_by_command(settings: &mut Value, event: &str, new_entry: Value)`, private to `init.rs`, added by Plan 2 Task 5 immediately after `upsert_hook` (which ends at `init.rs:1559`), together with its two unit tests `upsert_hook_by_command_replaces_ours_and_keeps_foreign` and `upsert_hook_by_command_creates_missing_event_array`. It replaces only entries whose command starts with `phr-mcp `, leaving every foreign hook in place. That is exactly the behaviour Task 1 below relies on: matcher-keyed `upsert_hook` (`init.rs:1544-1559`) would delete a foreign hook sharing our matcher (spec §Adjacent findings 7), and would leave a stale entry behind when *we* change our own matcher or command — both of which happen in Task 1.

**Files this plan owns exclusively:** `crates/phronesis-mcp/tests/fixtures/payloads/gemini/**` (Task 0). None of the *source* files are exclusive; the rest of this plan is registrations plus one install-output line. Its exclusive regions are listed in §Merge notes.

**Files shared with Plans 2 and 3** (regions disjoint; see §Merge notes): `src/init.rs` (only `write_gemini_settings`), `tests/init_integration.rs`, `tests/hook_integration.rs`, `tests/fixtures/hook_events.json` (only the `"gemini"` array), `tests/payload_contract.rs`, `CHANGELOG.md`.

---

### Task 0: Capture real Gemini payloads (rollout node 0 — REQUIRES THE HUMAN)

**This task gates the rest of the plan.** Spec §"Host adapters / Gemini CLI":
"Fixtures are a precondition for this adapter too, on the same terms as the
Claude ones and for the same reason — this repo has already misread a Gemini
field name once (§Adjacent findings 4). Before step 4: `BeforeAgent`,
`AfterAgent`, `SessionStart`, `SessionEnd`, and `BeforeTool`/`AfterTool` for
`invoke_agent`, committed under `tests/fixtures/payloads/gemini/`."

The misread is not hypothetical: `hook/mod.rs:57-64` says Gemini sends
`tool_output` when it actually sends `tool_response`. Tasks 3 and 4 below are
written against field names taken from Gemini's TypeScript sources, and this task
is what turns them from a reading into evidence. It runs alongside Plan 1, not
after it.

**Files:**
- Create: `crates/phronesis-mcp/tests/fixtures/payloads/gemini/raw/{BeforeAgent,AfterAgent,SessionStart,SessionEnd,BeforeTool-invoke_agent,AfterTool-invoke_agent}.json`
- Create: `crates/phronesis-mcp/tests/fixtures/payloads/gemini/raw/README.md`
- Modify: `crates/phronesis-mcp/tests/payload_contract.rs`

- [ ] **Step 1: Ask the human to add the capture hooks**

Give them this verbatim, for `.gemini/settings.json` in a scratch project (not
this repo — Gemini's `invoke_agent` needs a project it is willing to work in).
It uses `sh`, not `phr-mcp`, so the capture does not depend on anything this plan
builds:

```json
{
  "hooks": {
    "SessionStart": [{"matcher": "", "hooks": [{"type": "command", "command": "sh -c 'mkdir -p /tmp/phr-gemini && cat > /tmp/phr-gemini/SessionStart.json; echo {}'"}]}],
    "SessionEnd":   [{"matcher": "", "hooks": [{"type": "command", "command": "sh -c 'mkdir -p /tmp/phr-gemini && cat > /tmp/phr-gemini/SessionEnd.json; echo {}'"}]}],
    "BeforeAgent":  [{"matcher": "", "hooks": [{"type": "command", "command": "sh -c 'mkdir -p /tmp/phr-gemini && cat > /tmp/phr-gemini/BeforeAgent.json; echo {}'"}]}],
    "AfterAgent":   [{"matcher": "", "hooks": [{"type": "command", "command": "sh -c 'mkdir -p /tmp/phr-gemini && cat > /tmp/phr-gemini/AfterAgent.json; echo {}'"}]}],
    "BeforeTool":   [{"matcher": "^invoke_agent$", "hooks": [{"type": "command", "command": "sh -c 'mkdir -p /tmp/phr-gemini && cat > /tmp/phr-gemini/BeforeTool-invoke_agent.json; echo {}'"}]}],
    "AfterTool":    [{"matcher": "^invoke_agent$", "hooks": [{"type": "command", "command": "sh -c 'mkdir -p /tmp/phr-gemini && cat > /tmp/phr-gemini/AfterTool-invoke_agent.json; echo {}'"}]}]
  }
}
```

- [ ] **Step 2: Ask the human to drive one session**

1. Start `gemini` in the scratch project and **trust the folder** when prompted —
   project hooks are skipped until then, which is the second half of Task 2's
   install note.
2. Type `list the files here` — that writes `SessionStart.json`,
   `BeforeAgent.json`, and `AfterAgent.json`.
3. Ask for a sub-agent explicitly: `use a sub-agent to summarise README.md`.
   That writes the two `invoke_agent` files. Note the exact `agent_name` value:
   Gemini's built-ins use snake_case (`codebase_investigator`);
   `sanitize_agent_type` keeps `[a-z0-9][a-z0-9_.:-]{0,63}` after lowercasing, so
   the name survives as `lifecycle:agent:codebase_investigator`. Record the real
   value in the README; Task 3 below asserts the sanitizer on a snake_case name
   and on a hostile one.
4. Press Esc during a long answer, then type another prompt. `AfterAgent` does not
   fire on an abort, so its file keeps the *previous* turn's content — which is
   exactly the fact the `open_turn` inference rests on. Confirm the file's
   `prompt` is the older one and say so in the README.
5. `/quit` — that writes `SessionEnd.json`.

- [ ] **Step 3: Redact and commit**

Same treatment as the Claude captures: replace `prompt`, `prompt_response` and
any `tool_input.prompt` with `"<redacted:N bytes>"`, absolute home paths with
`/home/dev`, and real ids with synthetic ones (`gemini-s-001`). Keep every key,
including empty-string values.

```bash
mkdir -p crates/phronesis-mcp/tests/fixtures/payloads/gemini/raw
cp /tmp/phr-gemini/*.json crates/phronesis-mcp/tests/fixtures/payloads/gemini/raw/
```

`raw/README.md` records the Gemini CLI version (`gemini --version`), the capture
date, the observed `agent_name`, whether `SessionStart` carries a `source` field,
whether `AfterTool` uses `tool_response` (Adjacent finding 4), and the Esc
observation from step 4.

- [ ] **Step 4: Pin the field sets**

Append to `crates/phronesis-mcp/tests/payload_contract.rs`:

```rust
/// Pin the Gemini field sets the adapter reads. This repo has already misread
/// one of them (`tool_output` vs `tool_response`, spec Adjacent finding 4), so
/// a host that renames a field must fail here rather than silently stop
/// recording.
#[test]
fn captured_gemini_payloads_carry_the_documented_fields() {
    let raw_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/payloads/gemini/raw");
    let expected: &[(&str, &[&str])] = &[
        ("BeforeAgent", &["hook_event_name", "session_id", "prompt"]),
        ("AfterAgent", &["hook_event_name", "session_id", "prompt", "prompt_response"]),
        ("SessionStart", &["hook_event_name", "session_id"]),
        ("SessionEnd", &["hook_event_name", "session_id"]),
        ("BeforeTool-invoke_agent", &["hook_event_name", "tool_name", "tool_input"]),
        ("AfterTool-invoke_agent", &["hook_event_name", "tool_name", "tool_input", "tool_response"]),
    ];
    for (name, keys) in expected {
        let path = raw_dir.join(format!("{name}.json"));
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{}: {e} — capture it per this plan's Task 0", path.display()));
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        for key in *keys {
            assert!(v.get(key).is_some(), "{name}.json is missing {key}");
        }
        assert!(!raw.contains("/Users/"), "{name}.json still holds an unredacted home path");
    }
    // The misread field name, pinned from the other side.
    let after: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(raw_dir.join("AfterTool-invoke_agent.json")).unwrap(),
    )
    .unwrap();
    assert!(after.get("tool_output").is_none(), "Gemini sends tool_response, not tool_output");
    // And the sub-agent's name, whatever it turned out to be, must be a string.
    assert!(
        after.pointer("/tool_input/agent_name").and_then(|v| v.as_str()).is_some(),
        "invoke_agent carries tool_input.agent_name"
    );
}
```

If a key is genuinely absent from a capture, delete it from `expected` and add a
one-line comment naming the event that did not send it. Do not edit the capture.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/tests/fixtures/payloads/gemini crates/phronesis-mcp/tests/payload_contract.rs
git commit -m "test(fixtures): capture real Gemini CLI lifecycle payloads"
```

---

### Task 1: Gemini lifecycle registrations and anchored tool matcher

**Files:**
- Modify: `crates/phronesis-mcp/src/init.rs:676-706` (the hook block inside `write_gemini_settings`)
- Test: `crates/phronesis-mcp/tests/init_integration.rs` (append)

**Interfaces:**
- Consumes: `upsert_hook_by_command`, defined by **Plan 2 Task 5** in `init.rs`. This plan adds no definition of it.
- Produces: `.gemini/settings.json` with `hooks.BeforeTool`, `hooks.AfterTool`, `hooks.SessionStart`, `hooks.SessionEnd`, `hooks.BeforeAgent`, `hooks.AfterAgent`, each holding exactly one `phr-mcp` entry.

- [ ] **Step 1: Write the failing integration tests**

Append to `crates/phronesis-mcp/tests/init_integration.rs`:

```rust
fn gemini_settings(dir: &Path) -> serde_json::Value {
    serde_json::from_str(
        &std::fs::read_to_string(dir.join(".gemini/settings.json")).expect("gemini settings"),
    )
    .expect("gemini settings JSON")
}

fn only_command(settings: &serde_json::Value, event: &str) -> String {
    let arr = settings["hooks"][event]
        .as_array()
        .unwrap_or_else(|| panic!("no {event} hook array: {settings}"));
    assert_eq!(arr.len(), 1, "expected exactly one {event} entry: {arr:?}");
    arr[0]["hooks"][0]["command"].as_str().unwrap().to_string()
}

#[test]
fn init_registers_gemini_lifecycle_hooks() {
    let dir = tempfile::tempdir().unwrap();
    run_init(&[], dir.path());
    let s = gemini_settings(dir.path());
    for event in ["SessionStart", "SessionEnd", "BeforeAgent", "AfterAgent"] {
        assert_eq!(only_command(&s, event), format!("phr-mcp claude-hook {event}"));
        assert_eq!(s["hooks"][event][0]["matcher"], "", "{event} matcher must be empty");
    }
}

#[test]
fn init_anchors_gemini_tool_matcher_and_includes_invoke_agent() {
    let dir = tempfile::tempdir().unwrap();
    run_init(&[], dir.path());
    let s = gemini_settings(dir.path());
    for (event, command) in [("BeforeTool", "phr-mcp pre-check"), ("AfterTool", "phr-mcp post-check")] {
        assert_eq!(only_command(&s, event), command);
        assert_eq!(
            s["hooks"][event][0]["matcher"],
            "^(replace|write_file|run_shell_command|invoke_agent)$"
        );
    }
}

#[test]
fn init_migrates_old_gemini_registrations_in_place() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".gemini")).unwrap();
    std::fs::write(
        dir.path().join(".gemini/settings.json"),
        r#"{"hooks":{
            "BeforeTool":[{"matcher":"replace|write_file|run_shell_command",
                           "hooks":[{"type":"command","command":"phr-mcp pre-check"}]}],
            "SessionStart":[{"matcher":"","hooks":[{"type":"command","command":"phr-mcp session-context"}]}],
            "BeforeAgent":[{"matcher":"","hooks":[{"type":"command","command":"phr-mcp interaction-context"}]}]
        }}"#,
    )
    .unwrap();
    run_init(&[], dir.path());
    let s = gemini_settings(dir.path());
    assert_eq!(only_command(&s, "SessionStart"), "phr-mcp claude-hook SessionStart");
    assert_eq!(only_command(&s, "BeforeAgent"), "phr-mcp claude-hook BeforeAgent");
    assert_eq!(
        s["hooks"]["BeforeTool"][0]["matcher"],
        "^(replace|write_file|run_shell_command|invoke_agent)$"
    );
}

#[test]
fn init_gemini_hooks_are_idempotent_and_spare_foreign_hooks() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".gemini")).unwrap();
    std::fs::write(
        dir.path().join(".gemini/settings.json"),
        r#"{"hooks":{"AfterAgent":[{"matcher":"","hooks":[{"type":"command","command":"my-own-tool"}]}]}}"#,
    )
    .unwrap();
    run_init(&[], dir.path());
    run_init(&[], dir.path());
    let s = gemini_settings(dir.path());
    for event in ["BeforeTool", "AfterTool", "SessionStart", "SessionEnd", "BeforeAgent"] {
        assert_eq!(s["hooks"][event].as_array().unwrap().len(), 1, "{event} duplicated");
    }
    let after = s["hooks"]["AfterAgent"].as_array().unwrap();
    assert_eq!(after.len(), 2, "foreign AfterAgent hook must survive init: {after:?}");
    assert_eq!(after[0]["hooks"][0]["command"], "my-own-tool");
    assert_eq!(after[1]["hooks"][0]["command"], "phr-mcp claude-hook AfterAgent");
}
```

- [ ] **Step 2: Run the tests and watch them fail**

Run: `cargo test -p phronesis-mcp --test init_integration gemini`
Expected: FAIL — `SessionStart` is still `phr-mcp session-context`, `AfterAgent`/`SessionEnd` arrays are absent, matcher is unanchored.

- [ ] **Step 3: Replace the hook block in `write_gemini_settings`**

Replace everything from `// BeforeTool / AfterTool hooks` through the `upsert_hook(&mut settings, "BeforeAgent", …)` call (`init.rs:676-706`) with:

```rust
    // BeforeTool / AfterTool hooks. Gemini treats `matcher` as an unanchored
    // regex, so the previous `replace|write_file|run_shell_command` matched
    // any tool whose name merely contained one of those words. `invoke_agent`
    // joins the list because Gemini has no sub-agent event: pre-check and
    // post-check derive subagent_start / subagent_stop from that tool.
    let hook_entry = |cmd: &str| {
        json!({
            "matcher": "^(replace|write_file|run_shell_command|invoke_agent)$",
            "hooks": [{"type": "command", "command": cmd}]
        })
    };
    upsert_hook_by_command(&mut settings, "BeforeTool", hook_entry("phr-mcp pre-check"));
    upsert_hook_by_command(&mut settings, "AfterTool", hook_entry("phr-mcp post-check"));

    // Lifecycle + context hooks. An empty matcher fires on every event.
    // BeforeAgent is Gemini's UserPromptSubmit and AfterAgent is its Stop;
    // AfterAgent does not fire on interrupt, which is exactly what the
    // `open_turn` inference in `lifecycle::state::classify_prompt` keys on.
    let lifecycle_entry = |event: &str| {
        json!({
            "matcher": "",
            "hooks": [{"type": "command", "command": format!("phr-mcp claude-hook {event}")}]
        })
    };
    for event in ["SessionStart", "SessionEnd", "BeforeAgent", "AfterAgent"] {
        upsert_hook_by_command(&mut settings, event, lifecycle_entry(event));
    }
```

Then extend **only the `"gemini"` array** in `crates/phronesis-mcp/tests/fixtures/hook_events.json`, or `payload_contract.rs::init_wires_hooks_only_under_event_names_that_exist` fails with `init wired unknown Gemini hook event "AfterAgent"`. Plan 2 edits the `"claude-code"` line and Plan 3 the `"codex"` line, so the three-way merge is line-disjoint:

```json
  "gemini": ["BeforeTool", "AfterTool", "SessionStart", "SessionEnd", "BeforeAgent", "AfterAgent"],
```

- [ ] **Step 4: Run the tests and watch them pass**

Run: `cargo test -p phronesis-mcp --test init_integration gemini && cargo test -p phronesis-mcp --lib gemini && cargo test -p phronesis-mcp --test payload_contract init_wires_hooks`
Expected: PASS. `init.rs`'s own unit tests `write_gemini_settings_includes_session_and_before_agent_hooks` (`:4293`), `write_gemini_settings_creates_full_wiring` (`:4598`), `write_gemini_settings_preserves_user_hooks_and_drops_legacy_event` (`:4624`) and `write_gemini_settings_is_idempotent_and_dry_run_safe` (`:4665`) all assert the old commands or the unanchored matcher; update each expected string to `phr-mcp claude-hook <Event>` / `^(replace|write_file|run_shell_command|invoke_agent)$` in the same commit.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/init.rs crates/phronesis-mcp/tests/init_integration.rs crates/phronesis-mcp/tests/fixtures/hook_events.json
git commit -m "feat(init): register Gemini lifecycle hooks and anchor the tool matcher"
```

---

### Task 2: Install-output note for Gemini

**Files:**
- Modify: `crates/phronesis-mcp/src/init.rs:714` (after the `write_json` call in `write_gemini_settings`)
- Test: `crates/phronesis-mcp/tests/init_integration.rs` (append)

**Interfaces:**
- Consumes: `InitReport { steps: Vec<String>, warnings: Vec<String> }` (`init.rs:294`). `handle_init` in `main.rs:1948-1950` prints every `step` to stdout, so a pushed step is the install output.
- Produces: one extra line in `report.steps` after the `.gemini/settings.json` line.

**Why:** two Gemini facts from the spec that silently change what a user sees. Gemini HTML-escapes `additionalContext`, so `<` and `>` in rule text reach the model as entities (§Adjacent findings 6), and Gemini skips project-level hooks entirely until the folder is trusted, so a fresh `init` looks inert.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn init_prints_gemini_escaping_and_trust_note() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_init(&[], dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("HTML-escapes additionalContext"),
        "install output must warn about Gemini entity escaping: {stdout}"
    );
    assert!(
        stdout.contains("until the folder is trusted"),
        "install output must warn that project hooks are skipped until trust: {stdout}"
    );
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p phronesis-mcp --test init_integration init_prints_gemini_escaping_and_trust_note`
Expected: FAIL on the first assertion.

- [ ] **Step 3: Push the note**

In `write_gemini_settings`, directly after the existing `write_json(&path, &settings, opts, ".gemini/settings.json", report)?;`:

```rust
    report.steps.push(
        "  note: Gemini HTML-escapes additionalContext (< and > reach the model as entities) \
         and skips project hooks until the folder is trusted."
            .to_string(),
    );
```

- [ ] **Step 4: Run the test and watch it pass**

Run: `cargo test -p phronesis-mcp --test init_integration`
Expected: PASS, whole file green.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/init.rs crates/phronesis-mcp/tests/init_integration.rs
git commit -m "docs(init): note Gemini entity escaping and folder trust in install output"
```

---

### Task 3: End-to-end `invoke_agent` sub-agent pair

**Files:**
- Test: `crates/phronesis-mcp/tests/hook_integration.rs` (append)

**Interfaces:**
- Consumes: the `invoke_agent` derivation in `hook/pre.rs` and `hook/post.rs` (Plan 2), `lifecycle::state::{push_agent, pop_agent}` and `lifecycle::event::{Kind, Host}` (Plan 1).
- Produces: nothing. This is the acceptance test for "Gemini sub-agents are observable".

**Note for the executor:** Plan 2 is a hard prerequisite for this whole plan (see the header), so by the time you run this the derivation exists and the test must pass outright. Do **not** mark it `#[ignore]`: Task 5's full-suite run does not include ignored tests, so an ignore added "temporarily" is an acceptance test that never runs again. If Plan 2 has not merged, stop and merge it first.

- [ ] **Step 1: Write the failing test**

```rust
use serde_json::Value;

/// Runs a hook subcommand with `PHRONESIS_PROJECT_ROOT` pinned to `root` and
/// returns (exit code, stdout, stderr).
fn run_hook_at(root: &Path, args: &[&str], payload: &str) -> (i32, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(args)
        .env("PHRONESIS_PROJECT_ROOT", root)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn phr-mcp");
    child.stdin.take().unwrap().write_all(payload.as_bytes()).unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// `pre-check` and `post-check` print **nothing** on stdout when they allow —
/// verified against the current tree: neither `hook/pre.rs` nor `hook/post.rs`
/// contains a `println!`, and no plan in this set changes that. On Gemini that
/// empty stdout is benign (Gemini only turns *non-JSON* stdout into a
/// `systemMessage`), but it is not the `{}` the Gemini section of the spec asks
/// for, so this helper accepts empty-or-JSON-object and the gap is reported to
/// the spec owner rather than pinned as if some task implemented it.
fn assert_allow_stdout(stdout: &str) {
    let t = stdout.trim();
    if t.is_empty() {
        return;
    }
    let v: Value = serde_json::from_str(t)
        .unwrap_or_else(|e| panic!("pre/post stdout must be JSON when non-empty: {t:?}: {e}"));
    assert!(v.is_object(), "stdout must be a JSON object: {t}");
}

fn lifecycle_records(root: &Path) -> Vec<Value> {
    let path = root.join(".phronesis/journey/events.jsonl");
    let body = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
    body.lines()
        .map(|l| serde_json::from_str::<Value>(l).expect("journal line"))
        .filter(|r| r["tool"] == "__lifecycle")
        .collect()
}

fn lifecycle_log(root: &Path) -> Vec<Value> {
    let body = std::fs::read_to_string(root.join(".phronesis/log.jsonl")).expect("action log");
    body.lines()
        .map(|l| serde_json::from_str::<Value>(l).expect("log line"))
        .filter(|e| e["kind"] == "lifecycle")
        .collect()
}

#[test]
fn gemini_invoke_agent_pairs_a_subagent_start_and_stop() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // `agent_name` is sanitized before it becomes a tag: lowercased, then kept
    // only if it matches `[a-z0-9][a-z0-9_.:-]{0,63}` (spec §Correlation state,
    // "`agent_type` is sanitized at the adapter boundary"). Hyphenated and
    // snake_case names both survive; a hostile one (below) is dropped.
    let before = r#"{"hook_event_name":"BeforeTool","session_id":"g1","tool_name":"invoke_agent","tool_input":{"agent_name":"codebase-investigator","prompt":"x"}}"#;
    let (code, stdout, stderr) = run_hook_at(root, &["pre-check"], before);
    assert_eq!(code, 0, "pre-check must allow invoke_agent: {stderr}");
    assert_allow_stdout(&stdout);

    let after = r#"{"hook_event_name":"AfterTool","session_id":"g1","tool_name":"invoke_agent","tool_input":{"agent_name":"codebase-investigator","prompt":"x"},"tool_response":{"output":"done"}}"#;
    let (code, stdout, stderr) = run_hook_at(root, &["post-check"], after);
    assert_eq!(code, 0, "post-check must succeed: {stderr}");
    assert_allow_stdout(&stdout);

    let recs = lifecycle_records(root);
    let starts: Vec<&Value> = recs.iter().filter(|r| r["kind"] == "subagent_start").collect();
    let stops: Vec<&Value> = recs.iter().filter(|r| r["kind"] == "subagent_stop").collect();
    assert_eq!(starts.len(), 1, "exactly one subagent_start: {recs:?}");
    assert_eq!(stops.len(), 1, "exactly one subagent_stop: {recs:?}");
    for r in [starts[0], stops[0]] {
        assert_eq!(r["host"], "gemini");
        assert_eq!(r["path"], "");
        assert_eq!(r["agent_type"], "codebase-investigator");
        let tags = r["tags"].as_array().expect("tags");
        assert!(
            tags.iter().any(|t| t == "lifecycle:agent:codebase-investigator"),
            "agent tag missing: {r}"
        );
    }
    assert_eq!(
        starts[0]["agent"], stops[0]["agent"],
        "the stop must pop the id the start pushed"
    );

    let log = lifecycle_log(root);
    let stop_entry = log
        .iter()
        .find(|e| e["event"] == "subagent_stop")
        .expect("subagent_stop action-log entry");
    assert_eq!(stop_entry["agent_type"], "codebase-investigator");
    assert_eq!(stop_entry["matched_start"], true);
    assert_eq!(stop_entry["host"], "gemini");
    assert_eq!(
        log.iter().filter(|e| e["event"] == "subagent_start").count(),
        1
    );

    // `<invoke_agent>` is the tool record's path: never the sub-agent prompt,
    // which would put content in the journal and in `journey_distinct` on path.
    let tool_paths: Vec<String> = std::fs::read_to_string(root.join(".phronesis/journey/events.jsonl"))
        .unwrap()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|r| r["tool"] == "invoke_agent")
        .map(|r| r["path"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(!tool_paths.is_empty(), "invoke_agent is in both allowlists");
    assert!(tool_paths.iter().all(|p| p == "<invoke_agent>"), "{tool_paths:?}");
}

/// Gemini's built-in agent names are snake_case (`codebase_investigator`). The
/// sanitizer keeps underscores (spec §Correlation state), so the real built-in
/// name survives as a tag a rule can scope to. A hostile `agent_name` with a
/// path separator is dropped and the record carries no `lifecycle:agent:*` tag.
#[test]
fn a_snake_case_gemini_agent_name_survives_and_a_hostile_one_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let before = r#"{"hook_event_name":"BeforeTool","session_id":"g1","tool_name":"invoke_agent","tool_input":{"agent_name":"codebase_investigator","prompt":"x"}}"#;
    let (code, _, stderr) = run_hook_at(root, &["pre-check"], before);
    assert_eq!(code, 0, "{stderr}");
    let start = lifecycle_records(root)
        .into_iter()
        .find(|r| r["kind"] == "subagent_start")
        .expect("the pair is derived");
    assert_eq!(start["agent_type"], "codebase_investigator", "{start}");
    assert!(
        start["tags"].as_array().unwrap().iter().any(|t| t == "lifecycle:agent:codebase_investigator"),
        "snake_case survives as a tag: {start}"
    );

    let dir2 = tempfile::tempdir().unwrap();
    let root2 = dir2.path();
    let hostile = r#"{"hook_event_name":"BeforeTool","session_id":"g2","tool_name":"invoke_agent","tool_input":{"agent_name":"../../etc/passwd","prompt":"x"}}"#;
    let (code, _, stderr) = run_hook_at(root2, &["pre-check"], hostile);
    assert_eq!(code, 0, "{stderr}");
    let start = lifecycle_records(root2)
        .into_iter()
        .find(|r| r["kind"] == "subagent_start")
        .expect("the pair is still derived");
    assert!(
        start.get("agent_type").is_none() || start["agent_type"].is_null(),
        "an unsanitizable name is stored as absent: {start}"
    );
    assert!(
        !start["tags"].as_array().unwrap().iter().any(|t| {
            t.as_str().unwrap_or_default().starts_with("lifecycle:agent:")
        }),
        "and carries no agent tag: {start}"
    );
}
```

- [ ] **Step 2: Run it and read the failure**

Run: `cargo test -p phronesis-mcp --test hook_integration gemini_invoke_agent`
Expected (before Plan 2): FAIL at `lifecycle_records` — `events.jsonl` has no `__lifecycle` record, because `invoke_agent` falls through `exit_ok()` in `pre.rs:30-43`. That is the correct pre-Plan-2 failure, and it is the *first* assertion that fails: `assert_allow_stdout` tolerates today's empty stdout, so the failure names the missing behaviour rather than an unrelated stdout shape. Expected (after Plan 2): PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/phronesis-mcp/tests/hook_integration.rs
git commit -m "test(hooks): gemini invoke_agent produces a paired subagent start and stop"
```

---

### Task 4: End-to-end turn, interrupt, and correction on Gemini

**Files:**
- Test: `crates/phronesis-mcp/tests/hook_integration.rs` (append, reusing Task 3's helpers)

**Interfaces:**
- Consumes: Task 3's helpers, `phr-mcp claude-hook {SessionStart,SessionEnd,BeforeAgent,AfterAgent}` (Plan 2), and `lifecycle::state::classify_prompt`'s `open_turn` branch (Plan 1 Task 8).
- Produces: nothing. This is the acceptance test for "an aborted Gemini turn becomes an interrupt plus a correction".

**Why these tests together** (helpers `run_hook_at`, `lifecycle_records`, `lifecycle_log` come from Task 3, in the same file)**:** `open_turn` inference is only sound if `AfterAgent` reliably closes the turn. One test proves the inference fires when `AfterAgent` is missing; its twin proves it does *not* fire when `AfterAgent` ran. Without the second test a bug that ignores `AfterAgent` would pass.

- [ ] **Step 1: Write the failing tests**

```rust
const G_SESSION_START: &str = r#"{"hook_event_name":"SessionStart","session_id":"g1","source":"startup"}"#;
const G_PROMPT_1: &str = r#"{"hook_event_name":"BeforeAgent","session_id":"g1","prompt":"add a test"}"#;
const G_PROMPT_2: &str = r#"{"hook_event_name":"BeforeAgent","session_id":"g1","prompt":"no, use a temp dir"}"#;
const G_AFTER_AGENT: &str = r#"{"hook_event_name":"AfterAgent","session_id":"g1","prompt":"add a test","prompt_response":"done","stop_hook_active":false}"#;

/// A bare temp project has no rules and no durable context, so every render is
/// empty and `claude-hook` prints the empty object rather than nothing at all.
///
/// `AfterAgent` in particular must print `{}`: the confidence gate is suppressed
/// on Gemini (Plan 2 Task 2), because Gemini has no documented `decision`
/// semantics for that event, so the stop is recorded and nothing is decided.
fn assert_json_object_stdout(stdout: &str) {
    let v: Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("Gemini needs JSON on stdout, got {stdout:?}: {e}"));
    assert!(v.is_object(), "stdout must be a JSON object: {stdout}");
    assert_eq!(v, serde_json::json!({}), "bare project renders no context");
}

#[test]
fn gemini_second_prompt_without_after_agent_is_an_interrupt_and_correction() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    for (args, payload) in [
        (["claude-hook", "SessionStart"], G_SESSION_START),
        (["claude-hook", "BeforeAgent"], G_PROMPT_1),
        (["claude-hook", "BeforeAgent"], G_PROMPT_2),
    ] {
        let (code, stdout, stderr) = run_hook_at(root, &args, payload);
        assert_eq!(code, 0, "{args:?} must exit 0: {stderr}");
        assert_json_object_stdout(&stdout);
    }

    let recs = lifecycle_records(root);
    let kinds: Vec<&str> = recs.iter().filter_map(|r| r["kind"].as_str()).collect();
    assert_eq!(
        kinds,
        ["prompt", "interrupt", "prompt"],
        "a missing AfterAgent means the turn was aborted: {recs:?}"
    );
    assert_eq!(recs[0]["mode"], "fresh");
    assert_eq!(recs[2]["mode"], "correction");
    assert_eq!(recs[2]["host"], "gemini");
    assert!(
        recs[2]["tags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t == "lifecycle:prompt:correction"),
        "correction tag missing: {}",
        recs[2]
    );
    let journal_json = serde_json::to_string(&recs).unwrap();
    // Both prompts: they went through different classification paths, and only
    // checking the second would miss a leak on the `fresh` path.
    assert!(!journal_json.contains("add a test"), "the journal must never carry prompt text: {recs:?}");
    assert!(!journal_json.contains("temp dir"), "the journal must never carry prompt text: {recs:?}");

    let log = lifecycle_log(root);
    let interrupt = log.iter().find(|e| e["event"] == "interrupt").expect("interrupt entry");
    assert_eq!(interrupt["inferred_from"], "open_turn");
    assert_eq!(interrupt["host"], "gemini");
    let correction = log
        .iter()
        .rfind(|e| e["event"] == "prompt")
        .expect("prompt entry");
    assert_eq!(correction["mode"], "correction");
    assert_eq!(correction["prompt"], "no, use a temp dir");
}

#[test]
fn gemini_after_agent_closes_the_turn_so_the_next_prompt_is_fresh() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    for (args, payload) in [
        (["claude-hook", "SessionStart"], G_SESSION_START),
        (["claude-hook", "BeforeAgent"], G_PROMPT_1),
        (["claude-hook", "AfterAgent"], G_AFTER_AGENT),
        (["claude-hook", "BeforeAgent"], G_PROMPT_2),
    ] {
        let (code, stdout, stderr) = run_hook_at(root, &args, payload);
        assert_eq!(code, 0, "{args:?} must exit 0: {stderr}");
        assert_json_object_stdout(&stdout);
    }

    let recs = lifecycle_records(root);
    let kinds: Vec<&str> = recs.iter().filter_map(|r| r["kind"].as_str()).collect();
    assert_eq!(kinds, ["prompt", "stop", "prompt"], "no interrupt: {recs:?}");
    assert_eq!(recs[0]["mode"], "fresh");
    assert_eq!(recs[2]["mode"], "fresh", "AfterAgent must close the turn");
    assert!(
        lifecycle_log(root).iter().all(|e| e["event"] != "interrupt"),
        "a completed turn must not infer an interrupt"
    );
}

/// Task 1 registers `SessionEnd` but nothing else in this plan invokes it, so a
/// broken mapping (claude-hook not handling the Gemini registration, or not
/// recording the stop and truncating the session) would ship green.
/// Spec: "SessionEnd → if turn is open, record stop; truncate session; set turn
/// closed."
#[test]
fn gemini_session_end_records_a_stop_and_closes_the_turn() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    const G_SESSION_END: &str = r#"{"hook_event_name":"SessionEnd","session_id":"g1"}"#;

    for (args, payload) in [
        (["claude-hook", "SessionStart"], G_SESSION_START),
        (["claude-hook", "BeforeAgent"], G_PROMPT_1),
        (["claude-hook", "SessionEnd"], G_SESSION_END),
        (["claude-hook", "BeforeAgent"], G_PROMPT_2),
    ] {
        let (code, stdout, stderr) = run_hook_at(root, &args, payload);
        assert_eq!(code, 0, "{args:?} must exit 0: {stderr}");
        assert_json_object_stdout(&stdout);
    }

    let recs = lifecycle_records(root);
    let kinds: Vec<&str> = recs.iter().filter_map(|r| r["kind"].as_str()).collect();
    assert_eq!(kinds, ["prompt", "stop", "prompt"], "{recs:?}");
    assert_eq!(recs[1]["host"], "gemini");
    // The turn was closed by SessionEnd, so the next prompt is fresh and no
    // interrupt is inferred.
    assert_eq!(recs[2]["mode"], "fresh");
    assert!(lifecycle_log(root).iter().all(|e| e["event"] != "interrupt"), "{recs:?}");
    // SessionEnd does NOT truncate the session file: truncating would let any
    // stray hook between sessions mint a throwaway sid, and the next
    // session-begin SessionStart overwrites it anyway (spec §Correlation state).
    assert_eq!(
        std::fs::read_to_string(root.join(".phronesis/journey/session")).unwrap().trim(),
        "g1"
    );
}
```

- [ ] **Step 2: Run them and read the failure**

Run: `cargo test -p phronesis-mcp --test hook_integration gemini_`
Expected (before Plan 2): FAIL with `error: unrecognized subcommand 'claude-hook'` surfaced as a non-zero exit on the first assertion. Expected (after Plan 2): PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/phronesis-mcp/tests/hook_integration.rs
git commit -m "test(hooks): gemini turn, interrupt inference, and correction end to end"
```

---

### Task 5: CHANGELOG

**Files:**
- Modify: `CHANGELOG.md` (`## [Unreleased]` → `### Added`, after the existing bullets)

- [ ] **Step 1: Add the entry**

```markdown
- **Gemini CLI lifecycle hooks.** `phr-mcp init` now registers `BeforeAgent`,
  `AfterAgent`, `SessionStart`, and `SessionEnd` against
  `phr-mcp claude-hook <Event>`, so prompts, turn stops, and interrupts
  (inferred when `AfterAgent` is skipped by an abort) are recorded. The
  `BeforeTool`/`AfterTool` matcher is anchored and now includes `invoke_agent`,
  from which Phronesis derives Gemini sub-agent start/stop pairs. Registrations
  are replaced by command rather than by matcher, so a hook of your own sharing
  a matcher survives `init`. The install output notes that Gemini HTML-escapes
  injected context and skips project hooks until the folder is trusted.
```

- [ ] **Step 2: Run the whole suite**

Run: `cargo test -p phronesis-mcp`
Expected: PASS (Plan 2 is a prerequisite, so Tasks 3 and 4 are green; Task 0 is a prerequisite, so `captured_gemini_payloads_carry_the_documented_fields` is green).

The spec also makes a **live Gemini session the gate for this step**: "A Gemini
session is the gate for step 4 specifically: without one, step 4 does not ship in
this release rather than shipping untried." Task 0's session satisfies it if, and
only if, it covered a sub-agent spawn and an Esc; re-read Task 0 step 2 before
calling this plan done.

- [ ] **Step 3: Commit**

```bash
git add CHANGELOG.md
git commit -m "docs(changelog): gemini lifecycle hook registrations"
```

---

## Self-Review

**1. Spec coverage** (§"Host adapters / Gemini CLI", Rollout step 4):

| spec requirement | task |
|---|---|
| fixtures are a precondition (`BeforeAgent`, `AfterAgent`, `SessionStart`, `SessionEnd`, `BeforeTool`/`AfterTool` for `invoke_agent`) | **Task 0** |
| `write_gemini_settings` registers `AfterAgent` and `SessionEnd` with an empty matcher → `phr-mcp claude-hook <Event>` | Task 1 |
| repoints `SessionStart` and `BeforeAgent` at `claude-hook` | Task 1 (plus the in-place migration test) |
| `BeforeTool` matcher widened and anchored to `^(replace\|write_file\|run_shell_command\|invoke_agent)$` (§Adjacent findings 5) | Task 1 |
| command-keyed replacement, foreign hooks survive (§Adjacent findings 7) | **Plan 2 Task 5** (definition + unit tests); asserted end-to-end here in Task 1 |
| every response is `{}` or context JSON, never empty stdout | Tasks 3 and 4 assert stdout on every invocation |
| `subagent_start`/`subagent_stop` derived from `invoke_agent` with `agent_type = agent_name` and a synthesized `agent_id` | behaviour owned by Plan 2; end-to-end coverage in Task 3 |
| `open_turn` interrupt branch, `mid_turn` never emitted for Gemini | Task 4 (`kinds == ["prompt","interrupt","prompt"]`, mode `correction`, never `mid_turn`) |
| the confidence gate does not run on `AfterAgent`: the stop is recorded and `{}` is printed | behaviour owned by Plan 2 Task 2; asserted here in Task 4 |
| `agent_type` sanitized to `[a-z0-9][a-z0-9_.:-]{0,63}` before it becomes a tag | Task 3 (both the surviving and the dropped shape) |
| `invoke_agent` tool records carry the synthetic path `<invoke_agent>` | Task 3 (behaviour owned by Plan 2 Task 4) |
| `SessionEnd` does not truncate the `session` file | Task 4 |
| Gemini HTML-escapes `additionalContext`; project hooks skipped until trust (§Adjacent findings 6) | Task 2 |
| `tests/init_integration.rs`: "Gemini matcher is anchored", registrations idempotent | Task 1 |
| CHANGELOG `## [Unreleased] → ### Added` | Task 5 |

Out of scope by construction, and owned elsewhere: the `claude-hook` subcommand and its Gemini name mapping, the `invoke_agent` derivation in `pre.rs`/`post.rs`, and `upsert_hook_by_command` itself (all Plan 2); the `hook/mod.rs:57-64` `tool_output` comment fix (Plan 1 Task 10); nested-tool attribution inside a sub-agent (explicitly v1-deferred by the spec).

**2. Placeholder scan:** no TBD, no "add error handling", no "similar to Task N". Every code step is complete Rust or complete JSON. Task 3 and Task 4 restate their helpers' use rather than referring back, and Task 4's constants are spelled out.

**3. Type consistency:** `upsert_hook_by_command` and `is_phronesis_hook_command` have one definition each, in Plan 2 Task 5, and are only *called* here (Task 1). Test helpers `gemini_settings`/`only_command` are defined once in Task 1's file (`tests/init_integration.rs`) and `run_hook_at`/`lifecycle_records`/`lifecycle_log` once in Task 3's file (`tests/hook_integration.rs`), used again in Task 4 with the same names and arities. None of those five names is defined by Plan 2 or Plan 3 in the same file. Record field names (`kind`, `mode`, `host`, `agent`, `agent_type`, `tags`, `tool`, `path`) match Plan 1 Task 1's serialization order; action-log field names (`kind`, `event`, `host`, `mode`, `prompt`, `agent_type`, `matched_start`, `inferred_from`) match Plan 1 Task 4's `to_log_entry`. Selector strings (`lifecycle:prompt:correction`, `lifecycle:agent:<type>`) match Plan 1's `tags()`.

---

## Merge notes

Plans 2, 3, and 4 are executed in separate worktrees off the same Plan 1 base. **This plan must merge after Plan 2** (it calls `upsert_hook_by_command` and `phr-mcp claude-hook`, both of which Plan 2 adds). Plan 3 is independent of both. **Task 0 is not gated on anything** — it needs a live Gemini CLI, not a build — and should be started as early as Plan 1, because the rest of this plan cannot be verified without it.

| file | Plan 2 (Claude) owns | Plan 3 (Codex) owns | Plan 4 (this plan) owns |
|---|---|---|---|
| `src/init.rs` | `write_settings` (`:580-622`) and the new `upsert_hook_by_command` fn + its two unit tests, added after `upsert_hook` (ends `:1559`) | `write_codex_hooks`'s `for (event, matcher)` table (`:735-757`) only | **`write_gemini_settings`'s hook block (`:676-706`) and the `report.steps.push` note immediately after `write_json(&path, &settings, opts, ".gemini/settings.json", report)?` (`:714`) only.** Plus the four `write_gemini_settings_*` unit tests in `init.rs`'s `mod tests` (`:4293`, `:4598`, `:4624`, `:4665`). Nothing else in `init.rs`. |
| `tests/fixtures/hook_events.json` | the `"claude-code"` array | the `"codex"` array | **the `"gemini"` array only** (Task 1) |
| `tests/init_integration.rs` | appends `claude_settings`, `commands_for`, and its four `init_*` tests | nothing | **appends `gemini_settings`, `only_command`, and its five tests at the end of the file.** Helper names are disjoint from Plan 2's by construction. |
| `tests/payload_contract.rs` | appends `captured_claude_payloads_carry_the_documented_fields` | the `.codex/hooks.json` matcher assertion | **appends `captured_gemini_payloads_carry_the_documented_fields` (Task 0), at the end of the file** |
| `tests/fixtures/payloads/gemini/**` | — | — | **exclusively this plan's (Task 0)** |
| `tests/hook_integration.rs` | appends `run_claude_hook`, `journal_records`, `log_entries`, `inflight_keys` and its tests | nothing | **appends `run_hook_at`, `lifecycle_records`, `lifecycle_log`, `assert_json_object_stdout`, the four `G_*` constants and its `gemini_*` tests at the end of the file.** Names are disjoint from Plan 2's. If Plan 2 has already added `use serde_json::Value;` at the top, drop the duplicate `use` from Task 3's snippet. |
| `src/hook/mod.rs`, `src/main.rs`, `src/lib.rs`, `src/claude_hook.rs`, `src/codex_hook.rs` | Plan 2 / Plan 3 | — | **nothing** |
| `CHANGELOG.md` | two bullets under `## [Unreleased]` → `### Added` | one bullet after Plan 2's | one bullet appended last |

Deliberate duplicate coverage: Plan 2's `invoke_agent_derives_a_subagent_pair_before_the_allowlist` and this plan's `gemini_invoke_agent_pairs_a_subagent_start_and_stop` both exercise the `invoke_agent` derivation. This plan's is the acceptance test — it also pins stdout shape and start/stop id equality. Keep both.

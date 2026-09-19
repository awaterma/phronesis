# Lifecycle Events — Plan 1: Foundation (spec steps 1a + 1b)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the shared lifecycle event type, journal schema v2 with the tool projection, the correlation state files, prompt scrubbing, and the `kalpa` subcommand, so that the three host adapter plans (2, 3, 4) and the reporting plan (5) can be built in parallel against fixed signatures. Nothing emits a lifecycle event from a hook yet.

**Architecture:** A new `crates/phronesis-mcp/src/lifecycle/` module owns one type, `LifecycleEvent`, and the two on-disk projections of it (journal record, action-log entry). `journey/derive.rs` learns to compute every positional aggregate on the tool-record projection so existing rules see identical facts. Five small state files under `.phronesis/journey/` are read-modify-written under a sibling lock file. Everything is best-effort: a lifecycle failure never fails a hook.

**Tech Stack:** Rust 2024 edition (rust-version 1.90), serde/serde_json, fs2 advisory locks, tempfile for tests. No new dependencies.

**Spec:** `docs/specs/SPEC-agent-lifecycle-events.md` (revised 2026-09-18). Read it first; the plan argues from it.

**Depends on:** nothing. This plan lands first, before Plans 2, 3, 4 and 5. Merge order for the whole feature is `1 → (2, 3, 4 in parallel) → 5`.

**Files this plan owns exclusively** (no other plan in the set creates or edits them):

- `crates/phronesis-mcp/src/lifecycle/mod.rs`, `event.rs`, `state.rs`, `scrub.rs`, `record.rs`, `outcome.rs`
- `crates/phronesis-mcp/src/journey/journal.rs`, `crates/phronesis-mcp/src/journey/derive.rs`, `crates/phronesis-mcp/src/journey/tagger.rs`
- `crates/phronesis-mcp/src/payload_scrub.rs`
- `crates/phronesis-mcp/src/hook/mod.rs` — **Plan 1 is the sole owner of `HookPayload`, `redact_for_capture`, `capture_raw_payload`'s visibility and the `tool_output` comment.** Plan 2 adds exactly one line to this file (`mod lifecycle_wiring;`) and nothing else; Plan 3 does not touch it.
- `crates/phronesis-mcp/tests/lifecycle_state.rs`, `lifecycle_classify.rs`, `lifecycle_outcome.rs`, `kalpa_integration.rs` (created here; Plan 5 appends to `kalpa_integration.rs` only)
- `crates/phronesis-mcp/tests/journey_journal.rs`, `journey_derive.rs`, `payload_capture.rs`, `scrub_payload_integration.rs`
- `docs/specs/SPEC-journey-facts.md`

**Files this plan shares with later plans** (each edit is disjoint; see the per-file notes):

- `crates/phronesis-mcp/src/lifecycle/kalpa_cli.rs` — created here; Plan 5 Task 3 replaces only the `KalpaCmd::Show` arm and adds a private `report` fn.
- `crates/phronesis-mcp/src/lib.rs` — Plan 1 adds `pub mod lifecycle;`, Plan 2 adds `pub mod claude_hook;`. Both go in alphabetical position.
- `crates/phronesis-mcp/src/main.rs` — Plan 1 adds the `Kalpa` variant, Plan 2 adds `ClaudeHook`, Plan 5 adds fields to `Stats` and `Journey`. All four land in sequence, never in parallel.
- `CHANGELOG.md` — every plan appends its own bullet under `## [Unreleased]` → `### Added`.

## Global Constraints

- No new crate dependencies. Hash with `std::hash::DefaultHasher`, lock with `fs2::FileExt`.
- Prompt text never enters `JournalRecord`, a `phr::Fact`, a context render, or stdout. Only `LogEntry` under `kind: "lifecycle"`.
- Every lifecycle write is fail-open: swallow the error, `eprintln!("phronesis: ...")`, continue.
- Journal `tool` sentinel for lifecycle records is the exact string `__lifecycle`; `path` is `""`.
- Selector namespaces `lifecycle:` and `kalpa:` are exempt from tagger validation.
- Kalpa names match `^[a-z0-9][a-z0-9-]{0,63}$`.
- `inflight` entries older than 900 seconds are ignored and dropped.
- Conventional-commit messages. Run `cargo fmt` and `cargo clippy --all-targets -p phronesis-mcp -- -D warnings` before every commit.
- Machine note: if `cargo` fails with "You have not agreed to the Xcode license", stop and report; the human must run `sudo xcodebuild -license accept`.

## File structure

| path | responsibility |
|---|---|
| `crates/phronesis-mcp/src/journey/journal.rs` (modify) | `JournalRecord` v2 fields, `is_lifecycle()`, compaction retention for commit/kalpa records |
| `crates/phronesis-mcp/src/journey/derive.rs` (modify) | tool projection in `WindowContext`, selector exemption, read bound |
| `crates/phronesis-mcp/src/journey/tagger.rs` (modify) | `TaggerConfig.lifecycle` block (`prompt_text`) |
| `crates/phronesis-mcp/src/lifecycle/mod.rs` (create) | module root, re-exports |
| `crates/phronesis-mcp/src/lifecycle/event.rs` (create) | `Kind`, `Mode`, `Host`, `LifecycleEvent`, `Stamped`, projections |
| `crates/phronesis-mcp/src/lifecycle/state.rs` (create) | `with_locked`, session/agents/inflight/turn/kalpa files, `classify_prompt` |
| `crates/phronesis-mcp/src/lifecycle/scrub.rs` (create) | `scrub_prompt` |
| `crates/phronesis-mcp/src/lifecycle/record.rs` (create) | `record()` — stamp, journal, log, state |
| `crates/phronesis-mcp/src/lifecycle/outcome.rs` (create) | `detect_commit` |
| `crates/phronesis-mcp/src/lifecycle/kalpa_cli.rs` (create) | `phr-mcp kalpa start|end|show` handlers (counts arrive in Plan 5) |
| `crates/phronesis-mcp/src/payload_scrub.rs` (modify) | `scrub_str` → `pub(crate)` |
| `crates/phronesis-mcp/src/hook/mod.rs` (modify) | `HookPayload` widening, comment fix, `redact_for_capture` |
| `crates/phronesis-mcp/src/lib.rs`, `src/main.rs` (modify) | `pub mod lifecycle;`, `Kalpa` subcommand |
| `docs/specs/SPEC-journey-facts.md` (modify) | amendment paragraph |
| tests: `journey_journal.rs`, `journey_derive.rs`, `lifecycle_state.rs` (new), `lifecycle_classify.rs` (new), `lifecycle_outcome.rs` (new), `kalpa_integration.rs` (new), `scrub_payload_integration.rs` | as named per task |

---

### Task 1: JournalRecord v2

**Files:**
- Modify: `crates/phronesis-mcp/src/journey/journal.rs:55-91`
- Test: `crates/phronesis-mcp/tests/journey_journal.rs`

**Interfaces:**
- Produces: `pub const JOURNAL_V: u32 = 2;` and on `JournalRecord` the new optional fields `kind, mode, host, turn, agent, agent_type, kalpa: Option<String>` plus `pub fn is_lifecycle(&self) -> bool`. Field serialization order: `v, ts, sid, seq, tool, path, ext?, module?, tags[], subject?, command_exit?, kind?, mode?, host?, turn?, agent?, agent_type?, kalpa?`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/phronesis-mcp/tests/journey_journal.rs`:

```rust
#[test]
fn v1_record_reads_under_v2_with_kind_none() {
    let line = r#"{"v":1,"ts":1,"sid":"s-x","seq":1,"tool":"Edit","path":"a.rs","ext":"rs","tags":["edits"]}"#;
    let rec: JournalRecord = serde_json::from_str(line).unwrap();
    assert_eq!(rec.v, 1);
    assert!(rec.kind.is_none());
    assert!(!rec.is_lifecycle());
}

#[test]
fn v2_lifecycle_record_round_trips_in_field_order() {
    let rec = JournalRecord {
        v: journal::JOURNAL_V,
        ts: 10,
        sid: "s-x".into(),
        seq: 7,
        tool: "__lifecycle".into(),
        path: String::new(),
        ext: None,
        module: None,
        tags: vec!["lifecycle:prompt".into(), "lifecycle:prompt:fresh".into()],
        subject: None,
        command_exit: None,
        kind: Some("prompt".into()),
        mode: Some("fresh".into()),
        host: Some("claude".into()),
        turn: Some("t-1".into()),
        agent: None,
        agent_type: None,
        kalpa: Some("demo".into()),
    };
    assert!(rec.is_lifecycle());
    let s = serde_json::to_string(&rec).unwrap();
    assert_eq!(
        s,
        r#"{"v":2,"ts":10,"sid":"s-x","seq":7,"tool":"__lifecycle","path":"","tags":["lifecycle:prompt","lifecycle:prompt:fresh"],"kind":"prompt","mode":"fresh","host":"claude","turn":"t-1","kalpa":"demo"}"#
    );
    let back: JournalRecord = serde_json::from_str(&s).unwrap();
    assert_eq!(back, rec);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --test journey_journal v2_lifecycle 2>&1 | tail -20`
Expected: compile error, `no field kind` / `JOURNAL_V` not found.

- [ ] **Step 3: Implement**

In `journal.rs`, above the struct add:

```rust
/// Current on-disk record schema version. v1 = tool records only; v2 adds
/// the optional lifecycle fields below. Readers accept both.
pub const JOURNAL_V: u32 = 2;

/// `tool` value on every lifecycle record. Never a real tool name.
pub const LIFECYCLE_TOOL: &str = "__lifecycle";
```

Update the doc comment field order and append to the struct after `command_exit`:

```rust
    /// Lifecycle kind (`prompt`, `interrupt`, `subagent_start`, …). `None` on
    /// tool records. See SPEC-agent-lifecycle-events §"The journal record".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Prompt mode (`fresh` | `mid_turn` | `correction`); prompt records only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Emitting host (`claude` | `codex` | `gemini` | `cli`); lifecycle only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// Host turn id when the payload carried one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<String>,
    /// Agent id on sub-agent records and on prompts made inside a sub-agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Host agent type when supplied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    /// Open kalpa name at write time; lifecycle records only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kalpa: Option<String>,
}

impl JournalRecord {
    /// True for records written by a lifecycle event rather than a tool call.
    pub fn is_lifecycle(&self) -> bool {
        self.kind.is_some()
    }
}
```

Fix every struct literal that now fails to compile (`journal.rs` tests, `hook/journey_record.rs::build_journal_record`, `codex_hook.rs:1052-1074`, `tests/journey_derive.rs::make_record*`, `tests/journey_journal.rs` helpers): add `kind: None, mode: None, host: None, turn: None, agent: None, agent_type: None, kalpa: None`. Do not change `v: 1` in `build_journal_record`; tool records keep writing v1 until Plan 2 touches that file.

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --test journey_journal 2>&1 | tail -20`
Expected: all pass, including the two new tests.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/journey/journal.rs crates/phronesis-mcp/tests/journey_journal.rs crates/phronesis-mcp/src/hook/journey_record.rs crates/phronesis-mcp/src/codex_hook.rs crates/phronesis-mcp/tests/journey_derive.rs
git commit -m "feat(journey): JournalRecord v2 lifecycle fields"
```

---

### Task 2: Tool projection in derive

**Files:**
- Modify: `crates/phronesis-mcp/src/journey/derive.rs:64-67` (WindowContext), `:415-467` (validate_selectors), `:490-539` (assert_facts), `:543-563` (record_in_window), `:653-700` (since_ge, distinct), and the emit loops that call `record_in_window`
- Test: `crates/phronesis-mcp/tests/journey_derive.rs`

**Interfaces:**
- Consumes: `JournalRecord::is_lifecycle()` from Task 1.
- Produces: no public signature change. `WindowContext` (private) gains `tool_records: &'a [JournalRecord]`.

- [ ] **Step 1: Write the failing tests**

Add a lifecycle record helper and tests to `tests/journey_derive.rs`:

```rust
fn make_lifecycle(ts: u64, sid: &str, seq: u64, kind: &str, tags: &[&str]) -> JournalRecord {
    JournalRecord {
        v: 2, ts, sid: sid.into(), seq,
        tool: "__lifecycle".into(), path: String::new(),
        ext: None, module: None,
        tags: tags.iter().map(|s| s.to_string()).collect(),
        subject: None, command_exit: None,
        kind: Some(kind.into()), mode: None, host: Some("claude".into()),
        turn: None, agent: None, agent_type: None, kalpa: None,
    }
}

/// Goal 5 of the spec: interleaving lifecycle records changes no existing fact.
#[tokio::test]
async fn tool_projection_keeps_existing_facts_identical() {
    let cfg = cfg(r#"{"tags":{"edits":{"tool_is":"Edit"},"tests":{"path_contains":"tests/"}}}"#);
    let rules = vec![rule_with_script("r", vec![
        "facts_count('journey_count', ['edits','5c']) >= 0",
        "facts_count('journey_since_ge', ['tests', 1]) >= 0",
        "facts_count('journey_filtered_since_ge', ['tests','edits',1]) >= 0",
        "facts_count('journey_distinct', ['path','5c']) >= 0",
    ])];
    let tools: Vec<JournalRecord> = (0..10).map(|i| {
        if i == 4 { make_record_with_path(100 + i, "s-a", i, "Edit", "tests/x.rs", &["tests"]) }
        else { make_record_with_path(100 + i, "s-a", i, "Edit", &format!("src/f{i}.rs"), &["edits"]) }
    }).collect();
    let mut mixed = Vec::new();
    for (i, t) in tools.iter().enumerate() {
        mixed.push(t.clone());
        mixed.push(make_lifecycle(100 + i as u64, "s-a", 100 + i as u64, "prompt", &["lifecycle:prompt", "lifecycle:prompt:fresh"]));
    }
    let facts_for = |records: Vec<JournalRecord>| {
        let rules = rules.clone(); let cfg = cfg.clone();
        async move {
            let dir = tempfile::tempdir().unwrap();
            for r in &records { journal::append(dir.path(), r).unwrap(); }
            let mut net = phronesis_mcp::net::build_network();
            assert_facts(&mut net, derive_input(dir.path(), &rules, &cfg, "s-a", 200)).await.unwrap();
            let mut all: Vec<String> = Vec::new();
            for p in ["journey_count", "journey_since_ge", "journey_filtered_since_ge", "journey_distinct"] {
                for f in journey_facts(&net, p) { all.push(format!("{}:{}", f.predicate, f.args.join(","))); }
            }
            all.sort(); all
        }
    };
    assert_eq!(facts_for(tools).await, facts_for(mixed).await);
}

#[tokio::test]
async fn lifecycle_selectors_validate_without_journey_config() {
    let cfg = TaggerConfig::default();
    let rules = vec![rule_with_script("r", vec![
        "facts_count('journey_seen', ['lifecycle:interrupt','s']) >= 1",
        "facts_count('journey_count', ['kalpa:demo','s']) >= 1",
    ])];
    let dir = tempfile::tempdir().unwrap();
    journal::append(dir.path(), &make_lifecycle(5, "s-a", 1, "interrupt", &["lifecycle:interrupt", "kalpa:demo"])).unwrap();
    let mut net = phronesis_mcp::net::build_network();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &cfg, "s-a", 10)).await.unwrap();
    assert_eq!(journey_facts(&net, "journey_seen").len(), 1);
    assert_eq!(journey_facts(&net, "journey_count")[0].args, vec!["kalpa:demo", "s", "1"]);
}

#[tokio::test]
async fn since_ge_counts_tool_records_after_lifecycle_target() {
    let cfg = TaggerConfig::default();
    let rules = vec![rule_with_script("r", vec!["facts_count('journey_since_ge', ['lifecycle:interrupt', 3]) >= 0"])];
    let dir = tempfile::tempdir().unwrap();
    journal::append(dir.path(), &make_lifecycle(1, "s-a", 1, "interrupt", &["lifecycle:interrupt"])).unwrap();
    for i in 0..2 { journal::append(dir.path(), &make_record(2 + i, "s-a", 2 + i, "Edit", &["edits"])).unwrap(); }
    journal::append(dir.path(), &make_lifecycle(5, "s-a", 5, "stop", &["lifecycle:stop"])).unwrap();
    let mut net = phronesis_mcp::net::build_network();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &cfg, "s-a", 10)).await.unwrap();
    // two tool records after the interrupt; the trailing stop does not count
    let ks: Vec<String> = journey_facts(&net, "journey_since_ge").iter().map(|f| f.args[1].clone()).collect();
    assert_eq!(ks, vec!["1", "2"]);
}

#[tokio::test]
async fn calls_only_rule_over_reads_past_lifecycle_records() {
    let cfg = cfg(r#"{"tags":{"edits":{"tool_is":"Edit"}}}"#);
    let rules = vec![rule_with_script("r", vec!["facts_count('journey_count', ['edits','3c']) >= 0"])];
    let dir = tempfile::tempdir().unwrap();
    for i in 0..3 { journal::append(dir.path(), &make_record(i, "s-a", i, "Edit", &["edits"])).unwrap(); }
    for i in 3..9 { journal::append(dir.path(), &make_lifecycle(i, "s-a", i, "prompt", &["lifecycle:prompt"])).unwrap(); }
    let mut net = phronesis_mcp::net::build_network();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &cfg, "s-a", 10)).await.unwrap();
    assert_eq!(journey_facts(&net, "journey_count")[0].args[2], "3");
}
```

Adjust helper names/arities to the existing helpers in the file (`make_record`, `make_record_with_path`, `derive_input`); read their signatures at the top of the file before using them.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --test journey_derive tool_projection lifecycle_selectors since_ge_counts calls_only 2>&1 | tail -30`
Expected: `tool_projection_keeps_existing_facts_identical` fails (count differs), `lifecycle_selectors_validate_without_journey_config` fails with `UndefinedSelector`, `calls_only…` fails with `0` count.

- [ ] **Step 3: Implement**

In `derive.rs`:

```rust
struct WindowContext<'a> {
    /// Every record read, in append order. Time and session windows filter this.
    records: &'a [JournalRecord],
    /// `records` with lifecycle records removed. Positional (`Nc`) windows and
    /// `journey_distinct` use this so existing rules keep identical facts.
    tool_records: &'a [JournalRecord],
    scope: WindowScope<'a>,
}

/// Built-in selector namespaces that need no tagger definition.
fn is_builtin_selector(selector: &str) -> bool {
    selector.starts_with("lifecycle:") || selector.starts_with("kalpa:")
}
```

In `validate_selectors`, replace the whole `for selector in &referenced { … }` loop (`derive.rs:446-467`) with:

```rust
    for selector in &referenced {
        // `lifecycle:` and `kalpa:` are built-in namespaces written by the
        // lifecycle module itself, not by a tagger, so they have no entry in
        // `journey.json` and must not fail closed. Everything else still does.
        if is_builtin_selector(selector) {
            continue;
        }
        let ok = if let Some(name) = selector.strip_prefix("module:") {
            defined_modules.contains(selector) || cfg.modules.iter().any(|m| m.name == name)
        } else {
            defined_tags.contains(selector.as_str())
        };
        if !ok {
            let rule_id = rules
                .iter()
                .find(|r| rule_refs_selector(r, selector))
                .map(|r| r.id.clone())
                .unwrap_or_else(|| "<unknown>".to_string());
            return Err(DeriveError::UndefinedSelector {
                rule: rule_id,
                selector: selector.clone(),
            });
        }
    }
```

In `assert_facts`, replace the calls-only branch and context construction:

```rust
        } else {
            // Calls-only window: over-read so lifecycle records sharing the
            // file cannot starve the tool-record window, then project.
            (2 * max_calls as usize + 64).min(journal::SUFFIX_HARD_CAP).max(1)
        }
    };

    let records = journal::read_recent(input.project_root, read_n)?;
    let tool_records: Vec<JournalRecord> =
        records.iter().filter(|r| !r.is_lifecycle()).cloned().collect();
    let context = WindowContext {
        records: &records,
        tool_records: &tool_records,
        scope: input.scope,
    };

    emit_occurrence(network, &context, &scan).await;
    emit_count(network, &context, &scan).await;
    emit_seen(network, &context, &scan).await;
    emit_since_ge(network, &records, &scan).await;
    emit_distinct(network, &context, &scan).await;
    emit_filtered_since_ge(network, &records, &scan).await;
```

Add a helper that picks the record view for a window and rewrite the three occurrence-style emitters to iterate it (they currently iterate `context.records` with `enumerate()` and call `record_in_window(rec, win, i, context)`):

```rust
/// Records a window token evaluates over: positional windows use the tool
/// projection, time and session windows use every record.
fn window_records<'a>(context: &WindowContext<'a>, window_tok: &str) -> &'a [JournalRecord] {
    match Window::parse(window_tok) {
        Ok(Window::Calls(_)) => context.tool_records,
        _ => context.records,
    }
}

fn record_in_window(rec: &JournalRecord, window_tok: &str, rec_idx: usize, total: usize, scope: &WindowScope<'_>) -> bool {
    let window = match Window::parse(window_tok) { Ok(w) => w, Err(_) => return false };
    match window {
        Window::Calls(n) => rec_idx >= total.saturating_sub(n as usize),
        Window::Seconds(s) => rec.ts + s >= scope.now_ts,
        Window::Session => rec.sid == scope.current_sid,
    }
}
```

Each of the three occurrence-style emitters changes in exactly two places: bind `view` at the top of the per-pair loop, and pass `view.len()` plus `&context.scope` to `record_in_window` instead of `context`. Nothing else in their bodies moves. Written out for all three:

```rust
async fn emit_occurrence(network: &ReteNetwork, context: &WindowContext<'_>, scan: &RuleScan) {
    for (sel, win) in &scan.occurrence_pairs {
        let view = window_records(context, win);
        let mut n = 0u64;
        for (i, rec) in view.iter().enumerate() {
            if !matches_selector(rec, sel) {
                continue;
            }
            if !record_in_window(rec, win, i, view.len(), &context.scope) {
                continue;
            }
            n += 1;
            let id = format!("journey_occurrence:{}:{}:{}", sel, win, rec.seq);
            let _ = network
                .assert_fact(Fact {
                    id,
                    predicate: "journey_occurrence".to_string(),
                    args: vec![sel.clone(), win.clone()],
                    timestamp: 0,
                    source: Some("journey".to_string()),
                })
                .await;
            if n > journal::SUFFIX_HARD_CAP as u64 {
                break;
            }
        }
    }
}

async fn emit_count(network: &ReteNetwork, context: &WindowContext<'_>, scan: &RuleScan) {
    for (sel, win) in &scan.count_pairs {
        let view = window_records(context, win);
        let mut count = 0u64;
        for (i, rec) in view.iter().enumerate() {
            if !matches_selector(rec, sel) {
                continue;
            }
            if !record_in_window(rec, win, i, view.len(), &context.scope) {
                continue;
            }
            count += 1;
        }
        let id = format!("journey_count:{}:{}", sel, win);
        let _ = network
            .assert_fact(Fact {
                id,
                predicate: "journey_count".to_string(),
                args: vec![sel.clone(), win.clone(), count.to_string()],
                timestamp: 0,
                source: Some("journey".to_string()),
            })
            .await;
    }
}

async fn emit_seen(network: &ReteNetwork, context: &WindowContext<'_>, scan: &RuleScan) {
    for (sel, win) in &scan.seen_pairs {
        let view = window_records(context, win);
        let any = view.iter().enumerate().any(|(i, rec)| {
            matches_selector(rec, sel) && record_in_window(rec, win, i, view.len(), &context.scope)
        });
        if !any {
            continue;
        }
        let id = format!("journey_seen:{}:{}", sel, win);
        let _ = network
            .assert_fact(Fact {
                id,
                predicate: "journey_seen".to_string(),
                args: vec![sel.clone(), win.clone()],
                timestamp: 0,
                source: Some("journey".to_string()),
            })
            .await;
    }
}
```

`emit_distinct` takes the same two-line change but always binds `let view = context.tool_records;` regardless of window — a lifecycle record's `path` is `""` and must never add a distinct path. `emit_since_ge` keeps searching `records` for the last match but computes distance as the number of tool records after it, replacing `distance = Some((records.len() - 1 - i) as u32);`:

```rust
        for (i, rec) in records.iter().enumerate().rev() {
            if matches_selector(rec, sel) {
                let after = records[i + 1..].iter().filter(|r| !r.is_lifecycle()).count();
                distance = Some(after as u32);
                break;
            }
        }
```

`emit_filtered_since_ge` is unchanged (it already searches and counts over `records`).

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --test journey_derive 2>&1 | tail -30`
Expected: all pass, including `determinism_contract` and every pre-existing test.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/journey/derive.rs crates/phronesis-mcp/tests/journey_derive.rs
git commit -m "feat(journey): tool projection so lifecycle records leave existing facts unchanged"
```

---

### Task 3: Compaction retention for commit and kalpa records

**Files:**
- Modify: `crates/phronesis-mcp/src/journey/journal.rs:260-275` (`latest_outcome_indices`)
- Test: `crates/phronesis-mcp/tests/journey_journal.rs`

- [ ] **Step 1: Write the failing test**

`journal::maybe_compact(root, max_bytes, tail_records)` is `pub`, so an integration test can force compaction directly — `max_bytes: 1` puts the journal over cap unconditionally and `tail_records: 1` leaves everything but the last record in the compaction prefix. This mirrors `src/journey/journal.rs`'s own `compaction_preserves_grounded_outcome_over_later_unknown` unit test.

Append to `crates/phronesis-mcp/tests/journey_journal.rs`:

```rust
/// A lifecycle record, mirroring Task 2's `make_lifecycle` in
/// `tests/journey_derive.rs`.
fn lifecycle_record(ts: u64, seq: u64, kind: &str, tags: &[&str]) -> JournalRecord {
    JournalRecord {
        v: journal::JOURNAL_V,
        ts,
        sid: "s-a".into(),
        seq,
        tool: journal::LIFECYCLE_TOOL.into(),
        path: String::new(),
        ext: None,
        module: None,
        tags: tags.iter().map(|s| s.to_string()).collect(),
        subject: None,
        command_exit: None,
        kind: Some(kind.into()),
        mode: None,
        host: Some("claude".into()),
        turn: None,
        agent: None,
        agent_type: None,
        kalpa: None,
    }
}

/// A plain tool record, used here only as the compaction tail.
fn tool_record(ts: u64, seq: u64) -> JournalRecord {
    JournalRecord {
        v: journal::JOURNAL_V,
        ts,
        sid: "s-a".into(),
        seq,
        tool: "Edit".into(),
        path: format!("src/f{seq}.rs"),
        ext: Some("rs".into()),
        module: None,
        tags: vec!["edits".into()],
        subject: None,
        command_exit: None,
        kind: None,
        mode: None,
        host: None,
        turn: None,
        agent: None,
        agent_type: None,
        kalpa: None,
    }
}

#[test]
fn compaction_retains_commit_and_kalpa_records_in_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let mut commit = lifecycle_record(1, 1, "commit", &["lifecycle:commit"]);
    commit.kalpa = Some("demo".into());

    journal::append(
        dir.path(),
        &lifecycle_record(0, 0, "kalpa_start", &["lifecycle:kalpa_start", "kalpa:demo"]),
    )
    .unwrap();
    journal::append(dir.path(), &commit).unwrap();
    journal::append(dir.path(), &lifecycle_record(2, 2, "prompt", &["lifecycle:prompt"])).unwrap();
    journal::append(
        dir.path(),
        &lifecycle_record(3, 3, "kalpa_end", &["lifecycle:kalpa_end", "kalpa:demo"]),
    )
    .unwrap();
    // The one record the tail keeps, so every lifecycle record above lands in
    // the compaction prefix and is subject to the retention rule.
    journal::append(dir.path(), &tool_record(4, 4)).unwrap();

    // max_bytes = 1 forces compaction; tail_records = 1 keeps only the last.
    assert!(journal::maybe_compact(dir.path(), 1, 1).unwrap());

    let all = journal::read_recent(dir.path(), journal::SUFFIX_HARD_CAP).unwrap();
    let kinds: Vec<&str> = all.iter().filter_map(|r| r.kind.as_deref()).collect();
    assert!(kinds.contains(&"commit"), "{kinds:?}");
    assert!(kinds.contains(&"kalpa_start"), "{kinds:?}");
    assert!(kinds.contains(&"kalpa_end"), "{kinds:?}");
    assert!(!kinds.contains(&"prompt"), "a prompt compacts away: {kinds:?}");
    assert!(all.iter().any(|r| r.tool == "Edit"), "the tail survives");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --test journey_journal compaction_retains 2>&1 | tail -20`
Expected: FAIL on the `commit` assertion.

- [ ] **Step 3: Implement**

In `latest_outcome_indices`:

```rust
/// Lifecycle tags whose records survive compaction of the prefix: the
/// success signal and kalpa boundaries are the denominators of every
/// per-kalpa report and must not fall off before the log does.
const RETAINED_LIFECYCLE_TAGS: [&str; 3] =
    ["lifecycle:commit", "lifecycle:kalpa_start", "lifecycle:kalpa_end"];

fn latest_outcome_indices(prefix: &[JournalRecord]) -> Vec<usize> {
    let mut latest: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let mut keep: Vec<usize> = Vec::new();
    for (i, r) in prefix.iter().enumerate() {
        if r.tags.iter().any(|t| RETAINED_LIFECYCLE_TAGS.contains(&t.as_str())) {
            keep.push(i);
            continue;
        }
        if let Some(s) = r.subject.as_deref()
            && r.tags.iter().any(|t| crate::outcomes::is_grounded_outcome_tag(t))
        {
            latest.insert(s, i);
        }
    }
    keep.extend(latest.into_values());
    keep.sort_unstable();
    keep.dedup();
    keep
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --test journey_journal 2>&1 | tail -20` and `cargo test -p phronesis-mcp journal:: 2>&1 | tail -20`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/journey/journal.rs crates/phronesis-mcp/tests/journey_journal.rs
git commit -m "feat(journey): retain commit and kalpa lifecycle records through compaction"
```

---

### Task 4: `lifecycle::event` — the shared type and both projections

**Files:**
- Create: `crates/phronesis-mcp/src/lifecycle/mod.rs`, `crates/phronesis-mcp/src/lifecycle/event.rs`
- Modify: `crates/phronesis-mcp/src/lib.rs` (add `pub mod lifecycle;` in alphabetical position after `journey_cli`)
- Test: unit tests inside `event.rs`

**Interfaces (Produces — every later plan uses these exactly):**

```rust
pub enum Kind { SubagentStart, SubagentStop, Prompt, Interrupt, Stop, Commit, KalpaStart, KalpaEnd }
impl Kind { pub fn as_str(self) -> &'static str; /* snake_case */ pub fn tag(self) -> String; /* "lifecycle:<as_str>" */ }
pub enum Mode { Fresh, MidTurn, Correction }
impl Mode { pub fn as_str(self) -> &'static str; /* fresh | mid_turn | correction */ }
pub enum Host { Claude, Codex, Gemini, Cli }
impl Host { pub fn as_str(self) -> &'static str; }
pub struct LifecycleEvent {
    pub kind: Kind, pub host: Host, pub mode: Option<Mode>,
    pub session_id: Option<String>, pub turn_id: Option<String>,
    pub agent_id: Option<String>, pub agent_type: Option<String>,
    /// Already scrubbed. Never placed in the journal.
    pub prompt: Option<String>,
    /// Flat extra log fields: duration_secs, stop_hook_active, matched_start,
    /// inferred_from, sha, head_before, confidence_band, tool_use_id.
    pub extra: serde_json::Map<String, serde_json::Value>,
}
impl LifecycleEvent {
    pub fn new(kind: Kind, host: Host) -> Self;
    pub fn with_mode(self, m: Mode) -> Self;
    pub fn with_session(self, id: impl Into<String>) -> Self;
    pub fn with_turn(self, id: impl Into<String>) -> Self;
    pub fn with_agent(self, id: impl Into<String>, agent_type: Option<String>) -> Self;
    pub fn with_prompt(self, scrubbed: impl Into<String>) -> Self;
    pub fn with_extra(self, key: &str, v: impl Into<serde_json::Value>) -> Self;
    pub fn tags(&self, kalpa: Option<&str>) -> Vec<String>;
    pub fn to_journal_record(&self, s: &Stamped) -> JournalRecord;
    pub fn to_log_entry(&self, s: &Stamped, text: PromptText) -> LogEntry;
}
pub struct Stamped { pub ts: u64, pub sid: String, pub seq: u64, pub kalpa: Option<String>, pub subject: Option<String> }
pub enum PromptText { Full, None }
```

- [ ] **Step 1: Write the failing unit tests** (inside `event.rs` under `#[cfg(test)]`)

```rust
#[test]
fn tags_for_correction_prompt_with_kalpa() {
    let e = LifecycleEvent::new(Kind::Prompt, Host::Claude).with_mode(Mode::Correction);
    assert_eq!(e.tags(Some("demo")), vec!["lifecycle:prompt", "lifecycle:prompt:correction", "lifecycle:intervention", "kalpa:demo"]);
}
#[test]
fn intervention_tag_only_on_mid_turn_and_correction() {
    let fresh = LifecycleEvent::new(Kind::Prompt, Host::Claude).with_mode(Mode::Fresh);
    assert_eq!(fresh.tags(None), vec!["lifecycle:prompt", "lifecycle:prompt:fresh"]);
    let mid = LifecycleEvent::new(Kind::Prompt, Host::Codex).with_mode(Mode::MidTurn);
    assert!(mid.tags(None).contains(&"lifecycle:intervention".to_string()));
    let stop = LifecycleEvent::new(Kind::Stop, Host::Claude);
    assert!(!stop.tags(None).iter().any(|t| t.contains("intervention")));
}
#[test]
fn tags_for_subagent_with_type() {
    let e = LifecycleEvent::new(Kind::SubagentStop, Host::Codex).with_agent("a1", Some("reviewer".into()));
    assert_eq!(e.tags(None), vec!["lifecycle:subagent_stop", "lifecycle:agent:reviewer"]);
}
#[test]
fn journal_record_never_carries_prompt_text() {
    let e = LifecycleEvent::new(Kind::Prompt, Host::Claude).with_mode(Mode::Fresh).with_prompt("SECRET TEXT");
    let s = Stamped { ts: 1, sid: "s-a".into(), seq: 2, kalpa: None, subject: None };
    let rec = e.to_journal_record(&s);
    assert_eq!(rec.tool, "__lifecycle");
    assert_eq!(rec.path, "");
    assert_eq!(rec.v, crate::journey::journal::JOURNAL_V);
    assert!(!serde_json::to_string(&rec).unwrap().contains("SECRET"));
    assert_eq!(rec.kind.as_deref(), Some("prompt"));
    assert_eq!(rec.mode.as_deref(), Some("fresh"));
}
#[test]
fn log_entry_carries_prompt_only_when_full() {
    let e = LifecycleEvent::new(Kind::Prompt, Host::Claude).with_mode(Mode::Fresh).with_prompt("hello");
    let s = Stamped { ts: 1, sid: "s-a".into(), seq: 2, kalpa: Some("k".into()), subject: None };
    let full = serde_json::to_value(e.to_log_entry(&s, PromptText::Full)).unwrap();
    assert_eq!(full["kind"], "lifecycle"); assert_eq!(full["event"], "prompt");
    assert_eq!(full["prompt"], "hello"); assert_eq!(full["prompt_bytes"], 5);
    assert_eq!(full["kalpa"], "k"); assert_eq!(full["sid"], "s-a"); assert_eq!(full["seq"], 2);
    let none = serde_json::to_value(e.to_log_entry(&s, PromptText::None)).unwrap();
    assert!(none.get("prompt").is_none()); assert_eq!(none["prompt_bytes"], 5);
}
#[test]
fn extra_fields_flatten_into_log_entry() {
    let e = LifecycleEvent::new(Kind::SubagentStop, Host::Codex).with_extra("duration_secs", 12u64).with_extra("matched_start", true);
    let s = Stamped { ts: 1, sid: "s".into(), seq: 1, kalpa: None, subject: None };
    let v = serde_json::to_value(e.to_log_entry(&s, PromptText::Full)).unwrap();
    assert_eq!(v["duration_secs"], 12); assert_eq!(v["matched_start"], true);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp lifecycle::event 2>&1 | tail -20`
Expected: module not found.

- [ ] **Step 3: Implement**

`lifecycle/mod.rs`:

```rust
//! Agent lifecycle events: sub-agent start/stop, prompts, interrupts, turn
//! stops, commits, kalpa boundaries. One type, two on-disk projections
//! (journey journal record, action-log entry), five small correlation files.
//! See `docs/specs/SPEC-agent-lifecycle-events.md`.

pub mod event;
pub mod kalpa_cli;
pub mod outcome;
pub mod record;
pub mod scrub;
pub mod state;

pub use event::{Host, Kind, LifecycleEvent, Mode, PromptText, Stamped};
```

(Create empty `kalpa_cli.rs`, `outcome.rs`, `record.rs`, `scrub.rs`, `state.rs` with a one-line doc comment each so the crate compiles; later tasks fill them.)

`lifecycle/event.rs`:

```rust
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::action_log::LogEntry;
use crate::journey::journal::{JOURNAL_V, JournalRecord, LIFECYCLE_TOOL};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind { SubagentStart, SubagentStop, Prompt, Interrupt, Stop, Commit, KalpaStart, KalpaEnd }

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::SubagentStart => "subagent_start", Kind::SubagentStop => "subagent_stop",
            Kind::Prompt => "prompt", Kind::Interrupt => "interrupt", Kind::Stop => "stop",
            Kind::Commit => "commit", Kind::KalpaStart => "kalpa_start", Kind::KalpaEnd => "kalpa_end",
        }
    }
    pub fn tag(self) -> String { format!("lifecycle:{}", self.as_str()) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode { Fresh, MidTurn, Correction }
impl Mode {
    pub fn as_str(self) -> &'static str {
        match self { Mode::Fresh => "fresh", Mode::MidTurn => "mid_turn", Mode::Correction => "correction" }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Host { Claude, Codex, Gemini, Cli }
impl Host {
    pub fn as_str(self) -> &'static str {
        match self { Host::Claude => "claude", Host::Codex => "codex", Host::Gemini => "gemini", Host::Cli => "cli" }
    }
}

/// Whether the action-log projection carries prompt text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptText { Full, None }

/// Values stamped at write time by `record::record`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamped { pub ts: u64, pub sid: String, pub seq: u64, pub kalpa: Option<String>, pub subject: Option<String> }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleEvent {
    pub kind: Kind,
    pub host: Host,
    pub mode: Option<Mode>,
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    pub agent_id: Option<String>,
    pub agent_type: Option<String>,
    /// Already scrubbed by `lifecycle::scrub::scrub_prompt`. Never journaled.
    pub prompt: Option<String>,
    pub extra: Map<String, Value>,
}

impl LifecycleEvent {
    pub fn new(kind: Kind, host: Host) -> Self {
        Self { kind, host, mode: None, session_id: None, turn_id: None, agent_id: None, agent_type: None, prompt: None, extra: Map::new() }
    }
    pub fn with_mode(mut self, m: Mode) -> Self { self.mode = Some(m); self }
    pub fn with_session(mut self, id: impl Into<String>) -> Self { self.session_id = Some(id.into()); self }
    pub fn with_turn(mut self, id: impl Into<String>) -> Self { self.turn_id = Some(id.into()); self }
    pub fn with_agent(mut self, id: impl Into<String>, agent_type: Option<String>) -> Self {
        self.agent_id = Some(id.into()); self.agent_type = agent_type; self
    }
    pub fn with_prompt(mut self, scrubbed: impl Into<String>) -> Self { self.prompt = Some(scrubbed.into()); self }
    pub fn with_extra(mut self, key: &str, v: impl Into<Value>) -> Self { self.extra.insert(key.to_string(), v.into()); self }

    pub fn tags(&self, kalpa: Option<&str>) -> Vec<String> {
        let mut t = vec![self.kind.tag()];
        if let Some(m) = self.mode {
            t.push(format!("lifecycle:prompt:{}", m.as_str()));
            // The autonomy signal: the human changed the plan (steered mid-turn
            // or corrected after an interrupt), as opposed to replying.
            if matches!(m, Mode::MidTurn | Mode::Correction) { t.push("lifecycle:intervention".to_string()); }
        }
        if let Some(at) = self.agent_type.as_deref().filter(|s| !s.is_empty())
            && matches!(self.kind, Kind::SubagentStart | Kind::SubagentStop)
        { t.push(format!("lifecycle:agent:{at}")); }
        if let Some(k) = kalpa { t.push(format!("kalpa:{k}")); }
        t
    }

    pub fn to_journal_record(&self, s: &Stamped) -> JournalRecord {
        JournalRecord {
            v: JOURNAL_V, ts: s.ts, sid: s.sid.clone(), seq: s.seq,
            tool: LIFECYCLE_TOOL.to_string(), path: String::new(),
            ext: None, module: None,
            tags: self.tags(s.kalpa.as_deref()),
            subject: s.subject.clone(), command_exit: None,
            kind: Some(self.kind.as_str().to_string()),
            mode: self.mode.map(|m| m.as_str().to_string()),
            host: Some(self.host.as_str().to_string()),
            turn: self.turn_id.clone(), agent: self.agent_id.clone(),
            agent_type: self.agent_type.clone(), kalpa: s.kalpa.clone(),
        }
    }

    pub fn to_log_entry(&self, s: &Stamped, text: PromptText) -> LogEntry {
        let mut e = LogEntry::new("lifecycle", self.kind.as_str())
            .with("host", self.host.as_str()).with("sid", s.sid.clone()).with("seq", s.seq);
        e.ts = s.ts;
        if let Some(m) = self.mode { e = e.with("mode", m.as_str()); }
        if let Some(v) = &self.session_id { e = e.with("session_id", v.clone()); }
        if let Some(v) = &self.turn_id { e = e.with("turn_id", v.clone()); }
        if let Some(v) = &self.agent_id { e = e.with("agent_id", v.clone()); }
        if let Some(v) = &self.agent_type { e = e.with("agent_type", v.clone()); }
        if let Some(v) = &s.kalpa { e = e.with("kalpa", v.clone()); }
        if let Some(v) = &s.subject { e = e.with("subject", v.clone()); }
        if let Some(p) = &self.prompt {
            e = e.with("prompt_bytes", p.len() as u64);
            if text == PromptText::Full { e = e.with("prompt", p.clone()); }
        }
        for (k, v) in &self.extra { e.data.insert(k.clone(), v.clone()); }
        e
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp lifecycle::event 2>&1 | tail -20`
Expected: 5 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/lifecycle crates/phronesis-mcp/src/lib.rs
git commit -m "feat(lifecycle): LifecycleEvent with journal and action-log projections"
```

---

### Task 5: `lifecycle::state` — locked state files

**Files:**
- Create: `crates/phronesis-mcp/src/lifecycle/state.rs`
- Test: `crates/phronesis-mcp/tests/lifecycle_state.rs` (new)

**Interfaces (Produces):**

```rust
pub const INFLIGHT_TTL_SECS: u64 = 900;
pub fn with_locked<T>(root: &Path, name: &str, f: impl FnOnce(String) -> (Option<String>, T)) -> std::io::Result<T>;
//   `name` is the bare file name under .phronesis/journey/ (e.g. "agents"); the lock is "<name>.lock".
//   f receives current contents ("" if absent) and returns (new contents or None to leave unchanged, result).

pub fn set_session(root: &Path, sid: &str);       // overwrite .phronesis/journey/session
pub fn clear_session(root: &Path);                // truncate
pub fn reset_for_session_start(root: &Path);      // truncate agents + inflight; turn := closed

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)] pub struct OpenAgent { pub agent_id: String, pub agent_type: Option<String>, pub ts: u64, pub seq: u64 }
pub fn push_agent(root: &Path, a: OpenAgent);
pub fn pop_agent(root: &Path, agent_id: Option<&str>) -> Option<OpenAgent>;  // by id, else LIFO

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)] pub struct Inflight { pub key: String, pub tool: String, pub ts: u64, pub agent_id: Option<String>, pub head_before: Option<String> }
pub fn push_inflight(root: &Path, e: Inflight);
pub fn pop_inflight(root: &Path, key: &str) -> Option<Inflight>;
pub fn live_inflight(root: &Path, now: u64, agent_scope: Option<&str>) -> Vec<Inflight>; // drops expired, returns visible live entries
pub fn take_inflight_for_scope(root: &Path, now: u64, agent_scope: Option<&str>) -> Vec<Inflight>; // removes + returns visible live entries
pub fn inflight_key_for(tool_use_id: Option<&str>, tool_name: &str, tool_input: &serde_json::Value) -> String;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)] pub struct Turn { pub open: bool, pub turn_id: Option<String>, pub last_prompt_ts: u64, pub last_event: String }
pub fn read_turn(root: &Path) -> Turn;
pub fn open_turn(root: &Path, turn_id: Option<&str>, ts: u64);
pub fn close_turn(root: &Path, last_event: &str);

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)] pub struct Kalpa { pub name: String, pub started_ts: u64 }
pub fn read_kalpa(root: &Path) -> Option<Kalpa>;
pub fn write_kalpa(root: &Path, k: &Kalpa);
pub fn clear_kalpa(root: &Path);
pub fn valid_kalpa_name(name: &str) -> bool;
```

Visibility rule for `agent_scope`: an entry is visible when `entry.agent_id.is_none()` or `entry.agent_id == agent_scope`.

- [ ] **Step 1: Write the failing tests** in `tests/lifecycle_state.rs`

```rust
use phronesis_mcp::lifecycle::state::*;
use std::path::Path;

fn root() -> tempfile::TempDir { tempfile::tempdir().unwrap() }

#[test]
fn agents_pop_by_id_then_lifo() {
    let d = root();
    push_agent(d.path(), OpenAgent { agent_id: "a".into(), agent_type: Some("x".into()), ts: 1, seq: 1 });
    push_agent(d.path(), OpenAgent { agent_id: "b".into(), agent_type: None, ts: 2, seq: 2 });
    push_agent(d.path(), OpenAgent { agent_id: "c".into(), agent_type: None, ts: 3, seq: 3 });
    assert_eq!(pop_agent(d.path(), Some("a")).unwrap().agent_id, "a");
    assert_eq!(pop_agent(d.path(), None).unwrap().agent_id, "c");
    assert_eq!(pop_agent(d.path(), Some("zzz")), None);
    assert_eq!(pop_agent(d.path(), None).unwrap().agent_id, "b");
    assert_eq!(pop_agent(d.path(), None), None);
}

#[test]
fn inflight_ttl_and_scope() {
    let d = root();
    push_inflight(d.path(), Inflight { key: "old".into(), tool: "Bash".into(), ts: 100, agent_id: None, head_before: None });
    push_inflight(d.path(), Inflight { key: "parent".into(), tool: "Bash".into(), ts: 1000, agent_id: None, head_before: Some("abc".into()) });
    push_inflight(d.path(), Inflight { key: "child".into(), tool: "Edit".into(), ts: 1000, agent_id: Some("sub1".into()), head_before: None });
    let now = 100 + INFLIGHT_TTL_SECS + 1;
    let seen_parent = live_inflight(d.path(), now, None);
    assert_eq!(seen_parent.iter().map(|e| e.key.as_str()).collect::<Vec<_>>(), vec!["parent"]);
    let seen_child = live_inflight(d.path(), now, Some("sub1"));
    assert_eq!(seen_child.iter().map(|e| e.key.as_str()).collect::<Vec<_>>(), vec!["parent", "child"]);
    // expired entry was dropped on read
    assert!(pop_inflight(d.path(), "old").is_none());
    assert_eq!(pop_inflight(d.path(), "parent").unwrap().head_before.as_deref(), Some("abc"));
    let taken = take_inflight_for_scope(d.path(), now, Some("sub1"));
    assert_eq!(taken.len(), 1);
    assert!(live_inflight(d.path(), now, Some("sub1")).is_empty());
}

#[test]
fn inflight_key_prefers_tool_use_id_then_hashes_input() {
    let input = serde_json::json!({"b": 1, "a": [1, 2]});
    assert_eq!(inflight_key_for(Some("tu-1"), "Bash", &input), "tu-1");
    let k1 = inflight_key_for(None, "run_shell_command", &input);
    let k2 = inflight_key_for(None, "run_shell_command", &serde_json::json!({"a": [1, 2], "b": 1}));
    assert_eq!(k1, k2);
    assert_ne!(k1, inflight_key_for(None, "replace", &input));
}

#[test]
fn turn_transitions_and_session_reset() {
    let d = root();
    assert!(!read_turn(d.path()).open);
    open_turn(d.path(), Some("t1"), 50);
    let t = read_turn(d.path());
    assert!(t.open); assert_eq!(t.turn_id.as_deref(), Some("t1")); assert_eq!(t.last_prompt_ts, 50); assert_eq!(t.last_event, "prompt");
    close_turn(d.path(), "interrupt");
    assert!(!read_turn(d.path()).open);
    assert_eq!(read_turn(d.path()).last_event, "interrupt");
    push_agent(d.path(), OpenAgent { agent_id: "a".into(), agent_type: None, ts: 1, seq: 1 });
    push_inflight(d.path(), Inflight { key: "k".into(), tool: "Bash".into(), ts: 1, agent_id: None, head_before: None });
    open_turn(d.path(), None, 60);
    reset_for_session_start(d.path());
    assert!(pop_agent(d.path(), None).is_none());
    assert!(live_inflight(d.path(), 2, None).is_empty());
    assert!(!read_turn(d.path()).open);
}

#[test]
fn session_overwrite_and_clear() {
    let d = root();
    set_session(d.path(), "host-sid-1");
    assert_eq!(phronesis_mcp::journey::current_sid(d.path()), "host-sid-1");
    set_session(d.path(), "host-sid-2");
    assert_eq!(phronesis_mcp::journey::current_sid(d.path()), "host-sid-2");
    clear_session(d.path());
    assert!(phronesis_mcp::journey::current_sid(d.path()).starts_with("s-"));
}

#[test]
fn kalpa_round_trip_and_validation() {
    let d = root();
    assert!(read_kalpa(d.path()).is_none());
    write_kalpa(d.path(), &Kalpa { name: "demo-1".into(), started_ts: 9 });
    assert_eq!(read_kalpa(d.path()).unwrap().name, "demo-1");
    clear_kalpa(d.path());
    assert!(read_kalpa(d.path()).is_none());
    assert!(valid_kalpa_name("lifecycle-events"));
    assert!(!valid_kalpa_name("Bad Name"));
    assert!(!valid_kalpa_name("-lead"));
    assert!(!valid_kalpa_name(&"a".repeat(65)));
}

#[test]
fn with_locked_serializes_sixteen_writers() {
    let d = root();
    let p = d.path().to_path_buf();
    let handles: Vec<_> = (0..16).map(|_| {
        let p = p.clone();
        std::thread::spawn(move || {
            for _ in 0..50 {
                with_locked(&p, "counter", |cur| {
                    let n: u64 = cur.trim().parse().unwrap_or(0);
                    (Some((n + 1).to_string()), ())
                }).unwrap();
            }
        })
    }).collect();
    for h in handles { h.join().unwrap(); }
    let final_v = std::fs::read_to_string(p.join(".phronesis/journey/counter")).unwrap();
    assert_eq!(final_v.trim(), "800");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --test lifecycle_state 2>&1 | tail -20`
Expected: unresolved imports.

- [ ] **Step 3: Implement** `lifecycle/state.rs`

```rust
//! Correlation state under `.phronesis/journey/`: session, agents, inflight,
//! turn, kalpa. Every read-modify-write holds an exclusive advisory lock on a
//! sibling `<name>.lock` (stable inode; the data file may be truncated). All
//! public writers are best-effort: errors are logged to stderr and swallowed.

use std::collections::hash_map::DefaultHasher;
use std::fs::OpenOptions;
use std::hash::{Hash, Hasher};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

pub const INFLIGHT_TTL_SECS: u64 = 900;

fn dir(root: &Path) -> PathBuf { root.join(".phronesis").join("journey") }

pub fn with_locked<T>(root: &Path, name: &str, f: impl FnOnce(String) -> (Option<String>, T)) -> std::io::Result<T> {
    let d = dir(root);
    std::fs::create_dir_all(&d)?;
    let lock = OpenOptions::new().create(true).write(true).truncate(false).open(d.join(format!("{name}.lock")))?;
    lock.lock_exclusive()?;
    let path = d.join(name);
    let mut file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path)?;
    let mut cur = String::new();
    file.read_to_string(&mut cur)?;
    let (next, out) = f(cur);
    if let Some(next) = next {
        file.seek(SeekFrom::Start(0))?;
        file.set_len(0)?;
        file.write_all(next.as_bytes())?;
    }
    let _ = FileExt::unlock(&lock);
    Ok(out)
}

fn swallow<T>(r: std::io::Result<T>, what: &str) -> Option<T> {
    match r { Ok(v) => Some(v), Err(e) => { eprintln!("phronesis: lifecycle state {what}: {e}"); None } }
}

// ---- session ----
pub fn set_session(root: &Path, sid: &str) {
    swallow(with_locked(root, "session", |_| (Some(sid.to_string()), ())), "set_session");
}
pub fn clear_session(root: &Path) {
    swallow(with_locked(root, "session", |_| (Some(String::new()), ())), "clear_session");
}
pub fn reset_for_session_start(root: &Path) {
    swallow(with_locked(root, "agents", |_| (Some(String::new()), ())), "reset agents");
    swallow(with_locked(root, "inflight", |_| (Some(String::new()), ())), "reset inflight");
    close_turn(root, "session_start");
}

// ---- agents ----
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct OpenAgent { pub agent_id: String, pub agent_type: Option<String>, pub ts: u64, pub seq: u64 }

fn parse_lines<T: for<'de> Deserialize<'de>>(s: &str) -> Vec<T> {
    s.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}
fn to_lines<T: Serialize>(v: &[T]) -> String {
    let mut out = String::new();
    for x in v { if let Ok(l) = serde_json::to_string(x) { out.push_str(&l); out.push('\n'); } }
    out
}

pub fn push_agent(root: &Path, a: OpenAgent) {
    swallow(with_locked(root, "agents", |cur| {
        let mut v: Vec<OpenAgent> = parse_lines(&cur); v.push(a); (Some(to_lines(&v)), ())
    }), "push_agent");
}
pub fn pop_agent(root: &Path, agent_id: Option<&str>) -> Option<OpenAgent> {
    swallow(with_locked(root, "agents", |cur| {
        let mut v: Vec<OpenAgent> = parse_lines(&cur);
        let idx = match agent_id {
            Some(id) => v.iter().rposition(|a| a.agent_id == id),
            None => if v.is_empty() { None } else { Some(v.len() - 1) },
        };
        match idx { Some(i) => { let a = v.remove(i); (Some(to_lines(&v)), Some(a)) }, None => (None, None) }
    }), "pop_agent").flatten()
}

// ---- inflight ----
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Inflight { pub key: String, pub tool: String, pub ts: u64, pub agent_id: Option<String>, pub head_before: Option<String> }

fn visible(e: &Inflight, scope: Option<&str>) -> bool {
    e.agent_id.is_none() || e.agent_id.as_deref() == scope
}
fn live(e: &Inflight, now: u64) -> bool { now.saturating_sub(e.ts) <= INFLIGHT_TTL_SECS }

pub fn push_inflight(root: &Path, e: Inflight) {
    swallow(with_locked(root, "inflight", |cur| {
        let mut v: Vec<Inflight> = parse_lines(&cur); v.retain(|x| x.key != e.key); v.push(e); (Some(to_lines(&v)), ())
    }), "push_inflight");
}
pub fn pop_inflight(root: &Path, key: &str) -> Option<Inflight> {
    swallow(with_locked(root, "inflight", |cur| {
        let mut v: Vec<Inflight> = parse_lines(&cur);
        match v.iter().position(|x| x.key == key) {
            Some(i) => { let e = v.remove(i); (Some(to_lines(&v)), Some(e)) }
            None => (None, None),
        }
    }), "pop_inflight").flatten()
}
pub fn live_inflight(root: &Path, now: u64, agent_scope: Option<&str>) -> Vec<Inflight> {
    swallow(with_locked(root, "inflight", |cur| {
        let all: Vec<Inflight> = parse_lines(&cur);
        let kept: Vec<Inflight> = all.iter().filter(|e| live(e, now)).cloned().collect();
        let out: Vec<Inflight> = kept.iter().filter(|e| visible(e, agent_scope)).cloned().collect();
        let changed = kept.len() != all.len();
        (changed.then(|| to_lines(&kept)), out)
    }), "live_inflight").unwrap_or_default()
}
pub fn take_inflight_for_scope(root: &Path, now: u64, agent_scope: Option<&str>) -> Vec<Inflight> {
    swallow(with_locked(root, "inflight", |cur| {
        let all: Vec<Inflight> = parse_lines(&cur);
        let (taken, kept): (Vec<Inflight>, Vec<Inflight>) =
            all.into_iter().filter(|e| live(e, now)).partition(|e| visible(e, agent_scope));
        (Some(to_lines(&kept)), taken)
    }), "take_inflight_for_scope").unwrap_or_default()
}
pub fn inflight_key_for(tool_use_id: Option<&str>, tool_name: &str, tool_input: &serde_json::Value) -> String {
    if let Some(id) = tool_use_id.filter(|s| !s.is_empty()) { return id.to_string(); }
    // serde_json::Value::Object is a BTreeMap unless `preserve_order` is on, so
    // to_string is already canonical; re-serialize to be explicit.
    let canon = serde_json::to_string(tool_input).unwrap_or_default();
    let mut h = DefaultHasher::new();
    tool_name.hash(&mut h); canon.hash(&mut h);
    format!("h{:016x}", h.finish())
}

// ---- turn ----
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct Turn { pub open: bool, pub turn_id: Option<String>, pub last_prompt_ts: u64, pub last_event: String }

pub fn read_turn(root: &Path) -> Turn {
    std::fs::read_to_string(dir(root).join("turn")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}
pub fn open_turn(root: &Path, turn_id: Option<&str>, ts: u64) {
    let t = Turn { open: true, turn_id: turn_id.map(str::to_string), last_prompt_ts: ts, last_event: "prompt".into() };
    swallow(with_locked(root, "turn", |_| (serde_json::to_string(&t).ok(), ())), "open_turn");
}
pub fn close_turn(root: &Path, last_event: &str) {
    swallow(with_locked(root, "turn", |cur| {
        let mut t: Turn = serde_json::from_str(&cur).unwrap_or_default();
        t.open = false; t.last_event = last_event.to_string();
        (serde_json::to_string(&t).ok(), ())
    }), "close_turn");
}

// ---- kalpa ----
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Kalpa { pub name: String, pub started_ts: u64 }

pub fn read_kalpa(root: &Path) -> Option<Kalpa> {
    std::fs::read_to_string(dir(root).join("kalpa")).ok().and_then(|s| serde_json::from_str(&s).ok())
}
pub fn write_kalpa(root: &Path, k: &Kalpa) {
    swallow(with_locked(root, "kalpa", |_| (serde_json::to_string(k).ok(), ())), "write_kalpa");
}
pub fn clear_kalpa(root: &Path) {
    let _ = std::fs::remove_file(dir(root).join("kalpa"));
}
/// `^[a-z0-9][a-z0-9-]{0,63}$`. Written with explicit early returns rather
/// than a chained boolean: `&&` binds tighter than `||`, and the obvious
/// one-expression form silently accepts `-lead`.
pub fn valid_kalpa_name(name: &str) -> bool {
    let b = name.as_bytes();
    if !(1..=64).contains(&b.len()) { return false; }
    let first_ok = b[0].is_ascii_lowercase() || b[0].is_ascii_digit();
    first_ok && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --test lifecycle_state 2>&1 | tail -20`
Expected: 7 passed. `session_overwrite_and_clear` relies on `journey::current_sid` reading the file first, which it already does (`journey/mod.rs:42-48`).

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/lifecycle/state.rs crates/phronesis-mcp/tests/lifecycle_state.rs
git commit -m "feat(lifecycle): locked correlation state files"
```

---

### Task 6: `lifecycle::scrub::scrub_prompt`

**Files:**
- Create: `crates/phronesis-mcp/src/lifecycle/scrub.rs`
- Modify: `crates/phronesis-mcp/src/payload_scrub.rs:119` (`fn scrub_str` → `pub(crate) fn scrub_str`)
- Test: `crates/phronesis-mcp/tests/scrub_payload_integration.rs`

**Interfaces (Produces):**
```rust
pub fn scrub_prompt(project_root: &Path, text: &str) -> String;
```

- [ ] **Step 1: Write the failing tests**

Append to `tests/scrub_payload_integration.rs`:

```rust
#[test]
fn scrub_prompt_removes_bare_session_ids_transcripts_and_home_paths() {
    let home = std::env::var("HOME").unwrap();
    let root = tempfile::tempdir().unwrap();
    let text = format!(
        "resume session 0f3c9a1e-1234-4bcd-9ef0-abcdefabcdef please, transcript at {home}/.claude/projects/x/abc.jsonl and file {home}/secret/notes.txt"
    );
    let out = phronesis_mcp::lifecycle::scrub::scrub_prompt(root.path(), &text);
    assert!(!out.contains("0f3c9a1e-1234-4bcd-9ef0-abcdefabcdef"), "{out}");
    assert!(!out.contains("/.claude/projects/x/abc.jsonl"), "{out}");
    assert!(!out.contains(&format!("{home}/secret")), "{out}");
    assert!(out.contains("sess-00000000"), "{out}");
    assert!(out.contains("resume session"), "{out}");
}

#[test]
fn scrub_prompt_without_home_still_scrubs_project_root() {
    let root = tempfile::tempdir().unwrap();
    let text = format!("edit {}/src/main.rs now", root.path().display());
    let saved = std::env::var("HOME").ok();
    unsafe { std::env::remove_var("HOME"); }
    let out = phronesis_mcp::lifecycle::scrub::scrub_prompt(root.path(), &text);
    if let Some(h) = saved { unsafe { std::env::set_var("HOME", h); } }
    assert!(!out.contains(&root.path().display().to_string()), "{out}");
    assert!(out.contains("src/main.rs"), "{out}");
}
```

(If the file already has a serial-test pattern for env mutation, follow it; the `unsafe` blocks are required for `set_var` in edition 2024.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --test scrub_payload_integration scrub_prompt 2>&1 | tail -20`
Expected: unresolved import.

- [ ] **Step 3: Implement**

```rust
//! Prompt-text scrubbing for the action log. Wraps the text in a JSON object
//! so `Scrubber::scrub_value` applies its key-based rules, and pre-applies
//! regexes for session ids and transcript paths that appear as free text.

use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;

use crate::payload_scrub::Scrubber;

fn session_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)(session\S{0,20}?\s*[:=]?\s*)([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})").unwrap())
}
fn transcript_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\S*/\.(?:claude|codex|gemini)/\S*\.jsonl").unwrap())
}

/// Scrub free prompt text before it is written to `.phronesis/log.jsonl`.
/// Never fails: without a usable `$HOME` it falls back to project-root-only
/// scrubbing and logs once to stderr.
pub fn scrub_prompt(project_root: &Path, text: &str) -> String {
    let pre = session_re().replace_all(text, "${1}sess-00000000");
    let pre = transcript_re().replace_all(&pre, "/home/dev/.claude/transcript.jsonl");
    let root = project_root.display().to_string();
    let home = std::env::var("HOME").unwrap_or_default();
    let mut scrubber = match Scrubber::new(&home, &root) {
        Ok(s) => s,
        Err(_) => {
            eprintln!("phronesis: lifecycle scrub: HOME unusable, scrubbing project root only");
            return pre.replace(&root, "/home/dev/project");
        }
    };
    let mut v = serde_json::json!({ "prompt": pre.as_ref() });
    scrubber.scrub_value(&mut v);
    v["prompt"].as_str().unwrap_or_default().to_string()
}
```

The fallback branch's `"/home/dev/project"` is the same placeholder `Scrubber::scrub_str` uses for the project root (`payload_scrub.rs:57, 121`), so a prompt scrubbed with or without `$HOME` reads identically for the project-root prefix.

In `payload_scrub.rs`, change `fn scrub_str(&mut self, s: &str) -> String` (line 119) to `pub(crate) fn scrub_str(&mut self, s: &str) -> String`. `scrub_prompt` does not call it today — `scrub_value` on a one-key wrapper object applies it plus the session/transcript key rules — but the spec fixes the visibility so a future caller that already holds a plain `&str` has the same code path available.

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --test scrub_payload_integration 2>&1 | tail -20`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/lifecycle/scrub.rs crates/phronesis-mcp/src/payload_scrub.rs crates/phronesis-mcp/tests/scrub_payload_integration.rs
git commit -m "feat(lifecycle): scrub_prompt for action-log prompt text"
```

---

### Task 7: `lifecycle::record::record` and the `prompt_text` config

**Files:**
- Create: `crates/phronesis-mcp/src/lifecycle/record.rs`
- Modify: `crates/phronesis-mcp/src/journey/tagger.rs` (`TaggerConfig` gains `lifecycle: LifecycleConfig`)
- Test: `crates/phronesis-mcp/tests/lifecycle_state.rs` (append)

**Interfaces (Produces):**
```rust
// tagger.rs
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LifecycleConfig { #[serde(default)] pub prompt_text: PromptTextSetting }
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PromptTextSetting { #[default] Full, None }
// TaggerConfig: #[serde(default)] pub lifecycle: LifecycleConfig

// record.rs
pub fn record(root: &Path, event: LifecycleEvent) -> Option<Stamped>;  // None if journal append failed
pub fn prompt_text_setting(root: &Path) -> PromptText;               // reads journey.json, defaults Full
```

`record` does, in order: read kalpa; read `outcomes::subject::current(root)` for `subject`; `journey::current_sid`; `crate::hook::seq::next_seq`; `unix_secs_now`; build `Stamped`; `journal::append`; `action_log::append(&default_path(root), &to_log_entry(.., prompt_text_setting(root)))`; return `Some(stamped)`. Errors → stderr, and a failed journal append still attempts the log append.

Verified against the current tree: `hook/mod.rs:10` already declares `pub(crate) mod seq;` and `seq.rs:11` already declares `pub(crate) fn next_seq(project_root: &Path) -> u64`, so no visibility change is needed. `outcomes::subject::current(root) -> Option<String>` exists at `outcomes/subject.rs:33`. `journey::load_config(root) -> Result<TaggerConfig, ConfigError>` exists at `journey/mod.rs:109`.

- [ ] **Step 1: Write the failing tests** (append to `tests/lifecycle_state.rs`)

```rust
#[test]
fn record_writes_journal_and_log_with_kalpa_and_no_text_in_journal() {
    use phronesis_mcp::lifecycle::{Host, Kind, LifecycleEvent, Mode, record::record};
    let d = root();
    write_kalpa(d.path(), &Kalpa { name: "demo".into(), started_ts: 1 });
    let ev = LifecycleEvent::new(Kind::Prompt, Host::Claude).with_mode(Mode::Fresh).with_prompt("hello world");
    let stamped = record(d.path(), ev).unwrap();
    assert_eq!(stamped.kalpa.as_deref(), Some("demo"));
    let journal = std::fs::read_to_string(d.path().join(".phronesis/journey/events.jsonl")).unwrap();
    assert!(journal.contains(r#""kind":"prompt""#)); assert!(journal.contains(r#""kalpa":"demo""#));
    assert!(!journal.contains("hello world"));
    let log = std::fs::read_to_string(d.path().join(".phronesis/log.jsonl")).unwrap();
    assert!(log.contains(r#""kind":"lifecycle""#)); assert!(log.contains(r#""prompt":"hello world""#));
}

#[test]
fn record_honors_prompt_text_none() {
    use phronesis_mcp::lifecycle::{Host, Kind, LifecycleEvent, Mode, record::record};
    let d = root();
    std::fs::create_dir_all(d.path().join(".phronesis")).unwrap();
    std::fs::write(d.path().join(".phronesis/journey.json"), r#"{"tags":{},"lifecycle":{"prompt_text":"none"}}"#).unwrap();
    record(d.path(), LifecycleEvent::new(Kind::Prompt, Host::Codex).with_mode(Mode::Fresh).with_prompt("hidden")).unwrap();
    let log = std::fs::read_to_string(d.path().join(".phronesis/log.jsonl")).unwrap();
    assert!(!log.contains("hidden")); assert!(log.contains(r#""prompt_bytes":6"#));
}
```

Check `TaggerConfig`'s required fields (`tags`, `modules`) so the JSON above parses; adjust to the minimal valid document.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --test lifecycle_state record_ 2>&1 | tail -20`
Expected: unresolved import `record`.

- [ ] **Step 3: Implement**

`tagger.rs` additions (near `TaggerConfig`):

```rust
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PromptTextSetting { #[default] Full, None }

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LifecycleConfig {
    #[serde(default)]
    pub prompt_text: PromptTextSetting,
}
// in TaggerConfig:
    #[serde(default)]
    pub lifecycle: LifecycleConfig,
```

`record.rs`:

```rust
//! `record` — stamp a `LifecycleEvent` and write both projections.

use std::path::Path;

use crate::action_log;
use crate::journey;
use crate::journey::tagger::PromptTextSetting;
use crate::lifecycle::event::{LifecycleEvent, PromptText, Stamped};
use crate::lifecycle::state;

fn unix_secs_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub fn prompt_text_setting(root: &Path) -> PromptText {
    match journey::load_config(root).map(|c| c.lifecycle.prompt_text) {
        Ok(PromptTextSetting::None) => PromptText::None,
        _ => PromptText::Full,
    }
}

pub fn record(root: &Path, event: LifecycleEvent) -> Option<Stamped> {
    let stamped = Stamped {
        ts: unix_secs_now(),
        sid: journey::current_sid(root),
        seq: crate::hook::seq::next_seq(root),
        kalpa: state::read_kalpa(root).map(|k| k.name),
        subject: crate::outcomes::subject::current(root),
    };
    let journaled = match journey::journal::append(root, &event.to_journal_record(&stamped)) {
        Ok(()) => true,
        Err(e) => { eprintln!("phronesis: lifecycle journal append failed: {e}"); false }
    };
    let entry = event.to_log_entry(&stamped, prompt_text_setting(root));
    if let Err(e) = action_log::append(&action_log::default_path(root), &entry) {
        eprintln!("phronesis: lifecycle log append failed: {e}");
    }
    journaled.then_some(stamped)
}
```

No visibility changes are needed for this task: `pub(crate) mod seq;` and `pub(crate) fn next_seq` already exist, as does `outcomes::subject::current(root) -> Option<String>`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --test lifecycle_state 2>&1 | tail -20`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/lifecycle/record.rs crates/phronesis-mcp/src/journey/tagger.rs crates/phronesis-mcp/src/hook/mod.rs crates/phronesis-mcp/tests/lifecycle_state.rs
git commit -m "feat(lifecycle): record() writes journal and log; prompt_text config"
```

---

### Task 8: `classify_prompt`

**Files:**
- Modify: `crates/phronesis-mcp/src/lifecycle/state.rs` (append)
- Test: `crates/phronesis-mcp/tests/lifecycle_classify.rs` (new)

**Interfaces (Produces):**
```rust
pub struct PromptContext<'a> { pub host: Host, pub now: u64, pub agent_id: Option<&'a str>, pub turn_id: Option<&'a str>, pub transcript_path: Option<&'a Path>, pub last_journal_kind: Option<&'a str> }
pub enum InterruptSource { Hook, Inflight, Transcript, OpenTurn }
impl InterruptSource { pub fn as_str(self) -> &'static str; /* hook | inflight | transcript | open_turn */ }
pub struct Classification { pub mode: Mode, pub interrupt: Option<InterruptSource> }
pub fn classify_prompt(root: &Path, ctx: &PromptContext<'_>) -> Classification;
```

`last_journal_kind` is the `kind` of the most recent journal record for the current sid, or `None`; callers obtain it with `journey::journal::read_recent(root, 1)` filtered to lifecycle records (Plan 2/3 do this). The Codex branch fires when `last_journal_kind == Some("interrupt")`.

Algorithm exactly as spec §Classification. Transcript check: read the last 64 KiB of `transcript_path`, split into lines, parse each as JSON, and consider a hit when any object has `type == "user"` (or a nested `message.role == "user"`) whose text content (string, or first `content[].text`) equals `[Request interrupted by user]` or starts with `[Request interrupted by user for tool use]`, and whose `timestamp` (RFC3339, if present) is after `turn.last_prompt_ts`; if no timestamp is present, accept the hit.

- [ ] **Step 1: Write the failing tests** in `tests/lifecycle_classify.rs`

```rust
use phronesis_mcp::lifecycle::state::*;
use phronesis_mcp::lifecycle::{Host, Mode};

fn root() -> tempfile::TempDir { tempfile::tempdir().unwrap() }
fn ctx<'a>(host: Host, now: u64) -> PromptContext<'a> {
    PromptContext { host, now, agent_id: None, turn_id: None, transcript_path: None, last_journal_kind: None }
}

#[test]
fn closed_turn_is_fresh() {
    let d = root();
    let c = classify_prompt(d.path(), &ctx(Host::Claude, 100));
    assert_eq!(c.mode, Mode::Fresh); assert!(c.interrupt.is_none());
}

#[test]
fn open_turn_no_evidence_is_mid_turn_on_claude_and_codex() {
    for host in [Host::Claude, Host::Codex] {
        let d = root(); open_turn(d.path(), None, 10);
        let c = classify_prompt(d.path(), &ctx(host, 100));
        assert_eq!(c.mode, Mode::MidTurn, "{host:?}"); assert!(c.interrupt.is_none());
    }
}

#[test]
fn open_turn_on_gemini_is_interrupt_correction() {
    let d = root(); open_turn(d.path(), None, 10);
    let c = classify_prompt(d.path(), &ctx(Host::Gemini, 100));
    assert_eq!(c.mode, Mode::Correction); assert_eq!(c.interrupt.map(|i| i.as_str()), Some("open_turn"));
}

#[test]
fn live_inflight_in_scope_is_interrupt_and_is_consumed() {
    let d = root(); open_turn(d.path(), None, 10);
    push_inflight(d.path(), Inflight { key: "k".into(), tool: "Bash".into(), ts: 90, agent_id: None, head_before: None });
    let c = classify_prompt(d.path(), &ctx(Host::Claude, 100));
    assert_eq!(c.mode, Mode::Correction); assert_eq!(c.interrupt.map(|i| i.as_str()), Some("inflight"));
    assert!(live_inflight(d.path(), 100, None).is_empty());
}

#[test]
fn stale_inflight_and_foreign_agent_inflight_are_ignored() {
    let d = root(); open_turn(d.path(), None, 10);
    push_inflight(d.path(), Inflight { key: "stale".into(), tool: "Bash".into(), ts: 1, agent_id: None, head_before: None });
    push_inflight(d.path(), Inflight { key: "child".into(), tool: "Edit".into(), ts: 1000, agent_id: Some("sub".into()), head_before: None });
    let c = classify_prompt(d.path(), &ctx(Host::Claude, 1000));
    assert_eq!(c.mode, Mode::MidTurn);
}

#[test]
fn codex_interrupt_record_precedes_prompt() {
    let d = root(); open_turn(d.path(), None, 10);
    let mut c = ctx(Host::Codex, 100); c.last_journal_kind = Some("interrupt");
    let out = classify_prompt(d.path(), &c);
    assert_eq!(out.mode, Mode::Correction); assert_eq!(out.interrupt.map(|i| i.as_str()), Some("hook"));
}

#[test]
fn claude_transcript_marker_after_last_prompt_is_interrupt() {
    let d = root(); open_turn(d.path(), None, 1_700_000_000);
    let t = d.path().join("t.jsonl");
    std::fs::write(&t, concat!(
        r#"{"type":"user","message":{"role":"user","content":"do it"},"timestamp":"2023-11-14T22:13:00Z"}"#, "\n",
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"working"}]},"timestamp":"2023-11-14T22:13:30Z"}"#, "\n",
        r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"[Request interrupted by user]"}]},"timestamp":"2023-11-14T22:14:00Z"}"#, "\n",
    )).unwrap();
    let mut c = ctx(Host::Claude, 1_700_000_100); c.transcript_path = Some(&t);
    let out = classify_prompt(d.path(), &c);
    assert_eq!(out.mode, Mode::Correction); assert_eq!(out.interrupt.map(|i| i.as_str()), Some("transcript"));
}

#[test]
fn claude_transcript_marker_before_last_prompt_is_ignored() {
    let d = root(); open_turn(d.path(), None, 1_700_000_500);
    let t = d.path().join("t.jsonl");
    std::fs::write(&t, r#"{"type":"user","message":{"role":"user","content":"[Request interrupted by user]"},"timestamp":"2023-11-14T22:13:00Z"}"#).unwrap();
    let mut c = ctx(Host::Claude, 1_700_000_600); c.transcript_path = Some(&t);
    assert_eq!(classify_prompt(d.path(), &c).mode, Mode::MidTurn);
}
```

(1_700_000_000 is 2023-11-14T22:13:20Z; the marker at 22:14:00Z is after it, the one at 22:13:00Z is before 1_700_000_500.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --test lifecycle_classify 2>&1 | tail -20`
Expected: unresolved imports.

- [ ] **Step 3: Implement** (append to `state.rs`)

```rust
use crate::lifecycle::event::{Host, Mode};

pub struct PromptContext<'a> {
    pub host: Host, pub now: u64, pub agent_id: Option<&'a str>, pub turn_id: Option<&'a str>,
    pub transcript_path: Option<&'a Path>, pub last_journal_kind: Option<&'a str>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptSource { Hook, Inflight, Transcript, OpenTurn }
impl InterruptSource {
    pub fn as_str(self) -> &'static str {
        match self { Self::Hook => "hook", Self::Inflight => "inflight", Self::Transcript => "transcript", Self::OpenTurn => "open_turn" }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classification { pub mode: Mode, pub interrupt: Option<InterruptSource> }

pub fn classify_prompt(root: &Path, ctx: &PromptContext<'_>) -> Classification {
    let turn = read_turn(root);
    if !turn.open { return Classification { mode: Mode::Fresh, interrupt: None }; }
    let correction = |src| Classification { mode: Mode::Correction, interrupt: Some(src) };
    if ctx.host == Host::Codex && ctx.last_journal_kind == Some("interrupt") { return correction(InterruptSource::Hook); }
    if !take_inflight_for_scope(root, ctx.now, ctx.agent_id).is_empty() { return correction(InterruptSource::Inflight); }
    if ctx.host == Host::Claude
        && let Some(p) = ctx.transcript_path
        && transcript_has_interrupt_after(p, turn.last_prompt_ts)
    { return correction(InterruptSource::Transcript); }
    if ctx.host == Host::Gemini { return correction(InterruptSource::OpenTurn); }
    Classification { mode: Mode::MidTurn, interrupt: None }
}

const TRANSCRIPT_TAIL_BYTES: u64 = 64 * 1024;
const INTERRUPT_MARKER: &str = "[Request interrupted by user";

fn transcript_has_interrupt_after(path: &Path, after_ts: u64) -> bool {
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(TRANSCRIPT_TAIL_BYTES);
    if f.seek(SeekFrom::Start(start)).is_err() { return false; }
    let mut buf = String::new();
    if f.read_to_string(&mut buf).is_err() { return false; }
    buf.lines().skip(usize::from(start > 0)).any(|line| {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { return false };
        let is_user = v.get("type").and_then(|t| t.as_str()) == Some("user")
            || v.pointer("/message/role").and_then(|r| r.as_str()) == Some("user");
        if !is_user { return false; }
        let text = match v.pointer("/message/content").or_else(|| v.get("content")) {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(serde_json::Value::Array(items)) => items.iter().filter_map(|i| i.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join("\n"),
            _ => String::new(),
        };
        if !text.trim_start().starts_with(INTERRUPT_MARKER) { return false; }
        match v.get("timestamp").and_then(|t| t.as_str()).and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()) {
            Some(dt) => dt.timestamp() as u64 > after_ts,
            None => true,
        }
    })
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --test lifecycle_classify --test lifecycle_state 2>&1 | tail -20`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/lifecycle/state.rs crates/phronesis-mcp/tests/lifecycle_classify.rs
git commit -m "feat(lifecycle): classify_prompt with inflight, transcript, open-turn and Codex hook evidence"
```

---

### Task 9: `lifecycle::outcome::detect_commit`

**Files:**
- Create: `crates/phronesis-mcp/src/lifecycle/outcome.rs`
- Test: `crates/phronesis-mcp/tests/lifecycle_outcome.rs` (new)

**Interfaces (Produces):**
```rust
pub fn is_shell_tool(tool_name: &str) -> bool;                 // Bash | run_shell_command
pub fn command_may_move_head(command: &str) -> bool;            // cheap pre-filter
pub fn git_head(root: &Path) -> Option<String>;                 // `git rev-parse HEAD`, 2 s timeout
pub struct Commit { pub sha: String, pub head_before: String }
pub fn detect_commit(root: &Path, head_before: Option<&str>, command: &str, command_exit: Option<i32>) -> Option<Commit>;
```

- [ ] **Step 1: Write the failing tests** in `tests/lifecycle_outcome.rs`

```rust
use phronesis_mcp::lifecycle::outcome::*;
use std::process::Command;

fn git(dir: &std::path::Path, args: &[&str]) {
    let st = Command::new("git").args(args).current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t")
        .status().unwrap();
    assert!(st.success(), "git {args:?}");
}
fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q"]); std::fs::write(d.path().join("a"), "1").unwrap();
    git(d.path(), &["add", "a"]); git(d.path(), &["commit", "-q", "-m", "init"]);
    d
}

#[test]
fn prefilter_matches_head_moving_commands_only() {
    assert!(command_may_move_head("git commit -m x"));
    assert!(command_may_move_head("cargo test && git commit -am done"));
    assert!(command_may_move_head("git cherry-pick abc"));
    assert!(!command_may_move_head("git status"));
    assert!(!command_may_move_head("cargo build"));
}

#[test]
fn real_commit_is_detected_with_shas() {
    let d = repo();
    let before = git_head(d.path()).unwrap();
    std::fs::write(d.path().join("a"), "2").unwrap();
    git(d.path(), &["commit", "-q", "-am", "second"]);
    let c = detect_commit(d.path(), Some(&before), "git commit -am second", Some(0)).unwrap();
    assert_eq!(c.head_before, before); assert_ne!(c.sha, before); assert_eq!(c.sha.len(), 40);
}

#[test]
fn dry_run_heredoc_and_sibling_repo_are_not_commits() {
    let d = repo(); let before = git_head(d.path()).unwrap();
    assert!(detect_commit(d.path(), Some(&before), "git commit --dry-run", Some(0)).is_none());
    assert!(detect_commit(d.path(), Some(&before), "cat <<EOF > doc.md\nrun git commit -m x\nEOF", Some(0)).is_none());
    let other = repo(); std::fs::write(other.path().join("a"), "9").unwrap(); git(other.path(), &["commit", "-q", "-am", "elsewhere"]);
    assert!(detect_commit(d.path(), Some(&before), &format!("git -C {} commit -am x", other.path().display()), Some(0)).is_none());
}

#[test]
fn failed_chain_and_missing_head_before_are_not_commits() {
    let d = repo(); let before = git_head(d.path()).unwrap();
    std::fs::write(d.path().join("a"), "3").unwrap(); git(d.path(), &["commit", "-q", "-am", "third"]);
    assert!(detect_commit(d.path(), Some(&before), "git commit -am third && false", Some(1)).is_none());
    assert!(detect_commit(d.path(), None, "git commit -am third", Some(0)).is_none());
}

#[test]
fn shell_tools() {
    assert!(is_shell_tool("Bash")); assert!(is_shell_tool("run_shell_command")); assert!(!is_shell_tool("Edit"));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --test lifecycle_outcome 2>&1 | tail -20`
Expected: unresolved imports.

- [ ] **Step 3: Implement**

```rust
//! Commit detection from ground truth: HEAD before the shell call vs after.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;

pub fn is_shell_tool(tool_name: &str) -> bool { matches!(tool_name, "Bash" | "run_shell_command") }

fn prefilter() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\bgit\b[^\n|;&]*\b(commit|cherry-pick|revert|merge|rebase)\b").unwrap())
}
pub fn command_may_move_head(command: &str) -> bool { prefilter().is_match(command) }

pub fn git_head(root: &Path) -> Option<String> {
    let mut child = Command::new("git").args(["rev-parse", "HEAD"]).current_dir(root)
        .stdout(Stdio::piped()).stderr(Stdio::null()).stdin(Stdio::null()).spawn().ok()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() { return None; }
                let mut out = String::new();
                use std::io::Read;
                child.stdout.take()?.read_to_string(&mut out).ok()?;
                let sha = out.trim().to_string();
                return (sha.len() == 40).then_some(sha);
            }
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => { let _ = child.kill(); return None; }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit { pub sha: String, pub head_before: String }

pub fn detect_commit(root: &Path, head_before: Option<&str>, command: &str, command_exit: Option<i32>) -> Option<Commit> {
    let before = head_before?;
    if command_exit != Some(0) || !command_may_move_head(command) { return None; }
    let after = git_head(root)?;
    (after != before).then(|| Commit { sha: after, head_before: before.to_string() })
}
```

Note the heredoc test passes because `HEAD` did not move, not because of the regex; that is the point of the design.

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --test lifecycle_outcome 2>&1 | tail -20`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/lifecycle/outcome.rs crates/phronesis-mcp/tests/lifecycle_outcome.rs
git commit -m "feat(lifecycle): detect_commit from HEAD movement"
```

---

### Task 10: `HookPayload` widening and capture redaction

**Files:**
- Modify: `crates/phronesis-mcp/src/hook/mod.rs:55-65` (struct), `:57-64` (comment), `:88-114` (capture)
- Test: `crates/phronesis-mcp/tests/payload_capture.rs`

**Interfaces (Produces):**
```rust
// HookPayload gains, all #[serde(default)]:
pub(super) session_id: Option<String>, pub(super) tool_use_id: Option<String>,
pub(super) hook_event_name: Option<String>, pub(super) agent_id: Option<String>,
// hook/mod.rs:
pub(crate) const REDACTED_KEYS: [&str; 2] = ["prompt", "last_assistant_message"];
/// `pub`, not `pub(crate)`: `tests/payload_capture.rs` is an integration test
/// and reaches it as `phronesis_mcp::hook::redact_for_capture`.
pub fn redact_for_capture(raw: &str) -> String;   // replaces those keys' string values with "<redacted:N bytes>", returns raw unchanged if not a JSON object
/// Promoted from private `fn` so `claude_hook.rs` (Plan 2) and `codex_hook.rs`
/// (Plan 3) can tee their own stdin. Plan 1 makes this change; neither
/// dependent plan touches `hook/mod.rs` for it.
pub(crate) fn capture_raw_payload(phase: &str, raw: &str);
```

Wire `redact_for_capture` into `capture_raw_payload` so the tee writes the redacted string. Because `read_payload` already calls `capture_raw_payload`, this covers `pre-check` and `post-check` in the same edit. Plans 2 and 3 call `capture_raw_payload` directly from their adapters and rely on this task for both the visibility and the redaction.

- [ ] **Step 1: Write the failing test** (append to `tests/payload_capture.rs`, following the file's existing pattern for setting `PHRONESIS_CAPTURE_DIR` and invoking `phr-mcp pre-check`)

```rust
#[test]
fn capture_redacts_prompt_and_last_assistant_message() {
    let raw = r#"{"hook_event_name":"UserPromptSubmit","session_id":"s","prompt":"top secret words","tool_name":"Bash","tool_input":{"command":"ls"}}"#;
    let out = phronesis_mcp::hook::redact_for_capture(raw);
    assert!(!out.contains("top secret")); assert!(out.contains(r#""prompt":"<redacted:16 bytes>""#));
    assert!(out.contains(r#""tool_input":{"command":"ls"}"#));
    assert_eq!(phronesis_mcp::hook::redact_for_capture("not json"), "not json");
}
```

Also add an end-to-end case using the file's existing helper that runs `pre-check` with `PHRONESIS_CAPTURE_DIR` set and a payload containing `"prompt":"zzz-secret"`, asserting `payloads.jsonl` lacks `zzz-secret`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --test payload_capture capture_redacts 2>&1 | tail -20`
Expected: `redact_for_capture` not found.

- [ ] **Step 3: Implement**

In `hook/mod.rs`:

```rust
#[derive(Debug, Deserialize)]
pub(super) struct HookPayload {
    pub(super) tool_name: Option<String>,
    pub(super) tool_input: Option<serde_json::Value>,
    /// PostToolUse payloads carry the tool's output here. Claude Code and
    /// Gemini both send `tool_response`; this repo's own integration tests
    /// use `tool_output`. The alias accepts both.
    #[serde(default, alias = "tool_response")]
    pub(super) tool_output: Option<serde_json::Value>,
    #[serde(default)] pub(super) session_id: Option<String>,
    #[serde(default)] pub(super) tool_use_id: Option<String>,
    #[serde(default)] pub(super) hook_event_name: Option<String>,
    #[serde(default)] pub(super) agent_id: Option<String>,
}

pub(crate) const REDACTED_KEYS: [&str; 2] = ["prompt", "last_assistant_message"];

/// Redact free-text fields before the raw payload tee. Non-object input is
/// returned unchanged so malformed payloads stay reproducible.
pub fn redact_for_capture(raw: &str) -> String {
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(raw) else { return raw.to_string() };
    let Some(obj) = v.as_object_mut() else { return raw.to_string() };
    for k in REDACTED_KEYS {
        if let Some(serde_json::Value::String(s)) = obj.get(k) {
            let n = s.len();
            obj.insert(k.to_string(), serde_json::Value::String(format!("<redacted:{n} bytes>")));
        }
    }
    serde_json::to_string(&v).unwrap_or_else(|_| raw.to_string())
}
```

Change `fn capture_raw_payload(phase: &str, raw: &str)` (`hook/mod.rs:90`) to `pub(crate) fn capture_raw_payload(phase: &str, raw: &str)` and redact before the tee. The function currently embeds `raw` under a `"raw"` key; redact the string first so the parsed value it stores is already clean:

```rust
pub(crate) fn capture_raw_payload(phase: &str, raw: &str) {
    let Ok(dir) = std::env::var("PHRONESIS_CAPTURE_DIR") else {
        return;
    };
    // Redact free-text fields before anything is written. Prompt text must
    // never reach `payloads.jsonl`, which the corpus-promotion doc copies into
    // a committed tree (spec §"Payload capture").
    let raw = redact_for_capture(raw);
    let record = serde_json::json!({
        "ts": unix_secs_now(),
        "phase": phase,
        "raw": serde_json::from_str::<serde_json::Value>(&raw)
            .unwrap_or_else(|_| serde_json::Value::String(raw.clone())),
    });
    // …the rest of the function is unchanged.
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --test payload_capture --test hook_integration 2>&1 | tail -20`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/hook/mod.rs crates/phronesis-mcp/tests/payload_capture.rs
git commit -m "feat(hook): widen HookPayload and redact prompt text in payload capture"
```

---

### Task 11: `phr-mcp kalpa` subcommand

**Files:**
- Create: `crates/phronesis-mcp/src/lifecycle/kalpa_cli.rs`
- Modify: `crates/phronesis-mcp/src/main.rs` (Command enum + dispatch)
- Test: `crates/phronesis-mcp/tests/kalpa_integration.rs` (new)

**Interfaces (Produces):**
```rust
pub enum KalpaCmd { Start { name: String }, End, Show { name: Option<String> } }   // clap Subcommand
pub fn run(root: &Path, cmd: KalpaCmd) -> anyhow::Result<String>;              // returns text to print
pub fn header_line(root: &Path, now: u64) -> Option<String>;  // "kalpa: <name> (<age>)" [+ " (stale? run phr-mcp kalpa end)" past 30 days]; Plan 5 prints this in journey/stats headers
```

`Show` prints the header line and nothing else — no "counts: see Plan 5" stub, which would be a placeholder shipped to a user. Plan 5 Task 3 replaces the `Show` arm with one that appends the count block. `run` with `Show` and no name uses the open kalpa; with no open kalpa it returns `Err("no kalpa open")`.

- [ ] **Step 1: Write the failing tests** in `tests/kalpa_integration.rs` (use the same `phr-mcp` binary invocation helper style as `tests/journey_cli_integration.rs`; read that file first and copy its `cargo_bin`/`Command` setup)

```rust
#[test]
fn kalpa_start_show_end_round_trip_and_events() {
    let d = tempfile::tempdir().unwrap();
    let out = run_phr(d.path(), &["kalpa", "start", "lifecycle-events"]);
    assert!(out.status.success());
    let show = run_phr(d.path(), &["kalpa"]);
    assert!(String::from_utf8_lossy(&show.stdout).contains("kalpa: lifecycle-events"));
    let journal = std::fs::read_to_string(d.path().join(".phronesis/journey/events.jsonl")).unwrap();
    assert!(journal.contains(r#""kind":"kalpa_start""#)); assert!(journal.contains(r#""kalpa":"lifecycle-events""#));
    let end = run_phr(d.path(), &["kalpa", "end"]);
    assert!(end.status.success());
    let journal = std::fs::read_to_string(d.path().join(".phronesis/journey/events.jsonl")).unwrap();
    assert!(journal.contains(r#""kind":"kalpa_end""#));
    assert!(!d.path().join(".phronesis/journey/kalpa").exists());
}

#[test]
fn kalpa_rejects_bad_names_and_survives_session_reset() {
    let d = tempfile::tempdir().unwrap();
    assert!(!run_phr(d.path(), &["kalpa", "start", "Bad Name"]).status.success());
    assert!(run_phr(d.path(), &["kalpa", "start", "ok-1"]).status.success());
    phronesis_mcp::lifecycle::state::reset_for_session_start(d.path());
    assert_eq!(phronesis_mcp::lifecycle::state::read_kalpa(d.path()).unwrap().name, "ok-1");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --test kalpa_integration 2>&1 | tail -20`
Expected: `kalpa` unrecognized subcommand.

- [ ] **Step 3: Implement**

`kalpa_cli.rs`:

```rust
//! `phr-mcp kalpa` — name the theme a run of sessions belongs to.

use std::path::Path;

use crate::lifecycle::event::{Host, Kind, LifecycleEvent};
use crate::lifecycle::record::record;
use crate::lifecycle::state::{self, Kalpa};

#[derive(clap::Subcommand, Debug)]
pub enum KalpaCmd {
    /// Open a kalpa (ends any open one first). Name: [a-z0-9][a-z0-9-]{0,63}.
    Start { name: String },
    /// Close the open kalpa.
    End,
    /// Show the open kalpa, or a named one.
    Show { name: Option<String> },
}

const STALE_AFTER_SECS: u64 = 30 * 24 * 3600;

fn now() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }

fn age(secs: u64) -> String {
    match secs { s if s < 3600 => format!("{}m", s / 60), s if s < 86_400 => format!("{}h", s / 3600), s => format!("{}d", s / 86_400) }
}

pub fn header_line(root: &Path, now: u64) -> Option<String> {
    let k = state::read_kalpa(root)?;
    let elapsed = now.saturating_sub(k.started_ts);
    let mut line = format!("kalpa: {} ({})", k.name, age(elapsed));
    if elapsed > STALE_AFTER_SECS { line.push_str(" (stale? run phr-mcp kalpa end)"); }
    Some(line)
}

fn end_open(root: &Path) -> Option<String> {
    let k = state::read_kalpa(root)?;
    state::clear_kalpa(root);
    record(root, LifecycleEvent::new(Kind::KalpaEnd, Host::Cli).with_extra("kalpa_name", k.name.clone()));
    Some(k.name)
}

pub fn run(root: &Path, cmd: KalpaCmd) -> anyhow::Result<String> {
    match cmd {
        KalpaCmd::Start { name } => {
            if !state::valid_kalpa_name(&name) { anyhow::bail!("invalid kalpa name `{name}`: use [a-z0-9][a-z0-9-]{{0,63}}"); }
            let ended = end_open(root);
            state::write_kalpa(root, &Kalpa { name: name.clone(), started_ts: now() });
            record(root, LifecycleEvent::new(Kind::KalpaStart, Host::Cli));
            Ok(match ended { Some(e) => format!("ended kalpa {e}\nstarted kalpa {name}"), None => format!("started kalpa {name}") })
        }
        KalpaCmd::End => match end_open(root) { Some(n) => Ok(format!("ended kalpa {n}")), None => anyhow::bail!("no kalpa open") },
        KalpaCmd::Show { name } => {
            match (name, state::read_kalpa(root)) {
                (None, Some(_)) => Ok(header_line(root, now()).unwrap_or_default()),
                (Some(n), Some(k)) if k.name == n => Ok(header_line(root, now()).unwrap_or_default()),
                (Some(n), _) => Ok(format!("kalpa: {n} (closed)")),
                (None, None) => anyhow::bail!("no kalpa open"),
            }
        }
    }
}
```

Note `kalpa_end` records after `clear_kalpa`, so the record's `kalpa` field is `None`; the name travels in `extra.kalpa_name`. `kalpa_start` records after `write_kalpa`, so it is stamped with the new name.

`main.rs`: add to the `Command` enum. **Placement convention for this whole five-plan set:** every new `Command` variant goes in alphabetical position by variant name among the *newly added* lifecycle variants, and the group as a whole sits immediately after the existing `CodexHook` variant (`main.rs:424`). The set adds exactly two: `ClaudeHook` (Plan 2) and `Kalpa` (Plan 1). Alphabetically `ClaudeHook` precedes `Kalpa`, so the final order after both land is `CodexHook`, `ClaudeHook`, `Kalpa`. Plan 1 lands first and puts `Kalpa` directly after `CodexHook`; Plan 2 inserts `ClaudeHook` between them. Same rule in the `match cli.command` dispatch block, so the two edits never touch the same line.

```rust
    /// Name the theme (kalpa) the current run of sessions belongs to.
    Kalpa {
        #[command(subcommand)]
        cmd: Option<phronesis_mcp::lifecycle::kalpa_cli::KalpaCmd>,
    },
```

and in dispatch:

```rust
        Command::Kalpa { cmd } => {
            let root = phronesis_mcp::security::project_root();
            let cmd = cmd.unwrap_or(phronesis_mcp::lifecycle::kalpa_cli::KalpaCmd::Show { name: None });
            let out = phronesis_mcp::lifecycle::kalpa_cli::run(&root, cmd)?;
            println!("{out}");
            Ok(())
        }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --test kalpa_integration --test cli_smoke 2>&1 | tail -20`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/lifecycle/kalpa_cli.rs crates/phronesis-mcp/src/main.rs crates/phronesis-mcp/tests/kalpa_integration.rs
git commit -m "feat(cli): phr-mcp kalpa start/end/show"
```

---

### Task 12: Spec amendment and changelog

**Files:**
- Modify: `docs/specs/SPEC-journey-facts.md` (§"The journal record", after the "One line per executed tool call" paragraph, around line 161)
- Modify: `CHANGELOG.md` (`## [Unreleased]` → `### Added`)

- [ ] **Step 1: Add the amendment paragraph**

```markdown
> **Amendment (lifecycle events, 2026-09-18).** From record schema v2, one
> line per executed tool call **or lifecycle event**. Lifecycle records are
> written by the event's own hook, carry `tool: "__lifecycle"` and
> `kind: <event>`, and are excluded from every record-position and
> record-count computation by the tool projection described in
> `SPEC-agent-lifecycle-events.md` §"The journal record, v2". They never carry
> content beyond tags and ids.
```

- [ ] **Step 2: Add the changelog entry**

Under `## [Unreleased]` / `### Added`:

```markdown
- **Lifecycle events, foundation.** `JournalRecord` v2 with optional `kind`,
  `mode`, `host`, `turn`, `agent`, `agent_type`, `kalpa`; the derive pass
  computes positional windows on tool records only, so existing `journey_*`
  rules are unchanged; built-in `lifecycle:*` and `kalpa:*` selectors; new
  `lifecycle` module (`LifecycleEvent`, locked state files, `classify_prompt`,
  `detect_commit`, `scrub_prompt`); `phr-mcp kalpa start|end|show`; prompt
  text is redacted from `PHRONESIS_CAPTURE_DIR` captures. No host emits
  lifecycle events yet (adapters follow).
```

- [ ] **Step 3: Full verification**

Run: `cargo fmt --all -- --check && cargo clippy --all-targets -p phronesis-mcp -- -D warnings && cargo test -p phronesis-mcp 2>&1 | tail -30`
Expected: clean, all green.

- [ ] **Step 4: Commit**

```bash
git add docs/specs/SPEC-journey-facts.md CHANGELOG.md
git commit -m "docs: journey spec amendment and changelog for lifecycle foundation"
```

---

## Self-review

- **Spec coverage (steps 1a/1b):** journal v2 (T1), projection + selectors + read bound (T2), compaction (T3), shared type (T4), state files with lock, TTL, scope, session overwrite (T5), scrub_prompt (T6), record + prompt_text (T7), classify (T8), detect_commit (T9), HookPayload widening + comment fix + capture redaction (T10), kalpa subcommand + header (T11), spec amendment + changelog (T12). Not in this plan by design: pre/post inflight push/pop and `invoke_agent` derivation (Plan 2), Codex payload fields (Plan 3), Gemini registrations (Plan 4), stats/metrics/CLI rendering and `kalpa show` counts (Plan 5).
- **Type consistency:** `LifecycleEvent`, `Stamped`, `PromptText`, `Kind`, `Mode`, `Host` are defined once in T4 and used by T7, T8, T11 with the same names. `Inflight`, `OpenAgent`, `Turn`, `Kalpa`, `PromptContext`, `Classification`, `InterruptSource` are defined in T5/T8. `Commit` in T9. `redact_for_capture` in T10.
- **Placeholders:** none. T11's `Show` intentionally prints only the header until Plan 5 adds counts, and says so. T3's compaction test and T2's `validate_selectors` loop are written out in full against the real `maybe_compact` / `validate_selectors` bodies.
- **Ownership:** this plan is the sole owner of `hook/mod.rs` for the feature. `HookPayload`'s new fields, `redact_for_capture`, `capture_raw_payload`'s `pub(crate)` visibility, and the corrected `tool_output` comment all land in T10. Plans 2 and 3 consume them and must not re-make those changes; Plan 2 adds only the single `mod lifecycle_wiring;` line to that file.

# Lifecycle Events — Plan 1: Foundation (spec steps 1a + 1b)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the shared lifecycle event type, journal schema v2 with the tool projection, the correlation state files, prompt scrubbing, and the `kalpa` subcommand, so that the three host adapter plans (2, 3, 4) and the reporting plan (5) can be built in parallel against fixed signatures. Nothing emits a lifecycle event from a hook yet.

**Architecture:** A new `crates/phronesis-mcp/src/lifecycle/` module owns one type, `LifecycleEvent`, and the two on-disk projections of it (journal record, action-log entry). `journey/derive.rs` learns to compute every positional aggregate on the tool-record projection so existing rules see identical facts. Five small state files under `.phronesis/journey/` are read-modify-written under a sibling lock file. Everything is best-effort: a lifecycle failure never fails a hook.

**Tech Stack:** Rust 2024 edition (rust-version 1.90), serde/serde_json, fs2 advisory locks, tempfile for tests. No new dependencies.

**Spec:** `docs/specs/SPEC-agent-lifecycle-events.md` (revised 2026-09-18). Read it first; the plan argues from it.

**Depends on:** nothing. This plan lands first, before Plans 2, 3, 4 and 5. Merge order for the whole feature is `1 → (2, 3, 4 in parallel) → 5`.

**Files this plan owns exclusively** (no other plan in the set creates or edits them):

- `crates/phronesis-mcp/src/lifecycle/mod.rs`, `event.rs`, `state.rs`, `scrub.rs`, `record.rs`, `outcome.rs`
- `crates/phronesis-mcp/src/journey/journal.rs`, `crates/phronesis-mcp/src/journey/derive.rs`, `crates/phronesis-mcp/src/journey/tagger.rs`, `crates/phronesis-mcp/src/journey/mod.rs`
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

- No new crate dependencies. The `inflight` fallback key is hashed with an
  **inline FNV-1a 64-bit** implementation (Task 5), never
  `std::hash::DefaultHasher`, whose algorithm is explicitly unspecified across
  Rust releases; lock with `fs2::FileExt`.
- Prompt text never enters `JournalRecord`, a `phr::Fact`, a context render, or stdout. Only `LogEntry` under `kind: "lifecycle"`.
- Every lifecycle write is fail-open: swallow the error, `eprintln!("phronesis: ...")`, continue.
- Journal `tool` sentinel for lifecycle records is the exact string `__lifecycle`; `path` is `""`.
- Selector namespaces `lifecycle:` and `kalpa:` are exempt from tagger validation.
- Kalpa names match `^[a-z0-9][a-z0-9-]{0,63}$`.
- `inflight` entries older than 900 seconds are ignored **when classifying a
  prompt**, and dropped when a classification pass rewrites the file.
  `post-check` pops by key regardless of age, so a twenty-minute build still
  gets its commit detected.
- Conventional-commit messages. Run `cargo fmt` and `cargo clippy --all-targets -p phronesis-mcp -- -D warnings` before every commit.
- Machine note: if `cargo` fails with "You have not agreed to the Xcode license", stop and report; the human must run `sudo xcodebuild -license accept`.

## File structure

| path | responsibility |
|---|---|
| `crates/phronesis-mcp/src/journey/journal.rs` (modify) | `JournalRecord` v2 fields, `is_lifecycle()`, compaction retention for commit/kalpa records |
| `crates/phronesis-mcp/src/journey/derive.rs` (modify) | tool projection in `WindowContext`, selector exemption, read bound |
| `crates/phronesis-mcp/src/journey/tagger.rs` (modify) | `TaggerConfig.lifecycle` block (`prompt_text`) |
| `crates/phronesis-mcp/src/journey/mod.rs` (modify) | `ConfigError::ReservedTag`: a tagger tag in the `lifecycle:` / `kalpa:` namespaces is rejected at load |
| `crates/phronesis-mcp/src/lifecycle/mod.rs` (create) | module root, re-exports |
| `crates/phronesis-mcp/src/lifecycle/event.rs` (create) | `Kind`, `Mode`, `Host`, `LifecycleEvent`, `Stamped`, projections |
| `crates/phronesis-mcp/src/lifecycle/state.rs` (create) | `with_locked`, session/agents/inflight/turn/kalpa files, `classify_prompt` |
| `crates/phronesis-mcp/src/lifecycle/scrub.rs` (create) | `scrub_prompt` |
| `crates/phronesis-mcp/src/lifecycle/record.rs` (create) | `record()` — stamp, journal, log, state; `prompt_text_setting`, `correction_text` |
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

/// Spec §"Determinism and versioning": a downgraded binary reads a lifecycle
/// record as an odd `__lifecycle` tool record with no projection, which shifts
/// positional windows and adds `""` to `journey_distinct` on `path`. That is
/// the rollout hazard; pinning it here keeps it visible rather than
/// rediscovered.
#[test]
fn a_v1_reader_sees_a_lifecycle_record_as_a_tool_record() {
    /// The v1 shape, verbatim: no `kind`, no `mode`, no lifecycle fields.
    #[derive(serde::Deserialize)]
    struct V1Record {
        v: u32,
        tool: String,
        path: String,
        tags: Vec<String>,
    }
    let line = r#"{"v":2,"ts":10,"sid":"s-x","seq":7,"tool":"__lifecycle","path":"","tags":["lifecycle:prompt"],"kind":"prompt","mode":"fresh","host":"claude"}"#;
    let old: V1Record = serde_json::from_str(line).unwrap();
    assert_eq!(old.v, 2, "a v1 reader has no way to reject the record");
    assert_eq!(old.tool, "__lifecycle");
    assert_eq!(old.path, "", "which is what pollutes journey_distinct on path");
    assert_eq!(old.tags, vec!["lifecycle:prompt"]);
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
Expected: all pass, including the three new tests.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/journey/journal.rs crates/phronesis-mcp/tests/journey_journal.rs crates/phronesis-mcp/src/hook/journey_record.rs crates/phronesis-mcp/src/codex_hook.rs crates/phronesis-mcp/tests/journey_derive.rs
git commit -m "feat(journey): JournalRecord v2 lifecycle fields"
```

---

### Task 2: Tool projection in derive

**Files:**
- Modify: `crates/phronesis-mcp/src/journey/derive.rs:64-67` (WindowContext), `:415-467` (validate_selectors), `:490-539` (assert_facts), `:543-563` (record_in_window), `:653-700` (since_ge, distinct), and the emit loops that call `record_in_window`
- Modify: `crates/phronesis-mcp/src/journey/mod.rs` (`ConfigError::ReservedTag`, rejected in `load_config`)
- Test: `crates/phronesis-mcp/tests/journey_derive.rs`

**Interfaces:**
- Consumes: `JournalRecord::is_lifecycle()` from Task 1.
- Produces: no public signature change. `WindowContext` (private) gains `tool_records: &'a [JournalRecord]`. `journey::ConfigError` gains a `ReservedTag { path, tag }` variant.

- [ ] **Step 1: Write the failing tests**

Add a lifecycle record helper and tests to `tests/journey_derive.rs`.

**The file's existing helpers, verified against the current tree — use these exact
shapes; do not invent arities:**

```rust
fn derive_input<'a>(project_root: &'a Path, rules: &'a [Rule], config: &'a TaggerConfig, now_ts: u64) -> DeriveInput<'a>
//   `WindowScope::current_sid` is hard-coded to "s-now" inside this helper, so every
//   record a test wants an `s` window to see must carry sid "s-now".
fn make_record(timing: (u64, u64), identity: (&str, &[&str]), subject: Option<&str>) -> JournalRecord
//   timing is (seq, ts); tool "Edit", path "src/a.rs", ext "rs", v: 1.
fn make_record_with_path(timing: (u64, u64), identity: (&str, &[&str]), path: &str) -> JournalRecord
fn cfg(json: &str) -> TaggerConfig
//   TaggerConfig is { version, taggers: Vec<TaggerEntry { tag, when }>, modules }.
//   There is no `tags` map: a tag is defined by a `taggers[]` entry, e.g.
//   {"version":1,"taggers":[{"tag":"edits","when":[{"file_path_matches":"src/"}]}],"modules":[]}
fn rule_with_script(id: &str, scripts: Vec<&str>) -> Rule
fn journey_facts(net: &ReteNetwork, predicate: &str) -> Vec<Fact>
//   Networks are built with `ReteNetwork::new()` in this file.
```

`validate_selectors` only checks the selectors a *rule* references, never the tags on
a record, so lifecycle records may carry `lifecycle:*` tags that no config defines.

```rust
/// A lifecycle record, in the same `(seq, ts)` / `(sid, tags)` shape as the
/// file's existing helpers.
fn make_lifecycle(timing: (u64, u64), identity: (&str, &[&str]), kind: &str) -> JournalRecord {
    let (seq, ts) = timing;
    let (sid, tags) = identity;
    JournalRecord {
        v: journal::JOURNAL_V,
        ts,
        sid: sid.to_string(),
        seq,
        tool: journal::LIFECYCLE_TOOL.to_string(),
        path: String::new(),
        ext: None,
        module: None,
        tags: tags.iter().map(|s| s.to_string()).collect(),
        subject: None,
        command_exit: None,
        kind: Some(kind.to_string()),
        mode: None,
        host: Some("claude".to_string()),
        turn: None,
        agent: None,
        agent_type: None,
        kalpa: None,
    }
}

/// Goal 5 of the spec: interleaving lifecycle records changes no existing fact.
#[tokio::test]
async fn tool_projection_keeps_existing_facts_identical() {
    async fn facts_for(
        records: &[JournalRecord],
        rules: &[Rule],
        config: &TaggerConfig,
    ) -> Vec<String> {
        let dir = tempfile::tempdir().unwrap();
        for r in records {
            journal::append(dir.path(), r).unwrap();
        }
        let mut net = ReteNetwork::new();
        assert_facts(&mut net, derive_input(dir.path(), rules, config, 200))
            .await
            .unwrap();
        let mut all: Vec<String> = Vec::new();
        for p in [
            "journey_count",
            "journey_since_ge",
            "journey_filtered_since_ge",
            "journey_distinct",
        ] {
            for f in journey_facts(&net, p) {
                all.push(format!("{}:{}", f.predicate, f.args.join(",")));
            }
        }
        all.sort();
        all
    }

    let c = cfg(r#"{
        "version":1,
        "taggers":[
            {"tag":"edits","when":[{"file_path_matches":"src/"}]},
            {"tag":"tests","when":[{"file_path_matches":"tests/"}]}
        ],
        "modules":[]
    }"#);
    let rules = vec![rule_with_script(
        "r",
        vec![
            "facts_count('journey_count', ['edits','5c']) >= 0",
            "facts_count('journey_since_ge', ['tests', 1]) >= 0",
            "facts_count('journey_filtered_since_ge', ['tests','edits',1]) >= 0",
            "facts_count('journey_distinct', ['path','5c']) >= 0",
        ],
    )];

    let tools: Vec<JournalRecord> = (0..10u64)
        .map(|i| {
            if i == 4 {
                make_record_with_path((i, 100 + i), ("s-now", &["tests"]), "tests/x.rs")
            } else {
                make_record_with_path((i, 100 + i), ("s-now", &["edits"]), &format!("src/f{i}.rs"))
            }
        })
        .collect();
    let mut mixed = Vec::new();
    for (i, t) in tools.iter().enumerate() {
        let i = i as u64;
        mixed.push(t.clone());
        mixed.push(make_lifecycle(
            (100 + i, 100 + i),
            ("s-now", &["lifecycle:prompt", "lifecycle:prompt:fresh"]),
            "prompt",
        ));
    }

    let tools_only = facts_for(&tools, &rules, &c).await;
    // Golden values, so a symmetric off-by-one in both runs cannot pass: the
    // `5c` window is the last 5 tool records (indices 5..9), all `edits`, over
    // 5 distinct paths; the `tests` record at index 4 has 5 tool records after
    // it, all `edits`, so both ladders cap at k = 1.
    assert!(tools_only.contains(&"journey_count:edits,5c,5".to_string()), "{tools_only:?}");
    assert!(tools_only.contains(&"journey_distinct:path,5c,5".to_string()), "{tools_only:?}");
    assert!(tools_only.contains(&"journey_since_ge:tests,1".to_string()), "{tools_only:?}");
    assert!(
        tools_only.contains(&"journey_filtered_since_ge:tests,edits,1".to_string()),
        "{tools_only:?}"
    );
    assert_eq!(tools_only, facts_for(&mixed, &rules, &c).await);
}

#[tokio::test]
async fn lifecycle_selectors_validate_without_journey_config() {
    let c = TaggerConfig::default();
    let rules = vec![rule_with_script(
        "r",
        vec![
            "facts_count('journey_seen', ['lifecycle:interrupt','s']) >= 1",
            "facts_count('journey_count', ['kalpa:demo','s']) >= 1",
            // Spec: a `lifecycle:*` selector with an `Nc` window yields no facts,
            // because positional windows run over the tool projection.
            "facts_count('journey_occurrence', ['lifecycle:interrupt','5c']) >= 1",
        ],
    )];
    let dir = tempfile::tempdir().unwrap();
    journal::append(
        dir.path(),
        &make_lifecycle((1, 5), ("s-now", &["lifecycle:interrupt", "kalpa:demo"]), "interrupt"),
    )
    .unwrap();
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 10))
        .await
        .unwrap();
    assert_eq!(journey_facts(&net, "journey_seen").len(), 1);
    assert_eq!(journey_facts(&net, "journey_count")[0].args, vec!["kalpa:demo", "s", "1"]);
    assert!(journey_facts(&net, "journey_occurrence").is_empty());
}

/// Fail-closed is unchanged for everything outside the two built-in namespaces.
#[tokio::test]
async fn undefined_non_lifecycle_selector_still_fails_closed() {
    let c = cfg(r#"{
        "version":1,
        "taggers":[{"tag":"edits","when":[{"file_path_matches":"src/"}]}],
        "modules":[]
    }"#);
    let rules = vec![rule_with_script(
        "r",
        vec!["facts_count('journey_count', ['nonexistent','s']) >= 1"],
    )];
    let dir = tempfile::tempdir().unwrap();
    let mut net = ReteNetwork::new();
    let err = assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 10))
        .await
        .unwrap_err();
    assert!(matches!(err, derive::DeriveError::UndefinedSelector { .. }), "{err:?}");
}

#[tokio::test]
async fn since_ge_counts_tool_records_after_lifecycle_target() {
    let c = TaggerConfig::default();
    let rules = vec![rule_with_script(
        "r",
        vec!["facts_count('journey_since_ge', ['lifecycle:interrupt', 3]) >= 0"],
    )];
    let dir = tempfile::tempdir().unwrap();
    journal::append(
        dir.path(),
        &make_lifecycle((1, 1), ("s-now", &["lifecycle:interrupt"]), "interrupt"),
    )
    .unwrap();
    for i in 0..2u64 {
        journal::append(dir.path(), &rec!(2 + i, 2 + i, "s-now", &["edits"], None)).unwrap();
    }
    journal::append(
        dir.path(),
        &make_lifecycle((5, 5), ("s-now", &["lifecycle:stop"]), "stop"),
    )
    .unwrap();
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 10))
        .await
        .unwrap();
    // Distance is 2 (two *tool* records after the interrupt; the trailing stop
    // does not count), and `emit_since_ge` ladders k = 1..=min(max_k, distance)
    // = 1..=min(3, 2), so exactly "1" and "2" are emitted and "3" is not.
    let mut ks: Vec<String> = journey_facts(&net, "journey_since_ge")
        .iter()
        .map(|f| f.args[1].clone())
        .collect();
    ks.sort_by_key(|s| s.parse::<u32>().unwrap_or(u32::MAX));
    assert_eq!(ks, vec!["1", "2"]);
}

/// Spec §"The journal record, v2" / Read bound: "5 tool records followed by 200
/// lifecycle records under a `Calls(5)` rule (the iterative re-read must still
/// find all five)". A fixed multiple of `n` cannot do this; only doubling can.
#[tokio::test]
async fn calls_window_reads_iteratively_until_it_has_n_tool_records() {
    let c = cfg(r#"{
        "version":1,
        "taggers":[{"tag":"edits","when":[{"file_path_matches":"src/"}]}],
        "modules":[]
    }"#);
    let rules = vec![rule_with_script(
        "r",
        vec!["facts_count('journey_count', ['edits','5c']) >= 0"],
    )];
    let dir = tempfile::tempdir().unwrap();
    for i in 0..5u64 {
        journal::append(dir.path(), &rec!(i, i, "s-now", &["edits"], None)).unwrap();
    }
    for i in 5..205u64 {
        journal::append(
            dir.path(),
            &make_lifecycle((i, i), ("s-now", &["lifecycle:prompt"]), "prompt"),
        )
        .unwrap();
    }
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 1_000))
        .await
        .unwrap();
    // A single 5-line read would see only lifecycle records and count 0; a
    // 2n+64 read would reach line 74 and still count 0. Doubling reaches 256.
    assert_eq!(journey_facts(&net, "journey_count")[0].args[2], "5");
}

/// The over-read must stop as well as start: a file with fewer tool records
/// than the window asks for terminates rather than looping to the hard cap on
/// every hook.
#[tokio::test]
async fn calls_window_terminates_when_the_file_holds_fewer_tool_records() {
    let c = cfg(r#"{
        "version":1,
        "taggers":[{"tag":"edits","when":[{"file_path_matches":"src/"}]}],
        "modules":[]
    }"#);
    let rules = vec![rule_with_script(
        "r",
        vec!["facts_count('journey_count', ['edits','50c']) >= 0"],
    )];
    let dir = tempfile::tempdir().unwrap();
    for i in 0..2u64 {
        journal::append(dir.path(), &rec!(i, i, "s-now", &["edits"], None)).unwrap();
    }
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 100))
        .await
        .unwrap();
    assert_eq!(journey_facts(&net, "journey_count")[0].args[2], "2");
}

/// Spec §"The journal record, v2": "a `Seconds` window whose range contains
/// lifecycle records" — time windows enumerate **every** record, so a
/// lifecycle selector matches there.
#[tokio::test]
async fn seconds_window_enumerates_lifecycle_records() {
    let c = TaggerConfig::default();
    let rules = vec![rule_with_script(
        "r",
        vec!["facts_count('journey_count', ['lifecycle:interrupt','60s']) >= 1"],
    )];
    let dir = tempfile::tempdir().unwrap();
    journal::append(
        dir.path(),
        &make_lifecycle((1, 950), ("s-now", &["lifecycle:interrupt"]), "interrupt"),
    )
    .unwrap();
    // Outside the 60 s window: same selector, must not be counted.
    journal::append(
        dir.path(),
        &make_lifecycle((2, 100), ("s-now", &["lifecycle:interrupt"]), "interrupt"),
    )
    .unwrap();
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 1_000))
        .await
        .unwrap();
    assert_eq!(journey_facts(&net, "journey_count")[0].args[2], "1");
}

/// Spec §"The journal record, v2": "a `since_ge` whose last match sits behind
/// more lifecycle records than a single read would cover". `since_ge` already
/// reads the hard cap, so this pins that the branch selection did not regress
/// into the calls-only path.
#[tokio::test]
async fn since_ge_finds_a_target_behind_two_hundred_lifecycle_records() {
    let c = cfg(r#"{
        "version":1,
        "taggers":[{"tag":"edits","when":[{"file_path_matches":"src/"}]}],
        "modules":[]
    }"#);
    let rules = vec![rule_with_script(
        "r",
        vec!["facts_count('journey_since_ge', ['lifecycle:interrupt', 5]) >= 0"],
    )];
    let dir = tempfile::tempdir().unwrap();
    journal::append(
        dir.path(),
        &make_lifecycle((0, 0), ("s-now", &["lifecycle:interrupt"]), "interrupt"),
    )
    .unwrap();
    for i in 1..201u64 {
        journal::append(
            dir.path(),
            &make_lifecycle((i, i), ("s-now", &["lifecycle:prompt"]), "prompt"),
        )
        .unwrap();
    }
    for i in 201..204u64 {
        journal::append(dir.path(), &rec!(i, i, "s-now", &["edits"], None)).unwrap();
    }
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 1_000))
        .await
        .unwrap();
    let mut ks: Vec<String> = journey_facts(&net, "journey_since_ge")
        .iter()
        .map(|f| f.args[1].clone())
        .collect();
    ks.sort_by_key(|s| s.parse::<u32>().unwrap_or(u32::MAX));
    assert_eq!(ks, vec!["1", "2", "3"], "three tool records after the interrupt");
}

/// The exemption is a **closed set**: a typo must still fail closed, or a rule
/// that can never fire validates and sits silent forever.
#[tokio::test]
async fn a_misspelled_lifecycle_selector_still_fails_closed() {
    let c = TaggerConfig::default();
    for bad in [
        "lifecycle:prompt:corection",
        "lifecycle:subagent_started",
        "lifecycle:",
        "kalpa:",
    ] {
        let rules = vec![rule_with_script(
            "r",
            vec![&format!("facts_count('journey_count', ['{bad}','s']) >= 1")],
        )];
        let dir = tempfile::tempdir().unwrap();
        let mut net = ReteNetwork::new();
        let err = assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 10))
            .await
            .unwrap_err();
        assert!(
            matches!(err, derive::DeriveError::UndefinedSelector { .. }),
            "{bad}: {err:?}"
        );
    }
}

/// Spec §"The journal record, v2": "No tagger, no modules." A catch-all tagger
/// config must stamp nothing on a lifecycle record, so no user-defined tag can
/// ever land on one and no existing tag selector can match one.
#[tokio::test]
async fn a_catch_all_tagger_stamps_nothing_on_a_lifecycle_record() {
    let c = cfg(r#"{
        "version":1,
        "taggers":[{"tag":"everything","when":[]}],
        "modules":[]
    }"#);
    let rules = vec![rule_with_script(
        "r",
        vec!["facts_count('journey_count', ['everything','s']) >= 0"],
    )];
    let dir = tempfile::tempdir().unwrap();
    journal::append(
        dir.path(),
        &make_lifecycle((1, 5), ("s-now", &["lifecycle:prompt"]), "prompt"),
    )
    .unwrap();
    journal::append(dir.path(), &rec!(2, 6, "s-now", &["everything"], None)).unwrap();
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 10))
        .await
        .unwrap();
    // One tool record carries the tag; the lifecycle record does not, because
    // the tagger never runs for it (the record is written by `lifecycle::record`,
    // which stamps `tags()` and nothing else).
    assert_eq!(journey_facts(&net, "journey_count")[0].args[2], "1");
}

/// The `lifecycle:` and `kalpa:` namespaces are reserved: a tagger that claims
/// one is rejected at config load, not silently shadowed.
#[test]
fn a_tagger_tag_in_a_reserved_namespace_is_rejected_at_load() {
    for tag in ["lifecycle:prompt", "kalpa:demo"] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".phronesis")).unwrap();
        std::fs::write(
            dir.path().join(".phronesis/journey.json"),
            format!(
                r#"{{"version":1,"taggers":[{{"tag":"{tag}","when":[{{"file_path_matches":"src/"}}]}}],"modules":[]}}"#
            ),
        )
        .unwrap();
        let err = phronesis_mcp::journey::load_config(dir.path()).unwrap_err();
        assert!(
            matches!(err, phronesis_mcp::journey::ConfigError::ReservedTag { .. }),
            "{tag}: {err:?}"
        );
    }
}

/// Pairing a lifecycle selector with an `Nc` window yields no facts by
/// construction, so the rule can never fire. One stderr warning names the rule
/// rather than leaving it silent (spec §"The journal record, v2", tool
/// projection table).
#[tokio::test]
async fn a_lifecycle_selector_with_a_calls_window_warns_and_still_validates() {
    let c = TaggerConfig::default();
    let rules = vec![rule_with_script(
        "never-fires",
        vec!["facts_count('journey_count', ['lifecycle:interrupt','5c']) >= 1"],
    )];
    let scan = derive::scan_rules(&rules).expect("scan");
    let warnings = derive::lifecycle_window_warnings(&rules, &scan);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("never-fires"), "{warnings:?}");
    assert!(warnings[0].contains("lifecycle:interrupt"), "{warnings:?}");
    // It is a warning, not an error: the rule still validates.
    let dir = tempfile::tempdir().unwrap();
    let mut net = ReteNetwork::new();
    assert_facts(&mut net, derive_input(dir.path(), &rules, &c, 10))
        .await
        .unwrap();
}
```

`rec!` is the file's existing macro wrapping `make_record`; it takes
`(seq, ts, sid, tags, subject)`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --test journey_derive 2>&1 | tail -40`
Expected: compile error on `JOURNAL_V`/`LIFECYCLE_TOOL` if Task 1 has not landed; otherwise `tool_projection_keeps_existing_facts_identical` fails (the `5c` count differs), `lifecycle_selectors_validate_without_journey_config` fails with `UndefinedSelector`, `calls_window_reads_iteratively_until_it_has_n_tool_records` fails with a `0` count, `a_tagger_tag_in_a_reserved_namespace_is_rejected_at_load` fails to compile (`ReservedTag` not found), and `a_lifecycle_selector_with_a_calls_window_warns_and_still_validates` fails to compile (`lifecycle_window_warnings` not found). `undefined_non_lifecycle_selector_still_fails_closed` and `a_misspelled_lifecycle_selector_still_fails_closed` pass already — they are the regression pins for the loop this task rewrites.

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

/// The **closed** set of built-in lifecycle selectors, verbatim from
/// SPEC-agent-lifecycle-events §"Event model". Closed on purpose: a typo like
/// `lifecycle:prompt:corection` must still fail as `UndefinedSelector` rather
/// than validate and silently match nothing forever.
pub(crate) const LIFECYCLE_SELECTORS: [&str; 12] = [
    "lifecycle:subagent_start",
    "lifecycle:subagent_stop",
    "lifecycle:prompt",
    "lifecycle:prompt:fresh",
    "lifecycle:prompt:mid_turn",
    "lifecycle:prompt:correction",
    "lifecycle:intervention",
    "lifecycle:interrupt",
    "lifecycle:stop",
    "lifecycle:commit",
    "lifecycle:kalpa_start",
    "lifecycle:kalpa_end",
];

/// Built-in selectors that need no tagger definition: the closed set above plus
/// the two open-ended patterns `lifecycle:agent:<agent_type>` and
/// `kalpa:<name>`, both of which must carry a non-empty suffix.
fn is_builtin_selector(selector: &str) -> bool {
    LIFECYCLE_SELECTORS.contains(&selector)
        || selector
            .strip_prefix("lifecycle:agent:")
            .is_some_and(|rest| !rest.is_empty())
        || selector
            .strip_prefix("kalpa:")
            .is_some_and(|rest| !rest.is_empty())
}

/// One warning per rule that pairs a built-in lifecycle selector with a
/// positional (`Nc`) window. Positional windows run over the tool projection,
/// so such a pair yields no facts and the rule can never fire; the spec asks
/// that this say so instead of sitting silent. Returned as strings rather than
/// printed so a test can assert the text.
pub fn lifecycle_window_warnings(rules: &[Rule], scan: &RuleScan) -> Vec<String> {
    let mut out = Vec::new();
    let pairs = scan
        .occurrence_pairs
        .iter()
        .chain(scan.count_pairs.iter())
        .chain(scan.seen_pairs.iter());
    for (sel, win) in pairs {
        if !is_builtin_selector(sel) || !matches!(Window::parse(win), Ok(Window::Calls(_))) {
            continue;
        }
        let rule_id = rules
            .iter()
            .find(|r| rule_refs_selector(r, sel))
            .map(|r| r.id.clone())
            .unwrap_or_else(|| "<unknown>".to_string());
        out.push(format!(
            "phronesis: rule `{rule_id}` pairs lifecycle selector `{sel}` with call window \
             `{win}`; positional windows count tool calls only, so this condition can never \
             match — use an `s` or time window instead"
        ));
    }
    out.sort();
    out.dedup();
    out
}
```

`scan_rules` and `RuleScan` are already `pub` in `derive.rs`; `lifecycle_window_warnings` is `pub` for the same reason, so `tests/journey_derive.rs` can assert the text without capturing stderr.

In `validate_selectors`, emit the warnings just before the loop:

```rust
    for warning in lifecycle_window_warnings(rules, scan) {
        eprintln!("{warning}");
    }
```

and replace the whole `for selector in &referenced { … }` loop (`derive.rs:446-467`) with:

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

In `assert_facts`, replace the whole `let read_n = { … };` block, the
`read_recent` call, and the context construction. The calls-only branch becomes
an **iterative doubling read** rather than a fixed multiple, because no fixed
multiple preserves the window: a tail of 200 lifecycle records starves a
`Calls(5)` rule at any constant factor (spec §"The journal record, v2", Read
bound).

```rust
    let max_calls = scan.max_call_window();
    let max_seconds = scan.max_time_seconds();
    let needs_wide = scan.references_session()
        || !scan.since_max_k.is_empty()
        || !scan.filtered_since_max_k.is_empty()
        || max_seconds > 0;

    let records = if needs_wide {
        // Session floor / time window / distance-since-last all need an
        // open-ended look-back; the hard cap bounds the cost and the
        // per-record filter drops the rest. Unchanged from v1.
        journal::read_recent(input.project_root, journal::SUFFIX_HARD_CAP)?
    } else {
        read_for_call_window(input.project_root, (max_calls as usize).max(1))?
    };

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

and add the iterative reader beside `assert_facts`:

```rust
/// Read until `want` **tool** records are in hand. A `Calls(n)` window means
/// *n tool records*, and lifecycle records now share the file, so reading `n`
/// lines can return fewer than `n` tool records. Double the read until the
/// window is satisfied, the file start is reached, or `SUFFIX_HARD_CAP` binds.
/// Reading more lines never changes a fact: the `Nc` branch is positional and
/// the projection trims to the last `n` tool records.
///
/// The hard-cap stop is the one deviation the spec names and accepts: a tail so
/// lifecycle-dense that `SUFFIX_HARD_CAP` binds first returns fewer than `want`
/// tool records.
fn read_for_call_window(
    project_root: &Path,
    want: usize,
) -> Result<Vec<JournalRecord>, DeriveError> {
    let mut n = want.max(1);
    loop {
        let records = journal::read_recent(project_root, n)?;
        let tools = records.iter().filter(|r| !r.is_lifecycle()).count();
        // `records.len() < n` means the read reached the start of the file, so
        // no further doubling can find another record. Without that check a
        // short journal doubles all the way to the hard cap on every hook.
        if tools >= want || records.len() < n || n >= journal::SUFFIX_HARD_CAP {
            return Ok(records);
        }
        n = n.saturating_mul(2).min(journal::SUFFIX_HARD_CAP);
    }
}
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

`emit_distinct` takes the same change but always binds `let view = context.tool_records;` regardless of window — a lifecycle record's `path` is `""` and must never add a distinct path — and its `record_in_window(rec, win, i, context)` call becomes `record_in_window(rec, win, i, view.len(), &context.scope)` like the other three.

`record_in_window`'s signature change also breaks `derive.rs`'s own two unit tests, `record_in_window_session` (`derive.rs:770`) and `record_in_window_calls` (`derive.rs:804`). Both build a `WindowContext { records, scope }` literal and call `record_in_window(&rec, tok, i, &context)`. Fix both in this task: add `tool_records: records` (they contain no lifecycle records, so the two views are the same slice) to each literal, and change every call to pass `records.len()` and `&context.scope`. The `JournalRecord` literals in those two tests also need Task 1's seven `None` fields, which Task 1 already added.

`emit_since_ge` keeps searching `records` for the last match but computes distance as the number of tool records after it, replacing `distance = Some((records.len() - 1 - i) as u32);`:

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

In `journey/mod.rs`, reserve the two namespaces at config load. `ConfigError`
gains a variant, and `load_config` checks the parsed config before returning it:

```rust
    #[error("journey.json at {path}: tagger tag `{tag}` is in a reserved namespace \
             (`lifecycle:` and `kalpa:` are written by the lifecycle module)")]
    ReservedTag { path: String, tag: String },
```

```rust
    let cfg = serde_json::from_str::<tagger::TaggerConfig>(&raw).map_err(|e| {
        ConfigError::Malformed {
            path: path.display().to_string(),
            source: e,
        }
    })?;
    // The built-in namespaces are exempt from selector validation, so a tagger
    // that claimed one would stamp a tag no rule could distinguish from a
    // lifecycle record's own. Reject it where it is written, not where it is
    // read (spec §"The journal record, v2").
    if let Some(entry) = cfg
        .taggers
        .iter()
        .find(|t| t.tag.starts_with("lifecycle:") || t.tag.starts_with("kalpa:"))
    {
        return Err(ConfigError::ReservedTag {
            path: path.display().to_string(),
            tag: entry.tag.clone(),
        });
    }
    Ok(cfg)
```

Every existing caller already handles `ConfigError` by falling back to
`TaggerConfig::default()`, so a project with a reserved tag keeps working with
no taggers rather than failing a hook — and says why on stderr.

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --test journey_derive 2>&1 | tail -30`
Expected: all pass, including `determinism_contract` and every pre-existing test.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/journey/derive.rs crates/phronesis-mcp/src/journey/mod.rs crates/phronesis-mcp/tests/journey_derive.rs
git commit -m "feat(journey): tool projection so lifecycle records leave existing facts unchanged"
```

---

### Task 3: Compaction retention for commit, interrupt, correction and kalpa records

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
    journal::append(
        dir.path(),
        &lifecycle_record(2, 2, "prompt", &["lifecycle:prompt", "lifecycle:prompt:fresh"]),
    )
    .unwrap();
    // The friction record is the point of the feature: an interrupt and the
    // correction that follows it must survive compaction, or a rule like "two
    // corrections this session" stops firing because the journal compacted.
    journal::append(
        dir.path(),
        &lifecycle_record(5, 5, "interrupt", &["lifecycle:interrupt"]),
    )
    .unwrap();
    journal::append(
        dir.path(),
        &lifecycle_record(
            6,
            6,
            "prompt",
            &["lifecycle:prompt", "lifecycle:prompt:correction", "lifecycle:intervention"],
        ),
    )
    .unwrap();
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
    assert!(kinds.contains(&"interrupt"), "{kinds:?}");
    // Exactly one prompt survives: the correction, not the fresh one.
    let prompt_tags: Vec<&Vec<String>> = all
        .iter()
        .filter(|r| r.kind.as_deref() == Some("prompt"))
        .map(|r| &r.tags)
        .collect();
    assert_eq!(prompt_tags.len(), 1, "{prompt_tags:?}");
    assert!(
        prompt_tags[0].iter().any(|t| t == "lifecycle:prompt:correction"),
        "a fresh prompt compacts away, a correction does not: {prompt_tags:?}"
    );
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
/// success signal, the friction pair, and the kalpa boundaries. Commits and
/// kalpa boundaries are the denominators of every per-kalpa report; the
/// interrupt/correction pair is the friction record the feature exists for,
/// and a rule like "two corrections this session" must not stop firing because
/// the journal compacted (spec §"The journal record, v2", Compaction).
///
/// The retained set is bounded by human turns and commits, not by tool calls,
/// so the growth it adds is an order of magnitude below the tail it lives
/// beside; no further cap ships in v1.
const RETAINED_LIFECYCLE_TAGS: [&str; 5] = [
    "lifecycle:commit",
    "lifecycle:interrupt",
    "lifecycle:prompt:correction",
    "lifecycle:kalpa_start",
    "lifecycle:kalpa_end",
];

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
git commit -m "feat(journey): retain commit, interrupt, correction and kalpa records through compaction"
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
    /// Flat extra log fields, from the CLOSED vocabulary `EXTRA_KEYS`.
    pub extra: serde_json::Map<String, serde_json::Value>,
}
/// The closed `extra` vocabulary (spec §"Host adapters / Shared module"). No
/// key outside it may be added, and none of them ever carries message or
/// transcript content.
pub const EXTRA_KEYS: [&str; 9] = ["inferred_from", "stop_hook_active", "matched_start",
    "duration_secs", "sha", "head_before", "confidence_band", "tool_use_id", "detection"];
/// Lowercase, then keep only if it matches `[a-z0-9][a-z0-9_.:-]{0,63}` — wider
/// than the kalpa pattern so snake_case and colon-qualified agent names survive. `None` otherwise.
pub fn sanitize_agent_type(raw: &str) -> Option<String>;
impl LifecycleEvent {
    pub fn new(kind: Kind, host: Host) -> Self;
    pub fn with_mode(self, m: Mode) -> Self;
    pub fn with_session(self, id: impl Into<String>) -> Self;
    pub fn with_turn(self, id: impl Into<String>) -> Self;
    pub fn with_agent(self, id: impl Into<String>, agent_type: Option<String>) -> Self;  // sanitizes agent_type
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
/// `agent_type` is model-supplied free text on Gemini (`tool_input.agent_name`)
/// and reaches a journal tag, hence a RETE fact. It gets the kalpa name's
/// treatment: lowercase, then keep only `[a-z0-9][a-z0-9_.:-]{0,63}`. Anything
/// else is stored as absent and the tag is dropped entirely.
#[test]
fn agent_type_is_sanitized_before_it_becomes_a_tag() {
    assert_eq!(sanitize_agent_type("Explore").as_deref(), Some("explore"));
    assert_eq!(sanitize_agent_type("code-reviewer-2").as_deref(), Some("code-reviewer-2"));
    assert_eq!(sanitize_agent_type("codebase_investigator").as_deref(), Some("codebase_investigator"), "Gemini snake_case survives");
    assert_eq!(sanitize_agent_type("code-simplifier:code-simplifier").as_deref(), Some("code-simplifier:code-simplifier"), "Claude plugin agents survive");
    assert_eq!(sanitize_agent_type("../../etc/passwd"), None);
    assert_eq!(sanitize_agent_type("kalpa:evil").as_deref(), Some("kalpa:evil"), "colons are allowed; the tag is lifecycle:agent:kalpa:evil, which no kalpa:* selector matches");
    assert_eq!(sanitize_agent_type("spaces here"), None);
    assert_eq!(sanitize_agent_type(""), None);
    assert_eq!(sanitize_agent_type(&"a".repeat(65)), None);
    assert_eq!(sanitize_agent_type("-lead"), None);

    let e = LifecycleEvent::new(Kind::SubagentStart, Host::Claude).with_agent("a1", Some("Explore".into()));
    assert_eq!(e.agent_type.as_deref(), Some("explore"), "stored sanitized, not just tagged");
    assert_eq!(e.tags(None), vec!["lifecycle:subagent_start", "lifecycle:agent:explore"]);

    let hostile = LifecycleEvent::new(Kind::SubagentStart, Host::Gemini)
        .with_agent("a2", Some("hack me; rm -rf /".into()));
    assert_eq!(hostile.agent_type, None);
    assert_eq!(hostile.tags(None), vec!["lifecycle:subagent_start"]);
}
/// Spec §"Event model", Intervention: a prompt delivered *inside* a sub-agent
/// is never an intervention, because the human did not speak.
#[test]
fn a_sub_agent_prompt_is_never_tagged_as_an_intervention() {
    let inside = LifecycleEvent::new(Kind::Prompt, Host::Claude)
        .with_mode(Mode::Correction)
        .with_agent("sub-1", None);
    assert_eq!(
        inside.tags(None),
        vec!["lifecycle:prompt", "lifecycle:prompt:correction"],
        "no lifecycle:intervention on a sub-agent's prompt"
    );
    // The same mode at top level is an intervention.
    let top = LifecycleEvent::new(Kind::Prompt, Host::Claude).with_mode(Mode::Correction);
    assert!(top.tags(None).iter().any(|t| t == "lifecycle:intervention"));
}
/// The `extra` map is a closed vocabulary; nothing outside it, and never
/// message or transcript content.
#[test]
fn extra_keys_are_a_closed_vocabulary() {
    for k in EXTRA_KEYS {
        let e = LifecycleEvent::new(Kind::Stop, Host::Claude).with_extra(k, "x");
        assert!(e.extra.contains_key(k));
    }
    for forbidden in ["prompt_response", "last_assistant_message", "transcript_path", "agent_transcript_path"] {
        assert!(!EXTRA_KEYS.contains(&forbidden), "{forbidden}");
    }
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

/// The closed vocabulary of `extra` keys (spec §"Host adapters / Shared
/// module"). `extra` never carries message or transcript content:
/// `last_assistant_message`, `prompt_response`, `transcript_path` and
/// `agent_transcript_path` are read for decisions and dropped at the adapter
/// boundary, and `tests/hook_integration.rs` asserts no log entry contains them.
pub const EXTRA_KEYS: [&str; 9] = [
    "inferred_from",
    "stop_hook_active",
    "matched_start",
    "duration_secs",
    "sha",
    "head_before",
    "confidence_band",
    "tool_use_id",
    // Why commit detection was skipped for a shell call: "timeout" or
    // "no_exit_code" (spec §"Success signal: commit").
    "detection",
];

/// Lowercase, then keep only if the result matches `[a-z0-9][a-z0-9_.:-]{0,63}` —
/// the same treatment the kalpa name gets, and for the same reason: on Gemini
/// `agent_type` is `tool_input.agent_name`, model-generated free text that
/// reaches a journal tag (`lifecycle:agent:<agent_type>`, hence a RETE fact),
/// the action log, and the `agents` file. Anything else is stored as absent and
/// the `lifecycle:agent:*` tag is dropped (spec §"Correlation state").
pub fn sanitize_agent_type(raw: &str) -> Option<String> {
    let lowered = raw.to_ascii_lowercase();
    let b = lowered.as_bytes();
    if !(1..=64).contains(&b.len()) {
        return None;
    }
    if !(b[0].is_ascii_lowercase() || b[0].is_ascii_digit()) {
        return None;
    }
    // Wider than the kalpa pattern on purpose: Gemini built-ins are snake_case
    // and Claude plugin agents are colon-qualified; both must survive as tags.
    b.iter()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(*c, b'-' | b'_' | b'.' | b':'))
        .then_some(lowered)
}

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
    /// Sanitizes `agent_type` here, once, so no adapter can forget: on Gemini
    /// it is `tool_input.agent_name`, model-generated free text that reaches a
    /// journal tag and therefore a RETE fact.
    pub fn with_agent(mut self, id: impl Into<String>, agent_type: Option<String>) -> Self {
        self.agent_id = Some(id.into());
        self.agent_type = agent_type.as_deref().and_then(sanitize_agent_type);
        self
    }
    pub fn with_prompt(mut self, scrubbed: impl Into<String>) -> Self { self.prompt = Some(scrubbed.into()); self }
    /// `key` must come from `EXTRA_KEYS`. The debug assertion is the guard: a
    /// typo or a new field lands in the vocabulary deliberately, in this file,
    /// rather than appearing in the action log by accident.
    pub fn with_extra(mut self, key: &str, v: impl Into<Value>) -> Self {
        debug_assert!(EXTRA_KEYS.contains(&key), "extra key `{key}` is outside the closed vocabulary");
        self.extra.insert(key.to_string(), v.into());
        self
    }

    pub fn tags(&self, kalpa: Option<&str>) -> Vec<String> {
        let mut t = vec![self.kind.tag()];
        if let Some(m) = self.mode {
            t.push(format!("lifecycle:prompt:{}", m.as_str()));
            // The autonomy signal: the human changed the plan (steered mid-turn
            // or corrected after an interrupt), as opposed to replying. Only at
            // top level — a prompt carrying an `agent_id` was delivered inside a
            // sub-agent, so the human did not speak and it is not an
            // intervention (spec §"Event model", Intervention).
            if matches!(m, Mode::MidTurn | Mode::Correction) && self.agent_id.is_none() {
                t.push("lifecycle:intervention".to_string());
            }
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
Expected: 9 passed.

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

pub fn is_session_begin(source: Option<&str>) -> bool;  // startup|resume|clear (or absent) = begin; compact|fork = not
pub fn set_session(root: &Path, sid: &str);       // ATOMIC replace of .phronesis/journey/session (temp file + rename)
pub fn reset_for_session_start(root: &Path);      // truncate agents + inflight; turn := closed for the current sid
// There is deliberately no `clear_session`: SessionEnd does not truncate the
// session file. Truncation would let any stray hook between sessions mint a
// throwaway sid; the next session-begin SessionStart overwrites instead
// (spec §Correlation state).

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)] pub struct OpenAgent { pub agent_id: String, pub agent_type: Option<String>, pub ts: u64, pub seq: u64 }
pub fn push_agent(root: &Path, a: OpenAgent);
pub fn pop_agent(root: &Path, agent_id: Option<&str>) -> Option<OpenAgent>;  // by id, else LIFO

// A keyed MULTISET: push appends a line, pop removes the LAST line with a
// matching key, so two concurrent calls sharing a key push two lines and pop
// two lines instead of clobbering each other.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)] pub struct Inflight { pub key: String, pub tool: String, pub ts: u64, pub agent_id: Option<String>, pub head_before: Option<String>, pub detection: Option<String> }
pub fn push_inflight(root: &Path, e: Inflight);
pub fn pop_inflight(root: &Path, key: &str) -> Option<Inflight>;  // by key, LAST match, REGARDLESS OF AGE
pub fn live_inflight(root: &Path, now: u64, agent_scope: Option<&str>) -> Vec<Inflight>; // classification view: drops expired, returns visible live entries
pub fn take_inflight_for_scope(root: &Path, now: u64, agent_scope: Option<&str>) -> Vec<Inflight>; // removes + returns visible live entries
pub fn clear_inflight(root: &Path);   // every interrupt path drops the session's entries
pub fn inflight_key_for(tool_use_id: Option<&str>, tool_name: &str, tool_input: &serde_json::Value) -> String;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)] pub struct Turn { pub sid: String, pub open: bool, pub turn_id: Option<String>, pub last_prompt_ts: u64, pub last_event: String }
pub fn read_turn(root: &Path) -> Turn;  // a foreign `sid` reads as absent (Turn::default())
pub fn open_turn(root: &Path, turn_id: Option<&str>, ts: u64);   // stamps the current sid
pub fn close_turn(root: &Path, last_event: &str) -> bool;        // false when the write failed: the caller degrades to "closed"

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)] pub struct Kalpa { pub name: String, pub started_ts: u64 }
pub fn read_kalpa(root: &Path) -> Option<Kalpa>;
pub fn write_kalpa(root: &Path, k: &Kalpa);
pub fn clear_kalpa(root: &Path);
pub fn valid_kalpa_name(name: &str) -> bool;
```

Visibility rule for `agent_scope`: an entry is visible when `entry.agent_id.is_none()` or `entry.agent_id == agent_scope`.

**Two TTL rules, not one.** `INFLIGHT_TTL_SECS` applies to *classification only*
(`live_inflight`, `take_inflight_for_scope`). `pop_inflight` pops by key
regardless of age, so a twenty-minute build still gets its commit detected
(spec §Correlation state). Getting this backwards silently disables commit
detection for every slow command, which is why the two functions are separate.

**`close_turn` returns a bool.** Spec §Correlation state: "If the write that
would close the turn fails, the classifier treats the turn as **closed**." That
is what the file already says on a failed write — the stale contents still read
`open: true`, so the classifier would infer an intervention that never
happened. `close_turn` therefore reports its failure, and `read_turn` treats an
unparseable or foreign-`sid` file as absent, i.e. closed.

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

fn inflight(key: &str, ts: u64, agent: Option<&str>) -> Inflight {
    Inflight {
        key: key.into(),
        tool: "Bash".into(),
        ts,
        agent_id: agent.map(str::to_string),
        head_before: None,
        detection: None,
    }
}

#[test]
fn inflight_ttl_applies_to_classification_only() {
    let d = root();
    push_inflight(d.path(), inflight("old", 100, None));
    let mut parent = inflight("parent", 1000, None);
    parent.head_before = Some("abc".into());
    push_inflight(d.path(), parent);
    push_inflight(d.path(), inflight("child", 1000, Some("sub1")));
    let now = 100 + INFLIGHT_TTL_SECS + 1;

    let seen_parent = live_inflight(d.path(), now, None);
    assert_eq!(seen_parent.iter().map(|e| e.key.as_str()).collect::<Vec<_>>(), vec!["parent"]);
    let seen_child = live_inflight(d.path(), now, Some("sub1"));
    assert_eq!(seen_child.iter().map(|e| e.key.as_str()).collect::<Vec<_>>(), vec!["parent", "child"]);
    // `live_inflight` is a classification pass, so it rewrote the file without
    // the expired entry.
    assert!(pop_inflight(d.path(), "old").is_none());
    assert_eq!(pop_inflight(d.path(), "parent").unwrap().head_before.as_deref(), Some("abc"));
    let taken = take_inflight_for_scope(d.path(), now, Some("sub1"));
    assert_eq!(taken.len(), 1);
    assert!(live_inflight(d.path(), now, Some("sub1")).is_empty());
}

/// The TTL must not reach `pop_inflight`, or every command that runs longer
/// than 15 minutes silently loses its commit detection — the exact case the
/// feature exists to catch (a long `git rebase`, a slow release script).
#[test]
fn pop_inflight_ignores_the_ttl() {
    let d = root();
    let mut e = inflight("slow-build", 0, None);
    e.head_before = Some("deadbeef".into());
    push_inflight(d.path(), e);
    let popped = pop_inflight(d.path(), "slow-build").expect("a twenty-minute build still pops");
    assert_eq!(popped.head_before.as_deref(), Some("deadbeef"));
}

/// A keyed multiset: two concurrent calls sharing a key push two lines and pop
/// two lines. With a clobbering push the second pre-check would erase the
/// first's `head_before` and the first post-check would find nothing.
#[test]
fn inflight_is_a_multiset_keyed_by_key() {
    let d = root();
    let mut first = inflight("same", 10, None);
    first.head_before = Some("aaa".into());
    let mut second = inflight("same", 20, None);
    second.head_before = Some("bbb".into());
    push_inflight(d.path(), first);
    push_inflight(d.path(), second);
    // Pop removes the LAST matching line.
    assert_eq!(pop_inflight(d.path(), "same").unwrap().head_before.as_deref(), Some("bbb"));
    assert_eq!(pop_inflight(d.path(), "same").unwrap().head_before.as_deref(), Some("aaa"));
    assert!(pop_inflight(d.path(), "same").is_none());
}

#[test]
fn clear_inflight_drops_every_entry() {
    let d = root();
    push_inflight(d.path(), inflight("a", 10, None));
    push_inflight(d.path(), inflight("b", 10, Some("sub")));
    clear_inflight(d.path());
    assert!(pop_inflight(d.path(), "a").is_none());
    assert!(pop_inflight(d.path(), "b").is_none());
}

#[test]
fn inflight_key_prefers_tool_use_id_then_hashes_input() {
    let input = serde_json::json!({"b": 1, "a": [1, 2]});
    assert_eq!(inflight_key_for(Some("tu-1"), "Bash", &input), "tu-1");
    let k1 = inflight_key_for(None, "run_shell_command", &input);
    let k2 = inflight_key_for(None, "run_shell_command", &serde_json::json!({"a": [1, 2], "b": 1}));
    assert_eq!(k1, k2, "key order must not change the key");
    assert_ne!(k1, inflight_key_for(None, "replace", &input));
}

/// The hash is PINNED, not `std::hash::DefaultHasher` whose algorithm is
/// explicitly unspecified across Rust releases. A pre/post pair split across a
/// rebuild must still match, so this golden value is part of the on-disk
/// contract: if it changes, in-flight entries from the previous binary leak.
///
/// Computed by hand from the FNV-1a 64-bit definition over the bytes
/// `run_shell_command` followed by `{"command":"ls"}`.
#[test]
fn inflight_key_hash_is_pinned_fnv1a() {
    fn fnv1a(bytes: &[u8]) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }
    let expected = {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for chunk in [b"run_shell_command".as_slice(), br#"{"command":"ls"}"#.as_slice()] {
            for b in chunk {
                h ^= u64::from(*b);
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        h
    };
    assert_eq!(expected, fnv1a(br#"run_shell_command{"command":"ls"}"#));
    assert_eq!(
        inflight_key_for(None, "run_shell_command", &serde_json::json!({"command": "ls"})),
        format!("h{expected:016x}")
    );
}

#[test]
fn turn_transitions_and_session_reset() {
    let d = root();
    set_session(d.path(), "s-a");
    assert!(!read_turn(d.path()).open);
    open_turn(d.path(), Some("t1"), 50);
    let t = read_turn(d.path());
    assert!(t.open);
    assert_eq!(t.sid, "s-a", "the turn is stamped with the session that opened it");
    assert_eq!(t.turn_id.as_deref(), Some("t1"));
    assert_eq!(t.last_prompt_ts, 50);
    assert_eq!(t.last_event, "prompt");
    assert!(close_turn(d.path(), "interrupt"));
    assert!(!read_turn(d.path()).open);
    assert_eq!(read_turn(d.path()).last_event, "interrupt");

    push_agent(d.path(), OpenAgent { agent_id: "a".into(), agent_type: None, ts: 1, seq: 1 });
    push_inflight(d.path(), inflight("k", 1, None));
    open_turn(d.path(), None, 60);
    reset_for_session_start(d.path());
    assert!(pop_agent(d.path(), None).is_none());
    assert!(live_inflight(d.path(), 2, None).is_empty());
    assert!(!read_turn(d.path()).open);
}

/// A turn left open by a crashed session must not leak into the next one, and a
/// corrupt file must not either. Both read as absent, i.e. closed, i.e. the
/// next prompt is `fresh` — the conservative answer (spec §Correlation state).
#[test]
fn a_foreign_or_corrupt_turn_file_reads_as_absent() {
    let d = root();
    set_session(d.path(), "s-old");
    open_turn(d.path(), Some("t1"), 50);
    assert!(read_turn(d.path()).open);

    set_session(d.path(), "s-new");
    let t = read_turn(d.path());
    assert!(!t.open, "another session's open turn is not ours");
    assert_eq!(t.last_event, "", "nor is its last_event");

    std::fs::write(d.path().join(".phronesis/journey/turn"), "{not json").unwrap();
    assert!(!read_turn(d.path()).open);
}

/// Spec §Correlation state: the `session` write is an atomic replace, so a
/// lock-free `current_sid` reader never observes an empty file and never mints
/// a phantom sid. A truncate-then-write would expose exactly that window.
#[test]
fn session_write_is_an_atomic_replace_and_is_never_truncated() {
    let d = root();
    set_session(d.path(), "host-sid-1");
    assert_eq!(phronesis_mcp::journey::current_sid(d.path()), "host-sid-1");
    set_session(d.path(), "host-sid-2");
    assert_eq!(phronesis_mcp::journey::current_sid(d.path()), "host-sid-2");

    // A concurrent reader never sees an empty file: 200 overwrites while a
    // reader spins. `current_sid` mints a fresh `s-…` id when the file is empty,
    // so a phantom sid is observable as a value that is neither of the two.
    let path = d.path().to_path_buf();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reader_stop = stop.clone();
    let reader = std::thread::spawn(move || {
        let mut seen: Vec<String> = Vec::new();
        while !reader_stop.load(std::sync::atomic::Ordering::Relaxed) {
            let sid = phronesis_mcp::journey::current_sid(&path);
            if !seen.contains(&sid) {
                seen.push(sid);
            }
        }
        seen
    });
    for i in 0..200 {
        set_session(d.path(), if i % 2 == 0 { "host-sid-1" } else { "host-sid-2" });
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let seen = reader.join().unwrap();
    assert!(
        seen.iter().all(|s| s == "host-sid-1" || s == "host-sid-2"),
        "a reader observed a phantom sid: {seen:?}"
    );

    // And there is no way to truncate it: SessionEnd must not.
    reset_for_session_start(d.path());
    assert_eq!(
        std::fs::read_to_string(d.path().join(".phronesis/journey/session")).unwrap().trim(),
        "host-sid-2",
        "reset_for_session_start truncates agents/inflight/turn, never session"
    );
}

#[test]
fn session_begin_sources_are_startup_resume_and_clear() {
    for begin in ["startup", "resume", "clear"] {
        assert!(is_session_begin(Some(begin)), "{begin}");
    }
    for keep in ["compact", "fork"] {
        assert!(!is_session_begin(Some(keep)), "{keep}");
    }
    // A host that sends no source means a new session; that is what every host
    // predating the field meant.
    assert!(is_session_begin(None));
    assert!(is_session_begin(Some("")));
    // An unknown source is treated as a begin, the same conservative default.
    assert!(is_session_begin(Some("something-new")));
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

/// "The pattern is a property of the value, not of the CLI": a hand-edited or
/// stale file whose name fails validation reads as absent, so it can never
/// reach a journal tag, a log field, or the model-visible context header.
#[test]
fn a_kalpa_file_with_an_invalid_name_reads_as_absent() {
    let d = root();
    std::fs::create_dir_all(d.path().join(".phronesis/journey")).unwrap();
    for bad in [r#"{"name":"Bad Name","started_ts":1}"#, r#"{"name":"../../etc","started_ts":1}"#] {
        std::fs::write(d.path().join(".phronesis/journey/kalpa"), bad).unwrap();
        assert!(read_kalpa(d.path()).is_none(), "{bad}");
    }
}

/// Every state file corrupted still exits 0 and reads as absent: a torn write
/// or a hand edit must degrade to "unknown", never to a failed hook.
#[test]
fn corrupt_state_files_degrade_to_absent() {
    let d = root();
    std::fs::create_dir_all(d.path().join(".phronesis/journey")).unwrap();
    for (name, body) in [
        ("agents", "{not json
{"agent_id":"ok","agent_type":null,"ts":1,"seq":1}
{"trailing"),
        ("inflight", "garbage
{"key":"k","tool":"Bash","ts":1,"agent_id":null,"head_before":null,"detection":null}
{"partial"),
        ("turn", "}{"),
        ("kalpa", "["),
    ] {
        std::fs::write(d.path().join(".phronesis/journey").join(name), body).unwrap();
    }
    // Well-formed lines survive; malformed ones are skipped.
    assert_eq!(pop_agent(d.path(), None).unwrap().agent_id, "ok");
    assert_eq!(pop_inflight(d.path(), "k").unwrap().tool, "Bash");
    assert!(!read_turn(d.path()).open);
    assert!(read_kalpa(d.path()).is_none());
}

/// A read-only `.phronesis/journey` must not fail a hook: every writer swallows
/// its error. Skipped when running as root, which ignores the mode bits.
#[test]
fn a_read_only_state_directory_never_panics() {
    let d = root();
    let dir = d.path().join(".phronesis/journey");
    std::fs::create_dir_all(&dir).unwrap();
    let mut perms = std::fs::metadata(&dir).unwrap().permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o500);
        std::fs::set_permissions(&dir, perms.clone()).unwrap();
        if std::fs::write(dir.join("probe"), "x").is_ok() {
            // Running as root: the mode bits do not apply, so there is nothing
            // to test here.
            let _ = std::fs::remove_file(dir.join("probe"));
            return;
        }
        push_agent(d.path(), OpenAgent { agent_id: "a".into(), agent_type: None, ts: 1, seq: 1 });
        push_inflight(d.path(), inflight("k", 1, None));
        open_turn(d.path(), None, 1);
        close_turn(d.path(), "stop");
        set_session(d.path(), "s-x");
        write_kalpa(d.path(), &Kalpa { name: "demo".into(), started_ts: 1 });
        assert!(pop_agent(d.path(), None).is_none());
        perms.set_mode(0o700);
        std::fs::set_permissions(&dir, perms).unwrap();
    }
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

use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

/// How long an `inflight` entry is evidence of an interrupt. Applies to
/// **classification only**: `pop_inflight` pops by key regardless of age, so a
/// twenty-minute build still gets its commit detected.
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
/// Which `SessionStart` sources begin a *new* session. `compact` and `fork`
/// continue the current one, so they must leave `session`, `agents`,
/// `inflight` and `turn` alone: a mid-session compaction that orphaned an open
/// sub-agent or discarded an in-flight tool would be worse than no hook at all
/// (spec §Correlation state).
///
/// An absent or unrecognized source is a begin: that is what every host that
/// omits the field means, and resetting is the state the classifier already
/// copes with.
pub fn is_session_begin(source: Option<&str>) -> bool {
    !matches!(source.unwrap_or_default(), "compact" | "fork")
}

/// Overwrite the session id with an **atomic replace** — write a sibling temp
/// file, then rename over the target. `current_sid` reads this file without
/// taking the lock (`journey/mod.rs:42-48`), so a truncate-then-write would
/// give it a window in which the file is empty and it mints a phantom sid that
/// every record written in that millisecond then carries.
///
/// The lock is still held, against other writers; the rename is what protects
/// the lock-free reader.
pub fn set_session(root: &Path, sid: &str) {
    let d = dir(root);
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(&d)?;
        let lock = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(d.join("session.lock"))?;
        lock.lock_exclusive()?;
        // Same directory, so the rename is atomic and cannot cross a mount.
        let tmp = d.join(format!("session.{}.tmp", std::process::id()));
        std::fs::write(&tmp, sid.as_bytes())?;
        let result = std::fs::rename(&tmp, d.join("session"));
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        let _ = FileExt::unlock(&lock);
        result
    };
    swallow(write(), "set_session");
}

/// Truncate `agents` and `inflight` and reset `turn` to closed, for the current
/// session. Called only on a **session-begin** `SessionStart`
/// (`is_session_begin`). `session` itself is not touched here: the caller
/// overwrote it first, and nothing ever truncates it.
pub fn reset_for_session_start(root: &Path) {
    swallow(with_locked(root, "agents", |_| (Some(String::new()), ())), "reset agents");
    swallow(with_locked(root, "inflight", |_| (Some(String::new()), ())), "reset inflight");
    let fresh = Turn {
        sid: crate::journey::current_sid(root),
        open: false,
        turn_id: None,
        last_prompt_ts: 0,
        last_event: String::new(),
    };
    swallow(
        with_locked(root, "turn", |_| (serde_json::to_string(&fresh).ok(), ())),
        "reset turn",
    );
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
/// One in-flight tool call. A **multiset** keyed by `key`: two concurrent calls
/// with the same key are two lines, not one (spec §Correlation state).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Inflight {
    pub key: String,
    pub tool: String,
    pub ts: u64,
    pub agent_id: Option<String>,
    pub head_before: Option<String>,
    /// Why commit detection is disabled for this call, when it is:
    /// `"timeout"` (the pre-time `git rev-parse` timed out) or
    /// `"no_exit_code"` (the host sent no `command_exit`). Copied onto the
    /// record so the miss is auditable rather than silent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detection: Option<String>,
}

fn visible(e: &Inflight, scope: Option<&str>) -> bool {
    e.agent_id.is_none() || e.agent_id.as_deref() == scope
}
fn live(e: &Inflight, now: u64) -> bool { now.saturating_sub(e.ts) <= INFLIGHT_TTL_SECS }

/// Append. Never de-duplicates by key: two concurrent calls sharing a key must
/// push two lines, or the second pre-check erases the first's `head_before` and
/// the first post-check finds nothing.
pub fn push_inflight(root: &Path, e: Inflight) {
    swallow(with_locked(root, "inflight", |cur| {
        let mut v: Vec<Inflight> = parse_lines(&cur); v.push(e); (Some(to_lines(&v)), ())
    }), "push_inflight");
}

/// Remove the **last** line with a matching key and return it, **regardless of
/// age**: the TTL is a classification rule, not a retention rule, and a
/// twenty-minute build must still get its commit detected.
pub fn pop_inflight(root: &Path, key: &str) -> Option<Inflight> {
    swallow(with_locked(root, "inflight", |cur| {
        let mut v: Vec<Inflight> = parse_lines(&cur);
        match v.iter().rposition(|x| x.key == key) {
            Some(i) => { let e = v.remove(i); (Some(to_lines(&v)), Some(e)) }
            None => (None, None),
        }
    }), "pop_inflight").flatten()
}

/// Drop every entry. Every path that records an `interrupt` calls this, so one
/// Esc cannot yield two interrupts and a lingering entry cannot fake a third
/// for the next 900 s (spec §Classification step 2, §"Host adapters / Codex").
pub fn clear_inflight(root: &Path) {
    swallow(with_locked(root, "inflight", |_| (Some(String::new()), ())), "clear_inflight");
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
/// FNV-1a, 64-bit, inline. **Not** `std::hash::DefaultHasher`: its algorithm is
/// explicitly unspecified across Rust releases, so a pre/post pair split across
/// a binary upgrade would stop matching and leak an entry that then fakes an
/// interrupt for 900 s. Inline rather than a dependency: the whole algorithm is
/// four lines and the constants are part of the on-disk contract.
fn fnv1a_64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET_BASIS;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// `tool_use_id` when the host supplies one (Claude, Codex); otherwise the hex
/// of FNV-1a over `tool_name` followed by the canonical `tool_input` JSON
/// (Gemini, whose `BeforeTool`/`AfterTool` carry identical `tool_input`).
pub fn inflight_key_for(tool_use_id: Option<&str>, tool_name: &str, tool_input: &serde_json::Value) -> String {
    if let Some(id) = tool_use_id.filter(|s| !s.is_empty()) { return id.to_string(); }
    // `serde_json::Value::Object` is a BTreeMap unless the `preserve_order`
    // feature is on, so `to_string` already emits keys in sorted order. This
    // crate does not enable it; the sorted-key canonicalization the spec asks
    // for is therefore what `to_string` produces.
    let canon = serde_json::to_string(tool_input).unwrap_or_default();
    let mut bytes = tool_name.as_bytes().to_vec();
    bytes.extend_from_slice(canon.as_bytes());
    format!("h{:016x}", fnv1a_64(&bytes))
}

// ---- turn ----
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct Turn {
    /// The session that opened this turn. Carried so a turn left open by a
    /// crashed session cannot leak into the next one: a reader whose
    /// `current_sid` differs treats the file as absent.
    #[serde(default)]
    pub sid: String,
    pub open: bool,
    pub turn_id: Option<String>,
    pub last_prompt_ts: u64,
    /// `"prompt"` | `"stop"` | `"interrupt"`, or empty when absent. This is the
    /// single interrupt-already-recorded signal the classifier reads (spec
    /// §Classification step 1) — never a journal scan, so a concurrent
    /// `SubagentStop` record cannot hide it and compaction cannot erase it.
    pub last_event: String,
}

/// Read the turn for the **current** session. A missing, unparseable or
/// foreign-`sid` file reads as `Turn::default()` — absent, i.e. closed, i.e.
/// the next prompt is `fresh`. That is the conservative answer: a false `fresh`
/// undercounts an intervention, a false `open` invents one.
pub fn read_turn(root: &Path) -> Turn {
    let sid = crate::journey::current_sid(root);
    std::fs::read_to_string(dir(root).join("turn"))
        .ok()
        .and_then(|s| serde_json::from_str::<Turn>(&s).ok())
        .filter(|t| t.sid == sid)
        .unwrap_or_default()
}

/// Open the turn. Called only for a **top-level** prompt: a prompt carrying an
/// `agent_id` never writes this file, or a sub-agent's prompt would move the
/// parent's turn state and its `last_prompt_ts` (spec §Correlation state).
pub fn open_turn(root: &Path, turn_id: Option<&str>, ts: u64) {
    let t = Turn {
        sid: crate::journey::current_sid(root),
        open: true,
        turn_id: turn_id.map(str::to_string),
        last_prompt_ts: ts,
        last_event: "prompt".into(),
    };
    swallow(with_locked(root, "turn", |_| (serde_json::to_string(&t).ok(), ())), "open_turn");
}

/// Close the turn and record what closed it. Returns `false` when the write
/// failed, so the caller can degrade to the conservative answer: a failed close
/// leaves `open: true` on disk, and the classifier would otherwise read a
/// finished turn as running and report a false intervention.
pub fn close_turn(root: &Path, last_event: &str) -> bool {
    let sid = crate::journey::current_sid(root);
    swallow(with_locked(root, "turn", |cur| {
        let mut t: Turn = serde_json::from_str(&cur).unwrap_or_default();
        if t.sid != sid {
            t = Turn::default();
        }
        t.sid = sid.clone();
        t.open = false;
        t.last_event = last_event.to_string();
        (serde_json::to_string(&t).ok(), ())
    }), "close_turn").is_some()
}

// ---- kalpa ----
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Kalpa { pub name: String, pub started_ts: u64 }

/// The name pattern is a property of the value, not of the CLI: a stale or
/// hand-edited file whose `name` fails it reads as absent (with one stderr
/// warning), so it can never reach a journal tag, a log field, or the
/// model-visible context header (spec §"Naming the kalpa").
pub fn read_kalpa(root: &Path) -> Option<Kalpa> {
    let k: Kalpa = std::fs::read_to_string(dir(root).join("kalpa"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())?;
    if !valid_kalpa_name(&k.name) {
        eprintln!(
            "phronesis: ignoring .phronesis/journey/kalpa: name does not match [a-z0-9][a-z0-9-]{{0,63}}"
        );
        return None;
    }
    Some(k)
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
Expected: 15 passed. `session_write_is_an_atomic_replace_and_is_never_truncated` relies on `journey::current_sid` reading the file first, which it already does (`journey/mod.rs:42-48`).

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

`$HOME` is process-global and Rust runs a test binary's tests on parallel threads,
so one test removing it while another reads it is a race, not a hypothetical. Both
tests below take the same mutex and both go through one RAII guard that restores the
original value even on panic. The guard also means neither test depends on the
ambient `$HOME`: the positive case sets its own.

```rust
use std::sync::{Mutex, MutexGuard, OnceLock};

/// Serializes every test in this file that mutates `$HOME`.
fn home_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Sets `$HOME` (or removes it when `value` is `None`) and restores the previous
/// value on drop, including when the test panics.
struct HomeGuard {
    previous: Option<String>,
    _lock: MutexGuard<'static, ()>,
}

impl HomeGuard {
    fn set(value: Option<&str>) -> Self {
        let guard = Self {
            previous: std::env::var("HOME").ok(),
            _lock: home_lock(),
        };
        // SAFETY: edition 2024 requires `unsafe` for env mutation because it is
        // not thread-safe; `home_lock` is what makes it safe here, and every
        // other `$HOME` mutation in this file goes through this guard.
        unsafe {
            match value {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
        guard
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.previous {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
    }
}

#[test]
fn scrub_prompt_removes_bare_session_ids_transcripts_and_home_paths() {
    let fake_home = tempfile::tempdir().unwrap();
    let home = fake_home.path().display().to_string();
    let _guard = HomeGuard::set(Some(&home));
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

/// The three regexes are unconditional: a bare UUID with no `session` word
/// beside it, a phronesis sid, and a relative transcript path all go.
#[test]
fn scrub_prompt_removes_ids_with_no_surrounding_context() {
    let root = tempfile::tempdir().unwrap();
    let out = phronesis_mcp::lifecycle::scrub::scrub_prompt(
        root.path(),
        "compare 0f3c9a1e-1234-4bcd-9ef0-abcdefabcdef with s-2026-09-18-3a9f1c, see .codex/sessions/x.jsonl",
    );
    assert!(!out.contains("0f3c9a1e"), "{out}");
    assert!(!out.contains("s-2026-09-18-3a9f1c"), "{out}");
    assert!(!out.contains(".codex/sessions/x.jsonl"), "{out}");
    assert_eq!(out.matches("sess-00000000").count(), 2, "{out}");
    assert!(out.contains("compare") && out.contains("with"), "{out}");
}

#[test]
fn scrub_prompt_without_home_still_scrubs_project_root() {
    let _guard = HomeGuard::set(None);
    let root = tempfile::tempdir().unwrap();
    let text = format!("edit {}/src/main.rs now", root.path().display());
    let out = phronesis_mcp::lifecycle::scrub::scrub_prompt(root.path(), &text);
    assert!(!out.contains(&root.path().display().to_string()), "{out}");
    assert!(out.contains("src/main.rs"), "{out}");
}
```

If the file already has its own serial-test pattern for env mutation, use that one
instead and delete `home_lock`/`HomeGuard` — there must be exactly one lock in the
binary or the serialization is worthless.

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

/// Any UUID-shaped token, with **no `session` context required**: a bare id in
/// free text is still an id (spec §"Privacy and scrubbing"). Requiring the word
/// `session` nearby was the earlier shape and it misses `resume 0f3c9a1e-…`.
fn uuid_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b").unwrap()
    })
}
/// The phronesis sid shape, `s-YYYY-MM-DD-<hex>`, which is not a UUID and which
/// a human pastes into a prompt all the time ("what happened in s-2026-09-18-3a9f1c?").
fn phronesis_sid_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\bs-\d{4}-\d{2}-\d{2}-[0-9a-f]{1,8}\b").unwrap())
}
/// Any path ending in `.jsonl` under a `.claude`, `.codex` or `.gemini`
/// directory, accepting both separators and relative as well as absolute forms.
fn transcript_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"[^\s"']*[/\\]\.(?:claude|codex|gemini)[/\\][^\s"']*\.jsonl").unwrap()
    })
}

/// Scrub free prompt text before it is written to `.phronesis/log.jsonl`.
/// Never fails: without a usable `$HOME` it falls back to project-root-only
/// scrubbing and logs once to stderr.
pub fn scrub_prompt(project_root: &Path, text: &str) -> String {
    // All three run unconditionally, before `scrub_value`, and use the same
    // placeholders `scrub_value` uses for the keyed cases
    // (`payload_scrub.rs:107-109`). They run first so `scrub_value`'s
    // project-root and `$HOME` rewriting sees text with ids already gone.
    let pre = uuid_re().replace_all(text, "sess-00000000");
    let pre = phronesis_sid_re().replace_all(&pre, "sess-00000000");
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
- Test: `crates/phronesis-mcp/tests/lifecycle_state.rs` (append), `crates/phronesis-mcp/tests/journey_journal.rs` (append the negative test the spec §Testing names for that file)

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
/// The ONE accessor for the switch. Reads `.phronesis/journey.json` and fails
/// **closed**: a missing file or missing `lifecycle` block is `Full` (the
/// documented default), but an unreadable file, a malformed `lifecycle` block,
/// or any value other than `"full"` / `"none"` is `None`, with one stderr
/// warning.
pub fn prompt_text_setting(root: &Path) -> PromptText;
/// The ONE reader of correction text (spec §"Action log": "Every consumer of
/// correction text … goes through one accessor"). Consults the *current*
/// `prompt_text` value, so flipping the switch to `"none"` also hides prompts
/// already written under `"full"`. No consumer greps the log directly.
pub fn correction_text(root: &Path, entry: &LogEntry) -> Option<String>;
```

`record` does, in order: read kalpa; read `outcomes::subject::current(root)` for `subject`; `journey::current_sid`; `crate::hook::seq::next_seq`; `unix_secs_now`; build `Stamped`; `journal::append`; `action_log::append(&default_path(root), &to_log_entry(.., prompt_text_setting(root)))`; return `Some(stamped)`. Errors → stderr, and a failed journal append still attempts the log append.

`prompt_bytes` is the length of the **scrubbed** text — `to_log_entry` measures
`self.prompt`, which every adapter has already passed through `scrub_prompt`.
That, and the `<redacted:N bytes>` capture placeholder, deliberately reveal the
scrubbed length; the spec records this as a decision, not an oversight. Neither
`transcript_path` nor `agent_transcript_path` is ever persisted: they are not
fields of `LifecycleEvent` and are not in `EXTRA_KEYS`.

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
    std::fs::write(
        d.path().join(".phronesis/journey.json"),
        r#"{"version":1,"taggers":[],"modules":[],"lifecycle":{"prompt_text":"none"}}"#,
    ).unwrap();
    record(d.path(), LifecycleEvent::new(Kind::Prompt, Host::Codex).with_mode(Mode::Fresh).with_prompt("hidden")).unwrap();
    let log = std::fs::read_to_string(d.path().join(".phronesis/log.jsonl")).unwrap();
    assert!(!log.contains("hidden")); assert!(log.contains(r#""prompt_bytes":6"#));
}

/// The switch fails **closed**: a `journey.json` we cannot parse must omit the
/// text, never print it. Failing open here would leak prompts from exactly the
/// projects whose config says not to.
#[test]
fn malformed_journey_config_omits_prompt_text() {
    use phronesis_mcp::lifecycle::{Host, Kind, LifecycleEvent, Mode, record::record};
    let d = root();
    std::fs::create_dir_all(d.path().join(".phronesis")).unwrap();
    for bad in [
        r#"{"version":1,"taggers":[],"modules":[],"lifecycle":{"prompt_text":"maybe"}}"#,
        r#"{"version":1,"taggers":[],"modules":[],"lifecycle":"full"}"#,
        r#"{not json at all"#,
    ] {
        let _ = std::fs::remove_file(d.path().join(".phronesis/log.jsonl"));
        std::fs::write(d.path().join(".phronesis/journey.json"), bad).unwrap();
        record(d.path(), LifecycleEvent::new(Kind::Prompt, Host::Claude).with_mode(Mode::Fresh).with_prompt("hidden"));
        let log = std::fs::read_to_string(d.path().join(".phronesis/log.jsonl")).unwrap();
        assert!(!log.contains("hidden"), "{bad}: {log}");
        assert!(log.contains(r#""prompt_bytes":6"#), "{bad}: {log}");
    }
    // No config file at all is the documented default, and it is `full`.
    std::fs::remove_file(d.path().join(".phronesis/journey.json")).unwrap();
    let _ = std::fs::remove_file(d.path().join(".phronesis/log.jsonl"));
    record(d.path(), LifecycleEvent::new(Kind::Prompt, Host::Claude).with_mode(Mode::Fresh).with_prompt("hidden"));
    assert!(std::fs::read_to_string(d.path().join(".phronesis/log.jsonl")).unwrap().contains("hidden"));
}

/// `correction_text` is the one accessor, and it consults the *current* value,
/// so flipping the switch hides text already written under `"full"`.
#[test]
fn correction_text_hides_already_written_prompts_when_the_switch_flips() {
    use phronesis_mcp::action_log::LogEntry;
    use phronesis_mcp::lifecycle::record::correction_text;
    let d = root();
    let entry = LogEntry::new("lifecycle", "prompt")
        .with("mode", "correction")
        .with("prompt", "no, the other thing");
    assert_eq!(correction_text(d.path(), &entry).as_deref(), Some("no, the other thing"));

    std::fs::create_dir_all(d.path().join(".phronesis")).unwrap();
    std::fs::write(
        d.path().join(".phronesis/journey.json"),
        r#"{"version":1,"taggers":[],"modules":[],"lifecycle":{"prompt_text":"none"}}"#,
    )
    .unwrap();
    assert_eq!(correction_text(d.path(), &entry), None, "the switch is enforced at read time");
}

/// The journal append fails (the path is a directory), but the log entry —
/// which is where the prompt text and `prompt_bytes` live — is still written,
/// and `record` reports the failure by returning `None`. Spec: lifecycle
/// writes are fail-open and never fail a hook.
#[test]
fn record_still_logs_when_the_journal_append_fails() {
    use phronesis_mcp::lifecycle::{Host, Kind, LifecycleEvent, Mode, record::record};
    let d = root();
    std::fs::create_dir_all(d.path().join(".phronesis/journey/events.jsonl")).unwrap();
    let out = record(
        d.path(),
        LifecycleEvent::new(Kind::Prompt, Host::Claude).with_mode(Mode::Fresh).with_prompt("abc"),
    );
    assert!(out.is_none(), "a failed journal append must report itself");
    let log = std::fs::read_to_string(d.path().join(".phronesis/log.jsonl")).unwrap();
    assert!(log.contains(r#""prompt_bytes":3"#), "{log}");
}

/// The spec §Testing row for `tests/journey_journal.rs` names this test for
/// that file, so it goes there rather than here — appended verbatim to
/// `crates/phronesis-mcp/tests/journey_journal.rs`:
///
/// ```rust
/// /// No field of a lifecycle journal record ever contains prompt text.
/// /// Not "no `prompt` key": no field at all, checked over the serialized line.
/// #[test]
/// fn a_prompt_event_journals_no_field_containing_the_text() {
///     use phronesis_mcp::lifecycle::{Host, Kind, LifecycleEvent, Mode, record::record};
///     let d = tempfile::tempdir().unwrap();
///     record(
///         d.path(),
///         LifecycleEvent::new(Kind::Prompt, Host::Claude)
///             .with_mode(Mode::Correction)
///             .with_prompt("zzz-distinctive-prompt-text"),
///     );
///     let line = std::fs::read_to_string(d.path().join(".phronesis/journey/events.jsonl")).unwrap();
///     assert!(!line.contains("zzz-distinctive"), "{line}");
///     let rec: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
///     for (_, v) in rec.as_object().unwrap() {
///         assert!(!v.to_string().contains("zzz-distinctive"), "{rec}");
///     }
///     // …and the action log does hold it, so this is a placement test, not a
///     // "the text vanished" test.
///     assert!(
///         std::fs::read_to_string(d.path().join(".phronesis/log.jsonl"))
///             .unwrap()
///             .contains("zzz-distinctive-prompt-text")
///     );
/// }
/// ```
///
/// The join key between the two files (spec §"Action log").
#[test]
fn journal_record_and_log_entry_share_sid_and_seq() {
    use phronesis_mcp::lifecycle::{Host, Kind, LifecycleEvent, record::record};
    let d = root();
    record(d.path(), LifecycleEvent::new(Kind::Stop, Host::Claude)).unwrap();
    let journal: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(d.path().join(".phronesis/journey/events.jsonl"))
            .unwrap().lines().next_back().unwrap(),
    ).unwrap();
    let log: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(d.path().join(".phronesis/log.jsonl"))
            .unwrap().lines().next_back().unwrap(),
    ).unwrap();
    assert!(!journal["sid"].is_null());
    assert!(!journal["seq"].is_null());
    assert_eq!(journal["sid"], log["sid"]);
    assert_eq!(journal["seq"], log["seq"]);
}
```

`TaggerConfig`'s real shape, verified against the current tree, is
`{ version: u32, taggers: Vec<TaggerEntry>, modules: Vec<ModuleEntry> }` plus a
`#[serde(skip)] compiled: OnceLock<Vec<Rule>>` — there is no `tags` map. It has a
hand-written `Default` impl (`tagger.rs:58`) rather than `#[derive(Default)]`, so
adding the `lifecycle` field means adding `lifecycle: LifecycleConfig::default()` to
that impl. Grep for `TaggerConfig {` across `src/` and `tests/` and add the field to
every struct literal the compiler flags.

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

/// Fails **closed**. `load_config` returns `NotFound` when the project has no
/// `journey.json` at all, which the spec says means `"full"`; every other error
/// — unreadable file, malformed JSON, a `lifecycle` block serde could not
/// parse, a `prompt_text` value that is neither `"full"` nor `"none"` — is a
/// config we do not understand, and the switch must never fail open on one.
pub fn prompt_text_setting(root: &Path) -> PromptText {
    match journey::load_config(root) {
        Ok(cfg) => match cfg.lifecycle.prompt_text {
            PromptTextSetting::Full => PromptText::Full,
            PromptTextSetting::None => PromptText::None,
        },
        Err(journey::ConfigError::NotFound(_)) => PromptText::Full,
        Err(e) => {
            eprintln!("phronesis: lifecycle prompt_text unreadable ({e}); omitting prompt text");
            PromptText::None
        }
    }
}

/// The single accessor for correction text. `phr-mcp journey --corrections`,
/// the `extract_rules` hand-off, and any future MCP surface call this rather
/// than reading `entry.data["prompt"]`, so flipping `prompt_text` to `"none"`
/// hides text already on disk as well as text not yet written.
pub fn correction_text(root: &Path, entry: &crate::action_log::LogEntry) -> Option<String> {
    if prompt_text_setting(root) == PromptText::None {
        return None;
    }
    entry
        .data
        .get("prompt")
        .and_then(|v| v.as_str())
        .map(str::to_string)
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
git add crates/phronesis-mcp/src/lifecycle/record.rs crates/phronesis-mcp/src/journey/tagger.rs crates/phronesis-mcp/tests/lifecycle_state.rs crates/phronesis-mcp/tests/journey_journal.rs
git commit -m "feat(lifecycle): record() writes journal and log; prompt_text config"
```

---

### Task 8: `classify_prompt`

**Files:**
- Modify: `crates/phronesis-mcp/src/lifecycle/state.rs` (append)
- Test: `crates/phronesis-mcp/tests/lifecycle_classify.rs` (new)

**Interfaces (Produces):**
```rust
pub struct PromptContext<'a> { pub host: Host, pub now: u64, pub agent_id: Option<&'a str>, pub turn_id: Option<&'a str>, pub transcript_path: Option<&'a Path> }
/// How an interrupt was inferred, for the log's `inferred_from`. There is no
/// `Hook` variant: the Codex `Interrupt` hook writes its own record with
/// `inferred_from: "hook"` and sets `turn.last_event`, which the classifier
/// reads in step 1 without inferring anything.
pub enum InterruptSource { Inflight, Transcript, OpenTurn }
impl InterruptSource { pub fn as_str(self) -> &'static str; /* inflight | transcript | open_turn */ }
pub struct Classification { pub mode: Mode, pub interrupt: Option<InterruptSource> }
pub fn classify_prompt(root: &Path, ctx: &PromptContext<'_>) -> Classification;
/// Step 2's evidence check on its own, so `SessionEnd` can run the same
/// detection without classifying a prompt (spec §"Host adapters / Claude Code").
pub fn detect_interrupt(root: &Path, ctx: &PromptContext<'_>) -> Option<InterruptSource>;
```

**`interrupt: None` with `mode: Correction` means "already recorded".** Step 1's
`turn.last_event == "interrupt"` branch is the single interrupt-already-recorded
path: the record exists (the Codex `Interrupt` hook wrote it, or an earlier
inferred branch did), so the adapter writes the prompt and nothing else. Every
other `Correction` carries `Some(source)` and the adapter writes the `interrupt`
record first. That is the whole contract between this function and the three
adapters, and it is why there is no `Hook` variant to mistake for evidence.

**There is no journal scan.** An earlier draft derived the evidence from the
journal tail (`last_lifecycle_kind`); the spec now reads it from `turn.last_event`
instead, "so a concurrent `SubagentStop` record cannot hide it and compaction
cannot erase it". `last_lifecycle_kind` is therefore **not** part of this plan and
no adapter calls it.

Everything else is exactly as spec §Classification, including the order inside
step 2: **transcript before inflight**, because the transcript marker is direct
evidence and a queued mid-turn message also leaves a live `inflight` entry — only
the marker tells the two apart.

Transcript check: read the last 64 KiB of `transcript_path`, split into lines,
parse each as JSON, and consider a hit when any object has `type == "user"` (or a
nested `message.role == "user"`) whose text content (string, or the `text` of a
single text block) equals `[Request interrupted by user]` or starts with
`[Request interrupted by user for tool use]`, and whose `timestamp` (RFC3339, if
present) is after `turn.last_prompt_ts`; if no timestamp is present, accept the
hit.

- [ ] **Step 1: Write the failing tests** in `tests/lifecycle_classify.rs`

```rust
use phronesis_mcp::lifecycle::state::*;
use phronesis_mcp::lifecycle::{Host, Mode};

fn root() -> tempfile::TempDir { tempfile::tempdir().unwrap() }
fn ctx<'a>(host: Host, now: u64) -> PromptContext<'a> {
    PromptContext { host, now, agent_id: None, turn_id: None, transcript_path: None }
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
fn live_inflight_in_scope_is_interrupt_and_drops_every_entry() {
    let d = root(); open_turn(d.path(), None, 10);
    push_inflight(d.path(), Inflight { key: "k".into(), tool: "Bash".into(), ts: 90, agent_id: None, head_before: None, detection: None });
    push_inflight(d.path(), Inflight { key: "k2".into(), tool: "Edit".into(), ts: 90, agent_id: Some("sub".into()), head_before: None, detection: None });
    let c = classify_prompt(d.path(), &ctx(Host::Claude, 100));
    assert_eq!(c.mode, Mode::Correction); assert_eq!(c.interrupt.map(|i| i.as_str()), Some("inflight"));
    // Every branch that infers an interrupt drops the session's entries, so one
    // Esc cannot yield two interrupts — including the sub-agent's entry, which
    // was not evidence but is just as dead.
    assert!(pop_inflight(d.path(), "k").is_none());
    assert!(pop_inflight(d.path(), "k2").is_none());
}

#[test]
fn stale_inflight_and_foreign_agent_inflight_are_ignored() {
    let d = root(); open_turn(d.path(), None, 10);
    push_inflight(d.path(), Inflight { key: "stale".into(), tool: "Bash".into(), ts: 1, agent_id: None, head_before: None, detection: None });
    push_inflight(d.path(), Inflight { key: "child".into(), tool: "Edit".into(), ts: 1000, agent_id: Some("sub".into()), head_before: None, detection: None });
    let c = classify_prompt(d.path(), &ctx(Host::Claude, 1000));
    assert_eq!(c.mode, Mode::MidTurn);
    // A mid-turn classification leaves the sub-agent's live entry alone: its
    // tool is still running and its post-check must still find it.
    assert!(pop_inflight(d.path(), "child").is_some());
}

/// Spec §Classification step 1: `last_event == "interrupt"` yields `correction`
/// on **every** host, and it is the single interrupt-already-recorded path —
/// `interrupt` is `None`, so the adapter writes no second record.
#[test]
fn last_event_interrupt_is_a_correction_on_every_host() {
    for host in [Host::Claude, Host::Codex, Host::Gemini] {
        let d = root();
        open_turn(d.path(), Some("t1"), 10);
        close_turn(d.path(), "interrupt");
        let c = classify_prompt(d.path(), &ctx(host, 100));
        assert_eq!(c.mode, Mode::Correction, "{host:?}");
        assert!(c.interrupt.is_none(), "{host:?}: the record already exists");
    }
}

/// It is read from `turn`, not from the journal, so compaction cannot erase it
/// and a concurrent `SubagentStop` record cannot hide it.
#[test]
fn last_event_interrupt_survives_a_compacted_journal() {
    use phronesis_mcp::journey::journal;
    let d = root();
    set_session(d.path(), "s-a");
    open_turn(d.path(), Some("t1"), 10);
    close_turn(d.path(), "interrupt");
    // Everything in the journal compacts away; the turn file is untouched.
    journal::append(d.path(), &journal::JournalRecord {
        v: journal::JOURNAL_V, ts: 1, sid: "s-a".into(), seq: 1,
        tool: journal::LIFECYCLE_TOOL.into(), path: String::new(), ext: None, module: None,
        tags: vec!["lifecycle:interrupt".into()], subject: None, command_exit: None,
        kind: Some("interrupt".into()), mode: None, host: Some("codex".into()),
        turn: None, agent: None, agent_type: None, kalpa: None,
    }).unwrap();
    assert!(journal::maybe_compact(d.path(), 1, 0).unwrap());
    assert_eq!(classify_prompt(d.path(), &ctx(Host::Codex, 100)).mode, Mode::Correction);
}

/// A prompt delivered inside a sub-agent is not the human speaking: it is
/// `fresh`, it infers nothing, and it touches no state.
#[test]
fn a_sub_agent_prompt_is_fresh_and_reads_no_state() {
    let d = root();
    open_turn(d.path(), Some("t1"), 10);
    push_inflight(d.path(), Inflight { key: "k".into(), tool: "Bash".into(), ts: 90, agent_id: None, head_before: None, detection: None });
    let mut c = ctx(Host::Claude, 100);
    c.agent_id = Some("sub-1");
    let out = classify_prompt(d.path(), &c);
    assert_eq!(out.mode, Mode::Fresh);
    assert!(out.interrupt.is_none());
    // Nothing was consumed and the parent's turn is untouched.
    assert!(pop_inflight(d.path(), "k").is_some());
    let t = read_turn(d.path());
    assert!(t.open);
    assert_eq!(t.last_prompt_ts, 10);
}

/// A turn left open by a crashed session, and a corrupt turn file, both read as
/// absent — the conservative answer is `fresh`, not a false intervention.
#[test]
fn a_foreign_sid_or_corrupt_turn_yields_fresh() {
    let d = root();
    set_session(d.path(), "s-old");
    open_turn(d.path(), Some("t1"), 10);
    set_session(d.path(), "s-new");
    assert_eq!(classify_prompt(d.path(), &ctx(Host::Claude, 100)).mode, Mode::Fresh);

    std::fs::write(d.path().join(".phronesis/journey/turn"), "{{{").unwrap();
    assert_eq!(classify_prompt(d.path(), &ctx(Host::Claude, 100)).mode, Mode::Fresh);
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

/// Both marker strings the spec names, and the tool-use one is a prefix match.
#[test]
fn both_interrupt_marker_strings_are_recognized() {
    for marker in [
        "[Request interrupted by user]",
        "[Request interrupted by user for tool use]",
        "[Request interrupted by user for tool use: Bash]",
    ] {
        let d = root(); open_turn(d.path(), None, 1_700_000_000);
        let t = d.path().join("t.jsonl");
        std::fs::write(&t, format!(
            r#"{{"type":"user","message":{{"role":"user","content":"{marker}"}},"timestamp":"2023-11-14T22:14:00Z"}}"#
        )).unwrap();
        let mut c = ctx(Host::Claude, 1_700_000_100); c.transcript_path = Some(&t);
        assert_eq!(classify_prompt(d.path(), &c).interrupt.map(|i| i.as_str()), Some("transcript"), "{marker}");
    }
}

/// The transcript is checked **before** `inflight`, because a queued mid-turn
/// message also leaves a live `inflight` entry and only the marker tells the
/// two apart. With the order reversed this test still passes on `mode` — hence
/// the assertion on `inferred_from`, which is the part that carries the
/// confidence a human reads.
#[test]
fn the_transcript_marker_outranks_a_live_inflight_entry() {
    let d = root(); open_turn(d.path(), None, 1_700_000_000);
    push_inflight(d.path(), Inflight { key: "k".into(), tool: "Bash".into(), ts: 1_700_000_050, agent_id: None, head_before: None, detection: None });
    let t = d.path().join("t.jsonl");
    std::fs::write(&t, r#"{"type":"user","message":{"role":"user","content":"[Request interrupted by user]"},"timestamp":"2023-11-14T22:14:00Z"}"#).unwrap();
    let mut c = ctx(Host::Claude, 1_700_000_100); c.transcript_path = Some(&t);
    assert_eq!(classify_prompt(d.path(), &c).interrupt.map(|i| i.as_str()), Some("transcript"));
}

#[test]
fn claude_transcript_marker_before_last_prompt_is_ignored() {
    let d = root(); open_turn(d.path(), None, 1_700_000_500);
    let t = d.path().join("t.jsonl");
    std::fs::write(&t, r#"{"type":"user","message":{"role":"user","content":"[Request interrupted by user]"},"timestamp":"2023-11-14T22:13:00Z"}"#).unwrap();
    let mut c = ctx(Host::Claude, 1_700_000_600); c.transcript_path = Some(&t);
    assert_eq!(classify_prompt(d.path(), &c).mode, Mode::MidTurn);
}

/// The tail is bounded at 64 KiB, and the first (probably partial) line of a
/// seeked read is discarded. A marker pushed out by a big paste is a documented
/// miss, not a crash: the prompt falls through to `inflight` or `mid_turn`.
#[test]
fn a_marker_pushed_out_of_the_sixty_four_kib_tail_is_missed() {
    let d = root(); open_turn(d.path(), None, 1_700_000_000);
    let t = d.path().join("t.jsonl");
    let marker = format!(
        "{}\n",
        r#"{"type":"user","message":{"role":"user","content":"[Request interrupted by user]"},"timestamp":"2023-11-14T22:14:00Z"}"#
    );
    let filler = format!(
        "{{\"type\":\"assistant\",\"message\":{{\"role\":\"assistant\",\"content\":\"{}\"}}}}\n",
        "x".repeat(70 * 1024)
    );
    std::fs::write(&t, format!("{marker}{filler}")).unwrap();
    let mut c = ctx(Host::Claude, 1_700_000_100); c.transcript_path = Some(&t);
    assert_eq!(classify_prompt(d.path(), &c).mode, Mode::MidTurn, "no inflight, no marker in the tail");

    // The same marker inside the tail is found.
    std::fs::write(&t, format!("{filler}{marker}")).unwrap();
    assert_eq!(classify_prompt(d.path(), &c).interrupt.map(|i| i.as_str()), Some("transcript"));
}

/// An unreadable or absent transcript is not an error; the next branch runs.
#[test]
fn an_unreadable_transcript_falls_through_to_inflight() {
    let d = root(); open_turn(d.path(), None, 10);
    push_inflight(d.path(), Inflight { key: "k".into(), tool: "Bash".into(), ts: 90, agent_id: None, head_before: None, detection: None });
    let missing = d.path().join("does-not-exist.jsonl");
    let mut c = ctx(Host::Claude, 100); c.transcript_path = Some(&missing);
    assert_eq!(classify_prompt(d.path(), &c).interrupt.map(|i| i.as_str()), Some("inflight"));
}

/// `detect_interrupt` is the same evidence check without the classification, so
/// `SessionEnd` can ask "was this turn aborted?" (spec §"Host adapters /
/// Claude Code": "if `turn` is open, run the same interrupt detection step 2
/// runs and record `interrupt` when there is evidence, otherwise record `stop`").
#[test]
fn detect_interrupt_is_reusable_by_session_end() {
    let d = root(); open_turn(d.path(), None, 10);
    assert!(detect_interrupt(d.path(), &ctx(Host::Claude, 100)).is_none());
    push_inflight(d.path(), Inflight { key: "k".into(), tool: "Bash".into(), ts: 90, agent_id: None, head_before: None, detection: None });
    assert_eq!(
        detect_interrupt(d.path(), &ctx(Host::Claude, 100)).map(|i| i.as_str()),
        Some("inflight")
    );
    assert!(pop_inflight(d.path(), "k").is_none(), "it drops the entries too");
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
    pub host: Host,
    pub now: u64,
    /// `Some` when this prompt was delivered **inside** a sub-agent. Such a
    /// prompt is not the human speaking: it classifies `fresh` and touches no
    /// state at all.
    pub agent_id: Option<&'a str>,
    pub turn_id: Option<&'a str>,
    pub transcript_path: Option<&'a Path>,
}

/// How an interrupt was inferred, for the log's `inferred_from`. No `Hook`
/// variant: the Codex `Interrupt` hook writes its own record and sets
/// `turn.last_event`, which step 1 reads without inferring anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptSource { Inflight, Transcript, OpenTurn }
impl InterruptSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inflight => "inflight",
            Self::Transcript => "transcript",
            Self::OpenTurn => "open_turn",
        }
    }
}

/// `interrupt: Some(source)` means the caller must write an `interrupt` record
/// with that `inferred_from` before the prompt record. `interrupt: None` with
/// `mode: Correction` means the record already exists (step 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classification { pub mode: Mode, pub interrupt: Option<InterruptSource> }

/// Spec §"Classification at prompt time", in the spec's own order.
pub fn classify_prompt(root: &Path, ctx: &PromptContext<'_>) -> Classification {
    // A prompt carrying an agent_id is a sub-agent's prompt, not the human's.
    // It reads no state and writes none.
    if ctx.agent_id.is_some() {
        return Classification { mode: Mode::Fresh, interrupt: None };
    }

    // Step 1. `read_turn` already treats a missing, unparseable or foreign-sid
    // file as absent, which is `Turn::default()`: closed, last_event empty.
    let turn = read_turn(root);
    if turn.last_event == "interrupt" {
        // The single interrupt-already-recorded path, on every host. Read from
        // `turn` and not from the journal tail, so a concurrent SubagentStop
        // record cannot hide it and compaction cannot erase it.
        return Classification { mode: Mode::Correction, interrupt: None };
    }
    if !turn.open {
        return Classification { mode: Mode::Fresh, interrupt: None };
    }

    // Step 2. Look for evidence that the human aborted the running turn.
    match detect_interrupt(root, ctx) {
        Some(source) => Classification { mode: Mode::Correction, interrupt: Some(source) },
        // Step 3. Reachable on Claude and Codex only; the Gemini branch inside
        // `detect_interrupt` always fires for an open turn.
        None => Classification { mode: Mode::MidTurn, interrupt: None },
    }
}

/// Step 2's evidence check, in the spec's order, stopping at the first hit.
/// Every hit drops the session's `inflight` entries, so one Esc cannot yield
/// two interrupts and a lingering entry cannot fake a third.
///
/// Exposed on its own because `SessionEnd` runs the same check to decide
/// whether a quit out of an aborted turn is an `interrupt` or a `stop`.
pub fn detect_interrupt(root: &Path, ctx: &PromptContext<'_>) -> Option<InterruptSource> {
    let turn = read_turn(root);

    // Transcript marker (Claude only), FIRST: it is direct evidence. A queued
    // mid-turn message also leaves a live `inflight` entry, and only the marker
    // distinguishes the two.
    let source = if ctx.host == Host::Claude
        && let Some(p) = ctx.transcript_path
        && transcript_has_interrupt_after(p, turn.last_prompt_ts)
    {
        Some(InterruptSource::Transcript)
    // Inflight: a tool was still running when the human spoke. Not used on
    // Codex, where the `Interrupt` hook is authoritative and step 1 has already
    // spoken. `live_inflight` reads without consuming, so a `mid_turn` outcome
    // leaves a running tool's entry in place for its own post-check.
    } else if ctx.host != Host::Codex
        && !live_inflight(root, ctx.now, ctx.agent_id).is_empty()
    {
        Some(InterruptSource::Inflight)
    // Open turn (Gemini only): Gemini never delivers a prompt while a turn is
    // running, so an open turn here means AfterAgent was skipped, which only
    // happens on abort.
    } else if ctx.host == Host::Gemini && turn.open {
        Some(InterruptSource::OpenTurn)
    } else {
        None
    };

    if source.is_some() {
        clear_inflight(root);
    }
    source
}

const TRANSCRIPT_TAIL_BYTES: u64 = 64 * 1024;
const INTERRUPT_MARKER: &str = "[Request interrupted by user";

fn transcript_has_interrupt_after(path: &Path, after_ts: u64) -> bool {
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(TRANSCRIPT_TAIL_BYTES);
    if f.seek(SeekFrom::Start(start)).is_err() { return false; }
    let mut buf = Vec::new();
    if f.read_to_end(&mut buf).is_err() { return false; }
    // Lossy rather than `read_to_string`: a 64 KiB seek can land mid-codepoint,
    // and a transcript that happens to contain one must not disable the branch.
    let buf = String::from_utf8_lossy(&buf);
    // Skip the first line when the read was seeked: it is probably a fragment.
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
git commit -m "feat(lifecycle): classify_prompt from turn state, transcript marker and inflight evidence"
```

---

### Task 9: `lifecycle::outcome::detect_commit`

**Files:**
- Create: `crates/phronesis-mcp/src/lifecycle/outcome.rs`
- Test: `crates/phronesis-mcp/tests/lifecycle_outcome.rs` (new)

**Interfaces (Produces):**
```rust
pub fn is_shell_tool(tool_name: &str) -> bool;                 // Bash | run_shell_command
pub fn command_may_move_head(command: &str) -> bool;            // cheap text pre-filter, applied at PRE and at POST
/// Outcome of one `git rev-parse HEAD`, 2 s timeout. The timeout is
/// distinguished from "not a repo / git unavailable" because the spec wants a
/// timeout to leave `detection: "timeout"` on the `inflight` entry — a timed-out
/// probe is a miss worth auditing, an absent repo is not.
pub enum HeadProbe { Head(String), Timeout, Unavailable }
pub fn git_head_probe(root: &Path) -> HeadProbe;
pub fn git_head(root: &Path) -> Option<String>;                 // convenience: `Head(sha)` → `Some`
/// The `detection` marker for the `inflight` entry, or `None` when detection is
/// live. `"timeout"` at pre, `"no_exit_code"` at post.
pub const DETECTION_TIMEOUT: &str = "timeout";
pub const DETECTION_NO_EXIT_CODE: &str = "no_exit_code";
pub struct Commit { pub sha: String, pub head_before: String }
pub fn detect_commit(root: &Path, head_before: Option<&str>, command: &str, command_exit: Option<i32>) -> Option<Commit>;
```

**Where the pre-filter runs.** Spec §"Success signal: commit" step 1: the pre-time
`git rev-parse HEAD` runs only for a shell tool **whose command passes the text
pre-filter**, so a shell call that cannot be a commit spawns no git process at
all. `command_may_move_head` is therefore called twice — once in
`hook/lifecycle_wiring.rs::pre_push_inflight` (Plan 2 Task 4) and once inside
`detect_commit` — and this is the one function that defines it.

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

/// The two documented misses, pinned so the undercount is known rather than
/// discovered: `git -C .` in the *same* repo is a real commit the pre-filter
/// happens to catch (so it IS detected), and a wrapper script is not.
#[test]
fn git_dash_c_in_the_same_repo_is_detected_and_a_wrapper_script_is_not() {
    let d = repo();
    let before = git_head(d.path()).unwrap();
    std::fs::write(d.path().join("a"), "2").unwrap();
    git(d.path(), &["commit", "-q", "-am", "same repo"]);
    // `git -C .` names this repo, so HEAD really moved here: detection observes
    // the repository, not the string, and gets it right.
    assert!(detect_commit(d.path(), Some(&before), "git -C . commit -am x", Some(0)).is_some());

    // A wrapper script moves HEAD without the word `git commit` anywhere, so the
    // pre-filter never fires and the commit is missed. Commits are undercounted,
    // never overcounted; `kalpa show` says so under the commit line.
    let d2 = repo();
    let before2 = git_head(d2.path()).unwrap();
    std::fs::write(d2.path().join("a"), "3").unwrap();
    git(d2.path(), &["commit", "-q", "-am", "via release.sh"]);
    assert!(detect_commit(d2.path(), Some(&before2), "./release.sh", Some(0)).is_none());
    for missed in ["git pull", "git am patch.mbox", "git com -m x"] {
        assert!(!command_may_move_head(missed), "{missed}");
    }
}

/// Amends and rebases move HEAD and ARE recorded; `sha` is what distinguishes
/// them from a new commit for any consumer that cares (spec §Non-goals).
#[test]
fn an_amend_and_a_rebase_are_recorded_as_head_movements() {
    let d = repo();
    let before = git_head(d.path()).unwrap();
    std::fs::write(d.path().join("a"), "2").unwrap();
    git(d.path(), &["commit", "-q", "--amend", "-am", "amended"]);
    let amended = detect_commit(d.path(), Some(&before), "git commit --amend -am amended", Some(0))
        .expect("an amend moves HEAD");
    assert_ne!(amended.sha, before);

    let base = git_head(d.path()).unwrap();
    std::fs::write(d.path().join("b"), "1").unwrap();
    git(d.path(), &["add", "b"]);
    git(d.path(), &["commit", "-q", "-m", "second"]);
    let head = git_head(d.path()).unwrap();
    git(d.path(), &["rebase", "--quiet", "--onto", &base, &base, "HEAD"]);
    assert!(
        detect_commit(d.path(), Some(&head), "git rebase --onto main main", Some(0)).is_some()
            || git_head(d.path()).as_deref() == Some(head.as_str()),
        "either the rebase moved HEAD and was detected, or it was a no-op"
    );
}

/// A timed-out probe is distinguishable from "not a repo", because only the
/// first leaves an auditable `detection` marker on the `inflight` entry.
#[test]
fn head_probe_reports_unavailable_outside_a_repo() {
    let d = tempfile::tempdir().unwrap();
    assert!(matches!(git_head_probe(d.path()), HeadProbe::Unavailable));
    assert_eq!(git_head(d.path()), None);
    let r = repo();
    assert!(matches!(git_head_probe(r.path()), HeadProbe::Head(sha) if sha.len() == 40));
}

/// Without an exit code the check is skipped entirely: `git commit && false`
/// must not count, and a host that sends nothing cannot be given the benefit of
/// the doubt.
#[test]
fn a_missing_exit_code_skips_detection() {
    let d = repo();
    let before = git_head(d.path()).unwrap();
    std::fs::write(d.path().join("a"), "4").unwrap();
    git(d.path(), &["commit", "-q", "-am", "fourth"]);
    assert!(detect_commit(d.path(), Some(&before), "git commit -am fourth", None).is_none());
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

/// Why detection was disabled for a call. Stored on the `inflight` entry so the
/// miss is auditable rather than silent (spec §"Success signal: commit").
pub const DETECTION_TIMEOUT: &str = "timeout";
pub const DETECTION_NO_EXIT_CODE: &str = "no_exit_code";

/// Outcome of one `git rev-parse HEAD`. `Timeout` is separate from
/// `Unavailable` because only a timeout is worth marking: "not a repo" is a
/// permanent, uninteresting condition, while a 2 s timeout means the probe lost
/// a race with something and the commit may have been real.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadProbe { Head(String), Timeout, Unavailable }

pub fn git_head_probe(root: &Path) -> HeadProbe {
    let Ok(mut child) = Command::new("git").args(["rev-parse", "HEAD"]).current_dir(root)
        .stdout(Stdio::piped()).stderr(Stdio::null()).stdin(Stdio::null()).spawn()
    else {
        return HeadProbe::Unavailable;
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() { return HeadProbe::Unavailable; }
                let mut out = String::new();
                use std::io::Read;
                let Some(mut stdout) = child.stdout.take() else { return HeadProbe::Unavailable };
                if stdout.read_to_string(&mut out).is_err() { return HeadProbe::Unavailable; }
                let sha = out.trim().to_string();
                // 40 for SHA-1, 64 under `--object-format=sha256`.
                return if matches!(sha.len(), 40 | 64) && sha.chars().all(|c| c.is_ascii_hexdigit()) {
                    HeadProbe::Head(sha)
                } else {
                    HeadProbe::Unavailable
                };
            }
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20))
            }
            Ok(None) => { let _ = child.kill(); return HeadProbe::Timeout; }
            Err(_) => { let _ = child.kill(); return HeadProbe::Unavailable; }
        }
    }
}

pub fn git_head(root: &Path) -> Option<String> {
    match git_head_probe(root) {
        HeadProbe::Head(sha) => Some(sha),
        HeadProbe::Timeout | HeadProbe::Unavailable => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit { pub sha: String, pub head_before: String }

/// `command_exit != Some(0)` suppresses the check, which is what makes
/// `git commit && false` a non-commit — and what makes a host that sends no
/// exit code record `DETECTION_NO_EXIT_CODE` on the entry instead of guessing.
pub fn detect_commit(root: &Path, head_before: Option<&str>, command: &str, command_exit: Option<i32>) -> Option<Commit> {
    let before = head_before?;
    if command_exit != Some(0) || !command_may_move_head(command) { return None; }
    let after = git_head(root)?;
    (after != before).then(|| Commit { sha: after, head_before: before.to_string() })
}
```

Note the heredoc test passes because `HEAD` did not move, not because of the regex; that is the point of the design.

`git_head` is what `pre_push_inflight` calls when the pre-filter matched;
`git_head_probe` is what it calls to tell a timeout apart, so it can store
`detection: DETECTION_TIMEOUT` on the entry. Both are in this module, so the 2 s
timeout is defined once.

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
pub(crate) const REDACTED_KEYS: [&str; 3] = ["prompt", "prompt_response", "last_assistant_message"];
/// `pub`, not `pub(crate)`: `tests/payload_capture.rs` is an integration test
/// and reaches it as `phronesis_mcp::hook::redact_for_capture`.
/// Walks the value **recursively**, so `tool_input.prompt` (Gemini's
/// `invoke_agent`, a full sub-agent task) is covered as well as a top-level
/// `prompt`. Returns `None` when stdin is not valid JSON: it cannot be redacted,
/// so it is not written at all.
pub fn redact_for_capture(raw: &str) -> Option<String>;
/// Promoted from private `fn` so `claude_hook.rs` (Plan 2) and `codex_hook.rs`
/// (Plan 3) can tee their own stdin. Plan 1 makes this change; neither
/// dependent plan touches `hook/mod.rs` for it.
pub(crate) fn capture_raw_payload(phase: &str, raw: &str);
```

The redaction lives **inside `capture_raw_payload`**, so every caller inherits
it — `read_payload` (hence `pre-check` and `post-check`), Plan 2's
`claude_hook.rs`, and Plan 3's `codex_hook.rs`. That is what covers Gemini's
`invoke_agent`, whose `tool_input.prompt` is a full sub-agent task, and Gemini's
`AfterAgent`, whose `prompt_response` is the model's whole answer. Plans 2 and 3
call `capture_raw_payload` and rely on this task for both the visibility and the
redaction; neither re-implements it.

The capture is therefore a redacted **re-serialization**, not a verbatim tee.
`docs/payload-corpus-promotion.md` is amended in Task 12 to say so.

Redaction applies to the copy written to the capture dir only: the hook parses
the original stdin and the action log still receives the full scrubbed prompt.
Both halves are tested below.

- [ ] **Step 1: Write the failing test** (append to `tests/payload_capture.rs`, following the file's existing pattern for setting `PHRONESIS_CAPTURE_DIR` and invoking `phr-mcp pre-check`)

```rust
#[test]
fn capture_redacts_every_free_text_key_at_any_depth() {
    let raw = r#"{"hook_event_name":"UserPromptSubmit","session_id":"s","prompt":"top secret words","tool_name":"Bash","tool_input":{"command":"ls"}}"#;
    let out = phronesis_mcp::hook::redact_for_capture(raw).expect("valid JSON is captured");
    assert!(!out.contains("top secret"));
    assert!(out.contains(r#""prompt":"<redacted:16 bytes>""#));
    assert!(out.contains(r#""tool_input":{"command":"ls"}"#));

    // Nested: Gemini's invoke_agent carries the whole sub-agent task under
    // `tool_input.prompt`, which a top-level-only redaction would miss.
    let nested = r#"{"hook_event_name":"BeforeTool","tool_name":"invoke_agent","tool_input":{"agent_name":"reviewer","prompt":"review the auth module"}}"#;
    let out = phronesis_mcp::hook::redact_for_capture(nested).expect("valid JSON");
    assert!(!out.contains("review the auth"), "{out}");
    assert!(out.contains(r#""prompt":"<redacted:23 bytes>""#), "{out}");
    assert!(out.contains(r#""agent_name":"reviewer""#), "{out}");

    // Inside an array, too.
    let deep = r#"{"messages":[{"role":"user","prompt":"abc"},{"nested":{"prompt_response":"defg"}}]}"#;
    let out = phronesis_mcp::hook::redact_for_capture(deep).expect("valid JSON");
    assert!(out.contains(r#""prompt":"<redacted:3 bytes>""#), "{out}");
    assert!(out.contains(r#""prompt_response":"<redacted:4 bytes>""#), "{out}");

    // Gemini AfterAgent: `prompt_response` is the model's whole answer.
    let after = r#"{"hook_event_name":"AfterAgent","prompt":"hi","prompt_response":"a long answer","stop_hook_active":false}"#;
    let out = phronesis_mcp::hook::redact_for_capture(after).expect("valid JSON");
    assert!(!out.contains("a long answer"), "{out}");

    // Not valid JSON: it cannot be redacted, so it is not captured at all.
    assert_eq!(phronesis_mcp::hook::redact_for_capture("not json"), None);
}
```

The file's existing helpers are `run_hook_with_env(subcommand: &str, payload: &str, envs: &[(&str, &str)]) -> i32` and `read_capture(dir: &Path) -> Vec<serde_json::Value>`; `capture_raw_payload` stamps `phase` as `"pre"` / `"post"` (the value `read_payload` passes), not the subcommand name. The end-to-end case, spelled out because the spec makes it a hard requirement ("A test asserts no prompt text reaches `payloads.jsonl`"):

```rust
#[test]
fn prompt_text_never_reaches_the_capture_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let payload = r#"{"hook_event_name":"PreToolUse","session_id":"s","prompt":"zzz-secret","tool_name":"Read","tool_input":{"file_path":"src/main.rs"}}"#;
    let code = run_hook_with_env(
        "pre-check",
        payload,
        &[("PHRONESIS_CAPTURE_DIR", dir.path().to_str().expect("utf8"))],
    );
    assert_eq!(code, 0, "capture must not change hook behavior");
    let raw = std::fs::read_to_string(dir.path().join("payloads.jsonl")).expect("capture file");
    assert!(!raw.contains("zzz-secret"), "{raw}");
    assert!(raw.contains("<redacted:10 bytes>"), "{raw}");
    let records = read_capture(dir.path());
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["phase"], "pre");
    // Everything else is still captured verbatim.
    assert_eq!(records[0]["raw"]["tool_name"], "Read");
    assert_eq!(records[0]["raw"]["session_id"], "s");
}
```

The new `HookPayload` fields need their own parse test, because a misspelled serde
name yields `None` silently and every test in this plan still passes while Plans 2
and 3 lose correlation entirely. `HookPayload` is `pub(super)`, so this is a unit
test inside `hook/mod.rs`'s `#[cfg(test)] mod tests`, not an integration test:

```rust
    #[test]
    fn hook_payload_parses_the_correlation_fields() {
        let p: HookPayload = serde_json::from_str(
            r#"{"tool_name":"Bash","tool_input":{"command":"ls"},
                "session_id":"s-1","tool_use_id":"tu-1",
                "hook_event_name":"PreToolUse","agent_id":"a-1"}"#,
        )
        .expect("parse");
        assert_eq!(p.session_id.as_deref(), Some("s-1"));
        assert_eq!(p.tool_use_id.as_deref(), Some("tu-1"));
        assert_eq!(p.hook_event_name.as_deref(), Some("PreToolUse"));
        assert_eq!(p.agent_id.as_deref(), Some("a-1"));

        let bare: HookPayload =
            serde_json::from_str(r#"{"tool_name":"Bash","tool_input":{}}"#).expect("parse");
        assert!(bare.session_id.is_none());
        assert!(bare.tool_use_id.is_none());
        assert!(bare.hook_event_name.is_none());
        assert!(bare.agent_id.is_none());
    }
```

Existing `tests/payload_capture.rs` assertions read the capture through parsed JSON
(`records[0]["raw"]["session_id"]`), never as raw bytes, so re-serializing inside
`redact_for_capture` breaks none of them.

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

/// Keys whose string value is free human or model text and must never reach
/// `payloads.jsonl`, which `docs/payload-corpus-promotion.md` copies into a
/// committed tree. `prompt` covers Claude/Codex `UserPromptSubmit`, Gemini
/// `BeforeAgent`, and Gemini `invoke_agent`'s `tool_input.prompt`;
/// `prompt_response` covers Gemini `AfterAgent`; `last_assistant_message`
/// covers Codex `SubagentStop`.
pub(crate) const REDACTED_KEYS: [&str; 3] = ["prompt", "prompt_response", "last_assistant_message"];

/// Replace every `REDACTED_KEYS` string value, **at any depth**, with
/// `"<redacted:N bytes>"`. `None` when `raw` is not valid JSON: it cannot be
/// redacted, so the caller writes nothing rather than a verbatim copy.
///
/// The placeholder's `N` is the original byte length, which deliberately
/// reveals it; the spec records that as a decision.
pub fn redact_for_capture(raw: &str) -> Option<String> {
    fn walk(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(obj) => {
                for (k, val) in obj.iter_mut() {
                    if REDACTED_KEYS.contains(&k.as_str())
                        && let serde_json::Value::String(s) = val
                    {
                        *val = serde_json::Value::String(format!("<redacted:{} bytes>", s.len()));
                    } else {
                        walk(val);
                    }
                }
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(walk),
            _ => {}
        }
    }
    let mut v = serde_json::from_str::<serde_json::Value>(raw).ok()?;
    walk(&mut v);
    serde_json::to_string(&v).ok()
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
    let Some(redacted) = redact_for_capture(raw) else {
        // Stdin that is not valid JSON cannot be redacted, so it is not written
        // at all. One stderr line names the event; the payload itself is not
        // echoed, because the reason it failed to parse may be that it is
        // truncated free text.
        eprintln!("phronesis: capture skipped for {phase}: stdin is not valid JSON");
        return;
    };
    let record = serde_json::json!({
        "ts": unix_secs_now(),
        "phase": phase,
        // Already a redacted `Value` round-trip, so this parse cannot fail.
        "raw": serde_json::from_str::<serde_json::Value>(&redacted)
            .unwrap_or(serde_json::Value::Null),
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

- [ ] **Step 1: Write the failing tests** in `tests/kalpa_integration.rs`

`run` uses `security::project_root()`, which honours `PHRONESIS_PROJECT_ROOT` and
otherwise falls back to the process cwd, so the helper pins both:

```rust
use std::path::Path;
use std::process::{Command, Output};

/// Run `phr-mcp` against `root` as the project root.
fn run_phr(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(args)
        .env("PHRONESIS_PROJECT_ROOT", root)
        .current_dir(root)
        .output()
        .expect("spawn phr-mcp")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

#[test]
fn kalpa_start_show_end_round_trip_and_events() {
    let d = tempfile::tempdir().unwrap();
    let out = run_phr(d.path(), &["kalpa", "start", "lifecycle-events"]);
    assert!(out.status.success());
    let show = run_phr(d.path(), &["kalpa"]);
    assert!(stdout(&show).contains("kalpa: lifecycle-events"));
    let journal = std::fs::read_to_string(d.path().join(".phronesis/journey/events.jsonl")).unwrap();
    assert!(journal.contains(r#""kind":"kalpa_start""#)); assert!(journal.contains(r#""kalpa":"lifecycle-events""#));
    let end = run_phr(d.path(), &["kalpa", "end"]);
    assert!(end.status.success());
    // The closing record is attributed to the kalpa it closes.
    let last: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(d.path().join(".phronesis/journey/events.jsonl"))
            .unwrap().lines().next_back().unwrap(),
    ).unwrap();
    assert_eq!(last["kind"], "kalpa_end");
    assert_eq!(last["kalpa"], "lifecycle-events");
    assert!(
        last["tags"].as_array().unwrap().iter().any(|t| t == "kalpa:lifecycle-events"),
        "{last}"
    );
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

#[test]
fn kalpa_show_with_none_open_fails_and_starting_a_second_ends_the_first() {
    let d = tempfile::tempdir().unwrap();
    let show = run_phr(d.path(), &["kalpa"]);
    assert!(!show.status.success());
    assert!(String::from_utf8_lossy(&show.stderr).contains("no kalpa open"));

    assert!(run_phr(d.path(), &["kalpa", "start", "a"]).status.success());
    let second = run_phr(d.path(), &["kalpa", "start", "b"]);
    assert!(second.status.success());
    let text = stdout(&second);
    assert!(text.contains("ended kalpa a"), "{text}");
    assert!(text.contains("started kalpa b"), "{text}");
    assert_eq!(phronesis_mcp::lifecycle::state::read_kalpa(d.path()).unwrap().name, "b");
}

/// `header_line` takes `now` so the stale marker is testable without waiting a
/// month. This is the only caller that exercises the 30-day branch.
#[test]
fn header_line_marks_a_month_old_kalpa_stale() {
    use phronesis_mcp::lifecycle::kalpa_cli::header_line;
    use phronesis_mcp::lifecycle::state::{Kalpa, write_kalpa};
    let d = tempfile::tempdir().unwrap();
    let now = 1_800_000_000u64;
    write_kalpa(d.path(), &Kalpa { name: "old-one".into(), started_ts: now - 31 * 24 * 3600 });
    let line = header_line(d.path(), now).expect("a header");
    assert!(line.contains("kalpa: old-one"), "{line}");
    assert!(line.contains("(stale? run phr-mcp kalpa end)"), "{line}");

    write_kalpa(d.path(), &Kalpa { name: "new-one".into(), started_ts: now - 3600 });
    assert!(!header_line(d.path(), now).unwrap().contains("stale"));
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

/// Records `kalpa_end` **while the kalpa is still open**, then clears it, so the
/// boundary record carries `kalpa: "<name>"` and the `kalpa:<name>` tag like
/// every other record in the kalpa (spec §"Naming the kalpa": "Every lifecycle
/// record written while a kalpa is open carries `kalpa`"). Clearing first would
/// leave the one record that closes the theme unattributable to it, which is
/// exactly the record `phr-mcp kalpa show` needs to find the boundary.
fn end_open(root: &Path) -> Option<String> {
    let k = state::read_kalpa(root)?;
    record(root, LifecycleEvent::new(Kind::KalpaEnd, Host::Cli));
    state::clear_kalpa(root);
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

Both boundary records are stamped with the kalpa name: `kalpa_start` records
after `write_kalpa`, `kalpa_end` records before `clear_kalpa`. `record` reads the
kalpa file itself, so neither needs an `extra` field for the name.

This is exactly the write order spec §"Naming the kalpa" gives for
`kalpa start <other>`: record `kalpa_end` tagged with the **old** name (which
`end_open` does while the old file is still in place), replace the file, record
`kalpa_start` tagged with the **new** name. `kalpa_end` is stamped *before*
clearing, so the one record that closes a theme is attributable to it — which is
the record `phr-mcp kalpa show` needs to find the boundary.

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

### Task 12: Spec amendments and changelog

**Files:**
- Modify: `docs/specs/SPEC-journey-facts.md` (§"The journal record", after the "One line per executed tool call" paragraph, around line 161)
- Modify: `docs/payload-corpus-promotion.md`
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

- [ ] **Step 2: Amend `docs/payload-corpus-promotion.md`**

Task 10 changed what the capture file holds, and the promotion doc describes it
as a verbatim tee. Add, under the section that describes `payloads.jsonl`:

```markdown
> **From 0.35.0 the capture is a redacted re-serialization, not a verbatim
> tee.** `capture_raw_payload` parses stdin, replaces the value of any key named
> `prompt`, `prompt_response`, or `last_assistant_message` — at any depth — with
> `"<redacted:N bytes>"` where `N` is the original byte length, and serializes
> the result. Key order therefore follows `serde_json`'s object ordering rather
> than the host's, and stdin that is not valid JSON is not captured at all (one
> stderr line names the event). Everything else is captured unchanged, and
> `phr-mcp scrub-payload` still runs afterwards as the second stage.
```

- [ ] **Step 3: Add the changelog entry**

Under `## [Unreleased]` / `### Added`:

```markdown
- **Lifecycle events, foundation.** `JournalRecord` v2 with optional `kind`,
  `mode`, `host`, `turn`, `agent`, `agent_type`, `kalpa`; the derive pass
  computes positional windows on tool records only, so existing `journey_*`
  rules are unchanged; built-in `lifecycle:*` and `kalpa:*` selectors; new
  `lifecycle` module (`LifecycleEvent`, locked state files, `classify_prompt`,
  `detect_commit`, `scrub_prompt`); `phr-mcp kalpa start|end|show`; prompt
  text is redacted from `PHRONESIS_CAPTURE_DIR` captures, recursively, so a
  nested `tool_input.prompt` is covered too. No host emits lifecycle events yet
  (adapters follow).
```

- [ ] **Step 4: Full verification**

Run: `cargo fmt --all -- --check && cargo clippy --all-targets -p phronesis-mcp -- -D warnings && cargo test -p phronesis-mcp 2>&1 | tail -30`
Expected: clean, all green.

- [ ] **Step 5: Commit**

```bash
git add docs/specs/SPEC-journey-facts.md docs/payload-corpus-promotion.md CHANGELOG.md
git commit -m "docs: journey spec amendment, capture-redaction note, changelog"
```

---

## Self-review

- **Spec coverage (steps 1a/1b):** journal v2 + v1-reader pin (T1), projection + closed-set selectors + reserved namespaces + iterative read bound + `Nc` warning (T2), compaction retaining commit/interrupt/correction/kalpa (T3), shared type with sanitized `agent_type`, top-level-only `lifecycle:intervention` and the closed `extra` vocabulary (T4), state files with lock, keyed multiset `inflight`, pinned FNV-1a, classification-only TTL, `turn.sid`, atomic session replace (T5), `scrub_prompt` with three unconditional regexes (T6), `record` + fail-closed `prompt_text` + `correction_text` (T7), classify from `turn.last_event` with transcript-before-inflight (T8), `detect_commit` + `HeadProbe` + detection markers (T9), `HookPayload` widening + comment fix + recursive capture redaction (T10), kalpa subcommand + header + validate-at-read (T11), spec amendments + changelog (T12). Not in this plan by design: pre/post inflight push/pop and `invoke_agent` derivation (Plan 2), Codex payload fields (Plan 3), Gemini registrations (Plan 4), stats/metrics/CLI rendering and `kalpa show` counts (Plan 5).
- **Type consistency:** `LifecycleEvent`, `Stamped`, `PromptText`, `Kind`, `Mode`, `Host` are defined once in T4 and used by T7, T8, T11 with the same names. `Inflight`, `OpenAgent`, `Turn`, `Kalpa`, `PromptContext`, `Classification`, `InterruptSource` are defined in T5/T8. `Commit` in T9. `redact_for_capture` in T10.
- **Resolved against the revised spec.** The earlier draft evaluated a Codex
  `Hook` branch before the closed-turn short-circuit, because the closed-turn
  test came first and the `Interrupt` hook closes the turn. The spec now makes
  step 1 read `turn.last_event == "interrupt"` **before** declaring `fresh`, on
  every host, which resolves the contradiction at its source: the turn is closed
  *and* the last event was an interrupt, so the classifier says `correction`
  without any host-specific branch and without scanning the journal. The
  `InterruptSource::Hook` variant and `state::last_lifecycle_kind` are gone;
  Plans 2 and 3 no longer call either.
- **One spec line this plan could not implement as written, for the spec
  owner.** §"Success signal: commit" says a timed-out pre-time `git rev-parse`
  "stores `detection: "timeout"` on the entry so the miss is auditable rather
  than silent", and that a host sending no `command_exit` makes "the entry record
  `detection: "no_exit_code"`". The `inflight` entry is transient — it is popped
  and dropped at post-check — so a marker on it is auditable only for as long as
  the tool runs. This plan stores the marker on the entry (T5's `Inflight.detection`,
  T9's `DETECTION_*` constants) and keeps `detection` in the closed `extra`
  vocabulary so a record can carry it, and Plan 2 Task 4 prints one
  `phronesis:`-prefixed stderr line when detection is disabled for a
  pre-filter-matching shell call. It does **not** write a lifecycle record for a
  non-commit, because no `kind` in the event model fits one. If the spec wants a
  durable audit trail it needs to name the record that carries it.
- **Placeholders:** none. T11's `Show` intentionally prints only the header until Plan 5 adds counts, and says so. T3's compaction test and T2's `validate_selectors` loop are written out in full against the real `maybe_compact` / `validate_selectors` bodies.
- **Type consistency after the revision:** `PromptContext` has five fields, not six (`last_journal_kind` is gone); `InterruptSource` has three variants, not four; `Inflight` has six fields (`detection` added); `Turn` has five (`sid` added); `close_turn` returns `bool`; `redact_for_capture` returns `Option<String>`; `state::clear_session` does not exist. Plans 2 and 3 are updated to match in the same pass.
- **Ownership:** this plan is the sole owner of `hook/mod.rs` for the feature. `HookPayload`'s new fields, `redact_for_capture`, `capture_raw_payload`'s `pub(crate)` visibility, and the corrected `tool_output` comment all land in T10. Plans 2 and 3 consume them and must not re-make those changes; Plan 2 adds only the single `mod lifecycle_wiring;` line to that file.

# Lifecycle Events — Plan 6: Work items and governed throughput (spec node 6)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make one piece of work answerable — which spec it was built to, which rules were evaluated against it, what evidence it accumulated, where a human stepped in, and whether it landed a commit — and roll that up into a kalpa's governed throughput.

**Architecture:** No new storage. The work item *is* the existing `outcomes::subject` work unit. `phr-mcp unit start/end` sets and clears that subject and brackets it with two new lifecycle kinds (`unit_start` / `unit_end`) whose `extra` carries `spec`, `unit_id` and `implicit`. `pre_check` / `post_check` action-log entries gain the open `subject`, which is what finally makes "which rules were evaluated for this item" a join rather than a guess. `lifecycle/unit_report.rs` performs that join over the journal and the action log (plus its one rotated predecessor) and renders it; `stats::aggregate_lifecycle` learns the same join in aggregate and gains three lines in `kalpa show` and `stats`.

**Tech Stack:** Rust 2024 edition (rust-version 1.90), serde/serde_json, chrono (already a `phronesis-mcp` dependency), clap. No new dependencies.

**Spec:** `docs/specs/SPEC-agent-lifecycle-events.md` (revised 2026-09-19) — §"Outcomes and kalpas / Work items and governed throughput", and §Rollout node 6. Read it first; the plan argues from it.

**Depends on:** Plans 1, 2, 3, 4 **and 5**, all merged. This is the last node in the rollout graph. It consumes, unchanged:

- `lifecycle::event::{Kind, Mode, Host, LifecycleEvent, Stamped, PromptText, EXTRA_KEYS}` (Plan 1 Task 4)
- `lifecycle::record::{record, correction_text}` (Plan 1 Task 7)
- `lifecycle::state::read_kalpa` (Plan 1 Task 5), `lifecycle::kalpa_cli::{header_line, KalpaCmd}` (Plan 1 Task 11)
- `stats::{LifecycleOpts, LifecycleStats, aggregate_lifecycle, render_lifecycle, retention_line, humanize_duration, render_json_with_lifecycle}` (Plan 5 Task 1)
- `lifecycle::kalpa_cli`'s private `report` fn (Plan 5 Task 3)

**Files this plan owns exclusively:**

- `crates/phronesis-mcp/src/lifecycle/unit_cli.rs` (create)
- `crates/phronesis-mcp/src/lifecycle/unit_report.rs` (create)
- `crates/phronesis-mcp/tests/unit_cli_integration.rs` (create)

**Files this plan edits that earlier plans own** (all merged by now; each edit is additive and named per task):

- `crates/phronesis-mcp/src/lifecycle/event.rs` (Plan 1) — two `Kind` variants, three `EXTRA_KEYS` entries.
- `crates/phronesis-mcp/src/lifecycle/mod.rs` (Plan 1) — two `pub mod` lines.
- `crates/phronesis-mcp/src/lifecycle/kalpa_cli.rs` (Plans 1, 5) — the `report` fn's `read_recent` call.
- `crates/phronesis-mcp/src/outcomes/subject.rs` — `clear`.
- `crates/phronesis-mcp/src/hook/mod.rs` (Plan 1) — `LogEventInput.subject`, `log_hook_event`.
- `crates/phronesis-mcp/src/hook/pre.rs`, `src/hook/post.rs` (Plan 2) — five call sites.
- `crates/phronesis-mcp/src/main.rs` (Plans 1, 2, 5) — one `Command` variant, one dispatch arm, one `handle_stats` line.
- `crates/phronesis-mcp/src/stats.rs` (Plan 5) — `LifecycleStats` fields, `aggregate_lifecycle`, `render_lifecycle`, `render_json_with_lifecycle`.
- `crates/phronesis-mcp/tests/hook_integration.rs`, `tests/kalpa_integration.rs` — appended.
- `CHANGELOG.md`.

## Global Constraints

- No new crate dependencies.
- **`extra` stays a closed vocabulary.** This plan adds exactly three keys — `spec`, `unit_id`, `implicit` — and nothing else. `with_extra`'s `debug_assert!` is the guard; a key outside `EXTRA_KEYS` is a plan failure.
- **A second surface may print prompt text, and only a second one.** Plan 5's constraint was "prompt text appears in exactly one output: `phr-mcp journey --corrections`". The spec's §"Work items" amends that: `phr-mcp unit show` prints intervention text too. Both go through `lifecycle::record::correction_text`, so the `prompt_text: "none"` switch hides them at read time. No third surface, and never in `stats`, `kalpa show`, `get_journey`, a context render, or a metric label.
- **Every new reporting surface is read-only and fail-open**: an unreadable log or journal yields an empty section, never an error.
- **A lifecycle write never fails a CLI command that did real work.** `record` already swallows its errors; `unit start` reports the subject change even if the record was lost.
- `--spec` is repo-relative, must exist, must resolve inside the project root, and must contain no `..` component. `security::resolve_safe_path` enforces the last three; absolute paths are rejected explicitly so the recorded value stays repo-relative.
- A work item is **completed** when at least one `commit` record carries its `subject`; **governed** when it is completed, at least one `pre_check`/`post_check` entry carries its `subject`, and the confidence band at its last commit is not `low`.
- `main.rs` `Command` placement convention (Plan 1 Task 11): the lifecycle group sits immediately after `CodexHook`, alphabetical among itself. After Plans 1 and 2 the order is `CodexHook`, `ClaudeHook`, `Kalpa`; this plan appends `Unit` after `Kalpa`.
- Conventional-commit messages. Run `cargo fmt` and `cargo clippy --all-targets -p phronesis-mcp -- -D warnings` before every commit.
- Machine note: if `cargo` fails with "You have not agreed to the Xcode license", stop and report; the human must run `sudo xcodebuild -license accept`.

## File structure

| path | responsibility |
|---|---|
| `crates/phronesis-mcp/src/lifecycle/event.rs` (modify) | `Kind::UnitStart` / `Kind::UnitEnd`; `EXTRA_KEYS` gains `spec`, `unit_id`, `implicit` |
| `crates/phronesis-mcp/src/outcomes/subject.rs` (modify) | `clear` — the explicit close, beside the implicit `settle` |
| `crates/phronesis-mcp/src/lifecycle/unit_cli.rs` (create) | `UnitCmd`, `run` — `unit start` / `unit end` / `unit show` |
| `crates/phronesis-mcp/src/lifecycle/unit_report.rs` (create) | `UnitReport`, `build`, `render`, `render_json` — the journal × action-log join for one subject |
| `crates/phronesis-mcp/src/hook/mod.rs` (modify) | `LogEventInput.subject`, written onto `pre_check` / `post_check` entries |
| `crates/phronesis-mcp/src/hook/pre.rs`, `post.rs` (modify) | fill `subject` from `outcomes::subject::current` |
| `crates/phronesis-mcp/src/stats.rs` (modify) | work-item / governed counts and `interventions_per_work_item` in `LifecycleStats`, three lines in `render_lifecycle` |
| `crates/phronesis-mcp/src/main.rs` (modify) | `Unit` subcommand; `handle_stats` reads unfiltered entries |
| `crates/phronesis-mcp/src/lifecycle/kalpa_cli.rs` (modify) | `report` reads unfiltered entries so hook entries reach the aggregator |
| `crates/phronesis-mcp/tests/unit_cli_integration.rs` (create) | `unit start/end/show`, spec validation, text and `--json` rendering |
| `crates/phronesis-mcp/tests/hook_integration.rs` (modify) | `subject` present with an open unit, absent without |
| `crates/phronesis-mcp/tests/kalpa_integration.rs` (modify) | the three new `kalpa show` lines |
| `CHANGELOG.md` (modify) | `## [Unreleased]` → `### Added` |

---

### Task 1: `unit_start` / `unit_end` kinds and the three new `extra` keys

**Files:**
- Modify: `crates/phronesis-mcp/src/lifecycle/event.rs` (the `Kind` enum, `Kind::as_str`, `EXTRA_KEYS`)
- Test: unit tests inside `event.rs`

**Interfaces:**
- Consumes: Plan 1 Task 4's `Kind`, `LifecycleEvent`, `Stamped`, `PromptText`, `EXTRA_KEYS`.
- Produces: `Kind::UnitStart` (`as_str` → `"unit_start"`, `tag()` → `"lifecycle:unit_start"`), `Kind::UnitEnd` (`"unit_end"`, `"lifecycle:unit_end"`), and `pub const EXTRA_KEYS: [&str; 12]` with `"spec"`, `"unit_id"`, `"implicit"` appended.

- [ ] **Step 1: Write the failing unit tests** — append inside `event.rs`'s `#[cfg(test)] mod tests`

```rust
/// The two selectors spec §"Event model" lists for work items. They are in the
/// closed set `validate_selectors` exempts, so a rule may scope to them.
#[test]
fn unit_kinds_have_their_spec_names_and_tags() {
    assert_eq!(Kind::UnitStart.as_str(), "unit_start");
    assert_eq!(Kind::UnitEnd.as_str(), "unit_end");
    assert_eq!(Kind::UnitStart.tag(), "lifecycle:unit_start");
    assert_eq!(Kind::UnitEnd.tag(), "lifecycle:unit_end");
    let e = LifecycleEvent::new(Kind::UnitStart, Host::Cli);
    assert_eq!(e.tags(Some("demo")), vec!["lifecycle:unit_start", "kalpa:demo"]);
    // A unit boundary is not a prompt, so it can never be an intervention.
    assert!(!e.tags(None).iter().any(|t| t.contains("intervention")));
}

/// `extra` gains exactly three keys and no more (spec §"Work items / Storage").
#[test]
fn extra_vocabulary_gains_spec_unit_id_and_implicit() {
    for k in ["spec", "unit_id", "implicit"] {
        assert!(EXTRA_KEYS.contains(&k), "{k} must be in the closed vocabulary");
    }
    assert_eq!(EXTRA_KEYS.len(), 12, "three added, nothing else");
    for forbidden in ["prompt_response", "last_assistant_message", "transcript_path", "spec_hash"] {
        assert!(!EXTRA_KEYS.contains(&forbidden), "{forbidden}");
    }
}

/// The `unit_start` projection: `unit_id` and `spec` flatten into the action
/// log, and `subject` is the same id, which is what the report joins on.
#[test]
fn unit_start_log_entry_carries_unit_id_spec_and_subject() {
    let e = LifecycleEvent::new(Kind::UnitStart, Host::Cli)
        .with_extra("unit_id", "unit-42")
        .with_extra("spec", "docs/specs/SPEC-agent-lifecycle-events.md");
    let s = Stamped {
        ts: 7,
        sid: "s-a".into(),
        seq: 3,
        kalpa: Some("k".into()),
        subject: Some("unit-42".into()),
    };
    let v = serde_json::to_value(e.to_log_entry(&s, PromptText::Full)).unwrap();
    assert_eq!(v["kind"], "lifecycle");
    assert_eq!(v["event"], "unit_start");
    assert_eq!(v["unit_id"], "unit-42");
    assert_eq!(v["spec"], "docs/specs/SPEC-agent-lifecycle-events.md");
    assert_eq!(v["subject"], "unit-42");
    let rec = e.to_journal_record(&s);
    assert_eq!(rec.kind.as_deref(), Some("unit_start"));
    assert_eq!(rec.subject.as_deref(), Some("unit-42"));
    assert_eq!(rec.tool, "__lifecycle");
}

/// `implicit` marks a unit that was never explicitly started — the flag that
/// keeps the `explicit` / `implicit` split in `kalpa show` honest.
#[test]
fn unit_end_can_be_marked_implicit() {
    let e = LifecycleEvent::new(Kind::UnitEnd, Host::Cli)
        .with_extra("unit_id", "unit-9")
        .with_extra("implicit", true);
    let s = Stamped { ts: 1, sid: "s".into(), seq: 1, kalpa: None, subject: Some("unit-9".into()) };
    let v = serde_json::to_value(e.to_log_entry(&s, PromptText::Full)).unwrap();
    assert_eq!(v["implicit"], true);
    assert_eq!(v["event"], "unit_end");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --lib lifecycle::event 2>&1 | tail -20`
Expected: FAIL — `no variant named UnitStart found for enum Kind`.

- [ ] **Step 3: Implement** — in `event.rs`

Extend the enum (the variants go last so the `serde` discriminants of existing kinds are untouched in any future `repr` work):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    SubagentStart,
    SubagentStop,
    Prompt,
    Interrupt,
    Stop,
    Commit,
    KalpaStart,
    KalpaEnd,
    /// A human named a work item: `phr-mcp unit start`.
    UnitStart,
    /// The work item closed: `phr-mcp unit end`, or the next `unit start`.
    UnitEnd,
}
```

and the two `as_str` arms:

```rust
            Kind::KalpaStart => "kalpa_start",
            Kind::KalpaEnd => "kalpa_end",
            Kind::UnitStart => "unit_start",
            Kind::UnitEnd => "unit_end",
```

and the vocabulary:

```rust
pub const EXTRA_KEYS: [&str; 12] = [
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
    // Work items (spec §"Work items and governed throughput" / Storage). The
    // spec pointer is a repo-relative path, not a hash: if the spec changes
    // after the unit starts, the report shows the path only.
    "spec",
    // The work-unit id, duplicated out of `subject` so a log reader does not
    // have to know that `subject` and the unit id are the same thing.
    "unit_id",
    // `true` on a `unit_end` for a unit that was never explicitly started.
    "implicit",
];
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --lib lifecycle::event 2>&1 | tail -20`
Expected: PASS, including every pre-existing `event.rs` test.

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy --all-targets -p phronesis-mcp -- -D warnings
git add crates/phronesis-mcp/src/lifecycle/event.rs
git commit -m "feat(lifecycle): unit_start/unit_end kinds and the spec/unit_id/implicit extras"
```

---

### Task 2: `phr-mcp unit start` / `unit end`

**Files:**
- Modify: `crates/phronesis-mcp/src/outcomes/subject.rs` (add `clear` beside `settle`)
- Create: `crates/phronesis-mcp/src/lifecycle/unit_cli.rs`
- Modify: `crates/phronesis-mcp/src/lifecycle/mod.rs` (`pub mod unit_cli;`), `crates/phronesis-mcp/src/main.rs` (`Command::Unit` + dispatch)
- Test: `crates/phronesis-mcp/tests/unit_cli_integration.rs` (new)

**Interfaces:**
- Consumes: `outcomes::subject::{current, set, open}`; `security::{project_root, resolve_safe_path}`; `lifecycle::event::{Host, Kind, LifecycleEvent}`; `lifecycle::record::record`; `action_log::{default_path, read_recent, ReadOpts}`.
- Produces:

```rust
// outcomes/subject.rs
pub fn clear(root: &Path) -> Result<(), SubjectError>;

// lifecycle/unit_cli.rs
pub enum UnitCmd {                                   // clap Subcommand
    Start { id: Option<String>, spec: Option<String> },   // spec is `--spec <PATH>`
    End,
    Show { id: Option<String>, json: bool },              // json is `--json`
}
pub fn run(root: &Path, cmd: UnitCmd) -> anyhow::Result<String>;   // text to print
```

`Show` lands in Task 4; this task implements it as a delegation to `unit_report`, which does not exist yet, so Task 2's `Show` arm returns the id line only and Task 4 replaces the arm. To avoid shipping a stub, Task 2 does **not** register `Show` at all: `UnitCmd` here has only `Start` and `End`, and Task 4 adds the `Show` variant together with its implementation. The `Interfaces` block above is the shape after Task 4; the code below is Task 2's.

- [ ] **Step 1: Write the failing tests** in `crates/phronesis-mcp/tests/unit_cli_integration.rs`

```rust
//! `phr-mcp unit` CLI — explicit work items with a spec pointer.
//! See `docs/specs/SPEC-agent-lifecycle-events.md` §"Work items and governed
//! throughput".

use std::path::Path;
use std::process::{Command, Output};

/// Run `phr-mcp` against `root` as the project root. `run` honours
/// `PHRONESIS_PROJECT_ROOT` via `security::project_root`, and `current_dir`
/// pins the fallback too.
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

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

/// Every lifecycle entry in the action log, oldest first.
fn lifecycle_entries(root: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(root.join(".phronesis/log.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["kind"] == "lifecycle")
        .collect()
}

fn write_spec(root: &Path, rel: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, "# spec\n").unwrap();
}

#[test]
fn unit_start_sets_the_subject_and_records_unit_start_with_the_spec() {
    let d = tempfile::tempdir().unwrap();
    write_spec(d.path(), "docs/specs/SPEC-thing.md");
    let out = run_phr(
        d.path(),
        &["unit", "start", "item-1", "--spec", "docs/specs/SPEC-thing.md"],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("started work unit item-1"), "{}", stdout(&out));

    assert_eq!(
        std::fs::read_to_string(d.path().join(".phronesis/outcomes/current")).unwrap(),
        "item-1"
    );
    let entries = lifecycle_entries(d.path());
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0]["event"], "unit_start");
    assert_eq!(entries[0]["unit_id"], "item-1");
    assert_eq!(entries[0]["subject"], "item-1");
    assert_eq!(entries[0]["spec"], "docs/specs/SPEC-thing.md");
    assert_eq!(entries[0]["host"], "cli");

    let journal =
        std::fs::read_to_string(d.path().join(".phronesis/journey/events.jsonl")).unwrap();
    assert!(journal.contains(r#""kind":"unit_start""#), "{journal}");
    assert!(journal.contains(r#""subject":"item-1""#), "{journal}");
}

#[test]
fn unit_start_without_an_id_mints_one() {
    let d = tempfile::tempdir().unwrap();
    let out = run_phr(d.path(), &["unit", "start"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let id = std::fs::read_to_string(d.path().join(".phronesis/outcomes/current")).unwrap();
    assert!(id.starts_with("unit-"), "{id}");
    assert!(stdout(&out).contains(&id), "the minted id is reported: {}", stdout(&out));
}

/// A spec pointer that does not exist, escapes the root, or is absolute is
/// rejected *before* the subject moves: a unit must never point at a spec the
/// reader cannot open.
#[test]
fn unit_start_rejects_a_bad_spec_and_leaves_the_subject_untouched() {
    let d = tempfile::tempdir().unwrap();
    for bad in [
        "docs/specs/NOPE.md",
        "../outside.md",
        "/etc/passwd",
    ] {
        let out = run_phr(d.path(), &["unit", "start", "x", "--spec", bad]);
        assert!(!out.status.success(), "{bad} should be rejected");
        assert!(stderr(&out).contains("--spec"), "{bad}: {}", stderr(&out));
    }
    assert!(
        !d.path().join(".phronesis/outcomes/current").exists(),
        "a rejected start opens no unit"
    );
    assert!(lifecycle_entries(d.path()).is_empty(), "and records nothing");
}

/// "Starting a unit while one is open ends the open one first" (spec
/// §"Work items", 1). The closing record must carry the *old* subject.
#[test]
fn unit_start_ends_an_open_unit_first() {
    let d = tempfile::tempdir().unwrap();
    assert!(run_phr(d.path(), &["unit", "start", "first"]).status.success());
    let out = run_phr(d.path(), &["unit", "start", "second"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("ended work unit first"), "{text}");
    assert!(text.contains("started work unit second"), "{text}");

    let entries = lifecycle_entries(d.path());
    let events: Vec<&str> = entries.iter().map(|e| e["event"].as_str().unwrap()).collect();
    assert_eq!(events, ["unit_start", "unit_end", "unit_start"], "{entries:?}");
    assert_eq!(entries[1]["subject"], "first", "the end record carries the old subject");
    assert_eq!(entries[1]["unit_id"], "first");
    assert!(
        entries[1].get("implicit").is_none(),
        "`first` was started explicitly, so it is not implicit: {:?}",
        entries[1]
    );
    assert_eq!(entries[2]["subject"], "second");
}

#[test]
fn unit_end_records_and_clears_the_subject() {
    let d = tempfile::tempdir().unwrap();
    assert!(run_phr(d.path(), &["unit", "start", "item-1"]).status.success());
    let out = run_phr(d.path(), &["unit", "end"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("ended work unit item-1"), "{}", stdout(&out));
    assert!(
        !d.path().join(".phronesis/outcomes/current").exists(),
        "the subject is cleared"
    );
    let entries = lifecycle_entries(d.path());
    assert_eq!(entries.last().unwrap()["event"], "unit_end");
    assert_eq!(entries.last().unwrap()["subject"], "item-1");
}

/// An implicit unit — one minted by the outcomes path, never `unit start`ed —
/// is closed with `implicit: true` so the report can split the two populations.
#[test]
fn unit_end_marks_a_never_started_unit_implicit() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join(".phronesis/outcomes")).unwrap();
    std::fs::write(d.path().join(".phronesis/outcomes/current"), "unit-999").unwrap();
    let out = run_phr(d.path(), &["unit", "end"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let entries = lifecycle_entries(d.path());
    assert_eq!(entries.last().unwrap()["event"], "unit_end");
    assert_eq!(entries.last().unwrap()["implicit"], true);
}

#[test]
fn unit_end_with_nothing_open_fails() {
    let d = tempfile::tempdir().unwrap();
    let out = run_phr(d.path(), &["unit", "end"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("no work unit open"), "{}", stderr(&out));
}
```

Add the unit test for `clear` inside `outcomes/subject.rs`'s `mod tests`:

```rust
    #[test]
    fn clear_closes_the_open_subject_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        set(dir.path(), "item-1").unwrap();
        clear(dir.path()).unwrap();
        assert!(current(dir.path()).is_none());
        assert!(clear(dir.path()).is_ok(), "clearing nothing is not an error");
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --test unit_cli_integration 2>&1 | tail -20`
Expected: FAIL — `error: unrecognized subcommand 'unit'`, so every assertion on `status.success()` fails.

- [ ] **Step 3: Implement**

`outcomes/subject.rs`, directly after `settle`:

```rust
/// Clear the open subject — the explicit `phr-mcp unit end` path.
///
/// Identical in effect to [`settle`] and deliberately a separate name: `settle`
/// means "a build/test cycle closed this unit", `clear` means "a human said the
/// work item is done". Callers read differently, and a future change to either
/// meaning must not silently change the other.
pub fn clear(root: &Path) -> Result<(), SubjectError> {
    settle(root)
}
```

`lifecycle/mod.rs`, in alphabetical position among the `pub mod` lines (after `state`):

```rust
pub mod unit_cli;
```

`lifecycle/unit_cli.rs`:

```rust
//! `phr-mcp unit` — name the work item an agent is building, and close it.
//!
//! The work item *is* the existing `outcomes::subject` work unit (spec
//! §"Work items and governed throughput": "The work item is the existing work
//! unit"). This module adds the explicit bracket around it — a caller-chosen
//! id, a spec pointer, and the two lifecycle records that let a report find
//! the boundary — without introducing a second notion of "unit".

use std::path::Path;

use anyhow::{Context, bail};

use crate::action_log::{self, ReadOpts};
use crate::lifecycle::event::{Host, Kind, LifecycleEvent};
use crate::lifecycle::record::record;
use crate::outcomes::subject;
use crate::security;

#[derive(clap::Subcommand, Debug)]
pub enum UnitCmd {
    /// Open a work item (ends any open one first).
    Start {
        /// The work-item id. Omitted, a fresh `unit-<nanos>` id is minted.
        id: Option<String>,
        /// Repo-relative path to the spec this item is built to. Must exist.
        #[arg(long, value_name = "PATH")]
        spec: Option<String>,
    },
    /// Close the open work item.
    End,
}

/// Validate `--spec`: repo-relative, existing, inside the project root, no
/// `..`. `resolve_safe_path` enforces the last three (it canonicalizes, so
/// symlinks out of the tree are caught too); the absolute-path rejection is
/// ours, because the value we *record* must stay repo-relative — an absolute
/// path would leak a machine layout into the action log and would not resolve
/// on another checkout.
fn validate_spec(root: &Path, spec: &str) -> anyhow::Result<String> {
    if Path::new(spec).is_absolute() {
        bail!("--spec must be repo-relative, not `{spec}`");
    }
    security::resolve_safe_path(spec, root).with_context(|| format!("--spec `{spec}`"))?;
    Ok(spec.to_string())
}

/// Was `unit_id` opened by an explicit `unit start`? Read from the action log
/// (and its rotated predecessor) rather than tracked in a file: the spec says
/// "No new file", and the `unit_start` record is already the durable evidence.
/// A unit whose `unit_start` has rotated off reads as implicit, which is the
/// safe direction — it undercounts explicit units rather than claiming one.
fn was_started_explicitly(root: &Path, unit_id: &str) -> bool {
    action_log::read_recent(
        &action_log::default_path(root),
        &ReadOpts {
            kind: Some("lifecycle".to_string()),
            event: Some("unit_start".to_string()),
            ..ReadOpts::default()
        },
    )
    .unwrap_or_default()
    .iter()
    .any(|e| e.data.get("unit_id").and_then(|v| v.as_str()) == Some(unit_id))
}

/// Record `unit_end` for the open unit and clear the subject. Records **before**
/// clearing, so `record` stamps the closing record with the subject it closes —
/// the record a report joins on. Returns the id that was closed.
fn end_open(root: &Path) -> anyhow::Result<Option<String>> {
    let Some(id) = subject::current(root) else {
        return Ok(None);
    };
    let mut ev = LifecycleEvent::new(Kind::UnitEnd, Host::Cli).with_extra("unit_id", id.clone());
    if !was_started_explicitly(root, &id) {
        ev = ev.with_extra("implicit", true);
    }
    record(root, ev);
    subject::clear(root)?;
    Ok(Some(id))
}

pub fn run(root: &Path, cmd: UnitCmd) -> anyhow::Result<String> {
    match cmd {
        UnitCmd::Start { id, spec } => {
            // Validate first: a rejected spec must leave the open unit exactly
            // as it was, so a typo costs nothing.
            let spec = spec.map(|s| validate_spec(root, &s)).transpose()?;
            let ended = end_open(root)?;
            let id = match id {
                Some(id) => {
                    subject::set(root, &id)?;
                    id
                }
                None => subject::open(root)?,
            };
            let mut ev =
                LifecycleEvent::new(Kind::UnitStart, Host::Cli).with_extra("unit_id", id.clone());
            if let Some(s) = &spec {
                ev = ev.with_extra("spec", s.clone());
            }
            record(root, ev);
            let mut out = String::new();
            if let Some(e) = ended {
                out.push_str(&format!("ended work unit {e}\n"));
            }
            out.push_str(&format!("started work unit {id}"));
            if let Some(s) = &spec {
                out.push_str(&format!("   spec: {s}"));
            }
            Ok(out)
        }
        UnitCmd::End => match end_open(root)? {
            Some(id) => Ok(format!("ended work unit {id}")),
            None => bail!("no work unit open"),
        },
    }
}
```

`main.rs`, in the `Command` enum immediately after the `Kalpa` variant (the lifecycle group's alphabetical tail — `CodexHook`, `ClaudeHook`, `Kalpa`, `Unit`):

```rust
    /// Name the work item (unit) an agent is building, and report on it.
    Unit {
        #[command(subcommand)]
        cmd: phronesis_mcp::lifecycle::unit_cli::UnitCmd,
    },
```

and in the `match cli.command` dispatch, in the same position relative to `Command::Kalpa`:

```rust
        Command::Unit { cmd } => {
            let root = phronesis_mcp::security::project_root();
            let out = phronesis_mcp::lifecycle::unit_cli::run(&root, cmd)?;
            println!("{out}");
            Ok(())
        }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --test unit_cli_integration --test cli_smoke --lib outcomes::subject 2>&1 | tail -20`
Expected: PASS — 7 integration tests and the `clear` unit test.

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy --all-targets -p phronesis-mcp -- -D warnings
git add crates/phronesis-mcp/src/lifecycle/unit_cli.rs crates/phronesis-mcp/src/lifecycle/mod.rs \
        crates/phronesis-mcp/src/outcomes/subject.rs crates/phronesis-mcp/src/main.rs \
        crates/phronesis-mcp/tests/unit_cli_integration.rs
git commit -m "feat(cli): phr-mcp unit start/end with a validated spec pointer"
```

---

### Task 3: `subject` on `pre_check` / `post_check` log entries

**Files:**
- Modify: `crates/phronesis-mcp/src/hook/mod.rs:388-424` (`LogEventInput`, `log_hook_event`)
- Modify: `crates/phronesis-mcp/src/hook/pre.rs:206,221,232`, `crates/phronesis-mcp/src/hook/post.rs:181,202`
- Test: `crates/phronesis-mcp/tests/hook_integration.rs` (append)

**Interfaces:**
- Consumes: `outcomes::subject::current(root) -> Option<String>`; `security::project_root`.
- Produces: `LogEventInput` gains `pub(super) subject: Option<&'a str>`, written onto the entry as `"subject"` when `Some`. Every `pre_check` / `post_check` action-log entry now carries the open work unit, which is the join key `unit_report` and `aggregate_lifecycle` use.

Why the hook reads the subject rather than being handed it: `log_hook_event` is called from three points in `pre.rs` and two in `post.rs`, some of them on the exit-2 path after `process::exit` is imminent. One read at the top of each runner, before the branches, is one syscall on a path that already reads several files, and it cannot go stale within the call.

- [ ] **Step 1: Write the failing tests** — append to `tests/hook_integration.rs`

```rust
/// Read the last `pre_check` entry from a project's action log.
fn last_pre_check(dir: &Path) -> serde_json::Value {
    std::fs::read_to_string(dir.join(".phronesis/log.jsonl"))
        .expect("action log")
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["event"] == "pre_check")
        .next_back()
        .expect("a pre_check entry")
}

/// "`pre_check` / `post_check` action-log entries gain `subject` when a unit is
/// open. Today only journal records carry it, so 'which rules fired for this
/// work item' is not answerable from the log" (spec §"Work items", 2).
#[test]
fn pre_check_log_entry_carries_the_open_work_unit() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{"id":"policy","phase":"pre","priority":1,
            "when":[{"file_path_matches":"vendor/"}],
            "then":{"block":"vendored code is off limits"}}]}"#,
    );
    std::fs::create_dir_all(dir.path().join(".phronesis/outcomes")).unwrap();
    std::fs::write(dir.path().join(".phronesis/outcomes/current"), "item-7").unwrap();

    let payload = r#"{"tool_name":"Edit","tool_input":{"file_path":"src/lib.rs","old_string":"a","new_string":"b"}}"#;
    let (code, stderr) = run_hook_in("pre-check", payload, Some(dir.path()));
    assert_eq!(code, 0, "stderr: {stderr}");

    let entry = last_pre_check(dir.path());
    assert_eq!(entry["subject"], "item-7", "{entry}");
    assert_eq!(entry["exit"], 0);
}

/// No open unit → no `subject` key at all. An empty string or a `null` would
/// make every consumer special-case it; absence is the existing convention for
/// every other optional field on a log entry.
#[test]
fn pre_check_log_entry_omits_subject_when_no_unit_is_open() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{"id":"policy","phase":"pre","priority":1,
            "when":[{"file_path_matches":"vendor/"}],
            "then":{"block":"vendored code is off limits"}}]}"#,
    );
    let payload = r#"{"tool_name":"Edit","tool_input":{"file_path":"src/lib.rs","old_string":"a","new_string":"b"}}"#;
    let (code, stderr) = run_hook_in("pre-check", payload, Some(dir.path()));
    assert_eq!(code, 0, "stderr: {stderr}");
    let entry = last_pre_check(dir.path());
    assert!(entry.get("subject").is_none(), "{entry}");
}

/// The blocked path logs too, and must carry the subject: a block is exactly
/// the kind of rule evaluation the work-item report exists to show.
#[test]
fn a_blocked_pre_check_still_carries_the_subject() {
    let dir = tempfile::tempdir().unwrap();
    write_rules_file(
        dir.path(),
        r#"{"rules":[{"id":"policy","phase":"pre","priority":1,
            "when":[{"file_path_matches":"src/"}],
            "then":{"block":"project policy"}}]}"#,
    );
    std::fs::create_dir_all(dir.path().join(".phronesis/outcomes")).unwrap();
    std::fs::write(dir.path().join(".phronesis/outcomes/current"), "item-8").unwrap();

    let payload = r#"{"tool_name":"Edit","tool_input":{"file_path":"src/lib.rs","old_string":"a","new_string":"b"}}"#;
    let (code, _stderr) = run_hook_in("pre-check", payload, Some(dir.path()));
    assert_eq!(code, 2);
    let entry = last_pre_check(dir.path());
    assert_eq!(entry["subject"], "item-8", "{entry}");
    assert_eq!(entry["exit"], 2);
    assert_eq!(entry["consequences"][0]["rule_id"], "policy", "{entry}");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --test hook_integration subject 2>&1 | tail -20`
Expected: FAIL — `assertion failed: entry["subject"] == "item-7"` (the key is absent).

- [ ] **Step 3: Implement**

`hook/mod.rs` — add the field and write it:

```rust
pub(super) struct LogEventInput<'a> {
    pub(super) phase: &'static str,
    pub(super) tool_name: &'a str,
    pub(super) file_path: &'a str,
    pub(super) exit: i32,
    pub(super) command_exit: Option<i32>,
    pub(super) consequences: &'a [LoggedConsequence],
    /// The open work unit (`outcomes::subject::current`), when any. Absent —
    /// not empty, not null — when no unit is open. This is the join key
    /// `phr-mcp unit show` and the governed-throughput count use to answer
    /// "which rules were evaluated against this work item".
    pub(super) subject: Option<&'a str>,
}

pub(super) fn log_hook_event(input: &LogEventInput<'_>) {
    let LogEventInput {
        phase,
        tool_name,
        file_path,
        exit,
        command_exit,
        consequences,
        subject,
    } = input;
    let event = match *phase {
        "pre" => "pre_check",
        "post" => "post_check",
        _ => "hook_event",
    };
    let consequences_value = serde_json::to_value(consequences).unwrap_or(serde_json::Value::Null);
    let mut entry = LogEntry::new("hook", event)
        .with("phase", phase.to_string())
        .with("tool", tool_name.to_string())
        .with("file", file_path.to_string())
        .with("exit", *exit)
        .with("consequences", consequences_value);
    if let Some(ce) = command_exit {
        entry = entry.with("command_exit", *ce);
    }
    if let Some(s) = subject {
        entry = entry.with("subject", (*s).to_string());
    }
    let path = action_log::default_path(&security::project_root());
    let _ = action_log::append(&path, &entry);
}
```

`hook/pre.rs` — one read, before the three branches. Insert immediately after the existing `let logged = super::collect_logged(&consequences, &security::project_root());` at line 185:

```rust
    // The open work unit, read once for all three exit paths below. Stamping it
    // on the log entry is what makes "which rules were evaluated for this work
    // item" a join rather than a guess (spec §"Work items", 2).
    let subject = crate::outcomes::subject::current(&security::project_root());
```

and add `subject: subject.as_deref(),` as the last field of each of the three `LogEventInput { … }` literals (lines 206, 221, 232).

`hook/post.rs` — the same, inserted immediately before the `if evaluation.violations.is_empty() && evaluation.warnings.is_empty() {` at line 180:

```rust
    // Read once for both exit paths below; see `hook/pre.rs` for why.
    let subject = crate::outcomes::subject::current(&security::project_root());
```

and add `subject: subject.as_deref(),` as the last field of both `LogEventInput { … }` literals (lines 181, 202).

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --test hook_integration --test action_log_integration --test journey_hook_integration 2>&1 | tail -20`
Expected: PASS — the three new tests plus every pre-existing hook and action-log test (the new key is additive; no existing assertion enumerates the entry's keys).

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy --all-targets -p phronesis-mcp -- -D warnings
git add crates/phronesis-mcp/src/hook/mod.rs crates/phronesis-mcp/src/hook/pre.rs \
        crates/phronesis-mcp/src/hook/post.rs crates/phronesis-mcp/tests/hook_integration.rs
git commit -m "feat(hook): stamp the open work unit on pre_check/post_check log entries"
```

---

### Task 4: `phr-mcp unit show` — the work-item report

**Files:**
- Create: `crates/phronesis-mcp/src/lifecycle/unit_report.rs`
- Modify: `crates/phronesis-mcp/src/lifecycle/mod.rs` (`pub mod unit_report;`), `crates/phronesis-mcp/src/lifecycle/unit_cli.rs` (`UnitCmd::Show` + its arm)
- Test: `crates/phronesis-mcp/tests/unit_cli_integration.rs` (append)

**Interfaces:**
- Consumes: `action_log::{default_path, read_recent, ReadOpts, LogEntry}`; `journey::journal::{read_recent_subject, SUFFIX_HARD_CAP}`; `lifecycle::record::correction_text` (Plan 1 Task 7); `outcomes::{enabled, report, ConfidenceReport}`; `outcomes::subject::current`.
- Produces:

```rust
pub struct Intervention { pub ts: u64, pub mode: String, pub text: Option<String> }
pub struct CommitRow { pub sha: String, pub band: Option<String> }
pub struct UnitReport {
    pub unit_id: String, pub explicit: bool, pub spec: Option<String>,
    pub first_ts: Option<u64>, pub last_ts: Option<u64>, pub kalpa: Option<String>,
    pub rules_evaluated: u32, pub fired: u32, pub blocked: u32, pub warned: u32,
    pub per_rule: BTreeMap<String, u32>,
    pub signals: Vec<String>, pub band: Option<String>,
    pub interventions: Vec<Intervention>, pub mid_turn: u32, pub correction: u32,
    pub commits: Vec<CommitRow>,
}
pub fn build(root: &Path, unit_id: &str) -> UnitReport;
pub fn render(r: &UnitReport) -> String;
pub fn render_json(r: &UnitReport) -> String;
```

Two deliberate deviations from the spec's illustrative block, both stated so they are decisions rather than drift:

1. The evidence line in the spec reads `compile pass   tests pass (12/12)   band: high`. `outcomes::ConfidenceReport` carries the *names* of passed signals and a band; it has no test counts, and inventing a `(12/12)` the ledger does not hold would be a lie in a governance report. The renderer prints `compile pass   tests pass   band: high`.
2. The spec's per-rule and intervention lines end in `…`, indicating elision in the illustration. The report prints **every** rule and **every** intervention: a governance report that hides rules is worse than a long one. Intervention *text* is truncated at 72 characters with `…`, because one prompt can be a page.

- [ ] **Step 1: Write the failing tests** — append to `tests/unit_cli_integration.rs`

```rust
/// Seed one work item's worth of evidence: lifecycle entries written through
/// the real projection (so the field names under test are the ones
/// `to_log_entry` writes), plus hand-built `pre_check` entries in the shape
/// `log_hook_event` writes.
fn seed_unit(root: &Path, unit_id: &str) {
    use phronesis_mcp::action_log::{self, LogEntry};
    use phronesis_mcp::lifecycle::{Host, Kind, LifecycleEvent, Mode, PromptText, Stamped};

    let path = action_log::default_path(root);
    let stamp = |ts: u64, seq: u64| Stamped {
        ts,
        sid: "s-1".to_string(),
        seq,
        kalpa: Some("lifecycle-events".to_string()),
        subject: Some(unit_id.to_string()),
    };
    let events: Vec<(u64, LifecycleEvent)> = vec![
        (
            1_700_000_000,
            LifecycleEvent::new(Kind::UnitStart, Host::Cli)
                .with_extra("unit_id", unit_id)
                .with_extra("spec", "docs/specs/SPEC-thing.md"),
        ),
        (
            1_700_000_600,
            LifecycleEvent::new(Kind::Prompt, Host::Claude)
                .with_mode(Mode::MidTurn)
                .with_prompt("also update the changelog"),
        ),
        (
            1_700_000_900,
            LifecycleEvent::new(Kind::Prompt, Host::Claude)
                .with_mode(Mode::Correction)
                .with_prompt("no, keep the journal free of text"),
        ),
        (
            1_700_001_200,
            LifecycleEvent::new(Kind::Commit, Host::Claude)
                .with_extra("sha", "0f3c9a1e")
                .with_extra("confidence_band", "high"),
        ),
    ];
    for (i, (ts, ev)) in events.iter().enumerate() {
        action_log::append(
            &path,
            &ev.to_log_entry(&stamp(*ts, i as u64 + 1), PromptText::Full),
        )
        .unwrap();
    }

    // Rule evaluations, in `log_hook_event`'s shape.
    let hook = |ts: u64, exit: i32, cons: serde_json::Value| {
        let mut e = LogEntry::new("hook", "pre_check")
            .with("phase", "pre")
            .with("tool", "Edit")
            .with("file", "src/lib.rs")
            .with("exit", exit)
            .with("consequences", cons)
            .with("subject", unit_id);
        e.ts = ts;
        e
    };
    let cons = |id: &str, action: &str| {
        serde_json::json!([{ "rule_id": id, "action_type": action, "message": "m", "bindings": {} }])
    };
    for (ts, exit, cons) in [
        (1_700_000_100u64, 0, serde_json::json!([])),
        (1_700_000_200, 1, cons("warn-piped-verification", "warning")),
        (1_700_000_300, 1, cons("warn-piped-verification", "warning")),
        (1_700_000_400, 2, cons("block-await-on-sync", "constraint_violation")),
    ] {
        action_log::append(&path, &hook(ts, exit, cons)).unwrap();
    }
    // A rule evaluation for a *different* unit, which must not be counted.
    let mut other = hook(1_700_000_500, 1, cons("warn-piped-verification", "warning"));
    other.data.insert("subject".to_string(), serde_json::json!("other-unit"));
    action_log::append(&path, &other).unwrap();

    // Grounded outcome signals live in the journey journal, keyed by subject.
    let journey = root.join(".phronesis/journey");
    std::fs::create_dir_all(&journey).unwrap();
    let mut body = String::new();
    for (i, tag) in ["outcome:compile_ok", "outcome:test_pass"].iter().enumerate() {
        body.push_str(&serde_json::json!({
            "v": 1, "ts": 1_700_000_050u64 + i as u64, "sid": "s-1", "seq": 900 + i as u64,
            "tool": "Bash", "path": "<cmd>", "tags": [tag], "subject": unit_id,
        }).to_string());
        body.push('\n');
    }
    std::fs::write(journey.join("events.jsonl"), body).unwrap();
    std::fs::create_dir_all(root.join(".phronesis")).unwrap();
    std::fs::write(root.join(".phronesis/confidence.json"), "{}").unwrap();
}

#[test]
fn unit_show_renders_the_spec_block() {
    let d = tempfile::tempdir().unwrap();
    seed_unit(d.path(), "unit-1789095489589855000");
    let out = run_phr(d.path(), &["unit", "show", "unit-1789095489589855000"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);

    assert!(
        text.contains("unit: unit-1789095489589855000   explicit   spec: docs/specs/SPEC-thing.md"),
        "{text}"
    );
    assert!(text.contains("kalpa: lifecycle-events"), "{text}");
    assert!(text.contains("window: "), "{text}");
    assert!(text.contains("rules evaluated   4   fired 3   blocked 1   warned 2"), "{text}");
    assert!(text.contains("block-await-on-sync  1"), "{text}");
    assert!(text.contains("warn-piped-verification  2"), "{text}");
    assert!(
        text.contains("evidence         compile pass   tests pass   band: medium"),
        "{text}"
    );
    assert!(text.contains("interventions     2   mid_turn 1   correction 1"), "{text}");
    assert!(text.contains("correction  \"no, keep the journal free of text\""), "{text}");
    assert!(text.contains("commits           1   0f3c9a1e  band high"), "{text}");
    assert!(
        !text.contains("other-unit"),
        "another unit's rule evaluations are not this unit's: {text}"
    );
}

#[test]
fn unit_show_json_emits_one_object() {
    let d = tempfile::tempdir().unwrap();
    seed_unit(d.path(), "unit-1");
    let out = run_phr(d.path(), &["unit", "show", "unit-1", "--json"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["unit_id"], "unit-1");
    assert_eq!(v["explicit"], true);
    assert_eq!(v["spec"], "docs/specs/SPEC-thing.md");
    assert_eq!(v["kalpa"], "lifecycle-events");
    assert_eq!(v["rules_evaluated"], 4);
    assert_eq!(v["fired"], 3);
    assert_eq!(v["blocked"], 1);
    assert_eq!(v["warned"], 2);
    assert_eq!(v["per_rule"]["warn-piped-verification"], 2);
    assert_eq!(v["band"], "medium");
    assert_eq!(v["signals"][0], "compile");
    assert_eq!(v["interventions"].as_array().unwrap().len(), 2);
    assert_eq!(v["commits"][0]["sha"], "0f3c9a1e");
    assert_eq!(v["commits"][0]["band"], "high");
    assert_eq!(v["first_ts"], 1_700_000_000u64);
    assert_eq!(v["last_ts"], 1_700_001_200u64);
}

/// A unit nobody ran `unit start` for is reported as implicit, with no spec.
/// The split is the point: implicit units split on every build/test cycle and
/// would otherwise flatter every per-item number (spec §"Work items / Limits").
#[test]
fn unit_show_reports_an_implicit_unit_as_implicit() {
    let d = tempfile::tempdir().unwrap();
    use phronesis_mcp::action_log;
    use phronesis_mcp::lifecycle::{Host, Kind, LifecycleEvent, PromptText, Stamped};
    let stamped = Stamped {
        ts: 1_700_000_000,
        sid: "s-1".into(),
        seq: 1,
        kalpa: None,
        subject: Some("unit-implicit".into()),
    };
    action_log::append(
        &action_log::default_path(d.path()),
        &LifecycleEvent::new(Kind::Commit, Host::Claude)
            .with_extra("sha", "abc0123")
            .to_log_entry(&stamped, PromptText::Full),
    )
    .unwrap();

    let out = run_phr(d.path(), &["unit", "show", "unit-implicit"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("unit: unit-implicit   implicit"), "{text}");
    assert!(!text.contains("spec:"), "{text}");
    assert!(text.contains("commits           1   abc0123"), "{text}");
    assert!(!text.contains("band "), "no band segment without one: {text}");
    assert!(!text.contains("evidence"), "no evidence line without confidence scoring: {text}");
}

/// `unit show` with no id reports the open unit; with none open it fails.
#[test]
fn unit_show_defaults_to_the_open_unit() {
    let d = tempfile::tempdir().unwrap();
    let out = run_phr(d.path(), &["unit", "show"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("no work unit open"), "{}", stderr(&out));

    assert!(run_phr(d.path(), &["unit", "start", "item-1"]).status.success());
    let out = run_phr(d.path(), &["unit", "show"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("unit: item-1   explicit"), "{}", stdout(&out));
}

/// Intervention text is prompt text, so it obeys the `prompt_text` switch at
/// read time — the same accessor `journey --corrections` goes through.
#[test]
fn unit_show_hides_intervention_text_under_prompt_text_none() {
    let d = tempfile::tempdir().unwrap();
    seed_unit(d.path(), "unit-1");
    std::fs::write(
        d.path().join(".phronesis/journey.json"),
        r#"{"version":1,"taggers":[],"modules":[],"lifecycle":{"prompt_text":"none"}}"#,
    )
    .unwrap();
    let text = stdout(&run_phr(d.path(), &["unit", "show", "unit-1"]));
    assert!(text.contains("interventions     2"), "the count survives: {text}");
    assert!(!text.contains("keep the journal free of text"), "{text}");
    assert!(text.contains("correction  (text withheld)"), "{text}");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --test unit_cli_integration unit_show 2>&1 | tail -20`
Expected: FAIL — `error: unrecognized subcommand 'show'`.

- [ ] **Step 3: Implement**

`lifecycle/mod.rs`, after `pub mod unit_cli;`:

```rust
pub mod unit_report;
```

`lifecycle/unit_report.rs`:

```rust
//! `phr-mcp unit show` — one work item, joined across the journal and the
//! action log on `subject`.
//!
//! The question this answers is the one the spec opens §"Work items and
//! governed throughput" with: for one piece of work, which spec was it built
//! to, which rules fired, what evidence was recorded, and where did a human
//! step in. Nothing here writes; every read is fail-open.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::json;

use crate::action_log::{self, LogEntry, ReadOpts};
use crate::lifecycle::record::correction_text;
use crate::outcomes;

/// One human steer inside the work item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Intervention {
    pub ts: u64,
    /// `mid_turn` or `correction` — the two prompt modes that are steers.
    pub mode: String,
    /// The scrubbed prompt, or `None` under `prompt_text: "none"`.
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitRow {
    pub sha: String,
    pub band: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UnitReport {
    pub unit_id: String,
    /// True when a `unit_start` record exists for this id.
    pub explicit: bool,
    pub spec: Option<String>,
    pub first_ts: Option<u64>,
    pub last_ts: Option<u64>,
    pub kalpa: Option<String>,
    /// `pre_check` + `post_check` entries carrying this subject.
    pub rules_evaluated: u32,
    /// Those with at least one consequence.
    pub fired: u32,
    /// Those that exited 2.
    pub blocked: u32,
    /// Those that exited 1.
    pub warned: u32,
    /// Consequence counts by rule id, over the same entries.
    pub per_rule: BTreeMap<String, u32>,
    /// Names of passed grounded signals (`compile`, `tests`, `bug:<id>`).
    pub signals: Vec<String>,
    pub band: Option<String>,
    pub interventions: Vec<Intervention>,
    pub mid_turn: u32,
    pub correction: u32,
    pub commits: Vec<CommitRow>,
}

fn str_field<'a>(e: &'a LogEntry, key: &str) -> Option<&'a str> {
    e.data.get(key).and_then(|v| v.as_str())
}

/// Widen the `[first_ts, last_ts]` window to include `ts`.
fn widen(r: &mut UnitReport, ts: u64) {
    r.first_ts = Some(r.first_ts.map_or(ts, |t| t.min(ts)));
    r.last_ts = Some(r.last_ts.map_or(ts, |t| t.max(ts)));
}

/// Build the report for `unit_id`.
///
/// Reads the whole action log and its one rotated predecessor (`ReadOpts` with
/// no limit does both, oldest first) and filters on `subject` in process: the
/// filter is a field match, not a `kind`, so it cannot be pushed into
/// `read_recent`. The journal contributes the grounded outcome signals through
/// `outcomes::report`, which reads it per subject.
pub fn build(root: &Path, unit_id: &str) -> UnitReport {
    let mut r = UnitReport { unit_id: unit_id.to_string(), ..UnitReport::default() };

    let entries = action_log::read_recent(&action_log::default_path(root), &ReadOpts::default())
        .unwrap_or_default();
    for e in &entries {
        if str_field(e, "subject") != Some(unit_id) {
            continue;
        }
        widen(&mut r, e.ts);
        if let Some(k) = str_field(e, "kalpa") {
            r.kalpa = Some(k.to_string());
        }
        match (e.kind.as_str(), e.event.as_str()) {
            ("hook", "pre_check" | "post_check") => {
                r.rules_evaluated += 1;
                match e.data.get("exit").and_then(|v| v.as_i64()) {
                    Some(2) => r.blocked += 1,
                    Some(1) => r.warned += 1,
                    _ => {}
                }
                let cons = e.data.get("consequences").and_then(|v| v.as_array());
                let cons = cons.map(|c| c.as_slice()).unwrap_or(&[]);
                if !cons.is_empty() {
                    r.fired += 1;
                }
                for c in cons {
                    if let Some(id) = c.get("rule_id").and_then(|v| v.as_str()) {
                        *r.per_rule.entry(id.to_string()).or_insert(0) += 1;
                    }
                }
            }
            ("lifecycle", "unit_start") => {
                r.explicit = true;
                if let Some(s) = str_field(e, "spec") {
                    r.spec = Some(s.to_string());
                }
            }
            ("lifecycle", "prompt") => {
                let mode = str_field(e, "mode").unwrap_or("fresh");
                if !matches!(mode, "mid_turn" | "correction") {
                    continue;
                }
                if mode == "mid_turn" {
                    r.mid_turn += 1;
                } else {
                    r.correction += 1;
                }
                // The one accessor for prompt text: it consults the *current*
                // `prompt_text` value, so flipping the switch to "none" hides
                // text already written under "full" (spec §"Action log").
                r.interventions.push(Intervention {
                    ts: e.ts,
                    mode: mode.to_string(),
                    text: correction_text(root, e),
                });
            }
            ("lifecycle", "commit") => r.commits.push(CommitRow {
                sha: str_field(e, "sha").unwrap_or_default().to_string(),
                band: str_field(e, "confidence_band").map(str::to_string),
            }),
            _ => {}
        }
    }

    // Grounded evidence. Gated on `enabled`: with confidence scoring off there
    // are no signals to find, and `outcomes::report` would report band `low`
    // from an empty ledger, which reads as a measurement rather than as "not
    // measured".
    if outcomes::enabled(root)
        && let Some(rep) = outcomes::report(root, Some(unit_id))
    {
        r.signals = rep.signals;
        r.band = Some(rep.band.as_str().to_string());
    }

    r
}

fn hm(ts: u64) -> String {
    chrono::DateTime::from_timestamp(ts as i64, 0)
        .map(|dt| dt.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_else(|| ts.to_string())
}

fn ymd_hm(ts: u64) -> String {
    chrono::DateTime::from_timestamp(ts as i64, 0)
        .map(|dt| dt.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| ts.to_string())
}

fn ymd(ts: u64) -> String {
    chrono::DateTime::from_timestamp(ts as i64, 0)
        .map(|dt| dt.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

/// `2026-09-18 14:02 → 16:40` within one day, both dates across a boundary.
fn window(first: u64, last: u64) -> String {
    if ymd(first) == ymd(last) {
        format!("{} → {}", ymd_hm(first), hm(last))
    } else {
        format!("{} → {}", ymd_hm(first), ymd_hm(last))
    }
}

/// One prompt can be a page; the report is a scan, not a transcript.
fn clip(text: &str, max_chars: usize) -> String {
    let flat = text.replace('\n', " ");
    if flat.chars().count() <= max_chars {
        return flat;
    }
    let head: String = flat.chars().take(max_chars).collect();
    format!("{head} …")
}

/// The block from SPEC-agent-lifecycle-events §"Work items and governed
/// throughput", oldest first. Sections with nothing to say are omitted rather
/// than printed as zeros.
pub fn render(r: &UnitReport) -> String {
    let mut out = String::new();
    let kind = if r.explicit { "explicit" } else { "implicit" };
    out.push_str(&format!("unit: {}   {kind}", r.unit_id));
    if let Some(s) = &r.spec {
        out.push_str(&format!("   spec: {s}"));
    }
    out.push('\n');

    if let (Some(first), Some(last)) = (r.first_ts, r.last_ts) {
        out.push_str(&format!("window: {}", window(first, last)));
        if let Some(k) = &r.kalpa {
            out.push_str(&format!("   kalpa: {k}"));
        }
        out.push('\n');
    }

    if r.rules_evaluated > 0 {
        out.push_str(&format!(
            "{:<15}{:>4}   fired {}   blocked {}   warned {}\n",
            "rules evaluated", r.rules_evaluated, r.fired, r.blocked, r.warned
        ));
        if !r.per_rule.is_empty() {
            // Loudest first, then alphabetically, so the same data always
            // renders the same way. Every rule is printed: a governance report
            // that elides rules is worse than a long one.
            let mut rules: Vec<(&String, &u32)> = r.per_rule.iter().collect();
            rules.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
            let cells: Vec<String> = rules.iter().map(|(id, n)| format!("{id}  {n}")).collect();
            out.push_str(&format!("  {}\n", cells.join("   ")));
        }
    }

    if r.band.is_some() || !r.signals.is_empty() {
        // The spec's illustration reads `tests pass (12/12)`; the ledger holds
        // signal names and a band, not test counts, so the counts are not
        // printed rather than invented.
        let mut cells: Vec<String> = r.signals.iter().map(|s| format!("{s} pass")).collect();
        if let Some(b) = &r.band {
            cells.push(format!("band: {b}"));
        }
        out.push_str(&format!("{:<17}{}\n", "evidence", cells.join("   ")));
    }

    if !r.interventions.is_empty() {
        out.push_str(&format!(
            "{:<15}{:>4}   mid_turn {}   correction {}\n",
            "interventions",
            r.interventions.len(),
            r.mid_turn,
            r.correction
        ));
        for i in &r.interventions {
            let text = match &i.text {
                Some(t) => format!("\"{}\"", clip(t, 72)),
                None => "(text withheld)".to_string(),
            };
            out.push_str(&format!("  {}  {}  {text}\n", hm(i.ts), i.mode));
        }
    }

    if !r.commits.is_empty() {
        let cells: Vec<String> = r
            .commits
            .iter()
            .map(|c| match &c.band {
                Some(b) => format!("{}  band {b}", c.sha),
                None => c.sha.clone(),
            })
            .collect();
        out.push_str(&format!(
            "{:<15}{:>4}   {}\n",
            "commits",
            r.commits.len(),
            cells.join("   ")
        ));
    }
    out
}

/// `--json`: the same report as one object.
pub fn render_json(r: &UnitReport) -> String {
    json!({
        "unit_id": r.unit_id,
        "explicit": r.explicit,
        "spec": r.spec,
        "first_ts": r.first_ts,
        "last_ts": r.last_ts,
        "kalpa": r.kalpa,
        "rules_evaluated": r.rules_evaluated,
        "fired": r.fired,
        "blocked": r.blocked,
        "warned": r.warned,
        "per_rule": r.per_rule,
        "signals": r.signals,
        "band": r.band,
        "mid_turn": r.mid_turn,
        "correction": r.correction,
        "interventions": r.interventions.iter().map(|i| json!({
            "ts": i.ts, "mode": i.mode, "text": i.text
        })).collect::<Vec<_>>(),
        "commits": r.commits.iter().map(|c| json!({
            "sha": c.sha, "band": c.band
        })).collect::<Vec<_>>(),
    })
    .to_string()
}
```

`lifecycle/unit_cli.rs` — add the variant and the arm:

```rust
    /// Report on a work item: its spec, rules, evidence, interventions, commits.
    Show {
        /// The work item to report on. Omitted, the open one.
        id: Option<String>,
        /// Emit the report as one JSON object.
        #[arg(long)]
        json: bool,
    },
```

```rust
        UnitCmd::Show { id, json } => {
            let id = match id.or_else(|| subject::current(root)) {
                Some(id) => id,
                None => bail!("no work unit open"),
            };
            let report = crate::lifecycle::unit_report::build(root, &id);
            Ok(if json {
                crate::lifecycle::unit_report::render_json(&report)
            } else {
                crate::lifecycle::unit_report::render(&report).trim_end().to_string()
            })
        }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --test unit_cli_integration --test cli_smoke 2>&1 | tail -30`
Expected: PASS — 12 tests in `unit_cli_integration`.

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy --all-targets -p phronesis-mcp -- -D warnings
git add crates/phronesis-mcp/src/lifecycle/unit_report.rs crates/phronesis-mcp/src/lifecycle/unit_cli.rs \
        crates/phronesis-mcp/src/lifecycle/mod.rs crates/phronesis-mcp/tests/unit_cli_integration.rs
git commit -m "feat(cli): phr-mcp unit show joins the journal and action log per work item"
```

---

### Task 5: work items and governed throughput in `kalpa show` and `stats`

**Files:**
- Modify: `crates/phronesis-mcp/src/stats.rs` (`LifecycleStats`, `aggregate_lifecycle`, `render_lifecycle`, `render_json_with_lifecycle` — all Plan 5 Task 1)
- Modify: `crates/phronesis-mcp/src/lifecycle/kalpa_cli.rs` (the `report` fn's `read_recent` call — Plan 5 Task 3), `crates/phronesis-mcp/src/main.rs` (`handle_stats`' `read_recent` call — Plan 5 Task 2)
- Test: unit tests in `stats.rs`; `crates/phronesis-mcp/tests/kalpa_integration.rs` (append)

**Interfaces:**
- Consumes: Plan 5 Task 1's `LifecycleOpts`, `LifecycleStats`, `aggregate_lifecycle`, `render_lifecycle`, `render_json_with_lifecycle`.
- Produces, on `LifecycleStats`:

```rust
    pub work_items_explicit: u32,
    pub work_items_implicit: u32,
    pub work_items_completed: u32,
    pub governed: u32,
    /// Interventions that carry a `subject` — the numerator of the per-item ratio.
    pub subject_interventions: u32,
impl LifecycleStats { pub fn interventions_per_work_item(&self) -> Option<f64>; }
```

**The one behavioural change to Plan 5's callers.** `aggregate_lifecycle` now needs `pre_check` / `post_check` entries too, because "a rule was actually evaluated against its edits" is half the governed definition and those entries are `kind: "hook"`. Both callers therefore stop pre-filtering on `kind` and hand it everything; the function already ignores kinds it does not understand, and the read cost is unchanged (`ReadOpts` with no limit already parsed every line of both files).

**Column widths.** The existing lines in `render_lifecycle` put the number in a four-wide field ending at column 17 — `format!("{:<13}{:>4}", …)`, which is what Plan 5's own assertions pin (`"sessions        1"` is 17 characters). The three new lines use the same format, which reproduces the spec's block byte for byte: `work items` (10) and `governed` (8) both pad to 13.

- [ ] **Step 1: Write the failing tests** — append inside `stats.rs`'s `mod tests`

```rust
/// A `pre_check` entry in `log_hook_event`'s shape, carrying a subject.
fn evaluated(ts: u64, subject: &str) -> LogEntry {
    let mut e = LogEntry::new("hook", "pre_check")
        .with("phase", "pre")
        .with("tool", "Edit")
        .with("exit", 0)
        .with("consequences", json!([]))
        .with("subject", subject);
    e.ts = ts;
    e
}

/// A lifecycle entry carrying a subject.
fn life_for(ts: u64, event: &str, subject: &str, fields: &[(&str, serde_json::Value)]) -> LogEntry {
    let mut e = life(ts, event, fields);
    e.data.insert("subject".to_string(), json!(subject));
    e
}

/// Four work items in kalpa `k1`:
///   u-a  explicit, committed (band high), rules evaluated  → governed
///   u-b  implicit, committed (band low),  rules evaluated  → completed, not governed
///   u-c  explicit, committed (band high), never evaluated  → completed, not governed
///   u-d  implicit, never committed                         → neither
fn work_item_log() -> Vec<LogEntry> {
    let k = || ("kalpa", json!("k1"));
    vec![
        life_for(100, "unit_start", "u-a", &[k(), ("unit_id", json!("u-a"))]),
        evaluated(101, "u-a"),
        life_for(102, "prompt", "u-a", &[k(), ("mode", json!("correction"))]),
        life_for(103, "commit", "u-a", &[k(), ("sha", json!("aaa")), ("confidence_band", json!("high"))]),
        evaluated(110, "u-b"),
        life_for(111, "prompt", "u-b", &[k(), ("mode", json!("mid_turn"))]),
        life_for(112, "commit", "u-b", &[k(), ("sha", json!("bbb")), ("confidence_band", json!("low"))]),
        life_for(120, "unit_start", "u-c", &[k(), ("unit_id", json!("u-c"))]),
        life_for(121, "commit", "u-c", &[k(), ("sha", json!("ccc")), ("confidence_band", json!("high"))]),
        life_for(130, "prompt", "u-d", &[k(), ("mode", json!("fresh"))]),
    ]
}

#[test]
fn aggregate_lifecycle_counts_work_items_and_governed_throughput() {
    let s = aggregate_lifecycle(
        &work_item_log(),
        &LifecycleOpts { since_secs: None, kalpa: Some("k1".into()), now_secs: 1_000 },
    );
    assert_eq!(s.work_items_explicit, 2, "u-a and u-c carry a unit_start");
    assert_eq!(s.work_items_implicit, 2, "u-b and u-d do not");
    assert_eq!(s.work_items_completed, 3, "u-a, u-b, u-c each carry a commit");
    assert_eq!(
        s.governed, 1,
        "u-a alone: u-b's band is low, u-c had no rule evaluated against it"
    );
    assert_eq!(s.subject_interventions, 2, "the correction and the mid_turn");
    assert_eq!(s.interventions_per_work_item(), Some(2.0 / 3.0));
}

/// The definition is the spec's, word for word: governed means completed, with
/// a rule evaluated, and a last-commit band that is **not `low`**. An absent
/// band is not `low` — it means confidence scoring was off, and the definition
/// as written admits it. Pinned here so the reading is a decision.
#[test]
fn a_commit_with_no_band_is_not_disqualified_from_governed() {
    let entries = vec![
        evaluated(10, "u-x"),
        life_for(11, "commit", "u-x", &[("sha", json!("ddd"))]),
    ];
    let s = aggregate_lifecycle(&entries, &LifecycleOpts::default());
    assert_eq!(s.work_items_completed, 1);
    assert_eq!(s.governed, 1);
}

/// The band that counts is the one at the *last* commit, not the best one.
#[test]
fn governed_uses_the_band_at_the_last_commit() {
    let entries = vec![
        evaluated(10, "u-y"),
        life_for(11, "commit", "u-y", &[("sha", json!("e1")), ("confidence_band", json!("high"))]),
        life_for(12, "commit", "u-y", &[("sha", json!("e2")), ("confidence_band", json!("low"))]),
    ];
    let s = aggregate_lifecycle(&entries, &LifecycleOpts::default());
    assert_eq!(s.work_items_completed, 1);
    assert_eq!(s.governed, 0, "the last commit's band is low");
}

#[test]
fn render_lifecycle_prints_the_three_work_item_lines() {
    let s = aggregate_lifecycle(
        &work_item_log(),
        &LifecycleOpts { since_secs: None, kalpa: Some("k1".into()), now_secs: 1_000 },
    );
    let out = render_lifecycle(&s);
    assert!(out.contains("work items      4   explicit 2   implicit 2"), "{out}");
    assert!(out.contains("governed        1   (commit + rules evaluated + band ≥ medium)"), "{out}");
    assert!(out.contains("interventions / work item   0.67"), "{out}");
    // Plan 5's block is unchanged above it.
    assert!(out.contains("interventions / commit   0.67"), "{out}");
}

/// "omitted when zero completed items" (spec §"Work items"). A kalpa with work
/// but no landed commit prints the split and the governed count — both are
/// honest zeros — but no ratio, because dividing by zero items is not a number.
#[test]
fn render_lifecycle_omits_the_per_item_ratio_without_completed_items() {
    let entries = vec![life_for(10, "prompt", "u-z", &[("mode", json!("correction"))])];
    let s = aggregate_lifecycle(&entries, &LifecycleOpts::default());
    let out = render_lifecycle(&s);
    assert!(out.contains("work items      1   explicit 0   implicit 1"), "{out}");
    assert!(out.contains("governed        0"), "{out}");
    assert!(!out.contains("interventions / work item"), "{out}");
}

/// No subjects anywhere → no work-item section at all, rather than three rows
/// of zeros on every project that has not adopted work units.
#[test]
fn render_lifecycle_omits_the_work_item_section_when_there_are_no_units() {
    let s = aggregate_lifecycle(&fixture_log(), &LifecycleOpts::default());
    let out = render_lifecycle(&s);
    assert!(!out.contains("work items"), "{out}");
    assert!(!out.contains("governed"), "{out}");
}

#[test]
fn render_json_with_lifecycle_carries_the_work_item_numbers() {
    let values = Stats { window_label: "7d".into(), generated_at: 1, per_rule: vec![] };
    let s = aggregate_lifecycle(
        &work_item_log(),
        &LifecycleOpts { since_secs: None, kalpa: Some("k1".into()), now_secs: 1_000 },
    );
    let v: serde_json::Value =
        serde_json::from_str(&render_json_with_lifecycle(&values, Some(&s))).unwrap();
    assert_eq!(v["lifecycle"]["work_items"]["explicit"], 2);
    assert_eq!(v["lifecycle"]["work_items"]["implicit"], 2);
    assert_eq!(v["lifecycle"]["work_items"]["completed"], 3);
    assert_eq!(v["lifecycle"]["governed"], 1);
    assert!(
        (v["lifecycle"]["interventions_per_work_item"].as_f64().unwrap() - 2.0 / 3.0).abs() < 1e-9
    );
}
```

and append to `tests/kalpa_integration.rs` (reusing its `run_phr` and `seed_lifecycle_log` helpers from Plan 1 Task 11 / Plan 5 Task 2):

```rust
/// Seed one governed work item and one ungoverned one into `kalpa`.
fn seed_work_items(root: &std::path::Path, kalpa: &str) {
    use phronesis_mcp::action_log::{self, LogEntry};
    use phronesis_mcp::lifecycle::{Host, Kind, LifecycleEvent, Mode, PromptText, Stamped};

    let path = action_log::default_path(root);
    let mut seq = 100u64;
    let mut push = |ts: u64, subject: &str, ev: LifecycleEvent| {
        seq += 1;
        let stamped = Stamped {
            ts,
            sid: "s-1".to_string(),
            seq,
            kalpa: Some(kalpa.to_string()),
            subject: Some(subject.to_string()),
        };
        action_log::append(&path, &ev.to_log_entry(&stamped, PromptText::Full)).unwrap();
    };
    push(1_700_010_000, "w-1", LifecycleEvent::new(Kind::UnitStart, Host::Cli).with_extra("unit_id", "w-1"));
    push(1_700_010_100, "w-1", LifecycleEvent::new(Kind::Prompt, Host::Claude).with_mode(Mode::Correction).with_prompt("no, the other one"));
    push(1_700_010_200, "w-1", LifecycleEvent::new(Kind::Commit, Host::Claude).with_extra("sha", "0f3c").with_extra("confidence_band", "high"));
    push(1_700_010_300, "w-2", LifecycleEvent::new(Kind::Commit, Host::Claude).with_extra("sha", "aa11").with_extra("confidence_band", "low"));

    for (ts, subject) in [(1_700_010_050u64, "w-1"), (1_700_010_250, "w-2")] {
        let mut e = LogEntry::new("hook", "pre_check")
            .with("phase", "pre")
            .with("tool", "Edit")
            .with("exit", 0)
            .with("consequences", serde_json::json!([]))
            .with("subject", subject);
        e.ts = ts;
        action_log::append(&path, &e).unwrap();
    }
}

#[test]
fn kalpa_show_reports_work_items_and_governed_throughput() {
    let d = tempfile::tempdir().unwrap();
    seed_work_items(d.path(), "lifecycle-events");
    let out = run_phr(d.path(), &["kalpa", "show", "lifecycle-events"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("work items      2   explicit 1   implicit 1"), "{stdout}");
    assert!(stdout.contains("governed        1   (commit + rules evaluated + band ≥ medium)"), "{stdout}");
    assert!(stdout.contains("interventions / work item   0.50"), "{stdout}");
    assert!(!stdout.contains("no, the other one"), "prompt text never reaches kalpa show: {stdout}");
}

#[test]
fn stats_kalpa_reports_governed_throughput_as_json() {
    let d = tempfile::tempdir().unwrap();
    seed_work_items(d.path(), "lifecycle-events");
    let out = run_phr(d.path(), &["stats", "--kalpa", "lifecycle-events", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["lifecycle"]["work_items"]["explicit"], 1);
    assert_eq!(v["lifecycle"]["work_items"]["implicit"], 1);
    assert_eq!(v["lifecycle"]["work_items"]["completed"], 2);
    assert_eq!(v["lifecycle"]["governed"], 1);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p phronesis-mcp --lib stats:: 2>&1 | tail -30`
Expected: FAIL — `no field work_items_explicit on type LifecycleStats`.

- [ ] **Step 3: Implement**

In `stats.rs`, add the fields to `LifecycleStats`:

```rust
    /// Work items (= `outcomes::subject` work units) seen on a lifecycle entry
    /// in this window, split by whether a `unit_start` record exists for them.
    /// The split is printed because implicit units split on every build/test
    /// cycle and would otherwise flatter every per-item number.
    pub work_items_explicit: u32,
    pub work_items_implicit: u32,
    /// Work items with at least one `commit` record carrying their subject.
    pub work_items_completed: u32,
    /// Completed **and** rule-evaluated **and** band at the last commit not
    /// `low` — the spec's definition of governed, unabbreviated.
    pub governed: u32,
    /// Interventions carrying a `subject`: the per-item ratio's numerator.
    pub subject_interventions: u32,
```

and the accessor beside `interventions_per_commit`:

```rust
    /// `interventions carrying a subject / completed work items`, or `None`
    /// when nothing completed. The second and last ratio the spec ships.
    pub fn interventions_per_work_item(&self) -> Option<f64> {
        (self.work_items_completed > 0)
            .then(|| f64::from(self.subject_interventions) / f64::from(self.work_items_completed))
    }

    /// Any work item at all in this window?
    pub fn work_items(&self) -> u32 {
        self.work_items_explicit + self.work_items_implicit
    }
```

In `aggregate_lifecycle`, add the accumulator above the loop:

```rust
    /// Per-work-item state, accumulated over the window.
    #[derive(Default)]
    struct UnitAcc {
        explicit: bool,
        completed: bool,
        /// The band on the most recent commit, which is the one the governed
        /// definition reads.
        last_commit_band: Option<String>,
    }
    let mut units: BTreeMap<String, UnitAcc> = BTreeMap::new();
    /// Subjects with at least one rule evaluation. Hook entries carry no
    /// `kalpa` (the kalpa is a property of the lifecycle stream), so they are
    /// never kalpa-filtered; a subject's membership in the kalpa comes from its
    /// own lifecycle records.
    let mut evaluated: BTreeSet<String> = BTreeSet::new();
```

replace the early `continue` at the top of the loop:

```rust
    for e in entries {
        if e.kind == "hook" && matches!(e.event.as_str(), "pre_check" | "post_check") {
            if let Some(s) = e.data.get("subject").and_then(|v| v.as_str()) {
                evaluated.insert(s.to_string());
            }
            continue;
        }
        if e.kind != "lifecycle" {
            continue;
        }
```

after the kalpa filter and before the existing `match e.event.as_str()`, register the subject:

```rust
        let subject = e.data.get("subject").and_then(|v| v.as_str()).map(str::to_string);
        if let Some(s) = &subject {
            units.entry(s.clone()).or_default();
        }
```

add to the existing `"prompt"` arm, inside its `if matches!(mode, "mid_turn" | "correction")` block:

```rust
                    if subject.is_some() {
                        out.subject_interventions += 1;
                    }
```

add to the existing `"commit"` arm:

```rust
                if let Some(s) = &subject
                    && let Some(acc) = units.get_mut(s)
                {
                    acc.completed = true;
                    acc.last_commit_band = e.data
                        .get("confidence_band")
                        .and_then(|v| v.as_str())
                        .map(str::to_string);
                }
```

and add one new arm beside it:

```rust
            "unit_start" => {
                if let Some(s) = &subject
                    && let Some(acc) = units.get_mut(s)
                {
                    acc.explicit = true;
                }
            }
```

then, after the existing `out.sessions = …` / median block:

```rust
    out.work_items_explicit = units.values().filter(|u| u.explicit).count() as u32;
    out.work_items_implicit = units.values().filter(|u| !u.explicit).count() as u32;
    out.work_items_completed = units.values().filter(|u| u.completed).count() as u32;
    // Spec §"Work items": governed = completed, **and** at least one rule was
    // evaluated against its edits, **and** the band at its last commit is not
    // `low`. An absent band is not `low`: it means scoring was off, and the
    // definition as written admits it.
    out.governed = units
        .iter()
        .filter(|(id, u)| {
            u.completed && evaluated.contains(*id) && u.last_commit_band.as_deref() != Some("low")
        })
        .count() as u32;
```

In `render_lifecycle`, append after the `interventions / commit` line:

```rust
    // Omitted entirely on a project that has not adopted work units: three rows
    // of zeros would read as a measurement of nothing.
    if s.work_items() > 0 {
        out.push_str(&format!(
            "{:<13}{:>4}   explicit {}   implicit {}\n",
            "work items", s.work_items(), s.work_items_explicit, s.work_items_implicit
        ));
        out.push_str(&format!(
            "{:<13}{:>4}   (commit + rules evaluated + band ≥ medium)\n",
            "governed", s.governed
        ));
        if let Some(r) = s.interventions_per_work_item() {
            out.push_str(&format!("interventions / work item   {r:.2}\n"));
        }
    }
```

In `render_json_with_lifecycle`, add three keys to the `lifecycle` object:

```rust
            "work_items": {
                "explicit": l.work_items_explicit,
                "implicit": l.work_items_implicit,
                "completed": l.work_items_completed,
            },
            "governed": l.governed,
            "interventions_per_work_item": l.interventions_per_work_item(),
```

In `lifecycle/kalpa_cli.rs`'s `report`, hand the aggregator everything:

```rust
    // Not `kind: Some("lifecycle")`: `aggregate_lifecycle` also reads
    // `pre_check` / `post_check` entries, because "a rule was evaluated against
    // this work item" is half the governed definition. It ignores every other
    // kind itself.
    let entries = action_log::read_recent(
        &action_log::default_path(root),
        &ReadOpts::default(),
    )
    .unwrap_or_default();
```

and the `start_retained` check below it keeps working unchanged (it already filters on `e.event == "kalpa_start"`).

In `main.rs`'s `handle_stats`, the same change:

```rust
    // Unfiltered: `aggregate_lifecycle` needs the `pre_check` / `post_check`
    // entries for the governed count and ignores everything else itself.
    let life_entries = action_log::read_recent(&path, &ReadOpts::default()).unwrap_or_default();
```

Add `use std::collections::BTreeSet;` to `aggregate_lifecycle`'s existing local `use` if Plan 5's version scoped it inside the function (it did: `use std::collections::BTreeSet;` is already the first line of the body).

- [ ] **Step 4: Run tests**

Run: `cargo test -p phronesis-mcp --lib stats:: --test kalpa_integration --test journey_cli_integration 2>&1 | tail -30`
Expected: PASS, including every Plan 5 test — the new lines are appended below the existing block and the existing counts are untouched.

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy --all-targets -p phronesis-mcp -- -D warnings
git add crates/phronesis-mcp/src/stats.rs crates/phronesis-mcp/src/lifecycle/kalpa_cli.rs \
        crates/phronesis-mcp/src/main.rs crates/phronesis-mcp/tests/kalpa_integration.rs
git commit -m "feat(stats): work-item split, governed throughput, interventions per work item"
```

---

### Task 6: CHANGELOG

**Files:**
- Modify: `CHANGELOG.md` (`## [Unreleased]` → `### Added`)

- [ ] **Step 1: Append the entry**

Add this bullet at the end of the existing `### Added` list under `## [Unreleased]`, after the bullets Plans 1–5 appended:

```markdown
- **Work items and governed throughput.** `phr-mcp unit start [<id>] [--spec
  <path>]` names the piece of work an agent is building and points it at the
  spec it is built to; `phr-mcp unit end` closes it. Starting a unit while one
  is open ends the open one first. `pre_check` and `post_check` action-log
  entries now carry the open work unit, so `phr-mcp unit show [<id>]` can join
  the journal and the action log and report one item's spec, window, kalpa,
  rules evaluated / fired / blocked / warned with per-rule counts, grounded
  evidence and confidence band, human interventions with their scrubbed text,
  and commits — as text or `--json`. `phr-mcp kalpa show` and `phr-mcp stats
  --kalpa <name>` gain a work-item split (explicit vs implicit), a **governed**
  count (committed, with a rule evaluated against it, and a last-commit
  confidence band above `low`), and `interventions / work item`. Implicit work
  units keep working exactly as before and need no new file: the two new
  lifecycle records carry everything.
```

- [ ] **Step 2: Verify the file still parses as the changelog it claims to be**

Run: `head -40 CHANGELOG.md`
Expected: the new bullet sits under `## [Unreleased]` → `### Added`, above the `## [0.33.0]` heading, and no released section was touched.

- [ ] **Step 3: Commit**

```bash
git add CHANGELOG.md
git commit -m "docs(changelog): work items and governed throughput"
```

---

## Self-review

**1. Spec coverage**

| spec requirement (§"Work items and governed throughput", §Rollout node 6) | task |
|---|---|
| `phr-mcp unit start [<id>] [--spec <path>]` sets the open subject (chosen id or fresh mint) | 2 |
| `--spec` is repo-relative and the file must exist | 2 (`validate_spec` — absolute rejected, `resolve_safe_path` covers existence, containment, `..`) |
| records `unit_start` with `extra.spec` and `extra.unit_id` | 1 (vocabulary), 2 (emission) |
| `phr-mcp unit end` records `unit_end` and clears the open subject | 2 |
| starting a unit while one is open ends the open one first | 2 (`end_open` before `set`/`open`) |
| implicit units keep working, get no `unit_start`, and the report says which were implicit | 2 (nothing on the implicit path changes), 4 (`explicit` flag), 5 (the split) |
| `extra.implicit: true` on a `unit_end` for a never-started unit | 2 (`was_started_explicitly`) |
| `subject` on `pre_check` / `post_check` action-log entries | 3 |
| `phr-mcp unit show [<id>]` joins journal and action log on `subject`, oldest first | 4 |
| the report's text block: unit/explicit/spec, window/kalpa, rules evaluated + per-rule, evidence, interventions with scrubbed text, commits | 4 (`render`; two illustration deviations stated in the task) |
| `--json` emits the same as one object | 4 (`render_json`) |
| intervention text is subject to the `prompt_text` switch | 4 (`correction_text`, pinned by `unit_show_hides_intervention_text_under_prompt_text_none`) |
| `extra` gains `spec`, `unit_id`, `implicit` to its closed vocabulary | 1 |
| `unit_start` / `unit_end` are lifecycle records with `subject` set; no new file | 1, 2 (`record` stamps `subject` from `subject::current`) |
| completed = ≥1 `commit` carrying the subject | 5 |
| governed = completed + ≥1 `pre_check`/`post_check` with the subject + band at last commit not `low` | 5 |
| governed throughput is the count inside the retention window, printed with the boundary | 5 (the aggregator only sees retained entries; `retention_line` is already in the header from Plan 5) |
| interventions per work item = interventions with a subject ÷ completed items, omitted at zero | 5 |
| `kalpa show` gains the three lines | 5 |
| §Rollout: node 6 touches `hook/mod.rs::log_hook_event`, `main.rs`, a new `lifecycle/unit_cli.rs`, and Plan 5's report code | 2, 3, 4, 5 |
| §Rollout: hand-written CHANGELOG under `## [Unreleased]` | 6 |
| §Event model: `lifecycle:unit_start` / `lifecycle:unit_end` selectors | 1 (`Kind::tag`; Plan 1's `validate_selectors` exemption already lists both by name) |

Out of scope by design: the `kalpa` subcommand and `record`/`state` themselves (Plan 1), the adapters that emit prompts, interrupts and commits (Plans 2–4), and the rest of the reporting surface (Plan 5). §"Limits" needs no task: the spec-path-is-not-a-hash and per-project-subject limits are properties of this design, and the `explicit`/`implicit` split that makes the implicit-unit limit visible is Task 5.

**2. Placeholder scan**

No "TBD", no "add error handling", no "similar to Task N", no "write tests for the above". Every code step carries compilable Rust; every test step carries its assertions. Task 2 explicitly states that it does *not* register a `Show` variant rather than shipping a stub arm, and Task 4 adds the variant together with its implementation. Task 4's two departures from the spec's illustrative block (`(12/12)`, the `…` elisions) are written down as decisions with reasons, not deferred. Task 5 states the column-width derivation rather than leaving the format string to taste.

**3. Type consistency**

- `Kind::UnitStart` / `Kind::UnitEnd` and `EXTRA_KEYS: [&str; 12]` — defined Task 1, used in Tasks 2, 4, 5.
- `outcomes::subject::clear(&Path) -> Result<(), SubjectError>` — defined Task 2, used in Task 2 only.
- `UnitCmd::{Start { id, spec }, End}` (Task 2) + `Show { id, json }` (Task 4); `unit_cli::run(&Path, UnitCmd) -> anyhow::Result<String>` — one signature across both tasks, called once from `main.rs`.
- `unit_report::{UnitReport, Intervention, CommitRow, build, render, render_json}` — defined Task 4, used only by `unit_cli::run`'s `Show` arm.
- `LogEventInput.subject: Option<&'a str>` — defined Task 3, filled at all five call sites with `subject.as_deref()` from an `Option<String>` binding, matching the struct's existing borrow style.
- `LifecycleStats::{work_items_explicit, work_items_implicit, work_items_completed, governed, subject_interventions}`, `interventions_per_work_item()`, `work_items()` — defined Task 5, used by `render_lifecycle` and `render_json_with_lifecycle` in the same task.
- Plan 1 API used unchanged: `LifecycleEvent::{new, with_mode, with_prompt, with_extra, to_log_entry, to_journal_record}`, `Host::Cli`, `Stamped`, `PromptText`, `record::{record, correction_text}`.
- Plan 5 API used unchanged: `LifecycleOpts`, `aggregate_lifecycle`, `render_lifecycle`, `retention_line`, `render_json_with_lifecycle`, and `kalpa_cli::report`'s surrounding code — only its `read_recent` arguments change.
- Action-log field names read here — `subject`, `kalpa`, `mode`, `sha`, `confidence_band`, `unit_id`, `spec`, `implicit`, `exit`, `consequences[].rule_id` — are exactly those `to_log_entry` (Plan 1 Task 4, Task 1 here) and `log_hook_event` (Task 3 here) write.

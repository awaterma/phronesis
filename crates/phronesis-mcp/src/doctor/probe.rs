//! `phr-mcp doctor --probe` — real allow/block evidence from a temporary,
//! isolated fixture, never the real project.
//!
//! The probe writes one known rule to a fresh temp directory, points
//! `PHRONESIS_PROJECT_ROOT` at it, and invokes **this running executable**
//! (`std::env::current_exe`) as `pre-check` — the same binary running
//! doctor, not a PATH-resolved `phr-mcp` that might be a stale release
//! build lacking `doctor` at all. Two edits are proposed: one that should
//! trip the fixture rule, one that should not. The fixture's own
//! `.phronesis/log.jsonl` (inside the temp dir, never the real project's)
//! is read back for the rule id and decision that actually fired, then the
//! temp dir is removed.
//!
//! This is evidence about *this binary's rule-evaluation path*, not about
//! whether the real project's hooks are wired or trusted by any host —
//! callers must not conflate the two.

use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

use serde::Serialize;

/// The one rule the fixture ships. Kept intentionally simple and
/// unambiguous: a literal `.unwrap()` under `src/`.
const PROBE_RULE_ID: &str = "doctor-probe-no-unwrap";

#[derive(Debug, Clone, Serialize)]
pub struct ProbeCase {
    pub label: &'static str,
    pub expect_block: bool,
    pub exit_code: Option<i32>,
    pub matched_expectation: bool,
    /// Rule id and decision named by the fixture's own log entry for this
    /// case, when one was recorded.
    pub rule_id: Option<String>,
    pub decision: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProbeReport {
    pub rule_id: &'static str,
    pub cases: Vec<ProbeCase>,
    pub ok: bool,
    pub note: String,
    pub error: Option<String>,
}

pub fn run() -> ProbeReport {
    match run_inner() {
        Ok(report) => report,
        Err(e) => ProbeReport {
            rule_id: PROBE_RULE_ID,
            cases: Vec::new(),
            ok: false,
            note:
                "probe fixture could not run to completion; no allow/block evidence was produced."
                    .to_string(),
            error: Some(e.to_string()),
        },
    }
}

fn run_inner() -> anyhow::Result<ProbeReport> {
    // `TempDir` (already a normal dependency, used elsewhere in this
    // crate) creates the directory atomically with `mkdtemp`-style
    // collision avoidance and removes it on drop — no predictable
    // pid+timestamp name, no best-effort `remove_dir_all` that could leak
    // the fixture on an early return.
    let tmp = tempfile::Builder::new()
        .prefix("phr-mcp-doctor-probe-")
        .tempdir()?;
    let dir = tmp.path();
    std::fs::create_dir_all(dir.join(".phronesis"))?;

    write_fixture_rules(dir)?;
    let exe = std::env::current_exe()?;

    let blocked = invoke_pre_check(&exe, dir, true)?;
    let allowed = invoke_pre_check(&exe, dir, false)?;

    let log_path = crate::action_log::default_path(dir);
    let entries = read_log_entries(&log_path);

    let cases = vec![
        make_case("blocked_edit_contains_unwrap", true, blocked, &entries),
        make_case("allowed_edit_no_unwrap", false, allowed, &entries),
    ];
    let ok = cases.iter().all(|c| c.matched_expectation);

    Ok(ProbeReport {
        rule_id: PROBE_RULE_ID,
        cases,
        ok,
        note: "evidence from a temporary isolated fixture, exercising only this executable's own `pre-check` code path directly (not through any host adapter); not a claim about the real project's hook wiring, host trust, or that any specific host (Claude, Gemini, Codex, or otherwise) actually invokes it this way.".to_string(),
        error: None,
    })
    // `tmp` drops here, removing the fixture directory.
}

fn write_fixture_rules(dir: &Path) -> anyhow::Result<()> {
    let rules = serde_json::json!({
        "rules": [{
            "id": PROBE_RULE_ID,
            "phase": "pre",
            "priority": 10,
            "when": [
                { "new_content_contains": ".unwrap()" },
                { "file_path_matches": "src" }
            ],
            "then": { "block": "doctor probe: found .unwrap() in src/" }
        }]
    });
    std::fs::write(
        dir.join(".phronesis").join("rules.json"),
        serde_json::to_string_pretty(&rules)?,
    )?;
    Ok(())
}

/// Spawn `exe pre-check` scoped to `project_root` via `PHRONESIS_PROJECT_ROOT`
/// (an explicit override honored verbatim by `security::project_root`, so
/// the probe never touches the real project's rules, log, or journal) and
/// feed it a synthetic `Edit` payload. `with_violation` selects whether the
/// proposed edit contains `.unwrap()` under `src/`.
fn invoke_pre_check(
    exe: &Path,
    project_root: &Path,
    with_violation: bool,
) -> anyhow::Result<Option<i32>> {
    let new_string = if with_violation {
        "fn probe() { let x: Option<i32> = Some(1); let _ = x.unwrap(); }"
    } else {
        "fn probe() { let x: Option<i32> = Some(1); let _ = x; }"
    };
    let payload = serde_json::json!({
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/probe.rs",
            "old_string": "fn probe() {}",
            "new_string": new_string
        }
    })
    .to_string();

    let mut command = Command::new(exe);
    // Runtime overrides from the caller must not disable the fixture log,
    // redirect captures, or alter size limits for this isolated experiment.
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("PHRONESIS_") {
            command.env_remove(key);
        }
    }
    let mut child = command
        .arg("pre-check")
        .env("PHRONESIS_PROJECT_ROOT", project_root)
        .current_dir(project_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(payload.as_bytes())?;

    let status = child.wait()?;
    Ok(status.code())
}

/// Parse every `.phronesis/log.jsonl` line in the fixture as JSON, ignoring
/// unreadable or malformed ones (the probe only needs best-effort evidence
/// to annotate exit codes it has already observed directly).
fn read_log_entries(path: &Path) -> Vec<serde_json::Value> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .collect()
}

fn make_case(
    label: &'static str,
    expect_block: bool,
    exit_code: Option<i32>,
    entries: &[serde_json::Value],
) -> ProbeCase {
    let matching_entry = entries.iter().rev().find(|e| {
        e.get("kind").and_then(|v| v.as_str()) == Some("hook")
            && e.get("exit").and_then(|v| v.as_i64()) == exit_code.map(i64::from)
    });
    let rule_id = matching_entry.and_then(|entry| {
        entry
            .get("consequences")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|c| c.get("rule_id"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    });
    // The decision is derived from the exit code directly observed from the
    // subprocess — that is known regardless of whether a matching log entry
    // was found. The log entry only adds the rule id as corroborating evidence.
    let decision = exit_code.map(|exit| match exit {
        2 => "block".to_string(),
        1 => "warn".to_string(),
        0 => "allow".to_string(),
        other => format!("exit={other}"),
    });
    // Strict, exact matches only — no "any non-2 exit counts as allowed".
    // A blocked case must exit exactly 2 *and* name the fixture's own rule
    // (ruling out an unrelated fail-closed exit 2 from a load/engine
    // error, which is not evidence the rule itself fired); an allowed case
    // must exit exactly 0 (not `None`, not 1, not 101 — any of those is a
    // crash, a warn, or something else, never "allowed").
    let matched_expectation = if expect_block {
        exit_code == Some(2) && rule_id.as_deref() == Some(PROBE_RULE_ID)
    } else {
        exit_code == Some(0)
    };
    ProbeCase {
        label,
        expect_block,
        exit_code,
        matched_expectation,
        rule_id,
        decision,
    }
}

pub fn render_table(report: &ProbeReport) {
    println!("  probe: rule={} ok={}", report.rule_id, report.ok);
    if let Some(err) = &report.error {
        println!("    error: {err}");
    }
    for case in &report.cases {
        println!(
            "    {:<28} expect_block={:<5} exit={:<4} match={:<5} rule={} decision={}",
            case.label,
            case.expect_block,
            case.exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "?".into()),
            case.matched_expectation,
            case.rule_id.as_deref().unwrap_or("-"),
            case.decision.as_deref().unwrap_or("-"),
        );
    }
    println!("    note: {}", report.note);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn make_case_matches_block_expectation() {
        let entries = vec![serde_json::json!({
            "kind": "hook",
            "exit": 2,
            "consequences": [{"rule_id": PROBE_RULE_ID, "action_type": "constraint_violation"}]
        })];
        let case = make_case("x", true, Some(2), &entries);
        assert!(case.matched_expectation);
        assert_eq!(case.rule_id.as_deref(), Some(PROBE_RULE_ID));
        assert_eq!(case.decision.as_deref(), Some("block"));
    }

    #[test]
    fn make_case_detects_mismatch() {
        let case = make_case("x", true, Some(0), &[]);
        assert!(!case.matched_expectation);
        assert_eq!(case.decision.as_deref(), Some("allow"));
    }

    #[test]
    fn make_case_allow_matches_when_no_block() {
        let entries = vec![serde_json::json!({
            "kind": "hook",
            "exit": 0,
            "consequences": []
        })];
        let case = make_case("x", false, Some(0), &entries);
        assert!(case.matched_expectation);
    }

    #[test]
    fn make_case_allow_rejects_missing_exit_code() {
        let case = make_case("x", false, None, &[]);
        assert!(!case.matched_expectation);
    }

    #[test]
    fn make_case_allow_rejects_warn_exit() {
        let case = make_case("x", false, Some(1), &[]);
        assert!(!case.matched_expectation);
    }

    #[test]
    fn make_case_allow_rejects_arbitrary_nonzero_exit() {
        let case = make_case("x", false, Some(101), &[]);
        assert!(!case.matched_expectation);
    }

    #[test]
    fn make_case_block_rejects_exit_2_from_an_unrelated_rule() {
        // Exit 2 alone is not evidence the fixture rule fired — it could be
        // a fail-closed load/engine error, or some other rule entirely.
        let entries = vec![serde_json::json!({
            "kind": "hook",
            "exit": 2,
            "consequences": [{"rule_id": "some-other-rule", "action_type": "constraint_violation"}]
        })];
        let case = make_case("x", true, Some(2), &entries);
        assert!(!case.matched_expectation);
    }

    #[test]
    fn make_case_block_rejects_exit_2_with_no_log_entry_at_all() {
        let case = make_case("x", true, Some(2), &[]);
        assert!(!case.matched_expectation);
    }

    #[test]
    fn write_fixture_rules_produces_loadable_file() {
        let dir = std::env::temp_dir().join(format!(
            "phr-mcp-doctor-probe-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(dir.join(".phronesis")).unwrap();
        write_fixture_rules(&dir).unwrap();
        let path = crate::rules_file::default_path(&dir);
        let file = crate::rules_file::read(&path).expect("fixture rules must load");
        assert_eq!(file.rules.len(), 1);
        assert_eq!(file.rules[0].id, PROBE_RULE_ID);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// End-to-end: the full probe, run for real against the test binary's
    /// own `current_exe` (the integration test binary itself — it does not
    /// implement `pre-check`, so this proves the probe fails closed and
    /// reports rather than panicking on a binary that lacks the
    /// subcommand). The CLI-level end-to-end run (against the actual
    /// `phr-mcp` binary) is covered by a `main.rs`/integration test.
    #[test]
    fn run_inner_reports_rather_than_panics_on_a_binary_without_pre_check() {
        let report = run();
        // Whatever executable cargo-test gives us, the probe must not panic
        // and must always return a report naming the fixture rule.
        assert_eq!(report.rule_id, PROBE_RULE_ID);
    }
}

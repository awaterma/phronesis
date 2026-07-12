//! Data-driven contract runner that replays payload fixtures through the real
//! `phr-mcp` binary and asserts exit code, stdout-JSON, action-log, and
//! journey-journal effects.
//!
//! See `docs/superpowers/specs/2026-07-06-payload-contract-corpus-design.md`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// One fixture file under `tests/fixtures/payloads/<cli>/<name>.json`.
#[derive(serde::Deserialize, Debug)]
struct Fixture {
    schema: u32,
    #[allow(dead_code)]
    source: serde_json::Value,
    subcommand: String,
    packs: String,
    #[serde(rename = "payload")]
    payload: serde_json::Value,
    expect: Expect,
}

/// Assertions attached to each fixture.
#[derive(serde::Deserialize, Debug)]
struct Expect {
    exit: i32,
    stdout_json: bool,
    /// The named rule id must appear in the `consequences` array of the
    /// log entry emitted by this hook invocation.  Liveness: proves the rule
    /// matched *this* payload.
    log_rule_fired: Option<String>,
    /// Tags that must appear on a **fresh** journal record (not pre-existing
    /// from `init` scaffolding).
    #[serde(default)]
    journal_tag_new: Vec<String>,
    /// Tags that must appear on a **fresh** journal record — and whose
    /// derivation requires reading the tool-output field (the `outcome:*`
    /// tags from the confidence adapter).
    #[serde(default)]
    journal_tag_from_output: Vec<String>,
    /// Each substring must appear on stderr.
    #[serde(default)]
    stderr_contains: Vec<String>,
}

fn collect_fixtures() -> Vec<PathBuf> {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/payloads");
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&base) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                for f in std::fs::read_dir(&path).into_iter().flatten().flatten() {
                    let fp = f.path();
                    if fp.extension().is_some_and(|e| e == "json") {
                        out.push(fp);
                    }
                }
            }
        }
    }
    out.sort();
    out
}

/// Run `phr-mcp init --packs <packs>` in a temp project.
fn init_project(dir: &tempfile::TempDir, packs: &str) {
    let bin = env!("CARGO_BIN_EXE_phr-mcp");
    let output = Command::new(bin)
        .current_dir(dir.path())
        .arg("init")
        .arg(dir.path())
        .arg("--packs")
        .arg(packs)
        .arg("--force")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn phr-mcp init");
    assert!(
        output.status.success(),
        "init failed: {}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
}

/// Run a subcommand against a temp project with the given payload on stdin.
fn run_subcommand(
    dir: &tempfile::TempDir,
    subcommand: &str,
    payload: &str,
) -> (i32, String, String) {
    let bin = env!("CARGO_BIN_EXE_phr-mcp");
    let mut cmd = Command::new(bin);
    cmd.current_dir(dir.path())
        .arg(subcommand)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().expect("spawn hook");
    let mut stdin = child.stdin.take().expect("stdin handle");
    stdin
        .write_all(payload.to_string().as_bytes())
        .expect("write payload");
    drop(stdin);

    let output = child.wait_with_output().expect("wait");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (output.status.code().unwrap_or(-1), stdout, stderr)
}

/// Read lines from `.phronesis/journey/events.jsonl`.
fn journal_lines(dir: &Path) -> Vec<String> {
    let path = dir.join(".phronesis/journey/events.jsonl");
    std::fs::read_to_string(&path)
        .ok()
        .map(|s| s.lines().map(String::from).collect())
        .unwrap_or_default()
}

/// Read fresh journey records (after baseline) as parsed JSON objects.
fn fresh_records(
    dir: &Path,
    baseline: &std::collections::HashSet<String>,
) -> Vec<serde_json::Value> {
    let path = dir.join(".phronesis/journey/events.jsonl");
    std::fs::read_to_string(&path)
        .ok()
        .map(|content| {
            content
                .lines()
                .filter_map(|line| {
                    let val: serde_json::Value = serde_json::from_str(line).ok()?;
                    if baseline.contains(line) {
                        return None;
                    }
                    Some(val)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn fresh_record_has_tag(records: &[serde_json::Value], tag: &str) -> bool {
    records.iter().any(|r| {
        r["tags"]
            .as_array()
            .is_some_and(|tags| tags.iter().any(|t| t == tag))
    })
}

fn check_fixture(path: &Path) -> Result<(), String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("read: {e}"))?;
    let fx: Fixture = serde_json::from_str(&raw).map_err(|e| format!("parse: {e}"))?;
    if fx.schema != 1 {
        return Err(format!("unsupported fixture schema {}", fx.schema));
    }
    let dir = tempfile::tempdir().map_err(|e| format!("tempdir: {e}"))?;
    init_project(&dir, &fx.packs);

    // Baseline: journal lines that exist AFTER init but BEFORE the hook
    // runs.  Freshness guard so `init` scaffolding can't produce false-green.
    let baseline: std::collections::HashSet<String> =
        journal_lines(&dir.path()).into_iter().collect();

    // Path hermeticity: rewrite `/home/dev/project` prefix in file_path
    // fields to this temp project's root so an absolute path resolves
    // under the temp tree.
    let root = dir.path().display().to_string();
    let payload = fx.payload.to_string().replace("/home/dev/project", &root);

    let (code, stdout, stderr) = run_subcommand(&dir, &fx.subcommand, &payload);

    if code != fx.expect.exit {
        return Err(format!(
            "exit {code}, expected {} (stderr: {})",
            fx.expect.exit, stderr
        ));
    }
    if fx.expect.stdout_json && serde_json::from_str::<serde_json::Value>(stdout.trim()).is_err() {
        return Err(format!("stdout is not parseable JSON: {stdout:?}"));
    }
    for needle in &fx.expect.stderr_contains {
        if !stderr.contains(needle) {
            return Err(format!("stderr missing {:?} (stderr: {})", needle, stderr));
        }
    }

    // log_rule_fired: the named rule appears in a log entry's consequences.
    if let Some(rule) = &fx.expect.log_rule_fired {
        let log =
            std::fs::read_to_string(dir.path().join(".phronesis/log.jsonl")).unwrap_or_default();
        let fired = log
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter_map(|e| e["consequences"].as_array().cloned())
            .flatten()
            .any(|c| c["rule_id"] == rule.as_str());
        if !fired {
            return Err(format!(
                "rule {:?} never appears in any log entry's consequences \
                 (silent no-op)",
                rule
            ));
        }
    }

    // Journal-tag assertions against fresh records only.
    if !fx.expect.journal_tag_new.is_empty() || !fx.expect.journal_tag_from_output.is_empty() {
        let records = fresh_records(&dir.path(), &baseline);
        for tag in &fx.expect.journal_tag_new {
            if !fresh_record_has_tag(&records, tag) {
                return Err(format!(
                    "no FRESH journal record tagged {:?} \
                     (fresh records: {})",
                    tag,
                    records.len()
                ));
            }
        }
        for tag in &fx.expect.journal_tag_from_output {
            if !fresh_record_has_tag(&records, tag) {
                return Err(format!(
                    "no FRESH journal record tagged {:?} \
                     (fresh records: {})",
                    tag,
                    records.len()
                ));
            }
        }
    }
    Ok(())
}

#[test]
fn corpus_replays_green() {
    let mut failures = Vec::new();
    for path in collect_fixtures() {
        if let Err(msg) = check_fixture(&path) {
            failures.push(format!("{}: {msg}", path.display()));
        }
    }
    assert!(
        failures.is_empty(),
        "payload-contract failures:\n{}",
        failures.join("\n")
    );
}

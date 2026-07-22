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

/// Allowed fixture provenance values (spec Task 3). Any other string in
/// `source.provenance` fails fixture loading — unknown provenance must never
/// be silently accepted, and authored data must never pass as captured.
#[derive(serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum Provenance {
    Authored,
    Captured,
    CapturedAndScrubbed,
}

impl Provenance {
    fn is_captured(self) -> bool {
        matches!(self, Provenance::Captured | Provenance::CapturedAndScrubbed)
    }

    fn label(self) -> &'static str {
        match self {
            Provenance::Authored => "authored",
            Provenance::Captured => "captured",
            Provenance::CapturedAndScrubbed => "captured-and-scrubbed",
        }
    }
}

/// Validate the `source` block of a fixture envelope.
///
/// - `source.provenance` must be one of `authored | captured |
///   captured-and-scrubbed`; anything else is an error naming the file.
/// - Captured provenance additionally requires a `source.capture` object with
///   non-sensitive metadata: `host`, `capture_date`, `scrubber_version`
///   (non-empty strings) and `host_version` under an explicit-null policy —
///   the key must be present; `null` means "unknown at capture time".
///
/// Authored fixtures need no capture metadata: they are hand-written
/// approximations, and must never be relabeled as captured.
fn validate_provenance(fixture: &str, source: &serde_json::Value) -> Result<Provenance, String> {
    let raw = source.get("provenance").ok_or_else(|| {
        format!(
            "{fixture}: source.provenance is missing \
             (allowed: authored | captured | captured-and-scrubbed)"
        )
    })?;
    let provenance: Provenance = serde_json::from_value(raw.clone()).map_err(|_| {
        format!(
            "{fixture}: unknown provenance {raw} \
             (allowed: authored | captured | captured-and-scrubbed)"
        )
    })?;
    if !provenance.is_captured() {
        return Ok(provenance);
    }
    let capture = source
        .get("capture")
        .and_then(|c| c.as_object())
        .ok_or_else(|| {
            format!(
                "{fixture}: provenance {:?} requires a source.capture object with \
                 host, host_version, capture_date, scrubber_version",
                provenance.label()
            )
        })?;
    for key in ["host", "capture_date", "scrubber_version"] {
        match capture.get(key).and_then(|v| v.as_str()) {
            Some(s) if !s.trim().is_empty() => {}
            _ => {
                return Err(format!(
                    "{fixture}: source.capture.{key} must be a non-empty string \
                     for provenance {:?}",
                    provenance.label()
                ));
            }
        }
    }
    match capture.get("host_version") {
        Some(v) if v.is_null() || v.as_str().is_some_and(|s| !s.trim().is_empty()) => {}
        Some(_) => {
            return Err(format!(
                "{fixture}: source.capture.host_version must be a non-empty string \
                 or explicit null (unknown), for provenance {:?}",
                provenance.label()
            ));
        }
        None => {
            return Err(format!(
                "{fixture}: source.capture.host_version key is required (use explicit \
                 null when the host version is unknown), for provenance {:?}",
                provenance.label()
            ));
        }
    }
    Ok(provenance)
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
    assert!(
        !out.is_empty(),
        "corpus must not be empty — no fixtures found under {}",
        base.display()
    );
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
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("fixture");
    validate_provenance(name, &fx.source)?;
    let dir = tempfile::tempdir().map_err(|e| format!("tempdir: {e}"))?;
    init_project(&dir, &fx.packs);

    // Baseline: journal lines that exist AFTER init but BEFORE the hook
    // runs.  Freshness guard so `init` scaffolding can't produce false-green.
    let baseline: std::collections::HashSet<String> =
        journal_lines(dir.path()).into_iter().collect();

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
        let records = fresh_records(dir.path(), &baseline);
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

/// The hook-event-name registry: the exhaustive list of event names each CLI
/// actually dispatches. Future host additions must extend this fixture in the
/// same PR that adds wiring.
fn registry() -> serde_json::Value {
    let raw = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hook_events.json"),
    )
    .expect("read hook_events.json");
    serde_json::from_str(&raw).expect("registry is JSON")
}

fn hook_keys(settings: &serde_json::Value) -> Vec<String> {
    settings["hooks"]
        .as_object()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

#[test]
fn init_wires_hooks_only_under_event_names_that_exist() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_project(&dir, "llm");
    let reg = registry();

    let claude: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".claude/settings.local.json"))
            .expect("claude settings"),
    )
    .expect("claude settings JSON");
    let valid: Vec<&str> = reg["claude-code"]
        .as_array()
        .expect("list")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    let keys = hook_keys(&claude);
    assert!(!keys.is_empty(), "init wrote no Claude Code hooks at all");
    for key in keys {
        assert!(
            valid.contains(&key.as_str()),
            "init wired unknown Claude Code hook event {key:?}"
        );
    }

    let gemini: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".gemini/settings.json"))
            .expect("gemini settings"),
    )
    .expect("gemini settings JSON");
    let valid: Vec<&str> = reg["gemini"]
        .as_array()
        .expect("list")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    let keys = hook_keys(&gemini);
    assert!(!keys.is_empty(), "init wrote no Gemini hooks at all");
    for key in keys {
        assert!(
            valid.contains(&key.as_str()),
            "init wired unknown Gemini hook event {key:?}"
        );
    }

    let codex: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".codex/hooks.json")).expect("codex hooks"),
    )
    .expect("codex hooks JSON");
    let valid: Vec<&str> = reg["codex"]
        .as_array()
        .expect("list")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    let keys = hook_keys(&codex);
    assert!(!keys.is_empty(), "init wrote no Codex hooks at all");
    for key in keys {
        assert!(
            valid.contains(&key.as_str()),
            "init wired unknown Codex hook event {key:?}"
        );
    }
}

#[test]
fn before_model_request_never_reappears() {
    // 0.17.1 regression, pinned by name: this event does not exist in
    // Gemini CLI; wiring under it made per-turn injection silently never run.
    let dir = tempfile::tempdir().expect("tempdir");
    init_project(&dir, "llm");
    let gemini =
        std::fs::read_to_string(dir.path().join(".gemini/settings.json")).expect("gemini settings");
    assert!(
        !gemini.contains("BeforeModelRequest"),
        "dead Gemini hook event resurfaced"
    );
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

/// Coverage report (spec Task 3, requirement 4-6): per-host counts that
/// distinguish internal contract coverage (authored fixtures) from
/// host-observed coverage (captured fixtures). A host with zero captured
/// fixtures is reported informationally — this test must NOT fail merely
/// because live fixtures are unavailable. Run with `-- --nocapture` to see
/// the report on a green run.
#[test]
fn provenance_coverage_report() {
    use std::collections::BTreeMap;
    let reg = registry();
    let mut authored: BTreeMap<String, usize> = BTreeMap::new();
    let mut captured: BTreeMap<String, usize> = BTreeMap::new();
    // Seed every registry host so zero-coverage hosts still appear.
    for host in reg.as_object().expect("registry is an object").keys() {
        authored.insert(host.clone(), 0);
        captured.insert(host.clone(), 0);
    }
    let mut errors = Vec::new();
    for path in collect_fixtures() {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("fixture")
            .to_string();
        let raw = std::fs::read_to_string(&path).expect("read fixture");
        let val: serde_json::Value = serde_json::from_str(&raw).expect("fixture is JSON");
        let host = val["source"]["cli"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();
        match validate_provenance(&name, &val["source"]) {
            Ok(p) if p.is_captured() => *captured.entry(host).or_insert(0) += 1,
            Ok(_) => *authored.entry(host).or_insert(0) += 1,
            Err(msg) => errors.push(msg),
        }
    }
    assert!(
        errors.is_empty(),
        "fixture provenance validation failures:\n{}",
        errors.join("\n")
    );
    println!("payload-corpus provenance coverage:");
    for (host, n) in &authored {
        println!("  internal contract coverage (authored)   {host}: {n}");
    }
    for (host, n) in &captured {
        println!("  host-observed coverage (captured)       {host}: {n}");
        if *n == 0 {
            println!(
                "  NOTE: host {host:?} has ZERO captured fixtures — authored fixtures \
                 validate internal assumptions only, not the live host payload contract \
                 (informational, not a failure; see docs/payload-corpus-promotion.md)"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Provenance enforcement unit tests (spec Task 3, requirements 1-3).
//
// No captured fixtures exist yet, and none may be fabricated — enforcement is
// therefore proven with inline JSON, not fixture files.
// ---------------------------------------------------------------------------

#[test]
fn authored_provenance_requires_no_capture_metadata() {
    let source = serde_json::json!({
        "cli": "claude-code",
        "event": "PreToolUse",
        "provenance": "authored"
    });
    let p = validate_provenance("authored.json", &source).expect("authored must load");
    assert_eq!(p, Provenance::Authored);
}

#[test]
fn unknown_provenance_fails_loading_and_names_the_file() {
    let source = serde_json::json!({ "provenance": "live-totally-real" });
    let err = validate_provenance("bad-provenance.json", &source)
        .expect_err("unknown provenance must fail loading");
    assert!(
        err.contains("bad-provenance.json"),
        "error must name the file: {err}"
    );
    assert!(
        err.contains("unknown provenance"),
        "unexpected message: {err}"
    );
    assert!(
        err.contains("captured-and-scrubbed"),
        "error must list allowed values: {err}"
    );
}

#[test]
fn missing_provenance_fails_loading() {
    let source = serde_json::json!({ "cli": "gemini", "event": "BeforeTool" });
    let err = validate_provenance("no-provenance.json", &source)
        .expect_err("missing provenance must fail loading");
    assert!(
        err.contains("no-provenance.json"),
        "error must name the file: {err}"
    );
    assert!(err.contains("missing"), "unexpected message: {err}");
}

#[test]
fn captured_without_metadata_is_rejected() {
    let source = serde_json::json!({ "cli": "claude-code", "provenance": "captured" });
    let err = validate_provenance("captured-bare.json", &source)
        .expect_err("captured without metadata must fail");
    assert!(err.contains("source.capture"), "unexpected message: {err}");
}

#[test]
fn captured_with_full_metadata_is_accepted() {
    let source = serde_json::json!({
        "cli": "claude-code",
        "provenance": "captured",
        "capture": {
            "host": "claude-code",
            "host_version": "2.1.0",
            "capture_date": "2026-07-12",
            "scrubber_version": "1"
        }
    });
    let p = validate_provenance("captured-full.json", &source)
        .expect("captured with full metadata must load");
    assert_eq!(p, Provenance::Captured);
}

#[test]
fn captured_and_scrubbed_with_null_host_version_is_accepted() {
    // Explicit-null policy: the host_version key must be present; null means
    // "unknown at capture time".
    let source = serde_json::json!({
        "cli": "gemini",
        "provenance": "captured-and-scrubbed",
        "capture": {
            "host": "gemini",
            "host_version": null,
            "capture_date": "2026-07-12",
            "scrubber_version": "1"
        }
    });
    let p = validate_provenance("scrubbed-null-version.json", &source)
        .expect("explicit-null host_version must load");
    assert_eq!(p, Provenance::CapturedAndScrubbed);
}

#[test]
fn captured_missing_host_version_key_is_rejected() {
    let source = serde_json::json!({
        "cli": "gemini",
        "provenance": "captured-and-scrubbed",
        "capture": {
            "host": "gemini",
            "capture_date": "2026-07-12",
            "scrubber_version": "1"
        }
    });
    let err = validate_provenance("scrubbed-no-version-key.json", &source)
        .expect_err("missing host_version key must fail");
    assert!(err.contains("host_version"), "unexpected message: {err}");
}

#[test]
fn captured_with_empty_host_is_rejected() {
    let source = serde_json::json!({
        "cli": "claude-code",
        "provenance": "captured",
        "capture": {
            "host": "",
            "host_version": "2.1.0",
            "capture_date": "2026-07-12",
            "scrubber_version": "1"
        }
    });
    let err =
        validate_provenance("captured-empty-host.json", &source).expect_err("empty host must fail");
    assert!(
        err.contains("source.capture.host"),
        "unexpected message: {err}"
    );
}

#[test]
fn captured_missing_scrubber_version_is_rejected() {
    let source = serde_json::json!({
        "cli": "claude-code",
        "provenance": "captured",
        "capture": {
            "host": "claude-code",
            "host_version": "2.1.0",
            "capture_date": "2026-07-12"
        }
    });
    let err = validate_provenance("captured-no-scrubber.json", &source)
        .expect_err("missing scrubber_version must fail");
    assert!(
        err.contains("scrubber_version"),
        "unexpected message: {err}"
    );
}

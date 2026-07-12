# Payload-Contract Corpus Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Pin what Claude Code and Gemini CLI actually send to phr-mcp hooks with a committed, anonymized fixture corpus, and assert every hook produces its expected observable effect (no more silent no-ops).

**Architecture:** Four pieces per `docs/superpowers/specs/2026-07-06-payload-contract-corpus-design.md`: (1) an env-gated raw-payload capture tee in `hook::read_payload`, (2) a `phr-mcp scrub-payload` anonymizer for curating captures into committable fixtures, (3) a data-driven contract runner that replays each fixture through the real binary in a temp project and asserts exit code, stdout-JSON, action-log, and journey-journal effects, (4) a hook-event-name registry test that pins `init`'s wiring to event names that exist.

**Tech Stack:** Rust (edition 2024), serde_json, thiserror, clap 4, existing `CARGO_BIN_EXE_phr-mcp` integration-test harness pattern (see `crates/phronesis-mcp/tests/hook_integration.rs`).

## Global Constraints

- No `.unwrap()` / `.expect()` / `panic!()` in `crates/*/src/**` (tests exempt) — enforced by `enforce-no-unwrap-in-src` and friends. Use `let-else`, `ok()`, `?`.
- No `Result<_, String>` returns in src — use `thiserror` (enforced by `enforce-no-result-string-error`).
- All cargo invocations use `--workspace` (enforced by `warn-cargo-build-without-workspace`).
- Never pipe `cargo test` output through `grep`/`head` — it destroys the summary lines the confidence gate parses (ADR 2026-07-06-piped-test-output-loses-signal). Run it bare.
- Capture and logging are best-effort: they must never change a hook's exit code (contract stated at `hook/mod.rs:296-298`).
- Workspace version bumps to **0.18.0** lockstep (`[workspace.package] version` in root `Cargo.toml`); internal path-dep version pins on `phr`/`phronesis-rhai` in `crates/phronesis-mcp/Cargo.toml` bump to match.
- Commit messages end with `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`. Nothing is pushed; human reviews before any push.

## Corpus limitation (discovered while authoring this plan)

The llm-pack deflection rules are **content rules with no path gate** — they
fire on any file containing a blocked phrase. A fixture whose payload embeds
a deflection phrase therefore cannot even be *written* in this repo: the
PreToolUse hook blocks the Write of the fixture file itself (this happened
to the first draft of this plan). The starter corpus consequently contains
**no deflection-phrase fixture**; the exit-2 contract path is covered by the
unwrap-block fixture instead. Adding deflection fixtures needs a
human-approved scope decision first (e.g. exclude
`tests/fixtures/payloads/` from llm content rules via a `file_path_matches`
exclusion, recorded as an ADR) — proposed separately, not part of this plan.

---

### Task 1: Capture tee (`PHRONESIS_CAPTURE_DIR`)

**Files:**
- Modify: `crates/phronesis-mcp/src/hook/mod.rs:77-81` (`read_payload`), plus its two call sites `crates/phronesis-mcp/src/hook/pre.rs:22` and `crates/phronesis-mcp/src/hook/post.rs:28`
- Test: `crates/phronesis-mcp/tests/payload_capture.rs` (new)

**Interfaces:**
- Consumes: `security::read_stdin_capped()` (existing), `unix_secs_now()` (existing in `hook/mod.rs`)
- Produces: `read_payload(phase: &str) -> anyhow::Result<HookPayload>` (signature gains the phase arg); capture record shape `{"ts": u64, "phase": "pre"|"post", "raw": <json|string>}` appended to `$PHRONESIS_CAPTURE_DIR/payloads.jsonl` — Task 2's scrubber and the curation workflow read this shape.

- [ ] **Step 1: Write the failing integration test**

```rust
// crates/phronesis-mcp/tests/payload_capture.rs
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

fn run_hook_with_env(subcommand: &str, payload: &str, envs: &[(&str, &str)]) -> i32 {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_phr-mcp"));
    cmd.arg(subcommand)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("spawn hook");
    let mut stdin = child.stdin.take().expect("stdin handle");
    stdin.write_all(payload.as_bytes()).expect("write payload");
    drop(stdin);
    let output = child.wait_with_output().expect("wait");
    output.status.code().unwrap_or(-1)
}

fn read_capture(dir: &Path) -> Vec<serde_json::Value> {
    let raw = std::fs::read_to_string(dir.join("payloads.jsonl")).unwrap_or_default();
    raw.lines()
        .map(|l| serde_json::from_str(l).expect("capture line is JSON"))
        .collect()
}

#[test]
fn capture_dir_set_tees_raw_payload() {
    let dir = tempfile::tempdir().expect("tempdir");
    let payload = r#"{"tool_name": "Read", "tool_input": {"file_path": "src/main.rs"}, "session_id": "abc"}"#;
    let code = run_hook_with_env(
        "pre-check",
        payload,
        &[("PHRONESIS_CAPTURE_DIR", dir.path().to_str().expect("utf8"))],
    );
    assert_eq!(code, 0, "capture must not change hook behavior");

    let records = read_capture(dir.path());
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["phase"], "pre");
    // Raw payload preserved verbatim, including fields HookPayload ignores.
    assert_eq!(records[0]["raw"]["session_id"], "abc");
    assert_eq!(records[0]["raw"]["tool_name"], "Read");
}

#[test]
fn capture_appends_across_invocations_and_stamps_phase() {
    let dir = tempfile::tempdir().expect("tempdir");
    let envs = [("PHRONESIS_CAPTURE_DIR", dir.path().to_str().expect("utf8"))];
    run_hook_with_env("pre-check", r#"{"tool_name": "Read", "tool_input": {}}"#, &envs);
    run_hook_with_env("post-check", r#"{"tool_name": "Read", "tool_input": {}}"#, &envs);

    let records = read_capture(dir.path());
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["phase"], "pre");
    assert_eq!(records[1]["phase"], "post");
}

#[test]
fn capture_unset_writes_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    run_hook_with_env("pre-check", r#"{"tool_name": "Read", "tool_input": {}}"#, &[]);
    assert!(
        !dir.path().join("payloads.jsonl").exists(),
        "no capture file without the env var"
    );
}

#[test]
fn capture_preserves_non_json_stdin_as_string() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Malformed payload: hook exits 0 (allow) per existing behavior, but the
    // capture must still record the raw bytes — broken payloads are exactly
    // what we want ground truth on.
    run_hook_with_env(
        "pre-check",
        "not json at all",
        &[("PHRONESIS_CAPTURE_DIR", dir.path().to_str().expect("utf8"))],
    );
    let records = read_capture(dir.path());
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["raw"], "not json at all");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --workspace --test payload_capture`
Expected: FAIL — `payloads.jsonl` never created (capture code doesn't exist yet).

- [ ] **Step 3: Implement the tee in `hook/mod.rs`**

Replace `read_payload` (lines 77-81) with:

```rust
fn read_payload(phase: &str) -> anyhow::Result<HookPayload> {
    let input = security::read_stdin_capped()?;
    capture_raw_payload(phase, &input);
    let payload: HookPayload = serde_json::from_str(&input)?;
    Ok(payload)
}

/// When `PHRONESIS_CAPTURE_DIR` is set, append the raw stdin payload as one
/// JSONL record to `<dir>/payloads.jsonl`. Best-effort: capture must never
/// change hook behavior or exit codes, so every failure path returns silently.
/// This is the harvesting side of the payload-contract corpus — see
/// `docs/superpowers/specs/2026-07-06-payload-contract-corpus-design.md`.
fn capture_raw_payload(phase: &str, raw: &str) {
    let Ok(dir) = std::env::var("PHRONESIS_CAPTURE_DIR") else {
        return;
    };
    let record = serde_json::json!({
        "ts": unix_secs_now(),
        "phase": phase,
        "raw": serde_json::from_str::<serde_json::Value>(raw)
            .unwrap_or_else(|_| serde_json::Value::String(raw.to_string())),
    });
    let path = std::path::Path::new(&dir).join("payloads.jsonl");
    let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    use std::io::Write as _;
    let _ = writeln!(file, "{record}");
}
```

Update the two call sites: `super::read_payload()` → `super::read_payload("pre")` in `pre.rs:22`, `super::read_payload("post")` in `post.rs:28`. Note the capture call sits **before** `serde_json::from_str`, so malformed payloads are still captured.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --workspace --test payload_capture`
Expected: 4 passed. Also run `cargo test --workspace --test hook_integration` — expected: all pass (behavior unchanged when the var is unset).

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/hook/mod.rs crates/phronesis-mcp/src/hook/pre.rs crates/phronesis-mcp/src/hook/post.rs crates/phronesis-mcp/tests/payload_capture.rs
git commit -m "feat(hook): PHRONESIS_CAPTURE_DIR tees raw payloads to payloads.jsonl

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: Scrub module (`payload_scrub.rs`)

**Files:**
- Create: `crates/phronesis-mcp/src/payload_scrub.rs`
- Modify: `crates/phronesis-mcp/src/lib.rs` (add `pub mod payload_scrub;` alongside the existing module list)

**Interfaces:**
- Consumes: nothing project-specific (pure `serde_json`).
- Produces: `pub struct Scrubber` with `pub fn new(home: &str, project_root: &str) -> Scrubber`, `pub fn scrub_value(&mut self, v: &mut serde_json::Value)`, `pub fn verify(&self, v: &serde_json::Value) -> Result<(), ScrubError>` (hard leaks only), `pub fn warnings(&self, v: &serde_json::Value) -> Vec<String>` (soft residuals for human review); `pub enum ScrubError` (thiserror). Task 3's CLI handler calls all four.

- [ ] **Step 1: Write the file — tests first, implementation above them in the same pass**

The unit tests that define the behavior:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scrubber() -> Scrubber {
        Scrubber::new("/Users/alicejones", "/Users/alicejones/Git/myproject")
    }

    #[test]
    fn project_root_paths_become_home_dev_project() {
        let mut v = json!({"tool_input": {"file_path": "/Users/alicejones/Git/myproject/src/lib.rs"}});
        scrubber().scrub_value(&mut v);
        assert_eq!(v["tool_input"]["file_path"], "/home/dev/project/src/lib.rs");
    }

    #[test]
    fn external_home_paths_get_indexed_placeholders() {
        let mut v = json!({
            "a": "/Users/alicejones/Git/otherrepo/src/main.rs",
            "b": "/Users/alicejones/Git/otherrepo/src/main.rs",
            "c": "/Users/alicejones/.cargo/bin/tool"
        });
        let mut s = scrubber();
        s.scrub_value(&mut v);
        // Same external path → same placeholder; different path → different index.
        assert_eq!(v["a"], v["b"]);
        assert_ne!(v["a"], v["c"]);
        let a = v["a"].as_str().expect("string");
        assert!(a.starts_with("/home/dev/external/p"), "got {a}");
        // The sibling repo name must be gone entirely.
        assert!(!v.to_string().contains("otherrepo"));
    }

    #[test]
    fn username_is_replaced_everywhere() {
        let mut v = json!({"command": "echo hello alicejones"});
        scrubber().scrub_value(&mut v);
        assert_eq!(v["command"], "echo hello dev");
    }

    #[test]
    fn session_id_and_transcript_path_keys_get_fixed_placeholders() {
        let mut v = json!({
            "session_id": "550e8400-e29b-41d4-a716-446655440000",
            "transcript_path": "/Users/alicejones/.claude/projects/x/y.jsonl",
            "nested": {"session_id": "another-id"}
        });
        scrubber().scrub_value(&mut v);
        assert_eq!(v["session_id"], "sess-00000000");
        assert_eq!(v["transcript_path"], "/home/dev/.claude/transcript.jsonl");
        assert_eq!(v["nested"]["session_id"], "sess-00000000");
    }

    #[test]
    fn case_and_separator_variant_id_keys_are_scrubbed() {
        // Finding #3: a CLI sending camelCase or no-separator keys must not leak.
        let mut v = json!({
            "sessionId": "abc",
            "SessionID": "def",
            "transcriptPath": "/Users/alicejones/.claude/t.jsonl"
        });
        scrubber().scrub_value(&mut v);
        assert_eq!(v["sessionId"], "sess-00000000");
        assert_eq!(v["SessionID"], "sess-00000000");
        assert_eq!(v["transcriptPath"], "/home/dev/.claude/transcript.jsonl");
    }

    #[test]
    fn scrub_is_idempotent() {
        let mut v = json!({"file_path": "/Users/alicejones/Git/myproject/src/a.rs", "session_id": "x"});
        let mut s = scrubber();
        s.scrub_value(&mut v);
        let once = v.clone();
        s.scrub_value(&mut v);
        assert_eq!(v, once);
    }

    #[test]
    fn project_internal_content_is_untouched() {
        let mut v = json!({"tool_input": {"new_string": "fn main() { let x = 1; }", "file_path": "src/lib.rs"}});
        let before = v.clone();
        scrubber().scrub_value(&mut v);
        assert_eq!(v, before, "relative paths and code content must pass through verbatim");
    }

    #[test]
    fn verify_flags_residual_home_path() {
        let s = scrubber();
        let v = json!({"sneaky": "path is /Users/alicejones/secret"});
        assert!(s.verify(&v).is_err());
        let clean = json!({"ok": "/home/dev/project/src/lib.rs"});
        assert!(s.verify(&clean).is_ok());
    }

    #[test]
    fn verify_flags_username_as_path_component_but_not_as_free_token() {
        // Finding #1 resolution: username in a path is a hard leak; username
        // as a free-text word is a warning, not a verify failure — otherwise
        // a correctly-scrubbed fixture whose content mentions the word can
        // never pass, breaking idempotence.
        let s = scrubber();
        // Path component → Err.
        let leak = json!({"p": "/opt/alicejones/thing"});
        assert!(s.verify(&leak).is_err());
        // Free-text token → Ok from verify, but surfaced by warnings().
        let token = json!({"command": "echo alicejones was here"});
        assert!(s.verify(&token).is_ok());
        assert_eq!(s.warnings(&token).len(), 1);
        // Nothing to say about clean content.
        let clean = json!({"command": "cargo build"});
        assert!(s.warnings(&clean).is_empty());
    }

    #[test]
    fn short_usernames_are_not_blindly_replaced() {
        // A 1-2 char username would shred ordinary text; the scrubber must
        // refuse to substring-replace it and rely on path rules only.
        let mut s = Scrubber::new("/home/al", "/home/al/proj");
        let mut v = json!({"command": "cargo align --all"});
        s.scrub_value(&mut v);
        assert_eq!(v["command"], "cargo align --all");
        // ...and verify/warnings must not fire on the short name either.
        assert!(s.verify(&v).is_ok());
        assert!(s.warnings(&v).is_empty());
    }
}
```

The implementation:

```rust
//! Anonymize captured hook payloads before committing them as fixtures.
//!
//! Scrubs exactly the class of data that reaches outside the project —
//! `$HOME` paths, the OS username, session ids, transcript paths — and
//! leaves project-internal content byte-for-byte intact. See
//! `docs/superpowers/specs/2026-07-06-payload-contract-corpus-design.md`.

use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ScrubError {
    #[error("scrubbed output still contains the {what} in: {context}")]
    Residual { what: &'static str, context: String },
}

/// Minimum username length for bare-substring replacement. Shorter names
/// would corrupt ordinary text ("al" inside "align"); path-prefix rules
/// still cover them because paths embed the full `$HOME` prefix.
const MIN_BARE_USERNAME_LEN: usize = 3;

pub struct Scrubber {
    home: String,
    user: String,
    project_root: String,
    /// Unique external paths seen so far; index = placeholder number, so
    /// the same path always maps to the same `/home/dev/external/pN`.
    external: Vec<String>,
}

impl Scrubber {
    pub fn new(home: &str, project_root: &str) -> Self {
        let user = std::path::Path::new(home)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self {
            home: home.trim_end_matches('/').to_string(),
            user,
            project_root: project_root.trim_end_matches('/').to_string(),
            external: Vec::new(),
        }
    }

    /// Recursively rewrite every string in `v` per the scrub rules. Keys
    /// named `session_id` / `transcript_path` get fixed placeholder values.
    pub fn scrub_value(&mut self, v: &mut Value) {
        match v {
            Value::String(s) => *s = self.scrub_str(s),
            Value::Array(items) => {
                for item in items {
                    self.scrub_value(item);
                }
            }
            Value::Object(map) => {
                for (key, val) in map.iter_mut() {
                    if is_session_key(key) {
                        *val = Value::String("sess-00000000".to_string());
                    } else if is_transcript_key(key) {
                        *val = Value::String("/home/dev/.claude/transcript.jsonl".to_string());
                    } else {
                        self.scrub_value(val);
                    }
                }
            }
            _ => {}
        }
    }

    fn scrub_str(&mut self, s: &str) -> String {
        // 1. Project-root prefix → canonical fixture root.
        let mut out = s.replace(&self.project_root, "/home/dev/project");
        // 2. Any remaining $HOME-rooted path → indexed external placeholder.
        while let Some(start) = out.find(&self.home) {
            let end = out[start..]
                .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ':' | ','))
                .map(|off| start + off)
                .unwrap_or(out.len());
            let path = out[start..end].to_string();
            let n = match self.external.iter().position(|p| p == &path) {
                Some(i) => i,
                None => {
                    self.external.push(path);
                    self.external.len() - 1
                }
            };
            out.replace_range(start..end, &format!("/home/dev/external/p{n}"));
        }
        // 3. Bare username anywhere else (long enough to be unambiguous).
        if self.user.len() >= MIN_BARE_USERNAME_LEN {
            out = out.replace(&self.user, "dev");
        }
        out
    }

    /// Post-scrub verification. Split by residual shape (adversarial-review
    /// finding #1): a surviving `$HOME` path, or the username *as a path
    /// component*, is an unambiguous leak → `Err`. The bare username as a
    /// free-text token is NOT a hard failure — it is returned by
    /// [`warnings`](Self::warnings) for a human to adjudicate — so a
    /// legitimately-scrubbed fixture whose content happens to contain the
    /// username as a word stays idempotent and exits 0.
    pub fn verify(&self, v: &Value) -> Result<(), ScrubError> {
        let rendered = v.to_string();
        if rendered.contains(&self.home) {
            return Err(ScrubError::Residual {
                what: "home directory",
                context: excerpt(&rendered, &self.home),
            });
        }
        // Username *as a path component* (`/…/<user>/…`) is still a leak.
        if self.user.len() >= MIN_BARE_USERNAME_LEN {
            let as_path = format!("/{}/", self.user);
            let trailing = format!("/{}", self.user);
            if rendered.contains(&as_path) || rendered.ends_with(&trailing) {
                return Err(ScrubError::Residual {
                    what: "username in a path",
                    context: excerpt(&rendered, &self.user),
                });
            }
        }
        Ok(())
    }

    /// Non-fatal residuals for the human reviewer: the bare username appearing
    /// as a free-text token. Empty when nothing needs a look.
    pub fn warnings(&self, v: &Value) -> Vec<String> {
        let rendered = v.to_string();
        if self.user.len() >= MIN_BARE_USERNAME_LEN && rendered.contains(&self.user) {
            return vec![format!(
                "username {:?} appears as a free-text token (not a path) — review: {}",
                self.user,
                excerpt(&rendered, &self.user)
            )];
        }
        Vec::new()
    }
}

/// True for session-id-style keys, compared case- and separator-insensitively
/// (`session_id`, `sessionId`, `SessionID` all match) — finding #3: a CLI
/// sending `sessionId` must not evade scrubbing.
fn is_session_key(key: &str) -> bool {
    normalize_key(key) == "sessionid"
}

fn is_transcript_key(key: &str) -> bool {
    normalize_key(key) == "transcriptpath"
}

/// Lowercase and strip `_`/`-` so key-name variants collapse to one form.
fn normalize_key(key: &str) -> String {
    key.chars()
        .filter(|c| *c != '_' && *c != '-')
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// A short window around the first occurrence of `needle`, for error messages.
fn excerpt(haystack: &str, needle: &str) -> String {
    let Some(pos) = haystack.find(needle) else {
        return String::new();
    };
    let start = pos.saturating_sub(20);
    let end = (pos + needle.len() + 20).min(haystack.len());
    haystack[start..end].to_string()
}
```

Add `pub mod payload_scrub;` to `crates/phronesis-mcp/src/lib.rs`.

- [ ] **Step 2: Run the tests**

Run: `cargo test --workspace payload_scrub`
Expected: 10 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/phronesis-mcp/src/payload_scrub.rs crates/phronesis-mcp/src/lib.rs
git commit -m "feat(scrub): payload_scrub module — anonymize out-of-project data in captures

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: `phr-mcp scrub-payload` subcommand

**Files:**
- Modify: `crates/phronesis-mcp/src/main.rs` (new clap variant + `handle_scrub_payload` fn, following the existing one-handler-per-variant pattern)
- Test: `crates/phronesis-mcp/tests/scrub_payload_integration.rs` (new)

**Interfaces:**
- Consumes: `phronesis_mcp::payload_scrub::{Scrubber, ScrubError}` (Task 2), `security::read_file_capped` (existing).
- Produces: CLI `phr-mcp scrub-payload <path> [--write] [--home <dir>] [--project-root <dir>]`. Reads a JSONL capture file (or single JSON fixture), scrubs every line, verifies, prints scrubbed JSONL to stdout (or rewrites the file in place with `--write`, backing up to `<path>.bak`). Exits 1 with a named residual if verification fails. `--home` defaults to `$HOME`; `--project-root` defaults to the current directory — flags exist so tests don't depend on the runner's real environment.

- [ ] **Step 1: Write the failing integration test**

```rust
// crates/phronesis-mcp/tests/scrub_payload_integration.rs
use std::process::Command;

fn run_scrub(args: &[&str]) -> (i32, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .arg("scrub-payload")
        .args(args)
        .output()
        .expect("run scrub-payload");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

#[test]
fn scrubs_capture_jsonl_to_stdout() {
    let dir = tempfile::tempdir().expect("tempdir");
    let capture = dir.path().join("payloads.jsonl");
    std::fs::write(
        &capture,
        r#"{"ts":1,"phase":"pre","raw":{"session_id":"abc","cwd":"/Users/alicejones/Git/myproject","tool_input":{"file_path":"/Users/alicejones/Git/myproject/src/lib.rs"}}}"#,
    )
    .expect("write capture");

    let (code, stdout, _) = run_scrub(&[
        capture.to_str().expect("utf8"),
        "--home", "/Users/alicejones",
        "--project-root", "/Users/alicejones/Git/myproject",
    ]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).expect("stdout is JSON");
    assert_eq!(v["raw"]["session_id"], "sess-00000000");
    assert_eq!(v["raw"]["cwd"], "/home/dev/project");
    assert_eq!(v["raw"]["tool_input"]["file_path"], "/home/dev/project/src/lib.rs");
    assert!(!stdout.contains("alicejones"));
}

#[test]
fn write_flag_rewrites_in_place_with_backup() {
    let dir = tempfile::tempdir().expect("tempdir");
    let capture = dir.path().join("payloads.jsonl");
    let original = r#"{"ts":1,"phase":"pre","raw":{"cwd":"/Users/alicejones/Git/myproject"}}"#;
    std::fs::write(&capture, original).expect("write capture");

    let (code, _, _) = run_scrub(&[
        capture.to_str().expect("utf8"),
        "--write",
        "--home", "/Users/alicejones",
        "--project-root", "/Users/alicejones/Git/myproject",
    ]);
    assert_eq!(code, 0);
    let rewritten = std::fs::read_to_string(&capture).expect("read back");
    assert!(rewritten.contains("/home/dev/project"));
    assert!(!rewritten.contains("alicejones"));
    let backup = std::fs::read_to_string(dir.path().join("payloads.jsonl.bak")).expect("backup");
    assert_eq!(backup.trim(), original);
}

#[test]
fn username_as_free_token_warns_but_exits_zero() {
    // Finding #1: a captured command that mentions the username as a word is
    // scrubbed-and-shipped with a warning, not failed. (Here the username is
    // long enough to be replaced in free text, so it becomes "dev"; the point
    // is the run still exits 0 and the pipeline is idempotent.)
    let dir = tempfile::tempdir().expect("tempdir");
    let capture = dir.path().join("payloads.jsonl");
    std::fs::write(
        &capture,
        r#"{"ts":1,"phase":"pre","raw":{"tool_input":{"command":"git commit -m 'thanks alicejones'"}}}"#,
    )
    .expect("write capture");
    let (code, stdout, _) = run_scrub(&[
        capture.to_str().expect("utf8"),
        "--home", "/Users/alicejones",
        "--project-root", "/Users/alicejones/Git/myproject",
    ]);
    assert_eq!(code, 0, "free-text username must not fail the run");
    assert!(!stdout.contains("alicejones"));
}

#[test]
fn missing_file_exits_nonzero_with_message() {
    let (code, _, stderr) = run_scrub(&[
        "/nonexistent/nowhere.jsonl",
        "--home", "/Users/x",
        "--project-root", "/Users/x/p",
    ]);
    assert_ne!(code, 0);
    assert!(stderr.contains("nowhere.jsonl"));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --workspace --test scrub_payload_integration`
Expected: FAIL — clap rejects the unknown `scrub-payload` subcommand.

- [ ] **Step 3: Add the clap variant and handler in `main.rs`**

Add to the `Commands` enum (next to the other migrate/dev-tooling variants):

```rust
/// Anonymize a captured payload file for committing as a fixture.
///
/// Rewrites $HOME paths, the username, session ids, and transcript paths;
/// leaves project-internal content verbatim. Prints scrubbed JSONL to
/// stdout, or rewrites in place (with a .bak backup) under --write.
ScrubPayload {
    /// Capture file (JSONL from PHRONESIS_CAPTURE_DIR) or a single-JSON fixture.
    path: std::path::PathBuf,
    /// Rewrite the file in place, backing up the original to <path>.bak.
    #[arg(long)]
    write: bool,
    /// Home directory to scrub (defaults to $HOME).
    #[arg(long)]
    home: Option<String>,
    /// Project root whose paths map to /home/dev/project (defaults to CWD).
    #[arg(long)]
    project_root: Option<String>,
},
```

Add the dispatch arm and handler (mirror the shape of `handle_migrate_extracted`; the `.bak` path is the appended form `format!("{}.bak", path.display())` — the house pattern from `migrate-rules`, and what the test expects):

```rust
fn handle_scrub_payload(
    path: &std::path::Path,
    write: bool,
    home: Option<String>,
    project_root: Option<String>,
) -> anyhow::Result<()> {
    let home = match home {
        Some(h) => h,
        None => std::env::var("HOME")?,
    };
    let project_root = match project_root {
        Some(p) => p,
        None => std::env::current_dir()?.display().to_string(),
    };
    let raw = phronesis_mcp::security::read_file_capped(path)?;
    let mut scrubber = phronesis_mcp::payload_scrub::Scrubber::new(&home, &project_root);
    let mut out_lines = Vec::new();
    for (idx, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let mut value: serde_json::Value = serde_json::from_str(line)
            .map_err(|e| anyhow::anyhow!("line {}: not JSON: {}", idx + 1, e))?;
        scrubber.scrub_value(&mut value);
        // Hard leaks ($HOME path, username-as-path-component) abort the run.
        scrubber.verify(&value)?;
        // Soft residuals (username as a free-text token) are surfaced for the
        // human reviewer but do NOT fail the run — see the scrubber's verify /
        // warnings split (finding #1). Keeps scrubbing idempotent.
        for w in scrubber.warnings(&value) {
            eprintln!("phronesis: scrub warning (line {}): {}", idx + 1, w);
        }
        out_lines.push(value.to_string());
    }
    let rendered = out_lines.join("\n") + "\n";
    if write {
        std::fs::copy(path, format!("{}.bak", path.display()))?;
        std::fs::write(path, rendered)?;
        eprintln!(
            "scrubbed {} line(s) in place; original at {}.bak",
            out_lines.len(),
            path.display()
        );
    } else {
        print!("{rendered}");
    }
    Ok(())
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test --workspace --test scrub_payload_integration`
Expected: 4 passed. Then `cargo clippy --workspace -- -D warnings` — expected clean.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/main.rs crates/phronesis-mcp/tests/scrub_payload_integration.rs
git commit -m "feat(cli): scrub-payload subcommand — curate captures into committable fixtures

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: Starter fixture corpus + contract runner

**Files:**
- Create: `crates/phronesis-mcp/tests/fixtures/payloads/claude-code/pre-edit-clean.json`
- Create: `crates/phronesis-mcp/tests/fixtures/payloads/claude-code/pre-edit-unwrap-block.json`
- Create: `crates/phronesis-mcp/tests/fixtures/payloads/claude-code/post-bash-tool-response.json` (regression: bug #1)
- Create: `crates/phronesis-mcp/tests/fixtures/payloads/claude-code/post-bash-cargo-build.json` (regression: bug #2)
- Create: `crates/phronesis-mcp/tests/fixtures/payloads/gemini/pre-replace-clean.json`
- Create: `crates/phronesis-mcp/tests/fixtures/payloads/gemini/pre-run-shell-clean.json`
- Create: `crates/phronesis-mcp/tests/fixtures/payloads/gemini/post-write-file-tool-output.json`
- Test: `crates/phronesis-mcp/tests/payload_contract.rs` (new)

**Interfaces:**
- Consumes: `CARGO_BIN_EXE_phr-mcp` harness pattern; `phr-mcp init --packs <packs>` (existing); fixture wrapper schema from the design spec.
- Produces: the corpus layout and the `Fixture`/`Expect` serde structs later fixtures must conform to; helpers `init_project(dir, packs)` and `run_subcommand(dir, subcommand, payload)` that Task 5 reuses. Every fixture added later is picked up automatically — the runner walks the directory.

- [ ] **Step 1: Write the fixtures**

The `expect` vocabulary (per the revised design spec — no `log_event` or
`journey_tags`, which are false-green-prone):
- `exit` — exit code (guard).
- `stdout_json` — stdout parses as JSON (guard, pins `exit_ok()`).
- `log_rule_fired` — this rule id appears in the log entry's `consequences`
  array (real liveness: the rule matched *this* payload).
- `journal_tag_new` — a **freshly written** journal record (absent from the
  pre-hook baseline) carries this tag.
- `journal_tag_from_output` — a fresh record carries this tag, which is only
  derivable by *reading the tool-output field* (an `outcome:*` tag). This is
  the clause that actually pins bug #1.
- `stderr_contains` — substrings on stderr.

`claude-code/pre-edit-clean.json` — full real envelope, clean edit. A clean
fixture has no positive liveness clause to assert (nothing should fire); it
pins that a valid envelope is accepted and exits 0:

```json
{
  "schema": 1,
  "source": { "cli": "claude-code", "event": "PreToolUse", "provenance": "authored", "captured": "2026-07-06" },
  "subcommand": "pre-check",
  "packs": "llm,rust",
  "payload": {
    "session_id": "sess-00000000",
    "transcript_path": "/home/dev/.claude/transcript.jsonl",
    "cwd": "/home/dev/project",
    "hook_event_name": "PreToolUse",
    "tool_name": "Edit",
    "tool_input": {
      "file_path": "src/lib.rs",
      "old_string": "fn old_helper() -> u32 { 1 }",
      "new_string": "fn new_helper() -> u32 { 2 }"
    }
  },
  "expect": { "exit": 0, "stdout_json": true }
}
```

`claude-code/pre-edit-unwrap-block.json` — same envelope; `tool_input.new_string`
is `"fn f() { thing.unwrap(); }"`, `file_path` is `"src/lib.rs"`. The block is
real liveness — assert the specific rule fired, not just exit 2:

```json
  "expect": { "exit": 2, "log_rule_fired": "enforce-no-unwrap-in-src", "stderr_contains": ["unwrap"] }
```

`claude-code/post-bash-tool-response.json` — **bug #1 regression.** PostToolUse
envelope with `cargo test` output under `tool_response` (the real Claude Code
key — NOT `tool_output`). The assertion is `journal_tag_from_output`: an
`outcome:test_pass` tag is stamped on the journal record *only if the output
field was read and parsed by the cargo adapter*. On the pre-fix code (alias
absent) the output is unread, no outcome tag is written, and this fixture
FAILS — which is the whole point. `journal_tag_new: ["build"]` additionally
confirms the command-derived tagger fired:

```json
{
  "schema": 1,
  "source": { "cli": "claude-code", "event": "PostToolUse", "provenance": "authored", "captured": "2026-07-06" },
  "subcommand": "post-check",
  "packs": "llm,rust,confidence,journey",
  "payload": {
    "session_id": "sess-00000000",
    "transcript_path": "/home/dev/.claude/transcript.jsonl",
    "cwd": "/home/dev/project",
    "hook_event_name": "PostToolUse",
    "tool_name": "Bash",
    "tool_input": { "command": "cargo test --workspace" },
    "tool_response": { "stdout": "test result: ok. 12 passed; 0 failed; 0 ignored", "stderr": "", "interrupted": false }
  },
  "expect": { "exit": 0, "stdout_json": true, "journal_tag_new": ["build"], "journal_tag_from_output": ["outcome:test_pass"] }
}
```

`claude-code/post-bash-cargo-build.json` — **bug #2 regression** (the tagger
firing at all): `tool_input.command` `"cargo build --workspace"` with matching
compile output under `tool_response`; expect
`"journal_tag_new": ["build"]` and `"journal_tag_from_output": ["outcome:compile_ok"]`.

Note on exact tag strings: `outcome:test_pass`, `outcome:compile_ok`,
`outcome:compile_error`, `outcome:test_fail` are the cargo adapter's real
outputs (`src/outcomes/cargo.rs`) and `build` is the default journey tagger's
label. During Step 3, confirm the `build` label and the exact `stdout` text
the cargo adapter needs to emit each outcome (inspect
`.phronesis/journey/events.jsonl` in the temp project or read
`src/outcomes/cargo.rs`), and adjust the fixture `stdout` / tag strings to the
shipped behavior. The *contract* — an output-derived tag proves the output was
read — does not change; only the literals are tuned.

`gemini/pre-replace-clean.json` — Gemini tool names: `"tool_name": "replace"`
with `old_string`/`new_string` under `tool_input`, `file_path` `"src/lib.rs"`,
packs `"llm,rust"`, expect `{ "exit": 0, "stdout_json": true }`.

`gemini/pre-run-shell-clean.json` — `"tool_name": "run_shell_command"`,
`"tool_input": { "command": "ls" }`, packs `"llm"`, expect
`{ "exit": 0, "stdout_json": true }`.

`gemini/post-write-file-tool-output.json` — `"tool_name": "write_file"` with
`"tool_input": { "file_path": "src/gen.rs", "content": "pub fn generated() {}" }`
and output under `tool_output` (Gemini's key, pinning that the serde alias
accepts both keys), packs `"llm,rust"`, expect
`{ "exit": 0, "stdout_json": true }`.

- [ ] **Step 2: Write the contract runner**

```rust
// crates/phronesis-mcp/tests/payload_contract.rs
//! Data-driven payload-contract corpus runner. Every fixture under
//! tests/fixtures/payloads/ is replayed verbatim through the real binary
//! in a freshly-initialized temp project; the fixture's `expect` block is
//! asserted in full.
//!
//! Liveness is the point, and it is deliberately keyed on effects downstream
//! of the exercised code path — `log_rule_fired` (the rule matched THIS
//! payload) and `journal_tag_from_output` (a tag only derivable by reading
//! the tool-output field) — never on universal artifacts a hook emits
//! regardless (a bare log line, or a command-derived tag). Journal-tag
//! assertions are checked against a pre-hook baseline so a scaffold or
//! accumulated record can't satisfy them. See the design spec's §3, §3a, §3b.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(serde::Deserialize)]
struct Fixture {
    schema: u32,
    #[allow(dead_code)]
    source: serde_json::Value,
    subcommand: String,
    packs: String,
    payload: serde_json::Value,
    expect: Expect,
}

#[derive(serde::Deserialize)]
struct Expect {
    exit: i32,
    #[serde(default)]
    stdout_json: bool,
    /// A rule id that must appear in the log entry's `consequences` array.
    #[serde(default)]
    log_rule_fired: Option<String>,
    /// Tags that must appear on a journal record NOT present before the hook ran.
    #[serde(default)]
    journal_tag_new: Vec<String>,
    /// Like `journal_tag_new`, but these tags are only derivable by reading the
    /// tool-output field (the bug-#1 contract).
    #[serde(default)]
    journal_tag_from_output: Vec<String>,
    #[serde(default)]
    stderr_contains: Vec<String>,
}

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/payloads")
}

fn collect_fixtures() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for cli_dir in std::fs::read_dir(fixtures_root()).expect("fixtures dir") {
        let cli_dir = cli_dir.expect("dir entry").path();
        if !cli_dir.is_dir() {
            continue;
        }
        for f in std::fs::read_dir(&cli_dir).expect("cli dir") {
            let p = f.expect("file entry").path();
            if p.extension().and_then(|e| e.to_str()) == Some("json") {
                out.push(p);
            }
        }
    }
    out.sort();
    assert!(!out.is_empty(), "corpus must not be empty");
    out
}

fn init_project(dir: &Path, packs: &str) {
    let status = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(["init", "--packs", packs])
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run init");
    assert!(status.success(), "init --packs {packs} failed");
}

fn run_subcommand(dir: &Path, subcommand: &str, payload: &str) -> (i32, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .arg(subcommand)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(payload.as_bytes())
        .expect("write payload");
    let output = child.wait_with_output().expect("wait");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

/// Raw JSONL lines of the journey journal, or empty if it doesn't exist yet.
fn journal_lines(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join(".phronesis/journey/events.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// Records written since `baseline` (freshness guard): journal lines not present
/// before the hook ran, parsed to JSON.
fn fresh_records(dir: &Path, baseline: &std::collections::HashSet<String>) -> Vec<serde_json::Value> {
    journal_lines(dir)
        .into_iter()
        .filter(|l| !baseline.contains(l))
        .filter_map(|l| serde_json::from_str(&l).ok())
        .collect()
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
    init_project(dir.path(), &fx.packs);

    // Freshness baseline: journal lines that exist AFTER init but BEFORE the
    // hook runs. Any tag assertion must be satisfied by a record absent here,
    // so init scaffolding / accumulation can't produce a false-green (§3b).
    let baseline: std::collections::HashSet<String> =
        journal_lines(dir.path()).into_iter().collect();

    // Path hermeticity (§3a): rewrite the canonical fixture project prefix to
    // this temp project's real root, so an absolute captured `file_path`
    // resolves under the temp tree instead of silently failing to match.
    let root = dir.path().display().to_string();
    let payload = fx.payload.to_string().replace("/home/dev/project", &root);

    let (code, stdout, stderr) = run_subcommand(dir.path(), &fx.subcommand, &payload);

    if code != fx.expect.exit {
        return Err(format!("exit {code}, expected {} (stderr: {stderr})", fx.expect.exit));
    }
    if fx.expect.stdout_json && serde_json::from_str::<serde_json::Value>(stdout.trim()).is_err() {
        return Err(format!("stdout is not parseable JSON: {stdout:?}"));
    }
    for needle in &fx.expect.stderr_contains {
        if !stderr.contains(needle) {
            return Err(format!("stderr missing {needle:?} (stderr: {stderr})"));
        }
    }

    // log_rule_fired: the named rule appears in a hook entry's consequences.
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
                "rule {rule:?} never appears in any log entry's consequences — \
                 the rule did not match this payload (silent no-op)"
            ));
        }
    }

    // Journal-tag assertions, both against fresh records only.
    if !fx.expect.journal_tag_new.is_empty() || !fx.expect.journal_tag_from_output.is_empty() {
        let records = fresh_records(dir.path(), &baseline);
        for tag in &fx.expect.journal_tag_new {
            if !fresh_record_has_tag(&records, tag) {
                return Err(format!(
                    "no FRESH journal record tagged {tag:?} — tagger silently no-op'd \
                     (fresh records: {})",
                    records.len()
                ));
            }
        }
        for tag in &fx.expect.journal_tag_from_output {
            if !fresh_record_has_tag(&records, tag) {
                return Err(format!(
                    "no FRESH journal record tagged {tag:?} — the tool-output field was \
                     not read/parsed (this is the bug-#1 class; fresh records: {})",
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
```

- [ ] **Step 3: Run the runner; tune fixture data until green**

Run: `cargo test --workspace --test payload_contract`
Expected: first run may FAIL on exact tag/rule-id strings or the journal-record
field layout. Tune only the *fixture literals* against real behavior: confirm
the `build` tagger label and the `outcome:*` strings (`phr-mcp journey --json`
in a temp project, or read `src/outcomes/cargo.rs`), the exact `stdout` text
the cargo adapter needs to emit each outcome, and the `rule_id`/`consequences`
field names the log entry actually serializes (read `hook_logged.rs` /
`action_log.rs`). Adjust the runner's `c["rule_id"]` and `r["tags"]` accessors
only if the real serialized field names differ.

Do NOT weaken a liveness expectation to get green. In particular: if
`post-bash-tool-response.json` cannot produce an `outcome:test_pass` tag, that
is either the real bug-#1 regression (good — the corpus works) or a fixture
whose `stdout` doesn't match what the adapter parses (fix the stdout, not the
assertion). Verify the distinction by temporarily reverting the `tool_response`
serde alias in `hook/mod.rs` and confirming this fixture then FAILS — that
proves the fixture actually pins the bug. Restore the alias before committing.

- [ ] **Step 4: Commit**

```bash
git add crates/phronesis-mcp/tests/payload_contract.rs crates/phronesis-mcp/tests/fixtures/payloads/
git commit -m "test(contract): payload-contract corpus + data-driven liveness runner

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: Hook-event-name registry + init wiring contract

**Files:**
- Create: `crates/phronesis-mcp/tests/fixtures/hook_events.json`
- Modify: `crates/phronesis-mcp/tests/payload_contract.rs` (append two tests)

**Interfaces:**
- Consumes: `init_project` helper from Task 4 (same file).
- Produces: the registry file — future host additions must extend it in the same PR that adds wiring.

- [ ] **Step 1: Write the registry fixture**

```json
{
  "claude-code": ["PreToolUse", "PostToolUse", "SessionStart", "UserPromptSubmit"],
  "gemini": ["BeforeTool", "AfterTool", "SessionStart", "BeforeAgent"]
}
```

- [ ] **Step 2: Write the wiring tests (append to `payload_contract.rs`)**

```rust
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
    init_project(dir.path(), "llm");
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
}

#[test]
fn before_model_request_never_reappears() {
    // 0.17.1 regression, pinned by name: this event does not exist in
    // Gemini CLI; wiring under it made per-turn injection silently never run.
    let dir = tempfile::tempdir().expect("tempdir");
    init_project(dir.path(), "llm");
    let gemini = std::fs::read_to_string(dir.path().join(".gemini/settings.json"))
        .expect("gemini settings");
    assert!(
        !gemini.contains("BeforeModelRequest"),
        "dead Gemini hook event resurfaced"
    );
}
```

- [ ] **Step 3: Run the tests**

Run: `cargo test --workspace --test payload_contract`
Expected: PASS on current main (the 0.17.1 fix is in). If `hook_keys` misses because Gemini nests hooks differently than `settings["hooks"]`, adjust `hook_keys` to the real settings shape written by `init.rs:555-583` — the assertion set stays the same.

- [ ] **Step 4: Commit**

```bash
git add crates/phronesis-mcp/tests/fixtures/hook_events.json crates/phronesis-mcp/tests/payload_contract.rs
git commit -m "test(contract): hook-event-name registry pins init wiring to real events

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 6: Docs, changelog, version bump

**Files:**
- Modify: `Cargo.toml` (workspace version 0.17.1 → 0.18.0)
- Modify: `crates/phronesis-mcp/Cargo.toml` (internal `phr`/`phronesis-rhai` version pins → 0.18.0)
- Modify: `CHANGELOG.md` (new 0.18.0 entry at top)
- Modify: `crates/phronesis-mcp/CLAUDE.md` (document `PHRONESIS_CAPTURE_DIR`, `scrub-payload`, and the corpus-refresh workflow)

**Interfaces:**
- Consumes: everything above.
- Produces: the release-ready branch.

- [ ] **Step 1: Bump versions**

In root `Cargo.toml`: `[workspace.package] version = "0.18.0"`. In `crates/phronesis-mcp/Cargo.toml`, update the pinned versions on the `phr` and `phronesis-rhai` path dependencies to `0.18.0`.

- [ ] **Step 2: CHANGELOG entry**

```markdown
## [0.18.0] - <date of merge>

### Added
- **Payload-contract corpus.** Committed, anonymized fixtures of real
  Claude Code / Gemini CLI hook payloads under
  `crates/phronesis-mcp/tests/fixtures/payloads/`, replayed verbatim
  through the real binary by a data-driven runner that asserts exit
  codes, stdout-JSON (the Gemini exit-0 contract), action-log entries,
  and journey-journal tags. Liveness is the point: a hook or tagger
  that silently no-ops now fails CI. Regression fixtures pin the
  0.13.2 `tool_response` and build-tagger incidents; a hook-event-name
  registry pins `init` wiring to event names that exist (the 0.17.1
  `BeforeModelRequest` incident, by name).
- **`PHRONESIS_CAPTURE_DIR`** — when set, pre-check/post-check append
  the raw stdin payload to `<dir>/payloads.jsonl` before parsing.
  Best-effort, off by default; how the corpus is refreshed when a CLI
  changes its schema.
- **`phr-mcp scrub-payload <path> [--write]`** — anonymizes captured
  payloads for committing: `$HOME` paths, username, `session_id`,
  `transcript_path`, and extra-project paths are rewritten to
  deterministic placeholders; project-internal content is preserved
  byte-for-byte; residuals fail the run. Design:
  `docs/superpowers/specs/2026-07-06-payload-contract-corpus-design.md`.
```

- [ ] **Step 3: CLAUDE.md — add a "Payload-contract corpus" section**

Document: the fixture layout and wrapper schema, the capture → scrub → review → commit refresh workflow (set `PHRONESIS_CAPTURE_DIR=/tmp/cap` in the shell that launches the CLI, work normally, then `phr-mcp scrub-payload /tmp/cap/payloads.jsonl`, human-review, place under `tests/fixtures/payloads/<cli>/`), and the rule that adding a new host CLI requires extending `tests/fixtures/hook_events.json` in the same PR that adds its wiring.

- [ ] **Step 4: Full verification**

Run: `cargo fmt --all` then `cargo clippy --workspace -- -D warnings` then `cargo test --workspace` (bare, unpiped).
Expected: clean, all tests pass.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/phronesis-mcp/Cargo.toml CHANGELOG.md crates/phronesis-mcp/CLAUDE.md
git commit -m "chore(release): 0.18.0 — payload-contract corpus, capture tee, scrub-payload

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 7 (manual, human-in-loop): Harvest live captures

Not executable by a subagent — flagged for the human + a live session:

- [ ] Run a normal working session on this repo with `PHRONESIS_CAPTURE_DIR` set (e.g. export it in the shell that launches Claude Code).
- [ ] `phr-mcp scrub-payload <dir>/payloads.jsonl --write`, then human-review the scrubbed output for semantic leaks the mechanical scrub can't know about (private names inside command text, etc.).
- [ ] Promote reviewed records into `tests/fixtures/payloads/claude-code/` as `"provenance": "captured"` fixtures with `expect` blocks, superseding the authored equivalents where they overlap. Repeat for a Gemini CLI session when convenient.
- [ ] `cargo test --workspace --test payload_contract` green with the captured corpus.

---

## Self-review notes

- **Spec coverage:** design §1 → Task 1; §2 (incl. verify/warnings split, case-insensitive id-keys) → Tasks 2-3; §3 + §3a path hermeticity + §3b freshness guard → Task 4 (+7 for captured provenance); §4 → Task 5; versioning → Task 6. Acceptance bullet "fails on pre-fix code shape" is realized by construction and *proven* in Task 4 Step 3 (temporarily revert the `tool_response` alias, confirm the bug-#1 fixture fails, restore).
- **Adversarial-review reconciliation (this revision):** the runner no longer asserts `log_event` (written on every invocation → false-green) or a bare command-derived `journey_tags` (the `build` tag comes from the command, not the output, so it would pass even with the `tool_response` alias broken). Liveness now keys on `log_rule_fired` (rule matched *this* payload, checked in the entry's `consequences`) and `journal_tag_from_output` (an `outcome:*` tag only derivable by reading the output field). Journal-tag checks run against a pre-hook baseline (freshness guard) so scaffold/accumulated records can't satisfy them. The scrubber `verify` hard-fails only on path-shaped leaks; the bare username as a free token is a `warnings()` entry, preserving idempotence.
- **Deviation from the design spec:** the spec's fixture list implied a deflection-phrase fixture; it is excluded per the "Corpus limitation" section above (the repo's own llm-pack content rules block authoring it). The spec's acceptance criteria are still met without it.
- **Known soft spot:** exact tag/rule-id strings and the log/journal serialized field names in fixture `expect` blocks and runner accessors are tuned against live behavior in Task 4 Step 3, not guessed here; the runner *structure* is final, the literals follow the shipped code. Weakening a liveness expectation to get green is explicitly forbidden there.
- **Type consistency:** `Fixture`/`Expect`/`init_project`/`run_subcommand`/`journal_lines`/`fresh_records`/`fresh_record_has_tag` are defined once in `payload_contract.rs` (Task 4) and only appended to in Task 5. `Scrubber::new/scrub_value/verify/warnings` signatures match between Task 2 (definition) and Task 3 (call sites).

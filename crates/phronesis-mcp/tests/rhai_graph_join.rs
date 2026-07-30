//! Rhai-emitted predicates and code-graph predicates are one vocabulary.
//!
//! Both arrive in working memory as `Fact { predicate, args }`, so a rule can
//! join them. The trap is the join *key*: hosts send absolute paths while the
//! graph keys files repo-relative, so a rule joining a provider fact to a
//! graph fact on a path used to match nothing — and a rule that never fires
//! reports no error, which is the worst failure mode a rules engine has.
//!
//! These tests pin the seam through the real binary.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use tempfile::TempDir;

const RISKY_SOURCE: &str = r#"
pub fn danger(v: Vec<u32>) -> u32 {
    *v.first().expect("empty")
}
"#;

/// A project with one Rust file, one provider, and `rules` joining them.
fn project(provider: &str, rules: &str) -> TempDir {
    let d = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(d.path().join("src")).expect("mkdir src");
    std::fs::create_dir_all(d.path().join(".phronesis/predicates")).expect("mkdir predicates");
    std::fs::write(d.path().join("src/risky.rs"), RISKY_SOURCE).expect("source");
    std::fs::write(d.path().join(".phronesis/predicates/touch.rhai"), provider).expect("provider");
    std::fs::write(d.path().join(".phronesis/rules.json"), rules).expect("rules");

    let status = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(d.path())
        .args(["graph", "rebuild", "--path", "."])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run graph rebuild");
    assert!(status.success(), "graph rebuild failed");
    d
}

/// Run the real pre-check over an Edit, with the absolute path a host sends.
fn pre_check(dir: &Path) -> (i32, String) {
    let payload = format!(
        r#"{{"session_id":"s","cwd":"{root}","hook_event_name":"PreToolUse","tool_name":"Edit",
            "tool_input":{{"file_path":"{root}/src/risky.rs","old_string":"a","new_string":"b"}}}}"#,
        root = dir.display()
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(dir)
        .arg("pre-check")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pre-check");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(payload.as_bytes())
        .expect("write payload");
    let out = child.wait_with_output().expect("wait");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn join_rule(field: &str) -> String {
    format!(
        r#"{{"rules":[{{
        "id":"rhai-graph-join","phase":"pre","priority":100,
        "when":[{{"touched_path":["?file"]}},{{"defines_fn":["?file","?fn"]}}],
        "then":{{"warn":"JOINED {field} `?file` -> `?fn`"}}
    }}]}}"#
    )
}

#[test]
fn a_provider_fact_joins_a_graph_fact_on_the_relative_path() {
    let d = project(
        r#"emit_fact("touched_path", [event.file_rel]);"#,
        &join_rule("via file_rel"),
    );
    let (code, stderr) = pre_check(d.path());
    assert_eq!(code, 1, "the join must fire: {stderr}");
    assert!(stderr.contains("src/risky.rs"), "{stderr}");
    assert!(stderr.contains("rust:crate::risky::danger"), "{stderr}");
}

#[test]
fn the_absolute_path_still_does_not_join_the_graph() {
    // Not a bug to fix by coercing `file_path` — a host path is genuinely
    // absolute and other rules match on it. This pins *why* `file_rel`
    // exists, so nobody "simplifies" the two fields back into one.
    let d = project(
        r#"emit_fact("touched_path", [event.file_path]);"#,
        &join_rule("via file_path"),
    );
    let (code, stderr) = pre_check(d.path());
    assert_eq!(
        code, 0,
        "an absolute path shares no key with the graph: {stderr}"
    );
}

#[test]
fn a_provider_sees_a_relative_path_even_when_the_host_sends_an_absolute_one() {
    let d = project(
        r#"emit_fact("touched_path", [event.file_rel]);"#,
        r#"{"rules":[{
            "id":"echo","phase":"pre","priority":100,
            "when":[{"touched_path":["?p"]}],
            "then":{"warn":"path=`?p`"}
        }]}"#,
    );
    let (_, stderr) = pre_check(d.path());
    assert!(
        stderr.contains("path=`src/risky.rs`"),
        "provider must receive the repo-relative form: {stderr}"
    );
}

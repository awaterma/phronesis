//! End-to-end proof that the coverage evidence-gap rules (SPEC §5.2/§5.3)
//! reach a verdict through the real binary: hydrated closed-world gap facts,
//! RETE join, warn output — no LLM in the loop.
//!
//! Spec: `docs/specs/SPEC-coverage-evidence.md` §5.2 (rule 5.2 `warn-evidence-gap`,
//! acceptance A3), §5.3 (rule 5.3 `warn-commit-on-stale-coverage`, acceptance A5).

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use phronesis_mcp::coverage::store::{COVERAGE_FORMAT, CoverageIndex, HitRecord, write_store};
use tempfile::TempDir;

const OLD_SRC: &str = r#"pub fn safe_divide(numerator: i32, denominator: i32) -> Result<i32, &'static str> {
    if denominator == 0 {
        return Err("division by zero");
    }

    Ok(numerator / denominator)
}
"#;
const NEW_SRC: &str = r#"pub fn safe_divide(numerator: i32, denominator: i32) -> Result<i32, &'static str> {
    if denominator == 0 {
        return Err("invalid denominator");
    }

    Ok(numerator / denominator)
}
"#;

/// Rules 5.2 + 5.3 verbatim from SPEC §5.2/§5.3, in the repo's v2 disk shape.
fn rules_json() -> &'static str {
    r#"{"rules":[
    {
      "id":"warn-evidence-gap","phase":"post","priority":20,"audit":true,
      "when":[
        {"changed_region":["?change","?region"]},
        {"region_without_dynamic_evidence":["?region"]},
        {"region_without_formal_evidence":["?region"]}
      ],
      "then":{"warn":"changed region ?region has neither dynamic test evidence nor formal proof evidence"}
    },
    {
      "id":"warn-commit-on-stale-coverage","phase":"pre","priority":20,"audit":true,
      "when":[
        {"bash_command_matches":["git (commit|merge|rebase|cherry-pick|revert|pull)"]},
        {"coverage_stale": true}
      ],
      "then":{"warn":"coverage evidence is stale (imported revision differs from HEAD) - commits proceed, but relevance claims need re-import"}
    }
]}"#
}

fn project() -> TempDir {
    let d = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(d.path().join("src")).expect("mkdir src");
    std::fs::create_dir_all(d.path().join(".phronesis")).expect("mkdir .phronesis");
    std::fs::write(d.path().join("src/lib.rs"), OLD_SRC).expect("write source");
    std::fs::write(d.path().join(".phronesis/rules.json"), rules_json()).expect("write rules");
    d
}

fn hit(test: &str, region: &str, kind: &str) -> HitRecord {
    HitRecord {
        v: COVERAGE_FORMAT,
        kind: "hit".into(),
        test: test.into(),
        region: region.into(),
        file: "src/lib.rs".into(),
        start_line: 1,
        end_line: 7,
        hit_kind: kind.into(),
        revision: "a".repeat(40),
        tool: "cargo-llvm-cov".into(),
    }
}

fn install_store(dir: &Path, hits: &[HitRecord], rev: &str) {
    write_store(
        dir,
        hits,
        &CoverageIndex {
            format: COVERAGE_FORMAT,
            revision: rev.into(),
            imported_at: 1,
            tool: "cargo-llvm-cov".into(),
        },
    )
    .expect("write coverage store");
}

fn hook(dir: &Path, phase: &str, payload: String) -> (i32, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(dir)
        .arg(format!("{phase}-check"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn hook");
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

fn edit_event(dir: &Path) -> String {
    edit_event_at(dir, "src/lib.rs")
}

/// Same event with an explicit `file_path` — Claude Code sends absolute ones.
fn edit_event_at(dir: &Path, file_path: &str) -> String {
    format!(
        r#"{{"session_id":"s","cwd":"{}","hook_event_name":"PostToolUse","tool_name":"Edit",
            "tool_input":{{"file_path":{},"old_string":{},"new_string":{}}}}}"#,
        dir.display(),
        serde_json::to_string(file_path).expect("json"),
        serde_json::to_string(OLD_SRC).expect("json"),
        serde_json::to_string(NEW_SRC).expect("json"),
    )
}

#[test]
fn a_evidence_gap_rule_warns_on_empty_store() {
    let d = project();
    // Simulate the edit having applied: post-check reads disk, so the file
    // must already hold the new content while the payload carries old_string.
    std::fs::write(d.path().join("src/lib.rs"), NEW_SRC).expect("apply edit");
    // Empty store: no dynamic evidence exists at all.
    let (code, stderr) = hook(d.path(), "post", edit_event(d.path()));
    assert_eq!(code, 1, "expected warn exit, got {code}: {stderr}");
    assert!(
        stderr.contains("has neither dynamic test evidence nor formal proof evidence"),
        "expected the §7 risk statement on stderr: {stderr}"
    );
}

#[test]
fn a_evidence_gap_rule_stays_silent_when_the_store_covers_the_regions() {
    let d = project();
    // The committed fixture's evidence: all three tests cover the function;
    // the branch site is covered by exactly one of them. Region strings must
    // match the real anchors the region map mints, so use the fixture's own
    // export rather than hand-copied anchors.
    let export = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/coverage-sample/export.jsonl");
    let summary = phronesis_mcp::coverage::import::import_export(d.path(), &export, 1)
        .expect("import fixture export");
    assert_eq!(summary.tests, 3, "fixture export must have 3 tests");
    std::fs::write(d.path().join("src/lib.rs"), NEW_SRC).expect("apply edit");
    let (code, stderr) = hook(d.path(), "post", edit_event(d.path()));
    assert_eq!(code, 0, "covered regions must not gap: {stderr}");
}

#[test]
fn stale_coverage_warns_before_a_commit() {
    let d = project();
    // A real git repo: HEAD moves past the imported revision -> coverage_stale.
    let init = |args: &[&str]| {
        let status = Command::new("git")
            .current_dir(d.path())
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?} failed");
    };
    init(&["init", "-q"]);
    init(&["add", "."]);
    init(&["commit", "-q", "-m", "fixture"]);
    // Index claims a revision that cannot equal HEAD (store written fresh
    // here with a known-non-head revision; records agree with the index, or
    // the store would read as corrupt rather than stale).
    install_store(
        d.path(),
        &[hit("t", "fn:src/lib.rs::safe_divide", "region")],
        &"a".repeat(40),
    );

    let payload = format!(
        r#"{{"session_id":"s","cwd":"{}","hook_event_name":"PreToolUse","tool_name":"Bash",
            "tool_input":{{"command":"git commit -m x"}}}}"#,
        d.path().display()
    );
    let (code, stderr) = hook(d.path(), "pre", payload);
    assert_eq!(
        code, 1,
        "expected warn exit on stale coverage, got {code}: {stderr}"
    );
    assert!(
        stderr.contains("coverage evidence is stale"),
        "expected the §5.3 stale message: {stderr}"
    );
}

fn import_fixture(dir: &Path) {
    let export = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/coverage-sample/export.jsonl");
    phronesis_mcp::coverage::import::import_export(dir, &export, 1).expect("import fixture export");
}

// Claude Code sends `tool_input.file_path` absolute. Region ids are qualified
// with the repo-relative path (SPEC §3.2), so the hook must relativize before
// computing changed regions — otherwise `fn:/abs/.../src/lib.rs::safe_divide`
// never matches the store's `fn:src/lib.rs::safe_divide` and every covered
// region reports a false gap.
#[test]
fn absolute_file_path_joins_the_store_like_a_relative_one() {
    let d = project();
    import_fixture(d.path());
    std::fs::write(d.path().join("src/lib.rs"), NEW_SRC).expect("apply edit");
    let abs = d.path().join("src/lib.rs");
    let (code, stderr) = hook(
        d.path(),
        "post",
        edit_event_at(d.path(), &abs.display().to_string()),
    );
    assert_eq!(
        code, 0,
        "covered regions must not gap for an absolute path: {stderr}"
    );

    // The canonical spelling of the same file (macOS temp dirs live behind
    // the /var -> /private/var symlink) must join too.
    let canonical = abs.canonicalize().expect("canonicalize");
    let (code, stderr) = hook(
        d.path(),
        "post",
        edit_event_at(d.path(), &canonical.display().to_string()),
    );
    assert_eq!(code, 0, "canonical absolute path must join: {stderr}");
}

// The project reached through a symlink: cwd is the link, the payload path
// goes through the link or through the real directory — both must join.
#[cfg(unix)]
#[test]
fn absolute_file_path_through_a_symlinked_root_joins_the_store() {
    let d = project();
    import_fixture(d.path());
    std::fs::write(d.path().join("src/lib.rs"), NEW_SRC).expect("apply edit");
    let outer = TempDir::new().expect("tempdir");
    let link = outer.path().join("proj");
    std::os::unix::fs::symlink(d.path(), &link).expect("symlink");
    for file in [link.join("src/lib.rs"), d.path().join("src/lib.rs")] {
        let (code, stderr) = hook(
            &link,
            "post",
            edit_event_at(&link, &file.display().to_string()),
        );
        assert_eq!(
            code,
            0,
            "{} via symlinked root must join: {stderr}",
            file.display()
        );
    }
}

// An absolute path outside the project root names no region of this
// project: no changed regions, so no gap warning.
#[test]
fn absolute_file_path_outside_the_root_produces_no_regions() {
    let d = project();
    let elsewhere = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(elsewhere.path().join("src")).expect("mkdir");
    let outside = elsewhere.path().join("src/lib.rs");
    std::fs::write(&outside, NEW_SRC).expect("write outside");
    let (code, stderr) = hook(
        d.path(),
        "post",
        edit_event_at(d.path(), &outside.display().to_string()),
    );
    assert!(
        !stderr.contains("has neither dynamic test evidence"),
        "an edit outside the root must not produce gap facts (exit {code}): {stderr}"
    );
}

#[test]
fn corrupt_store_surfaces_store_corrupt_and_keeps_the_gap_warning() {
    let d = project();
    // A rule set that also reacts to a corrupt coverage store (D8).
    let rules = rules_json().replacen(
        r#"{"rules":["#,
        r#"{"rules":[
    {
      "id":"warn-coverage-store-corrupt","phase":"post","priority":30,"audit":true,
      "when":[{"store_corrupt":["coverage","?reason"]}],
      "then":{"warn":"coverage store is corrupt (?reason)"}
    },"#,
        1,
    );
    std::fs::write(d.path().join(".phronesis/rules.json"), rules).expect("write rules");
    install_store(
        d.path(),
        &[
            hit("t", "fn:src/lib.rs::safe_divide", "region"),
            hit("t", "branch:src/lib.rs::safe_divide:cd6054b02dde", "branch"),
        ],
        &"a".repeat(40),
    );
    std::fs::write(d.path().join(".phronesis/coverage.jsonl"), "{not json\n")
        .expect("corrupt store");
    std::fs::write(d.path().join("src/lib.rs"), NEW_SRC).expect("apply edit");

    let (code, stderr) = hook(d.path(), "post", edit_event(d.path()));
    assert_eq!(code, 1, "expected warn exit, got {code}: {stderr}");
    assert!(
        stderr.contains("coverage store is corrupt"),
        "store_corrupt rule must fire: {stderr}"
    );
    assert!(
        stderr.contains("has neither dynamic test evidence nor formal proof evidence"),
        "a corrupt store must not silence the gap warning: {stderr}"
    );
}

// A hung import holding the store lock must not hang the hook: the read
// gives up on the lock after a bounded wait and falls back.
#[test]
fn hook_does_not_hang_behind_an_exclusive_store_lock() {
    use fs2::FileExt;
    let d = project();
    import_fixture(d.path());
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(d.path().join(".phronesis/coverage.lock"))
        .expect("open lock");
    lock.lock_exclusive().expect("hold lock");
    std::fs::write(d.path().join("src/lib.rs"), NEW_SRC).expect("apply edit");

    let dir = d.path().to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(hook(&dir, "post", edit_event(&dir)));
    });
    let outcome = rx.recv_timeout(std::time::Duration::from_secs(5));
    drop(lock);
    let (code, stderr) = outcome.expect("hook blocked behind the store lock for 5 s");
    assert_eq!(code, 0, "fixture covers the regions: {stderr}");
}

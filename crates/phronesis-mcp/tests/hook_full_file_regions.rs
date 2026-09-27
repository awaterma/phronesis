//! Changed-region mapping through the real binary with the payloads real
//! hosts send: a Claude Code `Edit` carries a one-line `old_string` /
//! `new_string` snippet, not the whole file. The hooks must diff the FULL
//! pre-edit file against the FULL post-edit file, or pre-check sees no
//! changed region (gap rules never fire) and post-check diffs a snippet
//! against the whole file (every function looks changed).
//!
//! Fixture: `tests/fixtures/coverage-sample` (the SPEC-coverage-evidence §1
//! `safe_divide` sample) with its committed per-test export.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tempfile::TempDir;

const OLD_LINE: &str = r#"        return Err("division by zero");"#;
const NEW_LINE: &str = r#"        return Err("invalid denominator");"#;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/coverage-sample")
}

fn fixture_src() -> String {
    std::fs::read_to_string(fixture_dir().join("src/lib.rs")).expect("read fixture source")
}

/// The fixture source with only the zero-denominator error message changed.
fn edited_src() -> String {
    let src = fixture_src();
    assert_eq!(src.matches(OLD_LINE).count(), 1, "fixture drifted");
    src.replace(OLD_LINE, NEW_LINE)
}

/// Report every changed region and every gap, in both phases, as warnings.
fn rules_json() -> &'static str {
    r#"{"rules":[
    {"id":"changed-pre","phase":"pre","priority":10,"audit":false,
     "when":[{"changed_region":["?c","?region"]}],
     "then":{"warn":"CHANGED ?region"}},
    {"id":"gap-pre","phase":"pre","priority":10,"audit":false,
     "when":[{"changed_region":["?c","?region"]},{"region_without_dynamic_evidence":["?region"]}],
     "then":{"warn":"GAP ?region"}},
    {"id":"changed-post","phase":"post","priority":10,"audit":false,
     "when":[{"changed_region":["?c","?region"]}],
     "then":{"warn":"CHANGED ?region"}},
    {"id":"gap-post","phase":"post","priority":10,"audit":false,
     "when":[{"changed_region":["?c","?region"]},{"region_without_dynamic_evidence":["?region"]}],
     "then":{"warn":"GAP ?region"}}
]}"#
}

fn project(with_evidence: bool) -> TempDir {
    let d = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(d.path().join("src")).expect("mkdir src");
    std::fs::create_dir_all(d.path().join(".phronesis")).expect("mkdir .phronesis");
    std::fs::write(d.path().join("src/lib.rs"), fixture_src()).expect("write source");
    std::fs::write(d.path().join(".phronesis/rules.json"), rules_json()).expect("write rules");
    if with_evidence {
        let summary = phronesis_mcp::coverage::import::import_export(
            d.path(),
            &fixture_dir().join("export.jsonl"),
            1,
        )
        .expect("import fixture export");
        assert_eq!(summary.tests, 3, "fixture export must have 3 tests");
    }
    d
}

fn hook(dir: &Path, phase: &str, payload: &serde_json::Value) -> (i32, String) {
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
        .write_all(payload.to_string().as_bytes())
        .expect("write payload");
    let out = child.wait_with_output().expect("wait");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// Regions named by `<tag> <region>` warning lines.
fn tagged(stderr: &str, tag: &str) -> BTreeSet<String> {
    let marker = format!("— {tag} ");
    stderr
        .lines()
        .filter_map(|l| l.split_once(&marker).map(|(_, r)| r.trim().to_string()))
        .collect()
}

fn payload(dir: &Path, event: &str, tool: &str, input: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "session_id": "s",
        "cwd": dir.display().to_string(),
        "hook_event_name": event,
        "tool_name": tool,
        "tool_use_id": "toolu_t19",
        "tool_input": input,
    })
}

fn one_line_edit(dir: &Path, event: &str) -> serde_json::Value {
    payload(
        dir,
        event,
        "Edit",
        serde_json::json!({
            "file_path": dir.join("src/lib.rs").display().to_string(),
            "old_string": OLD_LINE,
            "new_string": NEW_LINE,
        }),
    )
}

/// Only `safe_divide`'s own regions — its function region and at least one
/// branch site — and nothing from the test module.
fn assert_only_safe_divide(changed: &BTreeSet<String>, stderr: &str) {
    assert!(
        changed.contains("fn:src/lib.rs::safe_divide"),
        "safe_divide must be changed: {changed:?}\n{stderr}"
    );
    assert!(
        changed
            .iter()
            .any(|r| r.starts_with("branch:src/lib.rs::safe_divide")),
        "the zero-denominator branch must be changed: {changed:?}\n{stderr}"
    );
    for r in changed {
        assert!(
            r.ends_with("::safe_divide") || r.starts_with("branch:src/lib.rs::safe_divide:"),
            "only safe_divide regions may change, got {r}: {changed:?}\n{stderr}"
        );
    }
}

fn all_function_regions(changed: &BTreeSet<String>) -> bool {
    [
        "safe_divide",
        "divides_positive_values",
        "divides_negative_values",
        "rejects_zero_denominator",
    ]
    .iter()
    .all(|f| {
        changed
            .iter()
            .any(|r| r.starts_with("fn:") && r.ends_with(f))
    })
}

#[test]
fn pre_check_one_line_edit_maps_to_safe_divide_and_gaps_on_an_empty_store() {
    let d = project(false);
    let (code, stderr) = hook(d.path(), "pre", &one_line_edit(d.path(), "PreToolUse"));
    assert_eq!(code, 1, "expected warn exit: {stderr}");
    let changed = tagged(&stderr, "CHANGED");
    assert_only_safe_divide(&changed, &stderr);
    assert_eq!(
        tagged(&stderr, "GAP"),
        changed,
        "with no evidence every changed region is a gap: {stderr}"
    );
}

#[test]
fn pre_check_one_line_edit_has_no_gap_when_evidence_covers_it() {
    let d = project(true);
    let (_code, stderr) = hook(d.path(), "pre", &one_line_edit(d.path(), "PreToolUse"));
    assert_only_safe_divide(&tagged(&stderr, "CHANGED"), &stderr);
    assert!(
        tagged(&stderr, "GAP").is_empty(),
        "imported evidence covers safe_divide: {stderr}"
    );
}

#[test]
fn post_check_one_line_edit_does_not_mark_the_test_module_changed() {
    let d = project(false);
    std::fs::write(d.path().join("src/lib.rs"), edited_src()).expect("apply edit");
    let (code, stderr) = hook(d.path(), "post", &one_line_edit(d.path(), "PostToolUse"));
    assert_eq!(code, 1, "expected warn exit: {stderr}");
    let changed = tagged(&stderr, "CHANGED");
    assert_only_safe_divide(&changed, &stderr);
    assert_eq!(tagged(&stderr, "GAP"), changed, "{stderr}");
}

#[test]
fn post_check_one_line_edit_has_no_gap_when_evidence_covers_it() {
    let d = project(true);
    std::fs::write(d.path().join("src/lib.rs"), edited_src()).expect("apply edit");
    let (code, stderr) = hook(d.path(), "post", &one_line_edit(d.path(), "PostToolUse"));
    assert_eq!(code, 1, "the CHANGED report still warns: {stderr}");
    assert_only_safe_divide(&tagged(&stderr, "CHANGED"), &stderr);
    assert!(tagged(&stderr, "GAP").is_empty(), "{stderr}");
}

#[test]
fn gemini_replace_one_line_maps_to_safe_divide_in_both_phases() {
    let d = project(false);
    let input = serde_json::json!({
        "file_path": "src/lib.rs",
        "old_string": OLD_LINE,
        "new_string": NEW_LINE,
    });
    let (_, stderr) = hook(
        d.path(),
        "pre",
        &payload(d.path(), "BeforeTool", "replace", input.clone()),
    );
    assert_only_safe_divide(&tagged(&stderr, "CHANGED"), &stderr);
    std::fs::write(d.path().join("src/lib.rs"), edited_src()).expect("apply edit");
    let (_, stderr) = hook(
        d.path(),
        "post",
        &payload(d.path(), "AfterTool", "replace", input),
    );
    assert_only_safe_divide(&tagged(&stderr, "CHANGED"), &stderr);
}

/// Two edits applied in order: the error message, then the assertion in the
/// one test that checks it. Exactly those two functions change.
#[test]
fn multiedit_changes_exactly_the_edited_functions_in_both_phases() {
    let d = project(false);
    let assert_old = r#"assert_eq!(safe_divide(8, 0), Err("division by zero"));"#;
    let assert_new = r#"assert_eq!(safe_divide(8, 0), Err("invalid denominator"));"#;
    let input = serde_json::json!({
        "file_path": "src/lib.rs",
        "edits": [
            {"old_string": OLD_LINE, "new_string": NEW_LINE},
            {"old_string": assert_old, "new_string": assert_new},
        ],
    });
    let expect_fns = |changed: &BTreeSet<String>, stderr: &str| {
        let fns: BTreeSet<&str> = changed
            .iter()
            .filter(|r| r.starts_with("fn:"))
            .map(String::as_str)
            .collect();
        assert_eq!(
            fns,
            BTreeSet::from([
                "fn:src/lib.rs::safe_divide",
                "fn:src/lib.rs::tests::rejects_zero_denominator",
            ]),
            "{stderr}"
        );
    };
    let (_, stderr) = hook(
        d.path(),
        "pre",
        &payload(d.path(), "PreToolUse", "MultiEdit", input.clone()),
    );
    expect_fns(&tagged(&stderr, "CHANGED"), &stderr);

    let after = edited_src().replace(assert_old, assert_new);
    std::fs::write(d.path().join("src/lib.rs"), after).expect("apply edits");
    let (_, stderr) = hook(
        d.path(),
        "post",
        &payload(d.path(), "PostToolUse", "MultiEdit", input),
    );
    expect_fns(&tagged(&stderr, "CHANGED"), &stderr);
}

#[test]
fn write_diffs_against_the_file_on_disk_at_pre_check() {
    let d = project(false);
    let input = serde_json::json!({"file_path": "src/lib.rs", "content": edited_src()});
    let (_, stderr) = hook(
        d.path(),
        "pre",
        &payload(d.path(), "PreToolUse", "Write", input),
    );
    assert_only_safe_divide(&tagged(&stderr, "CHANGED"), &stderr);
}

/// At post-check a `Write` leaves nothing to reverse-apply; Claude Code's
/// `tool_response.originalFile` is the pre-image when it is present.
#[test]
fn write_at_post_check_uses_the_hosts_original_file() {
    let d = project(false);
    std::fs::write(d.path().join("src/lib.rs"), edited_src()).expect("apply write");
    let mut p = payload(
        d.path(),
        "PostToolUse",
        "Write",
        serde_json::json!({"file_path": "src/lib.rs", "content": edited_src()}),
    );
    p["tool_response"] = serde_json::json!({
        "type": "update",
        "filePath": "src/lib.rs",
        "content": edited_src(),
        "originalFile": fixture_src(),
    });
    let (_, stderr) = hook(d.path(), "post", &p);
    assert_only_safe_divide(&tagged(&stderr, "CHANGED"), &stderr);
}

/// Without a pre-image, post-check over-reports (the whole file changed)
/// rather than under-reports: a gap rule must never go quiet for lack of
/// the old side.
#[test]
fn write_at_post_check_without_a_pre_image_treats_the_whole_file_as_changed() {
    let d = project(false);
    std::fs::write(d.path().join("src/lib.rs"), edited_src()).expect("apply write");
    let p = payload(
        d.path(),
        "PostToolUse",
        "Write",
        serde_json::json!({"file_path": "src/lib.rs", "content": edited_src()}),
    );
    let (_, stderr) = hook(d.path(), "post", &p);
    assert!(
        all_function_regions(&tagged(&stderr, "CHANGED")),
        "{stderr}"
    );
}

/// An `old_string` that does not occur in the file: the host will reject
/// the edit, but the hook must not crash, and must fall back to treating
/// the whole file as changed (over-report, never under-report).
#[test]
fn unmatched_old_string_falls_back_to_the_whole_file_without_crashing() {
    let d = project(false);
    let p = payload(
        d.path(),
        "PreToolUse",
        "Edit",
        serde_json::json!({
            "file_path": "src/lib.rs",
            "old_string": "this text is not in the file",
            "new_string": NEW_LINE,
        }),
    );
    let (code, stderr) = hook(d.path(), "pre", &p);
    assert_eq!(code, 1, "warn, not crash or block: {stderr}");
    assert!(
        all_function_regions(&tagged(&stderr, "CHANGED")),
        "{stderr}"
    );

    // Post-check: the new_string cannot be located in the file either.
    let (code, stderr) = hook(d.path(), "post", &p);
    assert_eq!(code, 1, "warn, not crash: {stderr}");
    assert!(
        all_function_regions(&tagged(&stderr, "CHANGED")),
        "{stderr}"
    );
}

/// An ambiguous `old_string` (occurs more than once) without `replace_all`
/// is rejected by the host; the hook falls back the same way.
#[test]
fn ambiguous_old_string_without_replace_all_falls_back_to_the_whole_file() {
    let d = project(false);
    let p = payload(
        d.path(),
        "PreToolUse",
        "Edit",
        serde_json::json!({
            "file_path": "src/lib.rs",
            "old_string": "#[test]",
            "new_string": "#[test]\n    #[ignore]",
        }),
    );
    let (code, stderr) = hook(d.path(), "pre", &p);
    assert_eq!(code, 1, "{stderr}");
    assert!(
        all_function_regions(&tagged(&stderr, "CHANGED")),
        "{stderr}"
    );
}

/// Property hydration maps changed regions the same way: a one-line edit to
/// `safe_divide` obligates the property that depends on it, and not one that
/// depends on a test function the edit never touched.
#[test]
fn property_obligations_follow_the_full_file_regions_in_both_phases() {
    use phronesis_mcp::properties::store::{
        PROPERTIES_FORMAT, PropertiesFile, Property, PropertySource, PropertyStatus,
        properties_path,
    };
    let d = project(false);
    let property = |id: &str, region: &str| Property {
        mutations: vec![],
        id: id.into(),
        subject: id.into(),
        kind: "postcondition".into(),
        condition: None,
        guarantee: None,
        depends_on: vec![region.into()],
        source: PropertySource::ExplicitSpec,
        status: PropertyStatus::Accepted,
        corroborated_by: vec![],
        encodings: vec![],
    };
    let file = PropertiesFile {
        version: PROPERTIES_FORMAT,
        properties: vec![
            property("divide", "fn:src/lib.rs::safe_divide"),
            property("positive", "fn:src/lib.rs::tests::divides_positive_values"),
        ],
    };
    std::fs::write(
        properties_path(d.path()),
        serde_json::to_string(&file).expect("serialize"),
    )
    .expect("write properties");
    std::fs::write(
        d.path().join(".phronesis/rules.json"),
        r#"{"rules":[
    {"id":"obligation-pre","phase":"pre","priority":10,"audit":false,
     "when":[{"property_obligation":["?p","?why"]}],"then":{"warn":"OBLIGATION ?p"}},
    {"id":"obligation-post","phase":"post","priority":10,"audit":false,
     "when":[{"property_obligation":["?p","?why"]}],"then":{"warn":"OBLIGATION ?p"}}
]}"#,
    )
    .expect("write rules");
    let only_divide = BTreeSet::from(["divide".to_string()]);

    let (_, stderr) = hook(d.path(), "pre", &one_line_edit(d.path(), "PreToolUse"));
    assert_eq!(tagged(&stderr, "OBLIGATION"), only_divide, "{stderr}");
    std::fs::write(d.path().join("src/lib.rs"), edited_src()).expect("apply edit");
    let (_, stderr) = hook(d.path(), "post", &one_line_edit(d.path(), "PostToolUse"));
    assert_eq!(tagged(&stderr, "OBLIGATION"), only_divide, "{stderr}");
}

/// A real file that is not valid UTF-8 (a stray Latin-1 byte in a comment)
/// was read as "missing": pre-check fell back to the one-line snippet and
/// reported no changed region at all for a real edit.
#[test]
fn non_utf8_file_still_maps_the_edit() {
    let d = project(false);
    let mut bytes = b"// caf\xe9 \xff\xfe\n".to_vec();
    bytes.extend_from_slice(fixture_src().as_bytes());
    std::fs::write(d.path().join("src/lib.rs"), &bytes).expect("write non-utf8");
    let (code, stderr) = hook(d.path(), "pre", &one_line_edit(d.path(), "PreToolUse"));
    assert_eq!(code, 1, "expected warn exit: {stderr}");
    let changed = tagged(&stderr, "CHANGED");
    assert_only_safe_divide(&changed, &stderr);
    assert_eq!(tagged(&stderr, "GAP"), changed, "{stderr}");

    let mut after = b"// caf\xe9 \xff\xfe\n".to_vec();
    after.extend_from_slice(edited_src().as_bytes());
    std::fs::write(d.path().join("src/lib.rs"), &after).expect("apply edit");
    let (_, stderr) = hook(d.path(), "post", &one_line_edit(d.path(), "PostToolUse"));
    assert_only_safe_divide(&tagged(&stderr, "CHANGED"), &stderr);
}

/// A file that exists but cannot be read maps to one coarse whole-file
/// region: the gap rule fires rather than going silent.
#[cfg(unix)]
#[test]
fn unreadable_file_counts_as_wholly_changed() {
    use std::os::unix::fs::PermissionsExt;
    let d = project(false);
    let file = d.path().join("src/lib.rs");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).expect("chmod");
    if std::fs::read(&file).is_ok() {
        // Running as root: permissions do not bind; nothing to prove.
        return;
    }
    let (code, stderr) = hook(d.path(), "pre", &one_line_edit(d.path(), "PreToolUse"));
    let (pcode, pstderr) = hook(d.path(), "post", &one_line_edit(d.path(), "PostToolUse"));
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).expect("chmod");
    let whole = BTreeSet::from(["file:src/lib.rs".to_string()]);
    assert_eq!(code, 1, "{stderr}");
    assert_eq!(tagged(&stderr, "CHANGED"), whole, "{stderr}");
    assert_eq!(tagged(&stderr, "GAP"), whole, "{stderr}");
    assert_eq!(pcode, 1, "{pstderr}");
    assert_eq!(tagged(&pstderr, "GAP"), whole, "{pstderr}");
}

/// A file over the read cap is neither read unbounded nor mapped per region:
/// pre-check returns promptly with one coarse whole-file region (it took
/// tens of seconds to minutes before). The bound is deliberately generous.
#[test]
fn oversized_file_is_bounded_and_wholly_changed() {
    let d = project(false);
    let mut src = fixture_src();
    for i in 0..60_000 {
        src.push_str(&format!("fn pad{i}(x: u32) -> u32 {{ x }}\n"));
    }
    assert!(src.len() > 1024 * 1024);
    std::fs::write(d.path().join("src/lib.rs"), &src).expect("write big");
    // Ambiguous edit (whole-file fallback) — the reviewer's worst case.
    let p = payload(
        d.path(),
        "PreToolUse",
        "Edit",
        serde_json::json!({
            "file_path": "src/lib.rs",
            "old_string": "#[test]",
            "new_string": "#[test]\n    #[ignore]",
        }),
    );
    let start = std::time::Instant::now();
    let (code, stderr) = hook(d.path(), "pre", &p);
    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(15),
        "pre-check took {elapsed:?}"
    );
    let whole = BTreeSet::from(["file:src/lib.rs".to_string()]);
    assert_eq!(code, 1, "{stderr}");
    assert_eq!(tagged(&stderr, "GAP"), whole, "{stderr}");
}

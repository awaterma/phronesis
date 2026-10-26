use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use phronesis_mcp::audit::{AuditOpts, run};
use phronesis_mcp::language_packs;
use phronesis_mcp::rules_file::RulesFile;
use serde_json::json;

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn fixture(root: &Path) {
    let source = root.join("source");
    fs::create_dir_all(&source).unwrap();
    let manifest = json!({
        "schema_version":1,"adapter_api":1,"id":"demo","version":"1.0.0",
        "language":"demo","extensions":["demox"],"grammar":"python",
        "capabilities":["structural_facts"],
        "queries":[{"id":"functions","path":"functions.scm","emit":[
            {"kind":"fact","name":"function_seen","args":["${name}"],"span_capture":"def"}
        ]}],
        "rules":"rules.json","context_files":[],"fixtures":[]
    });
    write(&source, "pack.json", &manifest.to_string());
    write(
        &source,
        "functions.scm",
        "(function_definition name: (identifier) @name) @def",
    );
    write(
        &source,
        "rules.json",
        r#"{"rules":[
        {"id":"demo.pre","phase":"pre","audit":true,"when":[{"pack_demo_function_seen":["?file","danger"]}],"then":{"block":"danger function"}},
        {"id":"demo.post","phase":"post","audit":true,"when":[{"pack_demo_function_seen":["?file","danger"]}],"then":{"warn":"danger function"}}
    ]}"#,
    );
    language_packs::install(root, &source, false).unwrap();
}

fn hook(root: &Path, phase: &str, payload: serde_json::Value) -> (i32, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .arg(phase)
        .current_dir(root)
        .env("PHRONESIS_PROJECT_ROOT", root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn query_fact_governs_pre_post_and_audit_without_project_rules() {
    let project = tempfile::tempdir().unwrap();
    fixture(project.path());
    let source = "def danger():\n    pass\n";
    let payload =
        json!({"tool_name":"Write","tool_input":{"file_path":"app.demox","content":source}});
    let (code, stderr) = hook(project.path(), "pre-check", payload.clone());
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("danger function"), "{stderr}");
    write(project.path(), "app.demox", source);
    let (code, stderr) = hook(project.path(), "post-check", payload);
    assert_eq!(code, 1, "{stderr}");
    let resolved = phronesis_mcp::rule_layers::resolve(project.path()).unwrap();
    let report = run(
        &RulesFile {
            rules: resolved.rules,
        },
        &AuditOpts {
            project_root: project.path().to_path_buf(),
            scan_root: project.path().to_path_buf(),
            rule_filter: None,
        },
    );
    assert!(report.incomplete.is_empty(), "{:?}", report.incomplete);
    let pre = report
        .per_rule
        .iter()
        .find(|r| r.rule_id == "demo.pre")
        .unwrap();
    assert_eq!(pre.files[0].lines, vec![1]);
    assert!(!project.path().join(".phronesis/rules.json").exists());
}

#[test]
fn comments_do_not_emit_facts_and_tampering_blocks_source_edits() {
    let project = tempfile::tempdir().unwrap();
    fixture(project.path());
    let safe = json!({"tool_name":"Write","tool_input":{"file_path":"app.demox","content":"# def danger():\nmessage = \"def danger():\"\ndef safe():\n    pass\n"}});
    let (code, stderr) = hook(project.path(), "pre-check", safe.clone());
    assert_eq!(code, 0, "{stderr}");
    write(
        project.path(),
        ".phronesis/packs/demo/functions.scm",
        "(identifier) @name",
    );
    let (code, stderr) = hook(project.path(), "pre-check", safe);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("digest"), "{stderr}");
    let post = json!({"tool_name":"Write","tool_input":{"file_path":"app.demox","content":"def safe():\n    pass\n"}});
    let (post_code, post_stderr) = hook(project.path(), "post-check", post);
    assert_eq!(post_code, 1, "{post_stderr}");
    assert!(post_stderr.contains("digest"), "{post_stderr}");
    assert!(!language_packs::inspect(project.path()).complete);
    let report = run(
        &RulesFile { rules: Vec::new() },
        &AuditOpts {
            project_root: project.path().to_path_buf(),
            scan_root: project.path().to_path_buf(),
            rule_filter: None,
        },
    );
    assert!(
        report
            .incomplete
            .iter()
            .any(|error| error.contains("digest"))
    );
}

#[test]
fn ignored_file_still_receives_structural_pack_audit() {
    let project = tempfile::tempdir().unwrap();
    fixture(project.path());
    write(project.path(), ".phronesisignore", "app.demox\n");
    write(project.path(), "app.demox", "def danger():\n    pass\n");
    let resolved = phronesis_mcp::rule_layers::resolve(project.path()).unwrap();
    let report = run(
        &RulesFile {
            rules: resolved.rules,
        },
        &AuditOpts {
            project_root: project.path().to_path_buf(),
            scan_root: project.path().to_path_buf(),
            rule_filter: None,
        },
    );
    assert!(report.incomplete.is_empty(), "{:?}", report.incomplete);
    assert!(
        report
            .per_rule
            .iter()
            .any(|r| r.rule_id == "demo.pre" && r.hits > 0)
    );
    assert!(
        report
            .lexical_excluded
            .iter()
            .any(|p| p.ends_with("app.demox"))
    );
}

#[test]
fn a_pack_load_error_keeps_native_audit_hits_and_names_the_failed_pack() {
    let project = tempfile::tempdir().unwrap();
    fixture(project.path());
    write(project.path(), "native.py", "# AUDIT_NEEDLE\n");
    write(project.path(), ".phronesis/packs/demo/pack.json", "{broken");
    write(
        project.path(),
        ".phronesis/rules.json",
        &json!({"rules":[{
            "id":"native-lexical","phase":"audit","audit":true,
            "when":[{"new_content_contains":"AUDIT_NEEDLE"}],"then":{"warn":"native hit"}
        }]})
        .to_string(),
    );
    let rules =
        phronesis_mcp::rules_file::read(&project.path().join(".phronesis/rules.json")).unwrap();
    let report = run(
        &rules,
        &AuditOpts {
            project_root: project.path().into(),
            scan_root: project.path().into(),
            rule_filter: None,
        },
    );
    assert!(report.per_rule.iter().any(|rule| {
        rule.rule_id == "native-lexical"
            && rule
                .files
                .iter()
                .any(|file| file.path.ends_with("native.py"))
    }));
    assert_eq!(report.incomplete.len(), 1, "{:?}", report.incomplete);
    assert!(
        report.incomplete[0].starts_with(".phronesis/packs/demo:"),
        "{:?}",
        report.incomplete
    );
    assert!(!report.incomplete[0].contains("native.py"));
}

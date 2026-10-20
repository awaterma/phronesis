//! A template emission that fails to render (it names a capture its query
//! never defines) is a diagnostic. Review findings 3 and 6: every consumer
//! must treat diagnostics as incomplete evidence (graph and selection
//! already did; hook and audit facts silently dropped them), and failed
//! emissions must be bounded like records are.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use phronesis_mcp::language_packs::PackError;
use phronesis_mcp::language_packs::install::install;
use serde_json::{Value, json};

fn write(dir: &Path, rel: &str, bytes: &[u8]) {
    let path = dir.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

/// A `.emx` pack whose fact templates reference `${nope}`, which the query
/// never captures: every match renders `templates` diagnostics.
fn broken_emission_pack(templates: usize) -> tempfile::TempDir {
    let src = tempfile::tempdir().unwrap();
    write(
        src.path(),
        "queries/defs.scm",
        b"(function_definition name: (identifier) @name) @def",
    );
    let emit: Vec<Value> = (0..templates)
        .map(
            |_| json!({"kind": "fact", "name": "seen", "args": ["${nope}"], "span_capture": "def"}),
        )
        .collect();
    let manifest = json!({
        "schema_version": 1, "adapter_api": 1, "id": "emx", "version": "0.1.0",
        "language": "emx", "extensions": ["emx"], "grammar": "python",
        "capabilities": ["structural_facts"],
        "queries": [{"id": "defs", "path": "queries/defs.scm", "emit": emit}],
        "context_files": [], "fixtures": []
    });
    write(src.path(), "pack.json", manifest.to_string().as_bytes());
    src
}

fn installed(templates: usize) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), ".phronesis/rules.json", br#"{"rules": []}"#);
    install(root.path(), broken_emission_pack(templates).path(), false).unwrap();
    root
}

fn facts(root: &Path, source: &str) -> Result<usize, PackError> {
    let registry = phronesis_mcp::language_packs::load(root).unwrap();
    let pack = registry.for_path("a.emx").unwrap().unwrap();
    let context = phronesis_mcp::language_packs::file_context(root, "a.emx", pack).unwrap();
    phronesis_mcp::language_packs::facts::facts_with_lines(&registry, &context, source)
        .map(|f| f.len())
}

#[test]
fn a_failed_emission_makes_hook_facts_incomplete_not_silently_empty() {
    let root = installed(1);
    let err = facts(root.path(), "def one():\n    pass\n")
        .unwrap_err()
        .to_string();
    assert!(err.contains("diagnostic"), "{err}");

    // pre-check fails closed rather than evaluating without the fact.
    let payload = json!({"tool_name": "Write", "tool_input": {
        "file_path": root.path().join("a.emx").display().to_string(),
        "content": "def one():\n    pass\n"}});
    let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .arg("pre-check")
        .current_dir(root.path())
        .env("PHRONESIS_PROJECT_ROOT", root.path())
        .env("PHRONESIS_NO_ACTION_LOG", "1")
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.to_string().as_bytes())
            .unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn audit_reports_a_failed_emission_as_incomplete() {
    let root = installed(1);
    // Audit scans pack files only for rules that take part in audit.
    write(
        root.path(),
        ".phronesis/rules.json",
        json!({"rules": [{"id": "emx-seen", "phase": "post", "audit": true,
            "when": [{"pack_emx_seen": ["?file", "?name"]}],
            "then": {"warn": "seen ?name"}}]})
        .to_string()
        .as_bytes(),
    );
    write(root.path(), "a.emx", b"def one():\n    pass\n");
    let out = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(["audit", "--json"])
        .current_dir(root.path())
        .env("PHRONESIS_PROJECT_ROOT", root.path())
        .env("PHRONESIS_NO_ACTION_LOG", "1")
        .output()
        .unwrap();
    // An incomplete audit is an error naming the file, never a clean report.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{stderr}");
    assert!(
        stderr.contains("audit incomplete") && stderr.contains("a.emx"),
        "{stderr}"
    );
}

#[test]
fn failed_emissions_are_bounded() {
    // 300 failing templates x 40 functions = 12,000 failed emissions,
    // far past the output-record cap.
    let root = installed(300);
    let source: String = (0..40)
        .map(|i| format!("def f{i}():\n    pass\n"))
        .collect();
    let err = facts(root.path(), &source).unwrap_err().to_string();
    assert!(err.contains("exceeds the limit"), "{err}");
}

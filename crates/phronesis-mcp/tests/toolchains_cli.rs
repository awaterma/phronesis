use phronesis_mcp::outcomes::toolchain::{CompiledDef, DefSource, load_project_defs};
use std::process::Command;

fn run_in(dir: &std::path::Path, args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(args)
        .current_dir(dir)
        .env("PHRONESIS_PROJECT_ROOT", dir)
        .output()
        .expect("spawn phr-mcp");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
    )
}

#[test]
fn toolchains_lists_builtin_cargo() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (code, stdout) = run_in(dir.path(), &["toolchains"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("cargo"), "built-in cargo listed: {stdout}");
    assert!(
        stdout.contains("built-in"),
        "source column present: {stdout}"
    );
}

#[test]
fn toolchains_json_includes_project_def() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join(".phronesis")).expect("mkdir");
    std::fs::write(
        dir.path().join(".phronesis/toolchains.json"),
        r#"[{"id":"pytest","matches":"pytest"}]"#,
    )
    .expect("write defs");
    let (code, stdout) = run_in(dir.path(), &["toolchains", "--json"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("json output");
    let ids: Vec<&str> = v
        .as_array()
        .expect("array")
        .iter()
        .filter_map(|e| e.get("id").and_then(|i| i.as_str()))
        .collect();
    assert_eq!(ids, vec!["cargo", "pytest"]);
    assert_eq!(v[1]["source"], "project");
}

#[test]
fn scaffolded_pytest_def_handles_whitespace_but_not_substring() {
    // Run init --packs confidence to create the actual scaffolded
    // toolchains.json, then load it through the toolchain loader to
    // assert correct matching behavior.
    let dir = tempfile::tempdir().unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(&["init", "--packs", "confidence"])
        .current_dir(dir.path())
        .output()
        .expect("spawn phr-mcp init");
    assert!(out.status.success(), "init stdout={:?}", out.stdout);

    let defs = load_project_defs(dir.path());
    let pytest_def = defs
        .iter()
        .find(|d| d.id == "pytest")
        .expect("pytest def in scaffold");

    let compiled =
        CompiledDef::compile(pytest_def.clone(), DefSource::Project).expect("compile pytest");

    assert!(
        compiled.handles("pytest -q"),
        "pytest -q should be recognized"
    );

    assert!(
        !compiled.handles("pip install pytest-cov"),
        "pip install pytest-cov must NOT match the pytest def"
    );
}

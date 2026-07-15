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
        .args(["init", "--packs", "confidence"])
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

#[test]
fn scaffolded_defs_recognize_command_position_only() {
    // Evidence-integrity Task 5: the init-scaffolded matchers are
    // head-anchored and evaluated per command segment. Also an
    // escape-fidelity guard: the scaffold's `\\s`/`\\d` must survive as
    // regex classes, or these positive/negative cases diverge.
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(["init", "--packs", "confidence"])
        .current_dir(dir.path())
        .output()
        .expect("spawn phr-mcp init");
    assert!(out.status.success(), "init stderr={:?}", out.stderr);

    let defs = load_project_defs(dir.path());
    let compile = |id: &str| {
        let def = defs.iter().find(|d| d.id == id).expect("scaffolded def");
        CompiledDef::compile(def.clone(), DefSource::Project).expect("compile scaffolded def")
    };
    let pytest = compile("pytest");
    let tsc = compile("tsc");

    assert!(pytest.handles("pytest"));
    assert!(pytest.handles("pytest -q"));
    assert!(pytest.handles("python -m pytest tests/"));
    assert!(pytest.handles("cd api && pytest -q"));
    assert!(pytest.handles("FOO=1 pytest"));
    assert!(!pytest.handles("echo pytest"));
    assert!(!pytest.handles("cat pytest.ini"));
    assert!(!pytest.handles("pip install pytest-cov"));
    assert!(!pytest.handles("# pytest"));

    assert!(tsc.handles("tsc"));
    assert!(tsc.handles("npx tsc --noEmit"));
    assert!(tsc.handles("cd web && npx tsc"));
    assert!(!tsc.handles("echo tsc failed"));
    assert!(!tsc.handles("touch tsc.log"));
}

#[test]
fn repository_toolchain_defs_match_the_confidence_scaffold() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(["init", "--packs", "confidence"])
        .current_dir(dir.path())
        .output()
        .expect("spawn phr-mcp init");
    assert!(out.status.success(), "init stderr={:?}", out.stderr);

    let generated: serde_json::Value = serde_json::from_slice(
        &std::fs::read(dir.path().join(".phronesis/toolchains.json"))
            .expect("read generated toolchains"),
    )
    .expect("parse generated toolchains");
    let repository_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(".phronesis/toolchains.json");
    let repository: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&repository_path).expect("read repository toolchains"),
    )
    .expect("parse repository toolchains");

    assert_eq!(
        repository,
        generated,
        "{} drifted",
        repository_path.display()
    );
}

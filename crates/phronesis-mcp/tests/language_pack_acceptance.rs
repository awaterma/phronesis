//! External contributor path: install declared assets and run the packaged
//! binary outside the source checkout. No test-only adapter dispatch.
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::json;

fn example(variant: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/language-packs/python-starter")
        .join(variant)
}

fn binary_copy(root: &Path) -> PathBuf {
    let binary = root.join("phr-mcp");
    fs::copy(env!("CARGO_BIN_EXE_phr-mcp"), &binary).unwrap();
    binary
}

fn call(binary: &Path, root: &Path, args: &[&str], input: Option<&str>) -> std::process::Output {
    let mut child = Command::new(binary)
        .args(args)
        .current_dir(root)
        .env("PHRONESIS_PROJECT_ROOT", root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(input) = input {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    }
    child.wait_with_output().unwrap()
}

fn rpc(
    stdin: &mut std::process::ChildStdin,
    stdout: &mut BufReader<std::process::ChildStdout>,
    id: u8,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    writeln!(
        stdin,
        "{}",
        json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
    )
    .unwrap();
    stdin.flush().unwrap();
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

#[test]
fn external_query_only_pack_governs_and_lists_from_copied_binary() {
    let sandbox = tempfile::tempdir().unwrap();
    let project = sandbox.path().join("project");
    fs::create_dir(&project).unwrap();
    let binary = binary_copy(sandbox.path());
    let pack = example("query-only");
    let pack_arg = pack.to_str().unwrap();

    let validated = call(
        &binary,
        &project,
        &["pack", "validate", pack_arg, "--json"],
        None,
    );
    assert!(
        validated.status.success(),
        "{}",
        String::from_utf8_lossy(&validated.stdout)
    );
    let installed = call(&binary, &project, &["pack", "install", pack_arg], None);
    assert!(
        installed.status.success(),
        "{}",
        String::from_utf8_lossy(&installed.stderr)
    );
    assert!(!project.join(".phronesis/rules.json").exists());

    let source = "def danger():\n    pass\n";
    let payload = json!({"tool_name":"Write","tool_input":{
        "file_path":project.join("sample.pystarter"),"content":source}})
    .to_string();
    let pre = call(&binary, &project, &["pre-check"], Some(&payload));
    assert_eq!(
        pre.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&pre.stderr)
    );
    assert!(String::from_utf8_lossy(&pre.stderr).contains("Review danger function"));
    fs::write(project.join("sample.pystarter"), source).unwrap();
    let audit = call(&binary, &project, &["audit", "--json"], None);
    assert!(
        audit.status.success(),
        "{}",
        String::from_utf8_lossy(&audit.stderr)
    );
    assert!(String::from_utf8_lossy(&audit.stdout).contains("python_starter.review_function"));

    let _ = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&project)
        .output()
        .unwrap();
    let _ = Command::new("git")
        .args(["add", "sample.pystarter"])
        .current_dir(&project)
        .output()
        .unwrap();
    let rebuilt = call(&binary, &project, &["graph", "rebuild"], None);
    assert!(
        rebuilt.status.success(),
        "{}",
        String::from_utf8_lossy(&rebuilt.stderr)
    );
    let query = call(
        &binary,
        &project,
        &["graph", "query", "defines_fn", "--json"],
        None,
    );
    assert!(
        query.status.success(),
        "{}",
        String::from_utf8_lossy(&query.stderr)
    );
    assert!(String::from_utf8_lossy(&query.stdout).contains("danger"));

    let mut server = Command::new(&binary)
        .arg("serve")
        .current_dir(&project)
        .env("PHRONESIS_PROJECT_ROOT", &project)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = server.stdin.take().unwrap();
    let mut stdout = BufReader::new(server.stdout.take().unwrap());
    let _ = rpc(
        &mut stdin,
        &mut stdout,
        1,
        "initialize",
        json!({
            "protocolVersion":"2024-11-05", "capabilities":{},
            "clientInfo":{"name":"acceptance","version":"1"}
        }),
    );
    writeln!(
        stdin,
        "{}",
        json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}})
    )
    .unwrap();
    stdin.flush().unwrap();
    let listed = rpc(
        &mut stdin,
        &mut stdout,
        2,
        "tools/call",
        json!({"name":"list_language_packs","arguments":{}}),
    );
    assert!(
        listed["result"]["structuredContent"]["packs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == "python_starter"),
        "{listed}"
    );
    server.kill().unwrap();
    server.wait().unwrap();
}

#[test]
fn rhai_variant_validates_separately() {
    let sandbox = tempfile::tempdir().unwrap();
    let binary = binary_copy(sandbox.path());
    let pack = example("rhai");
    let result = call(
        &binary,
        sandbox.path(),
        &["pack", "validate", pack.to_str().unwrap(), "--json"],
        None,
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
}

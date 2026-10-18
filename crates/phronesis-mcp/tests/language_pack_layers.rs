use std::fs;
use std::path::Path;

use phronesis_mcp::language_packs::{self, integration};
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
        "schema_version": 1, "adapter_api": 1, "id": "demo", "version": "1.0.0",
        "language": "demo", "extensions": ["demox"], "grammar": "python",
        "capabilities": ["structural_facts", "toolchains"],
        "queries": [{"id":"functions", "path":"functions.scm", "emit": [
            {"kind":"fact", "name":"function_seen", "args":["${name}"], "span_capture":"def"}
        ]}],
        "rules":"rules.json", "toolchains":"toolchains.json", "context_files":[], "fixtures":[]
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
        r#"{"rules":[{"id":"demo.function","phase":"pre","audit":true,"when":[{"pack_demo_function_seen":["?file","danger"]}],"then":{"block":"danger function"}}]}"#,
    );
    write(
        &source,
        "toolchains.json",
        r#"[{"id":"demo.test","matches":"^demo test"}]"#,
    );
    language_packs::install(root, &source, false).unwrap();
}

#[test]
fn pack_rules_are_lower_precedence_and_never_copied_to_project() {
    let project = tempfile::tempdir().unwrap();
    fixture(project.path());
    let registry = language_packs::load(project.path()).unwrap();
    let layers = integration::pack_rule_layers(&registry).unwrap();
    assert_eq!(layers.len(), 1);
    assert_eq!(layers[0].rules[0].id, "demo.function");
    assert!(!project.path().join(".phronesis/rules.json").exists());
    let resolved = phronesis_mcp::rule_layers::resolve(project.path()).unwrap();
    assert_eq!(resolved.origins["demo.function"].layer, "pack:demo");
    write(
        project.path(),
        ".phronesis/rules.json",
        r#"{"rules":[{"id":"demo.function","phase":"pre","when":[{"pack_demo_function_seen":["?file","safe"]}],"then":{"block":"project override"}}]}"#,
    );
    let resolved = phronesis_mcp::rule_layers::resolve(project.path()).unwrap();
    assert_eq!(resolved.origins["demo.function"].layer, "project");
    assert_eq!(resolved.overrides.len(), 1);
    assert_eq!(resolved.overrides[0].overridden.layer, "pack:demo");
}

#[test]
fn pack_toolchains_are_namespaced_and_live() {
    let project = tempfile::tempdir().unwrap();
    fixture(project.path());
    let registry = language_packs::load(project.path()).unwrap();
    let defs = integration::pack_toolchains(&registry).unwrap();
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].id, "demo.test");
    assert!(
        phronesis_mcp::outcomes::toolchain::registry(project.path())
            .iter()
            .any(|d| d.def.id == "demo.test")
    );
}

#[test]
fn inventory_distinguishes_installed_capabilities_and_project_override() {
    use phronesis_mcp::language_packs::{Activation, PackOrigin};
    let project = tempfile::tempdir().unwrap();
    fixture(project.path());
    let find = |root: &Path| {
        language_packs::inspect(root)
            .packs
            .into_iter()
            .find(|p| p.id.as_deref() == Some("demo") && p.origin == PackOrigin::Installed)
            .unwrap()
    };
    let info = find(project.path());
    assert_eq!(info.activation["structural_facts"], Activation::Active);
    assert_eq!(info.activation["rules"], Activation::Active);
    assert_eq!(info.activation["toolchains"], Activation::Active);
    write(
        project.path(),
        ".phronesis/rules.json",
        r#"{"rules":[{"id":"demo.function","when":[{"file_extension_is":"demox"}],"then":{"block":"override"}}]}"#,
    );
    write(
        project.path(),
        ".phronesis/toolchains.json",
        r#"[{"id":"demo.test","matches":"^custom test"}]"#,
    );
    let info = find(project.path());
    assert_eq!(info.activation["rules"], Activation::Overridden);
    assert_eq!(info.activation["toolchains"], Activation::Overridden);
}

#[test]
fn pack_resource_ids_cannot_impersonate_project_or_other_pack() {
    let project = tempfile::tempdir().unwrap();
    fixture(project.path());
    let src = project.path().join("source");
    write(
        &src,
        "rules.json",
        r#"{"rules":[{"id":"other.policy","when":[{"file_extension_is":"demox"}],"then":{"block":"bad"}}]}"#,
    );
    let assets = language_packs::read_assets(&src).unwrap();
    let pack = language_packs::compile_pack(assets).unwrap();
    let error = integration::validate_pack_resources(&pack)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("other.policy") && error.contains("demo."),
        "{error}"
    );
    write(
        &src,
        "rules.json",
        r#"{"rules":[{"id":"demo.function","when":[{"file_extension_is":"demox"}],"then":{"block":"ok"}}]}"#,
    );
    write(
        &src,
        "toolchains.json",
        r#"[{"id":"other.test","matches":"^demo"}]"#,
    );
    let pack = language_packs::compile_pack(language_packs::read_assets(&src).unwrap()).unwrap();
    let error = integration::validate_pack_resources(&pack)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("other.test") && error.contains("demo."),
        "{error}"
    );
}

struct Client {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    stdout: std::io::BufReader<std::process::ChildStdout>,
    id: u64,
}
impl Client {
    fn new(root: &Path) -> Self {
        use std::io::BufRead;
        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
            .arg("serve")
            .env("PHRONESIS_PROJECT_ROOT", root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = std::io::BufReader::new(child.stdout.take().unwrap());
        let mut client = Self {
            child,
            stdin,
            stdout,
            id: 0,
        };
        client.call("initialize", json!({"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1"}}));
        use std::io::Write;
        writeln!(
            client.stdin,
            "{}",
            json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}})
        )
        .unwrap();
        client.stdin.flush().unwrap();
        // Keep BufRead imported here so rustc checks the response channel type.
        let _ = &mut client.stdout as &mut dyn BufRead;
        client
    }
    fn call(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        use std::io::{BufRead, Write};
        self.id += 1;
        writeln!(
            self.stdin,
            "{}",
            json!({"jsonrpc":"2.0","id":self.id,"method":method,"params":params})
        )
        .unwrap();
        self.stdin.flush().unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }
    fn tool(&mut self, name: &str, args: serde_json::Value) -> serde_json::Value {
        self.call("tools/call", json!({"name":name,"arguments":args}))
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `get_rule` answers with the rule as a JSON text payload (no structured
/// content), matching the existing MCP tests; parse it.
fn rule_text(response: serde_json::Value) -> serde_json::Value {
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("get_rule returned no text payload: {response}"));
    serde_json::from_str(text).unwrap_or_else(|e| panic!("{e}: {text}"))
}

#[test]
fn same_connection_refreshes_pack_rules_and_keeps_session_fact() {
    let project = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    fixture(source.path());
    let mut client = Client::new(project.path());
    client.tool(
        "assert_fact",
        json!({"id":"session-one","predicate":"session_tag","args":["yes"]}),
    );
    let before = client.tool("list_rules", json!({}));
    assert_eq!(
        before["result"]["structuredContent"]["rules"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    fs::create_dir_all(project.path().join(".phronesis/packs")).unwrap();
    fs::rename(
        source.path().join(".phronesis/packs/demo"),
        project.path().join(".phronesis/packs/demo"),
    )
    .unwrap();
    let added = client.tool("list_rules", json!({}));
    assert!(
        added["result"]["structuredContent"]["rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"] == "demo.function"),
        "{added}"
    );
    let facts = client.tool("list_facts", json!({}));
    assert!(
        facts["result"]["structuredContent"]["facts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["id"] == "session-one")
    );
    write(
        project.path(),
        ".phronesis/rules.json",
        r#"{"rules":[{"id":"demo.function","phase":"pre","when":[{"pack_demo_function_seen":["?file","safe"]}],"then":{"block":"project override"}}]}"#,
    );
    let overridden = client.tool("list_rules", json!({}));
    assert!(
        overridden["result"]["structuredContent"]["rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"] == "demo.function"),
        "{overridden}"
    );
    let fetched = client.tool("get_rule", json!({"rule_id":"demo.function"}));
    assert_eq!(
        rule_text(fetched.clone())["actions"][0]["params"][0],
        "project override",
        "{fetched}"
    );
    fs::remove_file(project.path().join(".phronesis/rules.json")).unwrap();
    let restored = client.tool("get_rule", json!({"rule_id":"demo.function"}));
    assert_eq!(
        rule_text(restored.clone())["actions"][0]["params"][0],
        "danger function",
        "{restored}"
    );
    let facts = client.tool("list_facts", json!({}));
    assert!(
        facts["result"]["structuredContent"]["facts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["id"] == "session-one")
    );
    client.tool("save_rules", json!({"dry_run":false,"merge":false}));
    let saved = fs::read_to_string(project.path().join(".phronesis/rules.json")).unwrap();
    assert!(!saved.contains("demo.function"), "{saved}");
    fs::remove_dir_all(project.path().join(".phronesis/packs/demo")).unwrap();
    let removed = client.tool("list_rules", json!({}));
    assert!(
        !removed["result"]["structuredContent"]["rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"] == "demo.function"),
        "{removed}"
    );
    let facts = client.tool("list_facts", json!({}));
    assert!(
        facts["result"]["structuredContent"]["facts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["id"] == "session-one")
    );
}

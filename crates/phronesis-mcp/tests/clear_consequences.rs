use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    id: u64,
}

impl Client {
    fn spawn(root: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
            .arg("serve")
            .env("PHRONESIS_PROJECT_ROOT", root)
            .env("PHRONESIS_NO_AUTOPERSIST", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn MCP server");
        let mut client = Self {
            stdin: child.stdin.take(),
            stdout: BufReader::new(child.stdout.take().expect("server stdout")),
            child,
            id: 0,
        };
        client.request(
            "initialize",
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name":"clear-consequences-test","version":"0.1"}
            }),
        );
        client.notify("notifications/initialized", serde_json::json!({}));
        client
    }

    fn send(&mut self, message: serde_json::Value) {
        let stdin = self.stdin.as_mut().expect("server stdin open");
        writeln!(stdin, "{message}").expect("write MCP message");
        stdin.flush().expect("flush MCP message");
    }

    fn request(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        self.id += 1;
        let id = self.id;
        self.send(serde_json::json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}));
        let mut line = String::new();
        self.stdout.read_line(&mut line).expect("read MCP response");
        serde_json::from_str(&line).expect("valid MCP response")
    }

    fn notify(&mut self, method: &str, params: serde_json::Value) {
        self.send(serde_json::json!({"jsonrpc":"2.0", "method":method, "params":params}));
    }

    fn tool(&mut self, name: &str, arguments: serde_json::Value) -> serde_json::Value {
        let response = self.request(
            "tools/call",
            serde_json::json!({"name":name, "arguments":arguments}),
        );
        let text = response["result"]["content"][0]["text"]
            .as_str()
            .expect("tool text");
        serde_json::from_str(text).unwrap_or_else(|_| serde_json::Value::String(text.to_owned()))
    }

    fn finish(mut self) {
        drop(self.stdin.take());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let status = loop {
            if let Some(status) = self.child.try_wait().expect("poll MCP server exit") {
                break status;
            }
            if std::time::Instant::now() >= deadline {
                self.child.kill().expect("kill hung MCP server");
                let _ = self.child.wait();
                panic!("MCP server did not exit within 10 seconds after stdin EOF");
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        };
        let mut remaining = String::new();
        self.stdout
            .read_to_string(&mut remaining)
            .expect("drain server stdout");
        assert!(
            status.success(),
            "server exited with {status}; trailing output: {remaining}"
        );
    }
}

#[test]
fn clear_consequences_mcp_tool_clears_fired_consequences_and_reports_count() {
    let root = tempfile::tempdir().expect("project tempdir");
    let mut client = Client::spawn(root.path());
    client.tool(
        "add_rule",
        serde_json::json!({
            "id":"warn-ready", "priority":5,
            "conditions":[{"predicate":"status", "args":["ready"]}],
            "actions":[{"action_type":"constraint_warning", "params":["ready warning"]}]
        }),
    );
    client.tool(
        "assert_fact",
        serde_json::json!({"id":"ready", "predicate":"status", "args":["ready"]}),
    );
    let fired = client.tool("fire_rules", serde_json::json!({}));
    assert_eq!(fired["actions_fired"], 1);
    assert_eq!(
        client.tool("get_consequences", serde_json::json!({}))["consequences"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let cleared = client.tool("clear_consequences", serde_json::json!({}));
    assert_eq!(cleared, "Cleared 1 consequence(s)");
    assert!(
        client.tool("get_consequences", serde_json::json!({}))["consequences"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    client.finish();
}

//! End-to-end tests for `set_section_context` and `clear_section_context`,
//! driven through the MCP stdio server in a subprocess.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

struct McpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl McpClient {
    fn spawn() -> Self {
        // Disable autopersist so test rules never touch real disk state. These
        // tests verify in-memory rule + context behavior, not persistence.
        let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
            .arg("serve")
            .env("PHRONESIS_NO_AUTOPERSIST", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn server");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut c = Self {
            child,
            stdin,
            stdout,
            next_id: 0,
        };
        c.call(
            "initialize",
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name":"test","version":"0.1"}
            }),
        );
        c.notify("notifications/initialized", serde_json::json!({}));
        c
    }
    fn call(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        self.next_id += 1;
        let msg =
            serde_json::json!({"jsonrpc":"2.0","id":self.next_id,"method":method,"params":params});
        writeln!(self.stdin, "{}", msg).unwrap();
        self.stdin.flush().unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }
    fn notify(&mut self, method: &str, params: serde_json::Value) {
        let msg = serde_json::json!({"jsonrpc":"2.0","method":method,"params":params});
        writeln!(self.stdin, "{}", msg).unwrap();
        self.stdin.flush().unwrap();
    }
    fn tool(&mut self, name: &str, args: serde_json::Value) -> serde_json::Value {
        let r = self.call(
            "tools/call",
            serde_json::json!({"name":name,"arguments":args}),
        );
        let text = r["result"]["content"][0]["text"].as_str().unwrap();
        serde_json::from_str(text).unwrap_or_else(|_| serde_json::Value::String(text.into()))
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn set_section_context_asserts_markdown_rule_fact() {
    let mut c = McpClient::spawn();
    // Add a rule that only fires when working in the "Error Handling" section.
    c.tool(
        "add_rule",
        serde_json::json!({
            "id":"err-reminder",
            "priority": 5,
            "conditions": [{"predicate":"markdown_rule","args":["docs/X.md","Error Handling"]}],
            "actions": [{"action_type":"constraint_violation","params":["use thiserror"]}]
        }),
    );
    // Set context — should assert the matching fact.
    c.tool(
        "set_section_context",
        serde_json::json!({
            "file": "docs/X.md",
            "section": "Error Handling"
        }),
    );
    let fired = c.tool("fire_rules", serde_json::json!({}));
    assert_eq!(fired["actions_fired"], 1);
    assert!(fired["actions"][0]["params"][0]
        .as_str()
        .unwrap()
        .contains("thiserror"));
}

#[test]
fn clear_section_context_retracts_the_fact() {
    let mut c = McpClient::spawn();
    c.tool(
        "add_rule",
        serde_json::json!({
            "id":"x",
            "priority": 5,
            "conditions": [{"predicate":"markdown_rule","args":["doc.md","Idioms"]}],
            "actions": [{"action_type":"constraint_violation","params":["msg"]}]
        }),
    );
    c.tool(
        "set_section_context",
        serde_json::json!({"file":"doc.md","section":"Idioms"}),
    );
    // Confirm the rule fires while context is set.
    let fired = c.tool("fire_rules", serde_json::json!({}));
    assert_eq!(fired["actions_fired"], 1);

    // Clear context — rule should no longer fire.
    c.tool("clear_section_context", serde_json::json!({}));
    let after = c.tool("fire_rules", serde_json::json!({}));
    assert_eq!(after["actions_fired"], 0);
}

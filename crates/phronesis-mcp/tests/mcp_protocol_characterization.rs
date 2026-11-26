//! Characterization tests for the MCP stdio protocol surface
//! (`phr-mcp serve`) during the rmcp 0.17 -> 2.1 migration.
//! These pin preserved server identity, capabilities, instructions, and
//! tool schemas, plus the explicitly adopted SDK negotiation and error
//! behavior. The initial tests preceded the production migration; review
//! repairs and timeout mutation checks are retrospective validation.
//!
//! Both `2024-11-05` and `2025-11-25` are supported by rmcp 2.1,
//! so the server must echo each request verbatim. The old SDK instead
//! negotiated `2025-06-18` when asked for `2025-11-25`.
//!
//! `tools/list` is compared against a committed baseline fixture captured
//! from the pre-migration server. The only permitted difference is the
//! rmcp 2.1 schema generator's removal of the root `inputSchema.title` /
//! `inputSchema.description` keys (33 titles + 1 description, per the
//! baseline); `required`, `properties`, and every type stay byte-identical.
//! Tool order is normalized by name on both sides before comparing, since
//! ordering is not a contract rmcp guarantees.
//!
//! `tools/call` error shapes are pinned exactly rather than "either is
//! fine": malformed arguments for a known tool must come back as a
//! tool-level error (`isError: true`), and an unknown tool name must come
//! back as a JSON-RPC protocol error with code `-32602`, per the approved
//! rmcp 2.1 error semantics (`IntoCallToolResult` / `CallToolResult`).

use std::io::{Read, Write as _};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(20);

/// Reads whole stdout lines on a dedicated thread and forwards them over a
/// channel, so a caller can bound how long it waits for any one response
/// with `recv_timeout` instead of blocking forever in `read_line` if the
/// child hangs or never answers.
fn spawn_line_reader<R: Read + Send + 'static>(mut reader: R) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            match reader.read(&mut byte) {
                Ok(0) => break,
                Ok(_) => {
                    if byte[0] == b'\n' {
                        if let Ok(line) = String::from_utf8(std::mem::take(&mut buf)) {
                            if tx.send(line).is_err() {
                                break;
                            }
                        } else {
                            buf.clear();
                        }
                    } else {
                        buf.push(byte[0]);
                    }
                }
                Err(_) => break,
            }
        }
    });
    rx
}

/// The outcome of waiting for a response: distinct from a plain `Option` so
/// a caller (and a test) can tell a real timeout (the deadline elapsed,
/// nothing matched yet, but the sender may still be alive) apart from the
/// channel actually closing (the reader thread ended because the child's
/// stdout hit EOF) — conflating the two would let a server that silently
/// exited be misreported as merely slow.
#[derive(Debug)]
enum ReceiveOutcome {
    Matched(Value),
    TimedOut,
    Disconnected,
}

/// Waits for the JSON-RPC response whose `id` matches, against an *absolute*
/// deadline rather than a per-recv timeout: an intervening line that is not
/// our response (a notification, a reply to some other request) must not
/// reset how much longer we are willing to wait overall.
fn receive_response(lines: &mpsc::Receiver<String>, id: u64, deadline: Instant) -> ReceiveOutcome {
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return ReceiveOutcome::TimedOut;
        }
        match lines.recv_timeout(remaining) {
            Ok(line) => {
                let Ok(value) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if value.get("id").and_then(Value::as_u64) == Some(id) {
                    return ReceiveOutcome::Matched(value);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => return ReceiveOutcome::TimedOut,
            Err(mpsc::RecvTimeoutError::Disconnected) => return ReceiveOutcome::Disconnected,
        }
    }
}

struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
    next_id: u64,
}

impl Client {
    /// Spawn `phr-mcp serve` without sending anything yet, so a caller can
    /// drive the handshake itself or exercise the timeout path.
    fn spawn_raw(root: &Path) -> Self {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_phr-mcp"));
        cmd.arg("serve")
            .env("PHRONESIS_PROJECT_ROOT", root)
            .env("PHRONESIS_NO_AUTOPERSIST", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd.spawn().expect("spawn phr-mcp serve");
        let stdin = child.stdin.take().expect("server stdin");
        let stdout = child.stdout.take().expect("server stdout");
        let lines = spawn_line_reader(stdout);
        Self {
            child,
            stdin: Some(stdin),
            lines,
            next_id: 0,
        }
    }

    /// Spawn `phr-mcp serve` and send `initialize` with the given protocol
    /// version, returning both the client and the raw `initialize` response
    /// so callers can inspect it before `notifications/initialized` is sent.
    fn spawn(root: &Path, protocol_version: &str) -> (Self, Value) {
        let mut client = Self::spawn_raw(root);
        let response = client.call(
            "initialize",
            json!({
                "protocolVersion": protocol_version,
                "capabilities": {},
                "clientInfo": {"name": "protocol-characterization-test", "version": "0.1"}
            }),
        );
        client.notify("notifications/initialized", json!({}));
        (client, response)
    }

    /// Bound how long we wait for any single response: a hung server fails
    /// the test immediately (after reaping the child) rather than stalling
    /// the test binary indefinitely. The deadline is absolute across the
    /// whole call, via `receive_response`, so a stream of unrelated lines
    /// cannot extend how long we wait.
    fn call(&mut self, method: &str, params: Value) -> Value {
        self.call_with_timeout(method, params, RESPONSE_TIMEOUT)
    }

    /// As `call`, but with an explicit timeout — used by the timeout test
    /// itself so it does not have to wait out the full production
    /// `RESPONSE_TIMEOUT` to prove the reap-on-timeout path works.
    fn call_with_timeout(&mut self, method: &str, params: Value, timeout: Duration) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let stdin = self.stdin.as_mut().expect("server stdin is open");
        writeln!(stdin, "{msg}").expect("write MCP request");
        stdin.flush().expect("flush MCP request");
        let deadline = Instant::now() + timeout;
        match receive_response(&self.lines, id, deadline) {
            ReceiveOutcome::Matched(value) => value,
            ReceiveOutcome::TimedOut => {
                self.reap();
                panic!(
                    "no response to {method} (id {id}) within {timeout:?} \
                     (timed out; server may still have been running); child reaped"
                );
            }
            ReceiveOutcome::Disconnected => {
                self.reap();
                panic!(
                    "no response to {method} (id {id}): server closed its stdout (EOF) \
                     before answering; child reaped"
                );
            }
        }
    }

    fn notify(&mut self, method: &str, params: Value) {
        let msg = json!({"jsonrpc": "2.0", "method": method, "params": params});
        let stdin = self.stdin.as_mut().expect("server stdin is open");
        writeln!(stdin, "{msg}").expect("write MCP notification");
        stdin.flush().expect("flush MCP notification");
    }

    /// Kill and reap the child so a timeout or panic never leaves an orphan
    /// process behind.
    fn reap(&mut self) {
        let _ = self.stdin.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn try_wait(&mut self) -> Option<std::process::ExitStatus> {
        self.child.try_wait().ok().flatten()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.reap();
    }
}

#[test]
fn receive_response_does_not_accept_a_queued_match_after_the_deadline() {
    // Deterministic, no wall-clock assertion: the messages are enqueued up
    // front (not timed), and the deadline is already in the past before
    // `receive_response` is ever called, so a correct absolute-deadline
    // check must report TimedOut immediately without needing to drain (or
    // time) the queue at all.
    let (tx, rx) = mpsc::channel::<String>();
    for _ in 0..5 {
        tx.send(json!({"jsonrpc":"2.0","method":"notifications/progress","params":{}}).to_string())
            .expect("queue a non-matching notification");
    }
    tx.send(json!({"jsonrpc":"2.0","id":1,"result":{}}).to_string())
        .expect("queue a late matching response");
    let already_expired = Instant::now() - Duration::from_millis(1);
    match receive_response(&rx, 1, already_expired) {
        ReceiveOutcome::TimedOut => {}
        other => {
            panic!("an expired deadline must reject even a queued matching response, got {other:?}")
        }
    }
}

#[test]
fn receive_response_returns_an_already_queued_match_before_an_unexpired_deadline() {
    // Companion to the test above: without this case, a `receive_response`
    // that always returns `TimedOut` would also pass the expired-deadline
    // test. A matching message queued ahead of a deadline that has not
    // elapsed yet must be returned.
    let (tx, rx) = mpsc::channel::<String>();
    tx.send(json!({"jsonrpc":"2.0","method":"notifications/progress","params":{}}).to_string())
        .expect("queue a non-matching notification");
    tx.send(json!({"jsonrpc":"2.0","id":7,"result":{"ok":true}}).to_string())
        .expect("queue the matching response");
    let not_yet_expired = Instant::now() + Duration::from_secs(5);
    match receive_response(&rx, 7, not_yet_expired) {
        ReceiveOutcome::Matched(value) => assert_eq!(value["id"], 7),
        other => panic!("a queued match before the deadline must be returned, got {other:?}"),
    }
}

#[test]
fn client_timeout_branch_reaps_the_child_before_panicking() {
    let root = tempfile::tempdir().expect("project tempdir");
    let (mut client, _init) = Client::spawn(root.path(), "2024-11-05");
    assert!(client.try_wait().is_none());
    // An already-expired deadline takes the real timeout branch without
    // depending on scheduling speed or a malformed protocol message.
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.call_with_timeout("tools/list", json!({}), Duration::ZERO)
    }))
    .expect_err("expired response deadline must fail");
    let message = panic.downcast_ref::<String>().expect("timeout diagnosis");
    assert!(message.contains("timed out"), "{message}");
    assert!(
        client.try_wait().is_some(),
        "timeout branch must reap the child"
    );
}

#[test]
fn waiting_on_an_id_nobody_requested_times_out_and_reaps_a_live_server() {
    // A real, fully initialized `phr-mcp serve` process — no malformed
    // input, no assumption that a corrupted protocol stream keeps a server
    // silent. Waiting for a response to an id that was never sent as part
    // of any request is a deterministically silent wait: the server has no
    // way to ever produce a matching line, yet it stays alive and able to
    // answer everything else, which this test checks for before the wait.
    let root = tempfile::tempdir().expect("project tempdir");
    let (mut client, _init) = Client::spawn(root.path(), "2024-11-05");
    assert!(
        client.try_wait().is_none(),
        "server must still be running before the wait"
    );

    let deadline = Instant::now() + Duration::from_millis(300);
    match receive_response(&client.lines, 999_999, deadline) {
        ReceiveOutcome::TimedOut => {}
        other => {
            panic!("no request was ever sent for id 999999, so this must time out, not {other:?}")
        }
    }

    client.reap();
    assert!(
        client.try_wait().is_some(),
        "child must be reaped after the wait times out"
    );
}

#[test]
fn initialize_reports_server_identity_capabilities_and_instructions() {
    let root = tempfile::tempdir().expect("project tempdir");
    let (_client, response) = Client::spawn(root.path(), "2024-11-05");
    let result = &response["result"];
    assert!(
        response.get("error").is_none(),
        "initialize returned an error: {response}"
    );

    assert_eq!(result["serverInfo"]["name"], "phr-mcp");
    assert_eq!(
        result["serverInfo"]["version"],
        env!("CARGO_PKG_VERSION"),
        "serverInfo.version must track the crate version"
    );

    // Tools capability is advertised; resources/prompts are not part of this
    // server's surface, so we only assert what the server actually enables.
    assert!(
        result["capabilities"]["tools"].is_object(),
        "capabilities.tools must be present: {result}"
    );

    let instructions = result["instructions"]
        .as_str()
        .expect("instructions must be a string");
    assert!(instructions.contains("query_code_graph"));
    assert!(
        instructions.contains("https://github.com/awaterma/phronesis/blob/main/docs/FOR_AGENTS.md")
    );

    assert_eq!(
        result["protocolVersion"], "2024-11-05",
        "server must echo the exact requested protocol version: {result}"
    );
}

#[test]
fn initialize_echoes_the_exact_requested_protocol_version_2024_11_05() {
    let root = tempfile::tempdir().expect("project tempdir");
    let (_client, response) = Client::spawn(root.path(), "2024-11-05");
    assert!(response.get("error").is_none(), "{response}");
    assert_eq!(
        response["result"]["protocolVersion"], "2024-11-05",
        "requested 2024-11-05 must be echoed verbatim, not substituted: {response}"
    );
}

#[test]
fn initialize_echoes_the_exact_requested_protocol_version_2025_11_25() {
    let root = tempfile::tempdir().expect("project tempdir");
    let (_client, response) = Client::spawn(root.path(), "2025-11-25");
    assert!(response.get("error").is_none(), "{response}");
    assert_eq!(
        response["result"]["protocolVersion"], "2025-11-25",
        "requested 2025-11-25 must be echoed verbatim, not substituted: {response}"
    );
}

/// Strip the root `title`/`description` keys the rmcp 2.1 schema generator
/// drops relative to the pre-migration baseline. `required`, `properties`,
/// and every type stay untouched; this is the one documented, approved
/// metadata difference.
fn normalize_schema(mut tool: Value) -> Value {
    if let Some(schema) = tool.get_mut("inputSchema").and_then(Value::as_object_mut) {
        schema.remove("title");
        schema.remove("description");
    }
    tool
}

fn normalized_sorted_tools(tools: &[Value]) -> Vec<Value> {
    let mut normalized: Vec<Value> = tools.iter().cloned().map(normalize_schema).collect();
    normalized.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or_default()
            .cmp(b["name"].as_str().unwrap_or_default())
    });
    normalized
}

#[test]
fn tools_list_matches_the_full_baseline_contract() {
    let root = tempfile::tempdir().expect("project tempdir");
    let (mut client, _init) = Client::spawn(root.path(), "2024-11-05");
    let response = client.call("tools/list", json!({}));
    let tools = response["result"]["tools"]
        .as_array()
        .expect("tools/list result.tools must be an array")
        .clone();

    let baseline_fixture = include_str!("fixtures/mcp-protocol/wire-baseline-tools.json");
    let baseline: Value = serde_json::from_str(baseline_fixture).expect("valid baseline fixture");
    let baseline_tools = baseline["tools"]
        .as_array()
        .expect("fixture tools must be an array")
        .clone();

    let actual = normalized_sorted_tools(&tools);
    let expected = normalized_sorted_tools(&baseline_tools);

    let actual_names: Vec<&str> = actual
        .iter()
        .map(|t| t["name"].as_str().unwrap_or_default())
        .collect();
    let expected_names: Vec<&str> = expected
        .iter()
        .map(|t| t["name"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        actual_names, expected_names,
        "tool name set changed relative to the baseline"
    );

    for (name, (got, want)) in actual_names.iter().zip(actual.iter().zip(expected.iter())) {
        assert_eq!(
            got, want,
            "tool {name:?} schema diverged from the baseline (beyond the \
             documented root title/description removal)"
        );
    }
}

#[test]
fn tools_call_with_valid_arguments_returns_success_text_content() {
    let root = tempfile::tempdir().expect("project tempdir");
    let (mut client, _init) = Client::spawn(root.path(), "2024-11-05");
    let response = client.call("tools/call", json!({"name": "list_rules", "arguments": {}}));
    let result = &response["result"];
    assert!(response.get("error").is_none(), "{response}");
    assert_ne!(result["isError"], Value::Bool(true), "{response}");
    let content = result["content"]
        .as_array()
        .expect("content must be an array");
    assert!(!content.is_empty(), "{response}");
    assert_eq!(content[0]["type"], "text");
    assert!(
        content[0]["text"].as_str().is_some(),
        "text content block must carry a text field: {response}"
    );
}

#[test]
fn tools_call_with_invalid_arguments_is_a_tool_level_error() {
    let root = tempfile::tempdir().expect("project tempdir");
    let (mut client, _init) = Client::spawn(root.path(), "2024-11-05");
    // `add_rule` requires `id`; omit it entirely to trigger a schema/handler
    // rejection rather than a domain-level validation message.
    let response = client.call("tools/call", json!({"name": "add_rule", "arguments": {}}));
    // The approved rmcp 2.1 contract: malformed arguments for a known tool
    // are reported as a tool-level error result, never a JSON-RPC protocol
    // error and never a silent success.
    assert!(
        response.get("error").is_none(),
        "malformed arguments must not surface as a JSON-RPC protocol error: {response}"
    );
    assert_eq!(
        response["result"]["isError"],
        Value::Bool(true),
        "malformed arguments must be reported with isError: true: {response}"
    );
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .expect("error result must carry text content");
    assert!(!text.is_empty(), "error text must not be empty: {response}");
}

#[test]
fn tools_call_with_unknown_tool_name_is_a_json_rpc_protocol_error() {
    let root = tempfile::tempdir().expect("project tempdir");
    let (mut client, _init) = Client::spawn(root.path(), "2024-11-05");
    let response = client.call(
        "tools/call",
        json!({"name": "no_such_tool_exists", "arguments": {}}),
    );
    // The approved rmcp 2.1 contract: an unknown tool name is a JSON-RPC
    // protocol error (code -32602), never a tool result.
    assert!(
        response.get("result").is_none(),
        "unknown tool must not produce a tool result: {response}"
    );
    assert_eq!(
        response["error"]["code"],
        Value::from(-32602),
        "unknown tool must be reported as JSON-RPC error -32602: {response}"
    );
}

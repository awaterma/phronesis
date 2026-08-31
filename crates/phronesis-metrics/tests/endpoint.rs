//! End-to-end tests of the `/metrics` HTTP endpoint over a real socket.

use phronesis_metrics::families::Options;
use phronesis_metrics::serve::{self, ServeConfig};
use phronesis_metrics::{Error, Live};
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Build a project root with a `.phronesis/log.jsonl` holding `lines`.
fn project_with_log(lines: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let phr = dir.path().join(".phronesis");
    std::fs::create_dir_all(&phr).expect("mkdir");
    std::fs::write(phr.join("log.jsonl"), lines.join("\n") + "\n").expect("write log");
    dir
}

/// Issue a bare HTTP/1.1 GET and return (status line, body). Hand-rolled so the
/// test suite needs no HTTP client dependency.
async fn get(addr: SocketAddr, path: &str) -> (String, String) {
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    let req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).await.expect("write");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.expect("read");
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((text.as_str(), ""));
    let status = head.lines().next().unwrap_or_default().to_string();
    (status, body.to_string())
}

async fn spawn(cfg: ServeConfig, live: Option<Live>) -> SocketAddr {
    let (listener, addr) = serve::bind(&cfg).await.expect("bind");
    tokio::spawn(async move {
        let _ = serve::serve_on(listener, cfg, live).await;
    });
    addr
}

#[tokio::test]
async fn metrics_endpoint_serves_derived_counters() {
    let dir = project_with_log(&[
        r#"{"ts":100,"kind":"hook","event":"pre_check","phase":"pre","tool":"Edit","file":"/tmp/leaky_path.rs","exit":2,"consequences":[{"rule_id":"no-await","action_type":"constraint_violation"}]}"#,
        r#"{"ts":101,"kind":"mcp","event":"fire_rules"}"#,
        r#"{"ts":102,"kind":"context","event":"session","bytes":100,"estimated_tokens":25,"latency_micros":7,"omitted":{"rule":1}}"#,
    ]);
    let cfg = ServeConfig::new("127.0.0.1:0".parse().unwrap(), dir.path().to_path_buf());
    let addr = spawn(cfg, None).await;

    let (status, body) = get(addr, "/metrics").await;
    assert!(status.contains("200"), "status was {status}");
    assert!(body.ends_with("# EOF\n"), "body:\n{body}");

    // The numbers correspond to the log lines actually on disk.
    assert!(
        body.contains(r#"phronesis_hook_checks_total{phase="pre",tool="Edit",decision="block"} 1"#),
        "{body}"
    );
    assert!(
        body.contains(r#"phronesis_rule_fires_total{rule_id="no-await",outcome="blocked"} 1"#),
        "{body}"
    );
    assert!(
        body.contains(r#"phronesis_mcp_tool_calls_total{tool="fire_rules"} 1"#),
        "{body}"
    );
    assert!(
        body.contains(r#"phronesis_context_estimated_tokens_total{event="session"} 25"#),
        "{body}"
    );
    assert!(body.contains("phronesis_log_entries_total 3"), "{body}");
    // The path in the log must not have survived into the scrape.
    assert!(
        !body.contains("leaky_path"),
        "path leaked over HTTP:\n{body}"
    );
}

#[tokio::test]
async fn live_metrics_merge_into_the_same_scrape() {
    let dir = project_with_log(&[r#"{"ts":1,"kind":"mcp","event":"add_rule"}"#]);
    let live = Live::new("0.31.1", true);
    live.observe_tool("fire_rules", 0.004);
    live.record_tool_error("add_rule");
    live.set_rules_loaded(42);

    let cfg = ServeConfig::new("127.0.0.1:0".parse().unwrap(), dir.path().to_path_buf());
    let addr = spawn(cfg, Some(live)).await;
    let (_, body) = get(addr, "/metrics").await;

    // Live families...
    assert!(body.contains("phronesis_server_rules_loaded 42"), "{body}");
    assert!(
        body.contains(r#"phronesis_build_info{version="0.31.1",rhai="on"} 1"#),
        "{body}"
    );
    assert!(
        body.contains(r#"phronesis_server_tool_errors_total{tool="add_rule"} 1"#),
        "{body}"
    );
    assert!(
        body.contains(r#"phronesis_server_tool_latency_seconds_count{tool="fire_rules"} 1"#),
        "{body}"
    );
    assert!(body.contains("phronesis_server_uptime_seconds"), "{body}");
    // ...alongside log-derived ones, in one document with a single EOF.
    assert!(
        body.contains(r#"phronesis_mcp_tool_calls_total{tool="add_rule"} 1"#),
        "{body}"
    );
    assert_eq!(body.matches("# EOF").count(), 1, "{body}");
}

#[tokio::test]
async fn missing_log_yields_an_empty_but_valid_scrape() {
    // A project that has never run a hook must still scrape cleanly.
    let dir = tempfile::tempdir().unwrap();
    let cfg = ServeConfig::new("127.0.0.1:0".parse().unwrap(), dir.path().to_path_buf());
    let addr = spawn(cfg, None).await;
    let (status, body) = get(addr, "/metrics").await;
    assert!(status.contains("200"), "{status}");
    assert!(body.contains("phronesis_log_entries_total 0"), "{body}");
    assert!(body.ends_with("# EOF\n"), "{body}");
}

#[tokio::test]
async fn unknown_route_is_404_and_root_points_at_metrics() {
    let dir = project_with_log(&[r#"{"ts":1,"kind":"mcp","event":"get_stats"}"#]);
    let cfg = ServeConfig::new("127.0.0.1:0".parse().unwrap(), dir.path().to_path_buf());
    let addr = spawn(cfg, None).await;

    let (status, _) = get(addr, "/nope").await;
    assert!(status.contains("404"), "{status}");

    let (status, body) = get(addr, "/").await;
    assert!(status.contains("200"), "{status}");
    assert!(body.contains("/metrics"), "{body}");
}

#[tokio::test]
async fn non_loopback_bind_is_always_refused() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = ServeConfig::new("0.0.0.0:0".parse().unwrap(), dir.path().to_path_buf());
    // Guard must trip before any socket is opened.
    let err = serve::bind(&cfg).await.expect_err("must refuse");
    assert!(matches!(err, Error::NonLoopbackBind(_)), "got {err:?}");
    assert!(err.to_string().contains("non-loopback address"), "{err}");
}

#[tokio::test]
async fn cardinality_cap_is_enforced_over_http() {
    let mut consequences = String::new();
    for i in 0..10 {
        if i > 0 {
            consequences.push(',');
        }
        consequences.push_str(&format!(
            r#"{{"rule_id":"rule-{i:02}","action_type":"constraint_warning"}}"#
        ));
    }
    let line = format!(
        r#"{{"ts":5,"kind":"hook","event":"post_check","exit":0,"consequences":[{consequences}]}}"#
    );
    let dir = project_with_log(&[&line]);

    let mut cfg = ServeConfig::new("127.0.0.1:0".parse().unwrap(), dir.path().to_path_buf());
    cfg.options = Options {
        max_rule_series: 3,
        ..Default::default()
    };
    let addr = spawn(cfg, None).await;
    let (_, body) = get(addr, "/metrics").await;

    let series = body.matches("phronesis_rule_fires_total{").count();
    assert_eq!(series, 4, "3 named + __other__ expected, body:\n{body}");
    assert!(body.contains("__other__"), "{body}");
}

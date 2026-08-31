//! Derivation tests over synthetic log records.

use phronesis_metrics::families::{self, OTHER_RULE, Options};
use phronesis_metrics::log::{LogRead, LogRecord};

fn record(json: serde_json::Value) -> LogRecord {
    serde_json::from_value(json).expect("record parses")
}

fn read_of(records: Vec<LogRecord>) -> LogRead {
    LogRead {
        records,
        bytes: 0,
        malformed: 0,
    }
}

fn render(records: Vec<LogRecord>, opts: &Options) -> String {
    let registry = families::build(&read_of(records), opts);
    let mut buf = String::new();
    prometheus_client::encoding::text::encode(&mut buf, &registry).expect("encodes");
    buf
}

fn consequence(rule_id: &str, action_type: &str) -> serde_json::Value {
    serde_json::json!({ "rule_id": rule_id, "action_type": action_type })
}

#[test]
fn hook_decision_block_takes_priority_over_warning() {
    // Non-zero exit means the harness rejected the edit; that dominates even
    // when a warning consequence also rode along.
    let out = render(
        vec![record(serde_json::json!({
            "ts": 100, "kind": "hook", "event": "pre_check",
            "phase": "pre", "tool": "Edit", "exit": 2,
            "consequences": [consequence("r1", "constraint_warning")],
        }))],
        &Options::default(),
    );
    assert!(
        out.contains(r#"decision="block""#),
        "expected block decision, got:\n{out}"
    );
    assert!(!out.contains(r#"decision="warn""#));
}

#[test]
fn hook_decision_distinguishes_warn_from_allow() {
    let warned = render(
        vec![record(serde_json::json!({
            "ts": 100, "kind": "hook", "event": "post_check",
            "phase": "post", "tool": "Write", "exit": 0,
            "consequences": [consequence("r1", "constraint_warning")],
        }))],
        &Options::default(),
    );
    assert!(warned.contains(r#"decision="warn""#), "{warned}");

    let allowed = render(
        vec![record(serde_json::json!({
            "ts": 100, "kind": "hook", "event": "post_check",
            "phase": "post", "tool": "Write", "exit": 0, "consequences": [],
        }))],
        &Options::default(),
    );
    assert!(allowed.contains(r#"decision="allow""#), "{allowed}");
}

#[test]
fn outcome_mapping_matches_stats_aggregate() {
    // constraint_violation -> blocked, constraint_warning -> warned, and any
    // other action type is skipped rather than creating an empty series.
    let out = render(
        vec![record(serde_json::json!({
            "ts": 100, "kind": "hook", "event": "pre_check", "exit": 0,
            "consequences": [
                consequence("blocker", "constraint_violation"),
                consequence("warner", "constraint_warning"),
                consequence("noise", "some_other_action"),
            ],
        }))],
        &Options::default(),
    );
    assert!(
        out.contains(r#"rule_id="blocker",outcome="blocked""#),
        "{out}"
    );
    assert!(
        out.contains(r#"rule_id="warner",outcome="warned""#),
        "{out}"
    );
    assert!(
        !out.contains("noise"),
        "unknown action types must not appear:\n{out}"
    );
}

#[test]
fn since_cutoff_excludes_older_records() {
    let records = vec![
        record(serde_json::json!({
            "ts": 100, "kind": "hook", "event": "pre_check", "tool": "Edit", "exit": 0,
            "consequences": [consequence("old", "constraint_violation")],
        })),
        record(serde_json::json!({
            "ts": 900, "kind": "hook", "event": "pre_check", "tool": "Edit", "exit": 0,
            "consequences": [consequence("recent", "constraint_violation")],
        })),
    ];
    let opts = Options {
        since: Some(500),
        ..Default::default()
    };
    let out = render(records, &opts);
    assert!(out.contains("recent"), "{out}");
    assert!(
        !out.contains(r#"rule_id="old""#),
        "cutoff not applied:\n{out}"
    );
    assert!(
        out.contains("phronesis_log_entries_total 1"),
        "only the in-window record should be counted:\n{out}"
    );
}

#[test]
fn cardinality_cap_folds_overflow_into_other() {
    // Five rules, cap of two: the two busiest keep their identity and the rest
    // collapse into a single __other__ series.
    let mut consequences = Vec::new();
    for (rule, times) in [("r_a", 5), ("r_b", 4), ("r_c", 3), ("r_d", 2), ("r_e", 1)] {
        for _ in 0..times {
            consequences.push(consequence(rule, "constraint_violation"));
        }
    }
    let out = render(
        vec![record(serde_json::json!({
            "ts": 100, "kind": "hook", "event": "pre_check", "exit": 0,
            "consequences": consequences,
        }))],
        &Options {
            max_rule_series: 2,
            ..Default::default()
        },
    );

    assert!(out.contains(r#"rule_id="r_a""#), "{out}");
    assert!(out.contains(r#"rule_id="r_b""#), "{out}");
    for dropped in ["r_c", "r_d", "r_e"] {
        assert!(
            !out.contains(&format!(r#"rule_id="{dropped}""#)),
            "{dropped} should have been folded:\n{out}"
        );
    }
    // 3 + 2 + 1 fires from the folded rules.
    assert!(
        out.contains(&format!(
            r#"phronesis_rule_fires_total{{rule_id="{OTHER_RULE}",outcome="blocked"}} 6"#
        )),
        "expected folded total of 6:\n{out}"
    );
}

#[test]
fn file_paths_never_become_labels() {
    // Hard requirement: the log records the edited file, but a path must never
    // reach the exposition text — cardinality bomb and disclosure risk both.
    let secret = "/home/dev/example-app/private/secret_path.rs";
    let out = render(
        vec![record(serde_json::json!({
            "ts": 100, "kind": "hook", "event": "pre_check",
            "phase": "pre", "tool": "Edit", "file": secret, "exit": 1,
            "consequences": [consequence("r1", "constraint_violation")],
        }))],
        &Options::default(),
    );
    assert!(
        !out.contains(secret),
        "file path leaked into metrics output:\n{out}"
    );
    assert!(!out.contains("secret_path"), "{out}");
    // ...while the rest of the record still produced a series.
    assert!(out.contains(r#"tool="Edit""#), "{out}");
}

#[test]
fn context_records_accumulate_cost_and_omissions() {
    let out = render(
        vec![
            record(serde_json::json!({
                "ts": 100, "kind": "context", "event": "session",
                "bytes": 2048, "estimated_tokens": 512, "latency_micros": 900,
                "omitted": { "rule": 3, "activity": 1 },
            })),
            record(serde_json::json!({
                "ts": 200, "kind": "context", "event": "session",
                "bytes": 1024, "estimated_tokens": 256, "latency_micros": 100,
                "omitted": { "rule": 2 },
            })),
        ],
        &Options::default(),
    );
    assert!(
        out.contains(r#"phronesis_context_estimated_tokens_total{event="session"} 768"#),
        "{out}"
    );
    assert!(
        out.contains(r#"phronesis_context_bytes_total{event="session"} 3072"#),
        "{out}"
    );
    assert!(
        out.contains(r#"phronesis_context_omitted_total{kind="rule"} 5"#),
        "{out}"
    );
    assert!(
        out.contains(r#"phronesis_context_renders_total{event="session"} 2"#),
        "{out}"
    );
}

#[test]
fn exposition_is_well_formed() {
    let out = render(
        vec![record(serde_json::json!({
            "ts": 100, "kind": "hook", "event": "pre_check",
            "phase": "pre", "tool": "Edit", "exit": 0, "consequences": [],
        }))],
        &Options::default(),
    );
    assert!(
        out.contains("# TYPE phronesis_hook_checks counter"),
        "{out}"
    );
    assert!(out.contains("phronesis_hook_checks_total{"), "{out}");
    // Registered without the suffix; the encoder must add exactly one.
    assert!(!out.contains("_total_total"), "{out}");
    assert!(out.ends_with("# EOF\n"), "must terminate with EOF:\n{out}");
}

#[test]
fn malformed_lines_are_counted_not_fatal() {
    let read = LogRead {
        records: vec![record(serde_json::json!({
            "ts": 1, "kind": "mcp", "event": "fire_rules"
        }))],
        bytes: 4096,
        malformed: 3,
    };
    let registry = families::build(&read, &Options::default());
    let mut out = String::new();
    prometheus_client::encoding::text::encode(&mut out, &registry).unwrap();
    assert!(
        out.contains("phronesis_log_malformed_lines_total 3"),
        "{out}"
    );
    assert!(out.contains("phronesis_log_size_bytes 4096"), "{out}");
    assert!(
        out.contains(r#"phronesis_mcp_tool_calls_total{tool="fire_rules"} 1"#),
        "{out}"
    );
}

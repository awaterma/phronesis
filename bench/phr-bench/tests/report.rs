use phr_bench::record::{Arm, AuditSummary, GovernanceSummary, RunExit, RunRecord};
use phr_bench::report::{
    extract_note, load_records, merge_quality, render, render_with_extras, verified_arms,
    ReportExtras, SECTION_IDS,
};
use std::collections::BTreeMap;

fn rec(id: &str, arm: Arm, resolved: Option<bool>) -> RunRecord {
    RunRecord {
        instance_id: id.into(),
        arm,
        exit: RunExit::Completed,
        resolved,
        turns: 1,
        tokens_in: None,
        tokens_out: None,
        wall_clock_secs: 5,
        diff_bytes: 1,
        audit: None,
        governance: if arm == Arm::Treatment {
            Some(GovernanceSummary::default())
        } else {
            None
        },
    }
}

fn empty_gov() -> GovernanceSummary {
    GovernanceSummary::default()
}

fn busy_gov() -> GovernanceSummary {
    GovernanceSummary {
        blocks: BTreeMap::from([("llm-deflection-blame".to_string(), 2u32)]),
        warns: BTreeMap::from([("no-unwrap-in-src".to_string(), 1u32)]),
        fail_closed: 1,
    }
}

/// Two pairs: rust-a is discordant (control won); rust-b is concordant and
/// its treatment run errored (resolved unknown), exercising error rendering
/// and the n/r resolved cell.
fn sample() -> (phr_bench::aggregate::Aggregate, Vec<RunRecord>) {
    let mut a_c = rec("rust-a", Arm::Control, Some(true));
    a_c.turns = 3;
    a_c.wall_clock_secs = 60;
    a_c.diff_bytes = 10;
    a_c.audit = Some(AuditSummary {
        total_violations: 2,
        per_rule: BTreeMap::from([("no-unwrap-in-src".to_string(), 2u32)]),
    });

    let mut a_t = rec("rust-a", Arm::Treatment, Some(false));
    a_t.turns = 5;
    a_t.tokens_in = Some(100);
    a_t.tokens_out = Some(200);
    a_t.wall_clock_secs = 90;
    a_t.diff_bytes = 20;
    a_t.audit = Some(AuditSummary {
        total_violations: 0,
        per_rule: BTreeMap::new(),
    });
    a_t.governance = Some(busy_gov());

    let mut b_c = rec("rust-b", Arm::Control, Some(false));
    b_c.turns = 2;
    b_c.tokens_in = Some(10);
    b_c.tokens_out = Some(20);
    b_c.wall_clock_secs = 2;

    let mut b_t = rec("rust-b", Arm::Treatment, None);
    b_t.turns = 4;
    b_t.tokens_in = Some(30);
    b_t.tokens_out = Some(40);
    b_t.wall_clock_secs = 4;
    b_t.exit = RunExit::Error {
        reason: "router said see https://router.example/boom".into(),
    };

    let records = vec![a_c, a_t, b_c, b_t];
    let agg = phr_bench::aggregate::aggregate(&records).unwrap();
    (agg, records)
}

#[test]
fn all_seven_sections_present_in_order() {
    let (agg, recs) = sample();
    let html = render(&agg, &recs).unwrap();
    let positions: Vec<usize> = SECTION_IDS
        .iter()
        .map(|id| html.find(&format!("id=\"{id}\"")).expect(id))
        .collect();
    assert!(positions.windows(2).all(|w| w[0] < w[1]));
}

#[test]
fn no_external_references() {
    let (_agg, mut recs) = sample();
    for r in recs.iter_mut() {
        if matches!(r.exit, RunExit::Error { .. }) {
            r.exit = RunExit::Error {
                reason: "also mentions http://insecure.example".into(),
            };
        }
    }
    let extras = ReportExtras {
        notes: BTreeMap::from([(
            "rust-a".to_string(),
            "reviewer wrote see https://docs.example/page".to_string(),
        )]),
        ..ReportExtras::default()
    };
    let agg = phr_bench::aggregate::aggregate(&recs).unwrap();
    let html = render_with_extras(&agg, &recs, &extras).unwrap();
    assert!(!html.contains("http://"), "no http scheme in output");
    assert!(!html.contains("https://"), "no https scheme in output");
    assert!(!html.contains("<script"), "no script tags");
    assert!(!html.contains("<link"), "no link tags");
    assert!(!html.contains("src="), "no src attributes at all");
}

#[test]
fn null_tokens_render_as_nr_and_discordant_flagged() {
    let (agg, recs) = sample();
    let html = render(&agg, &recs).unwrap();
    assert!(html.contains("n/r"), "unreported tokens render as n/r");
    assert!(html.contains("discordant"), "discordant pairs are flagged");
    assert!(
        html.contains("class=\"discordant\""),
        "discordant rows carry the class"
    );
}

#[test]
fn deterministic_bytes() {
    let (agg, recs) = sample();
    let extras = ReportExtras {
        run_id: "pilot-20261002".into(),
        verified_arms: vec!["control".into(), "treatment".into()],
        notes: BTreeMap::from([("rust-a".to_string(), "same note".into())]),
    };
    let a = render_with_extras(&agg, &recs, &extras).unwrap();
    let b = render_with_extras(&agg, &recs, &extras).unwrap();
    assert_eq!(a, b, "same inputs render byte-identical HTML");
    let c = render(&agg, &recs).unwrap();
    let d = render(&agg, &recs).unwrap();
    assert_eq!(c, d);
}

#[test]
fn no_timestamps_in_output() {
    let (agg, recs) = sample();
    let html = render(&agg, &recs).unwrap();
    assert!(!html.contains("timestamp"));
    let bytes = html.as_bytes();
    for window in bytes.windows(10) {
        let date_like = window[4] == b'-'
            && window[0..4].iter().all(u8::is_ascii_digit)
            && window[5..7].iter().all(u8::is_ascii_digit)
            && window[8..10].iter().all(u8::is_ascii_digit);
        assert!(!date_like, "no YYYY-MM-DD pattern in the report");
    }
}

#[test]
fn headline_numbers_rendered() {
    let (agg, recs) = sample();
    let html = render(&agg, &recs).unwrap();
    assert!(html.contains("control 1 of 2"), "resolved rate control");
    assert!(html.contains("treatment 0 of 2"), "resolved rate treatment");
    assert!(
        html.contains("p = 1.000"),
        "sign test p rendered with 3 decimals"
    );
    assert!(
        html.contains("Debt delta (audit violations, treatment minus control): -2"),
        "debt delta from audit summaries"
    );
    assert!(html.contains("2.50"), "per-language mean turns control");
    assert!(html.contains("4.50"), "per-language mean turns treatment");
}

#[test]
fn headline_without_discordant_pairs_renders_na() {
    let records = vec![
        rec("py-a", Arm::Control, Some(true)),
        rec("py-a", Arm::Treatment, Some(true)),
    ];
    let agg = phr_bench::aggregate::aggregate(&records).unwrap();
    let html = render(&agg, &records).unwrap();
    assert!(
        html.contains("n/a (no discordant pairs)"),
        "sign test renders n/a with zero discordant pairs"
    );
}

#[test]
fn discordant_note_threaded_through_details() {
    let (agg, recs) = sample();
    let extras = ReportExtras {
        notes: BTreeMap::from([(
            "rust-a".to_string(),
            "final assistant said <b>done</b> & \"shipped\"".into(),
        )]),
        ..ReportExtras::default()
    };
    let html = render_with_extras(&agg, &recs, &extras).unwrap();
    assert!(html.contains("<details"), "note rendered inside details");
    assert!(
        html.contains("final assistant said &lt;b&gt;done&lt;/b&gt; &amp; &quot;shipped&quot;"),
        "note text is HTML-escaped"
    );
}

#[test]
fn friction_per_rule_and_fail_closed() {
    let (agg, recs) = sample();
    let html = render(&agg, &recs).unwrap();
    assert!(
        html.contains("llm-deflection-blame"),
        "per-rule row present"
    );
    assert!(
        html.contains("Fail-closed events across treatment: 1"),
        "fail-closed total rendered"
    );
}

#[test]
fn governance_recovery_rate() {
    let (agg, recs) = sample();
    let html = render(&agg, &recs).unwrap();
    assert!(
        html.contains("1 of 1"),
        "share of blocked tasks that ended completed"
    );
    assert!(html.contains("Deflection-rule blocks: 2"));
}

#[test]
fn efficiency_tables_rendered() {
    let (agg, recs) = sample();
    let html = render(&agg, &recs).unwrap();
    assert!(html.contains("Median turns"), "median turns per arm");
    assert!(html.contains("Median wall-clock (s)"), "median wall-clock");
    assert!(
        html.contains("Per-task wall-clock delta (treatment minus control)"),
        "per-task hook-overhead table"
    );
    assert!(html.contains("16.0"), "mean wall-clock delta (30+2)/2");
}

#[test]
fn caveats_carry_pilot_scope_and_token_fallback() {
    let (agg, recs) = sample();
    let html = render(&agg, &recs).unwrap();
    assert!(html.contains("Pilot-lite scope: 15 of 43 Rust instances"));
    assert!(html.contains("k=1"));
    assert!(html.contains("single model"));
    assert!(html.contains("This is a pilot report"));
    assert!(
        html.contains("1 of 4 records did not report token usage"),
        "token fallback stated from the data"
    );
}

#[test]
fn load_records_reads_both_arms_sorted() {
    let dir = tempfile::tempdir().unwrap();
    let runs = dir.path().join("runs");
    let a_records = vec![
        rec("rust-a", Arm::Control, Some(true)),
        rec("rust-a", Arm::Treatment, Some(false)),
    ];
    let b_records = vec![
        rec("rust-b", Arm::Control, Some(false)),
        rec("rust-b", Arm::Treatment, Some(false)),
    ];
    for (instance, pair) in [("rust-a", &a_records), ("rust-b", &b_records)] {
        for r in pair {
            let dir = runs.join(instance).join(r.arm.as_str());
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("record.json"), serde_json::to_vec(r).unwrap()).unwrap();
        }
    }
    let stray = runs.join("stray.txt");
    std::fs::write(&stray, b"not a record").unwrap();
    let unknown_arm = runs.join("rust-a").join("wip");
    std::fs::create_dir_all(&unknown_arm).unwrap();
    std::fs::write(unknown_arm.join("record.json"), b"{}").unwrap();

    let loaded = load_records(&runs).unwrap();
    assert_eq!(
        loaded.len(),
        4,
        "stray files and unknown arm dirs are skipped"
    );
    let sorted = loaded.windows(2).all(|w| {
        (w[0].instance_id.clone(), w[0].arm.as_str())
            < (w[1].instance_id.clone(), w[1].arm.as_str())
    });
    assert!(sorted, "records sorted by (instance, arm)");
    assert!(loaded
        .iter()
        .all(|r| { r.arm == Arm::Control || r.arm == Arm::Treatment }));
}

#[test]
fn merge_quality_fills_missing_audit_only() {
    let mut records = vec![
        rec("rust-a", Arm::Control, Some(true)),
        rec("rust-a", Arm::Treatment, Some(false)),
    ];
    let existing = AuditSummary {
        total_violations: 9,
        per_rule: BTreeMap::new(),
    };
    records[1].audit = Some(existing);
    let quality = BTreeMap::from([(
        "rust-a/control".to_string(),
        AuditSummary {
            total_violations: 7,
            per_rule: BTreeMap::from([("rule-x".to_string(), 7u32)]),
        },
    )]);
    merge_quality(&mut records, &quality);
    assert_eq!(records[0].audit.as_ref().unwrap().total_violations, 7);
    assert_eq!(records[1].audit.as_ref().unwrap().total_violations, 9);
}

#[test]
fn verified_arms_parsed_from_verify_json() {
    let arms =
        verified_arms(r#"{"verified_arms":["treatment","control"],"timestamp":123}"#).unwrap();
    assert_eq!(arms, vec!["control".to_string(), "treatment".to_string()]);
    assert!(verified_arms("not json").is_err());
}

#[test]
fn extract_note_takes_last_assistant_text() {
    let jsonl = concat!(
        "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"first message\"}]}}\n",
        "{\"type\":\"result\",\"num_turns\":2}\n",
        "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"x\"},{\"type\":\"text\",\"text\":\"final  message\\nwith newline\"}]}}\n",
    );
    assert_eq!(
        extract_note(jsonl).as_deref(),
        Some("final message with newline")
    );
    assert_eq!(extract_note("{\"type\":\"result\"}"), None);
    let long = "x".repeat(400);
    let transcript = format!(
        "{{\"type\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"{long}\"}}]}}}}\n"
    );
    let note = extract_note(&transcript).unwrap();
    assert!(note.len() <= 243, "note trimmed to a bounded length");
    assert!(note.ends_with("..."));
}

#[test]
fn governance_empty_maps_render_without_panic() {
    let records = vec![rec("py-a", Arm::Control, Some(false)), {
        let mut r = rec("py-a", Arm::Treatment, Some(false));
        r.governance = Some(empty_gov());
        r
    }];
    let agg = phr_bench::aggregate::aggregate(&records).unwrap();
    let html = render(&agg, &records).unwrap();
    assert!(html.contains("No rule friction recorded"));
    assert!(html.contains("0 of 0"));
}

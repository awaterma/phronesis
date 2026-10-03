use phr_bench::verify::{parse_harness_report, write_predictions};

#[test]
fn predictions_jsonl_is_one_line_per_instance() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("preds.jsonl");
    write_predictions(&[("i-1".into(), "diff --git a/x".into())], &out).unwrap();
    let text = std::fs::read_to_string(&out).unwrap();
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines.len(), 1);
    let v: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(v["instance_id"], "i-1");
    assert_eq!(v["model_patch"], "diff --git a/x");
}

#[test]
fn report_parser_maps_every_instance() {
    let json = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/testdata/harness-report.json"),
    )
    .unwrap();
    let m = parse_harness_report(&json).unwrap();
    assert_eq!(m.get("i-rust-1"), Some(&true));
    assert_eq!(m.get("i-rust-2"), Some(&false));
    assert_eq!(m.len(), 3);
}

#[test]
fn instance_missing_from_report_is_not_resolved() {
    let m = parse_harness_report("{}").unwrap();
    assert_eq!(
        m.get("i-rust-1"),
        None,
        "absent means not resolved, never assumed"
    );
}

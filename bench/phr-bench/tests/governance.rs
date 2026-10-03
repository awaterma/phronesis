use phr_bench::governance::{summarize, GovernanceError};

fn fixture() -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/testdata/log-with-block.jsonl"),
    )
    .unwrap()
}

#[test]
fn counts_blocks_warns_and_fail_closed() {
    let s = summarize(&fixture()).unwrap();
    assert_eq!(s.blocks.get("no-unwrap-in-src"), Some(&1));
    assert_eq!(s.warns.get("audit-file-loc-high"), Some(&1));
    assert_eq!(s.fail_closed, 1);
}

#[test]
fn empty_log_is_not_wired() {
    assert!(matches!(summarize(""), Err(GovernanceError::NotWired)));
    assert!(matches!(summarize("\n"), Err(GovernanceError::NotWired)));
}

#[test]
fn lifecycle_only_log_is_not_wired() {
    let log = "{\"ts\":1,\"kind\":\"lifecycle\",\"event\":\"session_start\",\"exit\":0}\n";
    assert!(
        matches!(summarize(log), Err(GovernanceError::NotWired)),
        "a treatment run where no hook ever fired must be invalid, not zero-friction"
    );
}

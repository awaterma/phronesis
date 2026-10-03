use phr_bench::telemetry::parse_transcript;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/testdata")
            .join(name),
    )
    .expect("transcript fixture should be readable")
}

#[test]
fn normal_transcript_counts_assistant_tools_and_result_usage() {
    let stats = parse_transcript(&fixture("transcript-normal.jsonl")).expect("parse transcript");
    assert_eq!(stats.assistant_events, 3);
    assert_eq!(stats.tool_use_events, 1);
    assert_eq!(stats.turns, 2);
    assert_eq!(stats.tokens_in, Some(300));
    assert_eq!(stats.tokens_out, Some(130));
    assert_eq!(stats.duration_ms, Some(1234));
}

#[test]
fn transcript_without_tools_is_valid() {
    let stats = parse_transcript(&fixture("transcript-no-tools.jsonl")).expect("parse transcript");
    assert_eq!(stats.tool_use_events, 0);
    assert_eq!(stats.turns, 2);
}

#[test]
fn malformed_events_are_counted_and_do_not_hide_valid_assistant_events() {
    let stats = parse_transcript(&fixture("transcript-malformed.jsonl")).expect("parse transcript");
    assert_eq!(stats.malformed_events, 2);
    assert_eq!(stats.assistant_events, 1);
}

#[test]
fn empty_transcript_is_an_error() {
    let error = parse_transcript("").expect_err("transcript without assistant events must fail");
    assert!(error.to_string().contains("transcript has no assistant events"));
}

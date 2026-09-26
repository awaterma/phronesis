//! SPEC-property-ontology.md §4: every `set_property_status` invocation lands
//! in `log.jsonl`. `PHRONESIS_NO_ACTION_LOG` silences routine logging, but it
//! must not let a promotion commit unjournaled.
//!
//! Its own test binary (one process, one test): the opt-out is process-wide
//! environment, so setting it here cannot leak into other tests.

use phronesis_mcp::properties::status::set_property_status_handler;

const ONE: &str = r#"{"version":1,"properties":[{"id":"p1","subject":"s","kind":"postcondition","depends_on":["fn:s"],"source":"explicit_spec","status":"candidate"}]}"#;

#[test]
fn the_action_log_opt_out_does_not_skip_the_transition_journal() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".phronesis")).unwrap();
    std::fs::write(root.path().join(".phronesis/properties.json"), ONE).unwrap();
    // SAFETY: this binary runs exactly one test, so no other thread reads the
    // environment concurrently.
    unsafe { std::env::set_var("PHRONESIS_NO_ACTION_LOG", "1") };

    set_property_status_handler(root.path(), "p1", "accepted", "human review")
        .expect("transition succeeds");

    let log = std::fs::read_to_string(root.path().join(".phronesis/log.jsonl"))
        .expect("the transition must be journaled even with the opt-out set");
    let entry: serde_json::Value =
        serde_json::from_str(log.lines().last().expect("one line")).unwrap();
    assert_eq!(entry["event"], "set_property_status");
    assert_eq!(entry["property"], "p1");
    assert_eq!(entry["new_status"], "accepted");
}

use phr_bench::manifest::{packs_for, Caps, TaskSpec};
use phr_bench::record::{validate, Arm, GovernanceSummary, RunExit, RunRecord};

fn record(arm: Arm, governance: Option<GovernanceSummary>) -> RunRecord {
    RunRecord {
        instance_id: "i-1".into(),
        arm,
        exit: RunExit::Completed,
        resolved: None,
        turns: 3,
        tokens_in: None,
        tokens_out: None,
        wall_clock_secs: 10,
        diff_bytes: 42,
        audit: None,
        governance,
    }
}

#[test]
fn manifest_round_trip_preserves_tasks() {
    let m = phr_bench::manifest::Manifest {
        dataset: phr_bench::manifest::DatasetRef {
            id: "d".into(),
            revision: "r1".into(),
        },
        seed: 20261001,
        prompt_hash: "h".into(),
        caps: Caps::default(),
        tasks: vec![TaskSpec {
            instance_id: "i".into(),
            language: "rust".into(),
            repo: "u".into(),
            base_commit: "c".into(),
            issue_text: "text".into(),
            fail_to_pass: vec!["t1".into()],
            pass_to_pass: vec![],
            packs: packs_for("rust"),
        }],
    };
    let json = serde_json::to_string(&m).unwrap();
    let back: phr_bench::manifest::Manifest = serde_json::from_str(&json).unwrap();
    assert_eq!(back.tasks[0].packs, vec!["llm", "rust"]);
    assert_eq!(back.caps.max_turns, 100);
}

#[test]
fn run_record_serializes_null_tokens_never_omits() {
    let json = serde_json::to_string(&record(Arm::Control, None)).unwrap();
    assert!(json.contains("\"tokens_in\":null"));
    assert!(json.contains("\"tokens_out\":null"));
}

#[test]
fn treatment_without_governance_is_loud() {
    let err = validate(&record(Arm::Treatment, None)).unwrap_err();
    assert!(err.to_string().contains("governance"));
    validate(&record(Arm::Control, None)).unwrap(); // control: fine
    validate(&record(Arm::Treatment, Some(GovernanceSummary::default()))).unwrap();
}

#[test]
fn pack_map_is_total() {
    assert_eq!(packs_for("rust"), vec!["llm", "rust"]);
    assert_eq!(packs_for("go"), vec!["llm"]);
}

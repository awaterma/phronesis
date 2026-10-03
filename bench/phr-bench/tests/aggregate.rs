use phr_bench::aggregate::{aggregate, sign_test_p};
use phr_bench::record::*;

fn rec(id: &str, arm: Arm, resolved: bool, gov: bool) -> RunRecord {
    RunRecord {
        instance_id: id.into(),
        arm,
        exit: RunExit::Completed,
        resolved: Some(resolved),
        turns: 1,
        tokens_in: None,
        tokens_out: None,
        wall_clock_secs: 1,
        diff_bytes: 1,
        audit: None,
        governance: if gov && arm == Arm::Treatment {
            Some(GovernanceSummary::default())
        } else {
            None
        },
    }
}

#[test]
fn sign_test_known_values() {
    assert!((sign_test_p(9, 1) - 0.021484375).abs() < 1e-9); // 2 * 11/1024
    assert!((sign_test_p(6, 0) - 0.03125).abs() < 1e-9);      // 2 * 1/64
    assert!((sign_test_p(2, 2) - 1.0).abs() < 1e-9);          // clamped
    assert!((sign_test_p(0, 0) - 1.0).abs() < 1e-9);         // no discordant pairs
}

#[test]
fn pairs_and_headline_roll_up() {
    let a = rec("a", Arm::Control, true, false);
    let b = rec("b", Arm::Control, false, false);
    let records = vec![
        a.clone(),
        rec("a", Arm::Treatment, false, true),   // control won
        b.clone(),
        rec("b", Arm::Treatment, true, true),    // treatment won
        rec("c", Arm::Control, false, false),
        rec("c", Arm::Treatment, false, true),
    ];
    let agg = aggregate(&records).unwrap();
    assert_eq!(agg.headline.n, 3);
    assert_eq!(agg.headline.discordant, 2);
    assert_eq!(agg.headline.control_won, 1);
    assert_eq!(agg.headline.treatment_won, 1);
    assert!(agg.pairs[0].discordant);
}

#[test]
fn treatment_missing_governance_fails_loud() {
    let records = vec![rec("x", Arm::Control, true, false), rec("x", Arm::Treatment, true, false)];
    let err = aggregate(&records).unwrap_err();
    assert!(err.to_string().contains("governance"));
}

#[test]
fn odd_arm_counts_are_rejected() {
    let records = vec![rec("x", Arm::Control, true, false)]; // no treatment twin
    assert!(aggregate(&records).is_err());
}

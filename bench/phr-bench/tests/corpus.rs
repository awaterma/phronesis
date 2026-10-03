use phr_bench::corpus::{build_manifest, Slice};
use phr_bench::manifest::DatasetRef;

fn fixture() -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/testdata/dataset-slice.jsonl"),
    )
    .expect("fixture should be readable")
}

fn dataset() -> DatasetRef {
    DatasetRef {
        id: "swe-bench/SWE-bench_Multilingual".into(),
        revision: "846e647b9f33c0b51b739d005d13d85493c9af09".into(),
    }
}

#[test]
fn pilot_is_all_rust_plus_seeded_proportional_fill() {
    let manifest = build_manifest(&fixture(), Slice::Pilot, 20261001, dataset()).expect("manifest");
    assert_eq!(manifest.tasks.len(), 30);
    assert_eq!(
        manifest
            .tasks
            .iter()
            .filter(|task| task.language == "rust")
            .count(),
        10
    );
    assert_eq!(
        manifest
            .tasks
            .iter()
            .filter(|task| task.language == "python")
            .count(),
        20
    );
}

#[test]
fn same_seed_produces_same_manifest_bytes() {
    let first = serde_json::to_string(
        &build_manifest(&fixture(), Slice::Pilot, 7, dataset()).expect("first manifest"),
    )
    .expect("first json");
    let second = serde_json::to_string(
        &build_manifest(&fixture(), Slice::Pilot, 7, dataset()).expect("second manifest"),
    )
    .expect("second json");
    assert_eq!(first, second);
}

#[test]
fn packs_and_prompt_hash_are_recorded() {
    let manifest = build_manifest(&fixture(), Slice::Pilot, 7, dataset()).expect("manifest");
    assert_eq!(
        manifest
            .tasks
            .iter()
            .find(|task| task.language == "rust")
            .expect("rust task")
            .packs,
        vec!["llm", "rust"]
    );
    assert_eq!(
        manifest
            .tasks
            .iter()
            .find(|task| task.language == "python")
            .expect("python task")
            .packs,
        vec!["llm", "python"]
    );
    assert_eq!(manifest.prompt_hash, phr_bench::prompt::template_hash());
}

#[test]
fn normalizes_real_dataset_columns_and_sorts() {
    let manifest = build_manifest(&fixture(), Slice::Pilot, 5, dataset()).expect("manifest");
    assert!(manifest
        .tasks
        .windows(2)
        .all(|pair| pair[0].instance_id <= pair[1].instance_id));
    let task = manifest
        .tasks
        .iter()
        .find(|task| task.instance_id == "r-01")
        .expect("rust instance");
    assert_eq!(task.repo, "https://github.com/rust-lang/rust.git");
    assert_eq!(task.issue_text, "rust issue 1");
    assert_eq!(task.fail_to_pass, vec!["test_failure"]);
    assert_eq!(task.pass_to_pass, vec!["test_pass"]);
}

#[test]
fn full_slice_caps_at_one_hundred_and_includes_all_rust() {
    let manifest = build_manifest(&fixture(), Slice::Full, 9, dataset()).expect("manifest");
    assert_eq!(manifest.tasks.len(), 40);
    assert_eq!(
        manifest
            .tasks
            .iter()
            .filter(|task| task.language == "rust")
            .count(),
        10
    );
}

//! D9 — verification results are bound to (property, revision, tier,
//! artifact, verifier); an unbound result never hydrates as a passing
//! `verification_result`. D8 (properties side) — a corrupt property store
//! asserts `store_corrupt(properties, <reason>)` instead of dropping every
//! property fact.
//!
//! Spec: `docs/specs/SPEC-property-ontology.md` §2 (result binding),
//! `docs/specs/SPEC-verification-artifact-generation.md` §S7/§S8.

use std::collections::HashSet;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use phronesis_mcp::properties::allowlist::{self, AllowlistEntry};
use phronesis_mcp::properties::hydrate::{
    EditedFile, PropertyFact, PropertyHydrationInput, facts_for_event,
};
use phronesis_mcp::properties::store::{
    Encoding, PROPERTIES_FORMAT, PropertiesFile, Property, PropertySource, PropertyStatus,
    properties_path, results_path,
};
use serde_json::json;
use tempfile::TempDir;

const OLD_SRC: &str = r#"pub fn safe_divide(numerator: i32, denominator: i32) -> Result<i32, &'static str> {
    if denominator == 0 {
        return Err("division by zero");
    }

    Ok(numerator / denominator)
}
"#;
const NEW_SRC: &str = r#"pub fn safe_divide(numerator: i32, denominator: i32) -> Result<i32, &'static str> {
    if denominator == 0 {
        return Err("invalid denominator");
    }

    Ok(numerator / denominator)
}
"#;

const PROP: &str = "safe_divide.zero_returns_error";
const BRANCH_REGION: &str = "branch:src/lib.rs::safe_divide:cd6054b02dde";
/// The record hand-seeded into this repository's own
/// `.phronesis/property-results.jsonl`, verbatim: a v1 `kani passed` result
/// with no tier, no artifact hash, for a property with no kani encoding.
const LIVE_REPO_RECORD: &str = r#"{"v": 1, "kind": "verification_result", "property": "safe_divide.zero_returns_error", "verifier": "kani", "status": "passed", "revision": "0ef2e37d80ee4be6d551cb9c7429a8a22720e712", "tool": "kani"}"#;

fn sha(tag: char) -> String {
    std::iter::repeat_n(tag, 64).collect()
}

fn property(encodings: Vec<Encoding>) -> Property {
    Property {
        mutations: vec![],
        id: PROP.into(),
        subject: "safe_divide".into(),
        kind: "postcondition".into(),
        condition: Some("denominator == 0".into()),
        guarantee: Some("result is Error".into()),
        depends_on: vec![BRANCH_REGION.into()],
        source: PropertySource::ExplicitSpec,
        status: PropertyStatus::Accepted,
        corroborated_by: vec![],
        encodings,
    }
}

fn encoding(verifier: &str) -> Encoding {
    Encoding {
        language: "rust".into(),
        verifier: verifier.into(),
        artifact: "verification/safe_divide_zero.rs".into(),
    }
}

fn write_properties(root: &Path, props: Vec<Property>) {
    std::fs::create_dir_all(root.join(".phronesis")).expect("mkdir");
    let file = PropertiesFile {
        version: PROPERTIES_FORMAT,
        properties: props,
    };
    std::fs::write(
        properties_path(root),
        serde_json::to_string_pretty(&file).expect("serialize"),
    )
    .expect("write properties.json");
}

fn approve(root: &Path, artifact_sha256: &str, property_id: &str) {
    allowlist::record(
        root,
        AllowlistEntry {
            principal_kind: Default::default(),
            quorum: None,
            artifact_sha256: artifact_sha256.into(),
            template_sha256: "template-hash".into(),
            property_id: property_id.into(),
            property_revision: "r1".into(),
            approver_principal: "awaterma (human)".into(),
            date: "2026-09-26".into(),
        },
    )
    .expect("record approval");
}

fn write_result_lines(root: &Path, lines: &[String]) {
    std::fs::create_dir_all(root.join(".phronesis")).expect("mkdir");
    let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(results_path(root), body).expect("write results");
}

/// A fully bound v2 record; callers override one field to unbind it.
fn bound_record(overrides: serde_json::Value) -> String {
    let mut rec = json!({
        "v": 2,
        "kind": "verification_result",
        "property": PROP,
        "verifier": "kani",
        "status": "passed",
        "revision": "a".repeat(40),
        "tool": "cargo-kani",
        "tier": "sandbox_exec",
        "artifact_sha256": sha('1'),
    });
    if let (Some(rec), Some(over)) = (rec.as_object_mut(), overrides.as_object()) {
        for (k, v) in over {
            if v.is_null() {
                rec.remove(k);
            } else {
                rec.insert(k.clone(), v.clone());
            }
        }
    }
    rec.to_string()
}

/// A project with the property (kani-encoded), its approved artifact, and
/// the given result lines.
fn project(lines: &[String]) -> TempDir {
    let d = TempDir::new().expect("tempdir");
    write_properties(d.path(), vec![property(vec![encoding("kani")])]);
    approve(d.path(), &sha('1'), PROP);
    write_result_lines(d.path(), lines);
    d
}

const ALL: &[&str] = &[
    "property",
    "property_status",
    "verification_result",
    "result_revision",
    "result_tier",
    "unbound_evidence",
    "stale_evidence",
    "property_obligation",
    "store_corrupt",
];

fn hydrate_edit(root: &Path, head: Option<&str>) -> Vec<PropertyFact> {
    let input = PropertyHydrationInput {
        root,
        rule_relations: ALL.iter().map(|s| s.to_string()).collect::<HashSet<_>>(),
        edited: vec![EditedFile {
            path: "src/lib.rs".into(),
            old: Some(OLD_SRC),
            new: NEW_SRC,
            whole_file: false,
        }],
        head_sha: head.map(str::to_string),
    };
    facts_for_event(&input).expect("hydrate")
}

fn with<'a>(facts: &'a [PropertyFact], predicate: &str) -> Vec<&'a [String]> {
    facts
        .iter()
        .filter(|f| f.predicate == predicate)
        .map(|f| f.args.as_slice())
        .collect()
}

fn assert_unbound(facts: &[PropertyFact], reason: &str) {
    assert!(
        with(facts, "verification_result").is_empty(),
        "an unbound result must never hydrate as verification_result: {facts:?}"
    );
    assert!(
        with(facts, "result_revision").is_empty(),
        "an unbound result carries no result_revision: {facts:?}"
    );
    let unbound = with(facts, "unbound_evidence");
    assert!(
        unbound.contains(&[PROP.to_string(), "kani".to_string(), reason.to_string()].as_slice()),
        "expected unbound_evidence({PROP}, kani, {reason}): {facts:?}"
    );
    assert!(
        with(facts, "property_obligation")
            .iter()
            .any(|a| a[0] == PROP),
        "unbound evidence must not suppress the proof obligation: {facts:?}"
    );
}

// ---- D9: unbound results ----

/// The live repository's hand-seeded record, even for a property that DOES
/// carry a kani encoding and even with HEAD at the record's own revision,
/// is legacy evidence: no tier, no artifact hash → unbound.
#[test]
fn d9_legacy_v1_record_hydrates_as_unbound() {
    let d = project(&[LIVE_REPO_RECORD.to_string()]);
    let facts = hydrate_edit(d.path(), Some("0ef2e37d80ee4be6d551cb9c7429a8a22720e712"));
    assert_unbound(&facts, "legacy_record");
}

/// The live repository has no properties.json at all: the record's
/// property is unknown, so it is unbound on that ground too — and it
/// still never hydrates as verified.
#[test]
fn d9_live_repo_record_without_properties_is_unbound() {
    let d = TempDir::new().unwrap();
    write_result_lines(d.path(), &[LIVE_REPO_RECORD.to_string()]);
    let facts = hydrate_edit(d.path(), None);
    assert!(with(&facts, "verification_result").is_empty(), "{facts:?}");
    assert!(
        !with(&facts, "unbound_evidence").is_empty(),
        "legacy record must surface as unbound_evidence: {facts:?}"
    );
}

/// A revisionless result was "at head" whenever HEAD was unknown (head
/// defaulted to ""), so it suppressed the first-proof obligation forever.
#[test]
fn d9_empty_revision_is_unbound_and_never_at_head() {
    let d = project(&[bound_record(json!({"revision": ""}))]);
    let facts = hydrate_edit(d.path(), None);
    assert_unbound(&facts, "missing_revision");
}

#[test]
fn d9_non_commit_revision_is_unbound() {
    let d = project(&[bound_record(json!({"revision": "r1"}))]);
    let facts = hydrate_edit(d.path(), Some("r1"));
    assert_unbound(&facts, "invalid_revision");
}

#[test]
fn d9_missing_tier_is_unbound() {
    let d = project(&[bound_record(json!({"tier": null}))]);
    let facts = hydrate_edit(d.path(), Some(&"a".repeat(40)));
    assert_unbound(&facts, "missing_tier");
}

#[test]
fn d9_missing_artifact_hash_is_unbound() {
    let d = project(&[bound_record(json!({"artifact_sha256": null}))]);
    let facts = hydrate_edit(d.path(), Some(&"a".repeat(40)));
    assert_unbound(&facts, "missing_artifact");
}

/// A kani result for a property whose only encoding is verus.
#[test]
fn d9_result_without_a_matching_encoding_is_unbound() {
    let d = project(&[bound_record(json!({}))]);
    write_properties(d.path(), vec![property(vec![encoding("verus")])]);
    let facts = hydrate_edit(d.path(), Some(&"a".repeat(40)));
    assert_unbound(&facts, "no_encoding");
}

/// The artifact hash is approved — but for a different property.
#[test]
fn d9_artifact_not_approved_for_this_property_is_unbound() {
    let d = TempDir::new().unwrap();
    write_properties(d.path(), vec![property(vec![encoding("kani")])]);
    approve(d.path(), &sha('1'), "some.other_property");
    write_result_lines(d.path(), &[bound_record(json!({}))]);
    let facts = hydrate_edit(d.path(), Some(&"a".repeat(40)));
    assert_unbound(&facts, "artifact_not_approved");
}

/// A fully bound result hydrates as verified evidence, labeled with the
/// confinement tier that ran it, and suppresses the obligation at HEAD.
#[test]
fn d9_bound_result_hydrates_with_its_tier() {
    let head = "a".repeat(40);
    let d = project(&[bound_record(json!({"tier": "raw"}))]);
    let facts = hydrate_edit(d.path(), Some(&head));
    assert!(
        with(&facts, "verification_result")
            .contains(&[PROP.to_string(), "kani".into(), "passed".into()].as_slice()),
        "{facts:?}"
    );
    assert!(
        with(&facts, "result_revision")
            .contains(&[PROP.to_string(), "kani".into(), head.clone()].as_slice()),
        "{facts:?}"
    );
    assert!(
        with(&facts, "result_tier")
            .contains(&[PROP.to_string(), "kani".into(), "raw".into()].as_slice()),
        "raw-tier results must be labeled so rules can refuse them: {facts:?}"
    );
    assert!(with(&facts, "unbound_evidence").is_empty(), "{facts:?}");
    assert!(
        with(&facts, "property_obligation").is_empty(),
        "a bound result at HEAD satisfies the first proof: {facts:?}"
    );
    // At another HEAD, with the dependent region changed, it is stale.
    let facts = hydrate_edit(d.path(), Some(&"b".repeat(40)));
    assert!(
        with(&facts, "stale_evidence").contains(&[PROP.to_string(), "kani".into()].as_slice()),
        "{facts:?}"
    );
}

/// D9 back-compat, pinned by name: `bound_record`'s baseline JSON has no
/// `template_origin` key at all — the shape of a result written before this
/// field existed. `hydrate.rs`'s `binding()` treats an absent field the same
/// as `Some("templates")` (`None | Some("templates") => {}`), so it still
/// binds as verified evidence. This is *not* a integrity guarantee — nothing
/// stops a hand-written record from omitting the field to look legacy — it
/// is `phr-mcp verify run` was never asked to change; the trust boundary is
/// the allowlist plus the executor, not this field (see
/// SPEC-verification-artifact-generation.md "Integrity limits").
#[test]
fn d9_missing_template_origin_still_binds_like_the_trusted_templates_origin() {
    let head = "a".repeat(40);
    let d = project(&[bound_record(json!({}))]);
    let facts = hydrate_edit(d.path(), Some(&head));
    assert!(
        with(&facts, "verification_result")
            .contains(&[PROP.to_string(), "kani".into(), "passed".into()].as_slice()),
        "a record with no template_origin field must still bind, matching hydrate.rs's \
         `None | Some(\"templates\") => {{}}` arm: {facts:?}"
    );
    assert!(with(&facts, "unbound_evidence").is_empty(), "{facts:?}");
}

/// With HEAD unknown, no result can be shown to be at HEAD: the obligation
/// stays (conservative), and staleness is not claimed.
#[test]
fn d9_unknown_head_never_suppresses_the_obligation() {
    let d = project(&[bound_record(json!({}))]);
    let facts = hydrate_edit(d.path(), None);
    assert!(
        with(&facts, "property_obligation")
            .iter()
            .any(|a| a[0] == PROP),
        "{facts:?}"
    );
}

// ---- D8 (properties): corrupt stores ----

fn assert_corrupt_but_conservative(facts: &[PropertyFact], reason: &str) {
    assert!(
        with(facts, "store_corrupt")
            .contains(&["properties".to_string(), reason.to_string()].as_slice()),
        "expected store_corrupt(properties, {reason}): {facts:?}"
    );
    assert!(with(facts, "verification_result").is_empty(), "{facts:?}");
    assert!(
        with(facts, "property").iter().any(|a| a[0] == PROP),
        "a corrupt results sidecar must not drop the property record: {facts:?}"
    );
    assert!(
        with(facts, "property_obligation")
            .iter()
            .any(|a| a[0] == PROP),
        "unreadable results are no evidence: the obligation stays: {facts:?}"
    );
}

#[test]
fn d8_malformed_results_line_asserts_store_corrupt() {
    let d = project(&[bound_record(json!({})), "{not json".to_string()]);
    let facts = hydrate_edit(d.path(), Some(&"a".repeat(40)));
    assert_corrupt_but_conservative(&facts, "invalid_result");
}

/// The result status set is closed (S8): `passed | failed | inconclusive |
/// timeout | unknown`. Anything else is a corrupt record, not a new state.
#[test]
fn d8_status_outside_the_closed_set_is_corrupt() {
    let d = project(&[bound_record(json!({"status": "green"}))]);
    let facts = hydrate_edit(d.path(), Some(&"a".repeat(40)));
    assert_corrupt_but_conservative(&facts, "invalid_result");
}

#[test]
fn d8_unsupported_results_format_is_corrupt() {
    let d = project(&[bound_record(json!({"v": 9}))]);
    let facts = hydrate_edit(d.path(), Some(&"a".repeat(40)));
    assert_corrupt_but_conservative(&facts, "unsupported_format");
}

#[test]
fn d8_malformed_properties_file_asserts_store_corrupt() {
    let d = project(&[]);
    std::fs::write(properties_path(d.path()), "{ nope").unwrap();
    let facts = hydrate_edit(d.path(), None);
    assert!(
        with(&facts, "store_corrupt")
            .contains(&["properties".to_string(), "invalid_properties".to_string()].as_slice()),
        "{facts:?}"
    );
}

/// Demand-gated: no rule mentions `store_corrupt` → no fact (the hook
/// still warns on stderr).
#[test]
fn d8_store_corrupt_is_demand_gated() {
    let d = project(&["{not json".to_string()]);
    let input = PropertyHydrationInput {
        root: d.path(),
        rule_relations: ["property_obligation".to_string()].into_iter().collect(),
        edited: vec![EditedFile {
            path: "src/lib.rs".into(),
            old: Some(OLD_SRC),
            new: NEW_SRC,
            whole_file: false,
        }],
        head_sha: None,
    };
    let facts = facts_for_event(&input).expect("hydrate");
    assert!(with(&facts, "store_corrupt").is_empty(), "{facts:?}");
    assert!(!with(&facts, "property_obligation").is_empty(), "{facts:?}");
}

// ---- Real binary: rules consume the facts ----

fn rules_json() -> &'static str {
    r#"{"rules":[
    {"id":"warn-first-proof","phase":"post","priority":20,"audit":true,
     "when":[{"property_obligation":["?p","first_proof"]}],
     "then":{"warn":"property ?p has no proof at HEAD"}},
    {"id":"warn-verified","phase":"post","priority":20,"audit":true,
     "when":[{"verification_result":["?p","?v","passed"]}],
     "then":{"warn":"property ?p VERIFIED by ?v"}},
    {"id":"warn-unbound","phase":"post","priority":20,"audit":true,
     "when":[{"unbound_evidence":["?p","?v","?why"]}],
     "then":{"warn":"result for ?p by ?v is unbound (?why)"}},
    {"id":"warn-raw-tier","phase":"post","priority":20,"audit":true,
     "when":[{"result_tier":["?p","?v","raw"]}],
     "then":{"warn":"proof of ?p by ?v ran unconfined"}},
    {"id":"warn-property-store-corrupt","phase":"post","priority":30,"audit":true,
     "when":[{"store_corrupt":["properties","?reason"]}],
     "then":{"warn":"property store is corrupt (?reason)"}}
]}"#
}

fn hook_project(lines: &[String]) -> TempDir {
    let d = project(lines);
    std::fs::create_dir_all(d.path().join("src")).unwrap();
    std::fs::write(d.path().join("src/lib.rs"), NEW_SRC).unwrap();
    std::fs::write(d.path().join(".phronesis/rules.json"), rules_json()).unwrap();
    d
}

fn post_check(dir: &Path) -> (i32, String) {
    let payload = format!(
        r#"{{"session_id":"s","cwd":"{}","hook_event_name":"PostToolUse","tool_name":"Edit",
            "tool_input":{{"file_path":"src/lib.rs","old_string":{},"new_string":{}}}}}"#,
        dir.display(),
        serde_json::to_string(OLD_SRC).unwrap(),
        serde_json::to_string(NEW_SRC).unwrap(),
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(dir)
        .arg("post-check")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn post-check");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(payload.as_bytes())
        .expect("write payload");
    let out = child.wait_with_output().expect("wait");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// The hand-seeded kani record never satisfies a rule that trusts
/// `verification_result(…, passed)`; the obligation still warns.
#[test]
fn hook_legacy_kani_record_is_unbound_not_verified() {
    let d = hook_project(&[LIVE_REPO_RECORD.to_string()]);
    let (code, stderr) = post_check(d.path());
    assert_eq!(code, 1, "expected warn exit: {stderr}");
    assert!(!stderr.contains("VERIFIED"), "{stderr}");
    assert!(
        stderr.contains("is unbound (legacy_record)"),
        "unbound rule must fire: {stderr}"
    );
    assert!(stderr.contains("has no proof at HEAD"), "{stderr}");
}

/// A raw-tier bound result is labeled, so a rule can refuse it.
#[test]
fn hook_raw_tier_result_is_labeled() {
    let d = hook_project(&[bound_record(json!({"tier": "raw"}))]);
    let (_code, stderr) = post_check(d.path());
    assert!(stderr.contains("ran unconfined"), "{stderr}");
}

/// One malformed results line no longer drops every property fact: the
/// corrupt-store rule fires, the obligation still warns, and stderr says so.
#[test]
fn hook_corrupt_results_surface_store_corrupt_and_keep_the_obligation() {
    let d = hook_project(&[LIVE_REPO_RECORD.to_string(), "{not json".to_string()]);
    let (code, stderr) = post_check(d.path());
    assert_eq!(code, 1, "expected warn exit: {stderr}");
    assert!(
        stderr.contains("property store is corrupt (invalid_result)"),
        "store_corrupt rule must fire: {stderr}"
    );
    assert!(stderr.contains("has no proof at HEAD"), "{stderr}");
    assert!(
        stderr.contains("WARNING") && stderr.contains("property-results.jsonl"),
        "stderr diagnostic must name the store: {stderr}"
    );
}

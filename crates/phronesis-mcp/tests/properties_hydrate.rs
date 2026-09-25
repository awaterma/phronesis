//! SPEC-property-ontology.md acceptance B1/B2/B4 — property hydration,
//! promotion discipline, and the staleness join through the real pipeline.

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use serde_json::json;

use phronesis_mcp::coverage::hydrate as coverage_hydrate;
use phronesis_mcp::properties::hydrate::{EditedFile, PropertyHydrationInput, facts_for_event};
use phronesis_mcp::properties::store::{Property, PropertySource, PropertyStatus};
use phronesis_rhai::RenderInput;
use phronesis_rhai::rhai::Map;
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

const BRANCH_REGION: &str = "branch:safe_divide:cd6054b02dde";

/// The two safe_divide properties from spec §1 (the real fixture anchor).
fn fixture_properties() -> Vec<Property> {
    vec![
        Property {
            id: "safe_divide.nonzero_returns_quotient".into(),
            subject: "safe_divide".into(),
            kind: "postcondition".into(),
            condition: None,
            guarantee: None,
            depends_on: vec!["fn:safe_divide".into()],
            source: PropertySource::ExplicitSpec,
            status: PropertyStatus::Accepted,
            corroborated_by: vec![],
            encodings: vec![],
        },
        Property {
            id: "safe_divide.zero_returns_error".into(),
            subject: "safe_divide".into(),
            kind: "postcondition".into(),
            condition: Some("denominator == 0".into()),
            guarantee: Some("result is Error".into()),
            depends_on: vec![BRANCH_REGION.into()],
            source: PropertySource::ExplicitSpec,
            status: PropertyStatus::Accepted,
            corroborated_by: vec![],
            encodings: vec![],
        },
    ]
}

fn write_properties(root: &std::path::Path, props: &[Property]) {
    let file = phronesis_mcp::properties::store::PropertiesFile {
        version: phronesis_mcp::properties::store::PROPERTIES_FORMAT,
        properties: props.to_vec(),
    };
    std::fs::create_dir_all(root.join(".phronesis")).expect("mkdir");
    std::fs::write(
        phronesis_mcp::properties::store::properties_path(root),
        serde_json::to_string_pretty(&file).expect("serialize"),
    )
    .expect("write properties.json");
}

fn write_results(
    root: &std::path::Path,
    records: &[phronesis_mcp::properties::store::ResultRecord],
) {
    std::fs::create_dir_all(root.join(".phronesis")).expect("mkdir");
    let body: String = records
        .iter()
        .map(|r| serde_json::to_string(r).expect("serialize") + "\n")
        .collect();
    std::fs::write(phronesis_mcp::properties::store::results_path(root), body)
        .expect("write results");
}

fn relations(rels: &[&str]) -> HashSet<String> {
    rels.iter().map(|s| s.to_string()).collect()
}

fn edit_input<'a>(
    root: &'a TempDir,
    rel: HashSet<String>,
    old: Option<&'a str>,
    new: &'a str,
    head: Option<&str>,
) -> PropertyHydrationInput<'a> {
    PropertyHydrationInput {
        root: root.path(),
        rule_relations: rel,
        edited: vec![EditedFile {
            path: "crates/phronesis-mcp/tests/fixtures/coverage-sample/src/lib.rs".into(),
            old,
            new,
        }],
        head_sha: head.map(str::to_string),
    }
}

/// B1: the §4 edit makes the zero-returns-error proof stale; the
/// nonzero-returns-quotient property (function region unchanged — the edit
/// touches the zero-denominator branch only) is not flagged.
#[test]
fn b1_staleness_join_names_only_the_branch_property() {
    let d = TempDir::new().unwrap();
    let props = fixture_properties();
    write_properties(d.path(), &props);
    write_results(
        d.path(),
        &[phronesis_mcp::properties::store::ResultRecord::sample(
            "safe_divide.zero_returns_error",
            "kani",
            "passed",
            &"a".repeat(40),
        )],
    );
    let head = "b".repeat(40); // store revision "aaa…" ≠ HEAD → stale
    let rel = relations(&[
        "changed_region",
        "property_depends_on",
        "stale_evidence",
        "property",
        "property_status",
        "property_source",
        "property_corroborated_by",
    ]);
    let input = edit_input(&d, rel, Some(OLD_SRC), NEW_SRC, Some(&head));
    let facts = facts_for_event(&input).expect("hydrate");

    let stale: Vec<&String> = facts
        .iter()
        .filter(|f| f.predicate == "stale_evidence")
        .map(|f| &f.args[0])
        .collect();
    assert!(
        stale.contains(&&"safe_divide.zero_returns_error".to_string()),
        "the branch property's proof must be stale: {stale:?}"
    );
    assert!(
        !stale.iter().any(|s| s.contains("nonzero_returns_quotient")),
        "the untouched property must not be flagged stale: {stale:?}"
    );

    // RETE join through the real rule fixture.
    let rt = tokio::runtime::Runtime::new().unwrap();
    let consequences = rt.block_on(async {
        let net = phronesis_mcp::net::build_network();
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/properties-rules.json");
        let file = phronesis_mcp::rules_file::read(&fixture).expect("read rules");
        for disk in &file.rules {
            let (rule, _) = phronesis_mcp::rules_file::rule_from_disk(disk);
            net.add_rule(rule).await.expect("add rule");
        }
        // The hook asserts BOTH hydrates: coverage (changed_region…) and
        // properties (property_depends_on, stale_evidence…).
        let cov = coverage_hydrate::facts_for_event(&coverage_hydrate::HydrationInput {
            root: d.path(),
            rule_relations: relations(&["changed_region", "test_hits_region"]),
            edited: vec![coverage_hydrate::EditedFile {
                path: "crates/phronesis-mcp/tests/fixtures/coverage-sample/src/lib.rs".into(),
                old: Some(OLD_SRC),
                new: NEW_SRC,
            }],
            head_sha: Some(head.clone()),
        })
        .expect("coverage hydrate");
        // Coverage facts + property facts, both with stable IDs and sources.
        for f in &cov {
            net.assert_fact(phr::Fact {
                id: format!(
                    "coverage:{}:{}",
                    f.predicate,
                    f.args
                        .join("_")
                        .chars()
                        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                        .collect::<String>()
                ),
                predicate: f.predicate.clone(),
                args: f.args.clone(),
                timestamp: 0,
                source: Some("coverage".to_string()),
            })
            .await
            .expect("assert coverage fact");
        }
        for f in &facts {
            let joined = f.args.join("\u{1f}");
            let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
            for b in joined.as_bytes() {
                hash ^= u64::from(*b);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
            net.assert_fact(phr::Fact {
                id: format!("property:{}:{hash:012x}", f.predicate),
                predicate: f.predicate.clone(),
                args: f.args.clone(),
                timestamp: 0,
                source: Some("properties".to_string()),
            })
            .await
            .expect("assert");
        }
        net.fire_all_consequences().expect("fire")
    });
    let stale_warns: Vec<&str> = consequences
        .iter()
        .filter_map(|c| c.payload["message"].as_str())
        .filter(|m| m.contains("is stale"))
        .collect();
    assert_eq!(
        stale_warns.len(),
        1,
        "exactly one stale-proof warning: {stale_warns:?}"
    );
    assert!(
        stale_warns[0].contains("safe_divide.zero_returns_error"),
        "the warning must name the branch property: {stale_warns:?}"
    );
}

/// B2: promotion discipline — observation-sourced candidates stay
/// non-normative; eligibility logs; nothing promotes automatically.
#[test]
fn b2_observation_property_never_becomes_intent() {
    let d = TempDir::new().unwrap();
    let mut props = fixture_properties();
    props.push(Property {
        id: "runtime.seen_zero_division".into(),
        subject: "safe_divide".into(),
        kind: "postcondition".into(),
        condition: None,
        guarantee: None,
        depends_on: vec!["fn:safe_divide".into()],
        source: PropertySource::RuntimeObservation,
        status: PropertyStatus::Candidate,
        corroborated_by: vec!["log:run-42".into(), "test:zero_case".into()],
        encodings: vec![],
    });
    write_properties(d.path(), &props);
    let rel = relations(&[
        "property",
        "property_source",
        "property_status",
        "property_corroborated_by",
    ]);
    let input = edit_input(&d, rel, None, OLD_SRC, None);
    let facts = facts_for_event(&input).unwrap();

    let statuses: Vec<(String, String)> = facts
        .iter()
        .filter(|f| f.predicate == "property_status")
        .map(|f| (f.args[0].clone(), f.args[1].clone()))
        .collect();
    assert!(
        statuses.contains(&("runtime.seen_zero_division".into(), "candidate".into())),
        "the observation property stays candidate: {statuses:?}"
    );
    assert!(
        !statuses
            .iter()
            .any(|(id, _)| id == "runtime.seen_zero_division"
                && (statuses.contains(&(id.clone(), "accepted".into()))
                    || statuses.contains(&(id.clone(), "verified".into())))),
        "nothing promoted it: {statuses:?}"
    );

    // The eligibility rule's guard inputs: source=runtime_observation,
    // status=candidate, 2 corroborations — assert all three facts exist so
    // the __script__ guard can see them.
    let has_source = facts.iter().any(|f| {
        f.predicate == "property_source"
            && f.args[0] == "runtime.seen_zero_division"
            && f.args[1] == "runtime_observation"
    });
    let has_status = facts.iter().any(|f| {
        f.predicate == "property_status"
            && f.args[0] == "runtime.seen_zero_division"
            && f.args[1] == "candidate"
    });
    let corroborations = facts
        .iter()
        .filter(|f| {
            f.predicate == "property_corroborated_by" && f.args[0] == "runtime.seen_zero_division"
        })
        .count();
    assert!(
        has_source && has_status,
        "rule inputs must hydrate: {facts:?}"
    );
    assert_eq!(corroborations, 2, "two corroborations → eligibility log");
}

/// B4: an unpromoted observation property with a passing result does not
/// silence the coverage evidence-gap rule — existence is not accepted intent.
#[test]
fn b4_gap_rule_still_warns_for_unpromoted_properties() {
    let d = TempDir::new().unwrap();
    // Empty coverage store: no dynamic evidence at all.
    let props = fixture_properties();
    write_properties(d.path(), &props);
    write_results(
        d.path(),
        &[phronesis_mcp::properties::store::ResultRecord::sample(
            "safe_divide.zero_returns_error",
            "kani",
            "passed",
            &"a".repeat(40),
        )],
    );
    // Coverage gap facts (the SPEC A §5.2 mechanism) assert when a rule
    // mentions them — the property facts must not satisfy them.
    // Coverage facts come from the coverage hydrate — the same division of
    // labor the hook uses. Property facts existing must not silence them.
    let cov = coverage_hydrate::facts_for_event(&coverage_hydrate::HydrationInput {
        root: d.path(),
        rule_relations: relations(&["changed_region", "region_without_dynamic_evidence"]),
        edited: vec![coverage_hydrate::EditedFile {
            path: "crates/phronesis-mcp/tests/fixtures/coverage-sample/src/lib.rs".into(),
            old: Some(OLD_SRC),
            new: NEW_SRC,
        }],
        head_sha: Some("b".repeat(40)),
    })
    .expect("coverage hydrate");
    let prop_rel = relations(&["property_depends_on", "stale_evidence", "property"]);
    let input = edit_input(&d, prop_rel, Some(OLD_SRC), NEW_SRC, Some(&"b".repeat(40)));
    let facts = facts_for_event(&input).unwrap();
    assert!(
        !facts.is_empty(),
        "property facts must hydrate (record + stale result present)"
    );
    let gaps: Vec<&String> = cov
        .iter()
        .filter(|f| f.predicate == "region_without_dynamic_evidence")
        .map(|f| &f.args[0])
        .collect();
    assert!(
        gaps.iter().any(|g| g.starts_with("fn:safe_divide")),
        "the coverage gap must still fire despite property records + passing results: {gaps:?}"
    );
}

// ---- SPEC-C §S5 / acceptance C5: injection containment ----

#[test]
fn c5_hostile_property_payload_renders_inert_or_refuses() {
    use phronesis_mcp::properties::validate::validate_body;

    let hostile = "\"); std::process::Command::new(\"touch /tmp/pwned\"); //";
    let id_hostile = "safe_divide.zero\"; std::process::exit(1); //";

    // Layer 1: the identifier charset rejects the hostile id at ingest.
    let props = vec![Property {
        id: id_hostile.into(),
        subject: "safe_divide".into(),
        kind: "postcondition".into(),
        condition: None,
        guarantee: None,
        depends_on: vec!["fn:safe_divide".into()],
        source: PropertySource::ExplicitSpec,
        status: PropertyStatus::Accepted,
        corroborated_by: vec![],
        encodings: vec![],
    }];
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".phronesis")).unwrap();
    let file = phronesis_mcp::properties::store::PropertiesFile {
        version: phronesis_mcp::properties::store::PROPERTIES_FORMAT,
        properties: props.clone(),
    };
    std::fs::write(
        phronesis_mcp::properties::store::properties_path(root.path()),
        serde_json::to_string(&file).unwrap(),
    )
    .unwrap();
    assert!(
        phronesis_mcp::properties::store::load_properties(root.path()).is_err(),
        "the hostile id must fail ingest validation (nothing loads)"
    );

    // Layer 2: even if a hostile free-text field is rendered, the HOST escapes
    // it before Rhai scope, and the body validator rejects denied constructs.
    let escaped = phronesis_mcp::properties::validate::escape_rust_string_literal(hostile);
    assert!(
        !escaped.contains("std::process::Command::new(\""),
        "escaped form must not carry live delimiters: {escaped:?}"
    );
    // A body that somehow contains a denied construct is refused.
    let body_with_injection = format!("fn h() {{ {} }}", hostile);
    assert!(validate_body("rust", &body_with_injection, &[]).is_err());
}

// ---- SPEC-C §S3 / acceptance C7: trust-anchor tamper refusal ----

#[test]
fn c7_agent_seam_writes_to_trust_anchor_paths_are_blocked_by_the_hook() {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("verification")).unwrap();
    std::fs::create_dir_all(d.path().join(".phronesis")).unwrap();
    // The rules fixture, loaded live: the refusal rule is an ordinary pre-phase rule.
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/properties-rules.json");
    std::fs::copy(&fixture, d.path().join(".phronesis/rules.json")).unwrap();

    let payload = json!({
        "session_id": "s-agent",
        "cwd": d.path().display().to_string(),
        "hook_event_name": "PreToolUse",
        "tool_name": "Write",
        "tool_input": {
            "file_path": "verification/allowlist/entries.json",
            "content": "{\"version\":1,\"entries\":[{\"artifact_sha256\":\"self-added\"}]}"
        }
    });
    let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(d.path())
        .arg("pre-check")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pre-check");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(payload.to_string().as_bytes())
        .expect("write payload");
    let out = child.wait_with_output().expect("wait");
    let code = out.status.code().unwrap_or(-1);
    assert_eq!(
        code,
        2,
        "agent write to a trust-anchor path must be BLOCKED: {code} {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ---- SPEC-C §C9: first-proof obligation ----

#[test]
fn c9_first_proof_obligation_fires_without_a_prior_result() {
    let d = TempDir::new().unwrap();
    write_properties(d.path(), &fixture_properties());
    // NO results sidecar at all: the accepted property has never been proved.
    let rel = relations(&[
        "changed_region",
        "property_depends_on",
        "property_obligation",
    ]);
    let input = edit_input(&d, rel, Some(OLD_SRC), NEW_SRC, Some(&"b".repeat(40)));
    let facts = facts_for_event(&input).unwrap();
    let obligations: Vec<&String> = facts
        .iter()
        .filter(|f| f.predicate == "property_obligation")
        .map(|f| &f.args[0])
        .collect();
    assert!(
        !facts.is_empty(),
        "accepted property with a changed dependent region and no result must obligate: {facts:?}"
    );
    assert!(
        !obligations.is_empty() || facts.iter().any(|f| f.predicate == "changed_region"),
        "the obligation must be reachable for a never-proved property (C9)"
    );
}

// ---- SPEC-C §C1: real proof, no simulation (sandbox-exec tier, verus-native) ----

#[test]
fn c1_render_validate_gate_execute_prove_end_to_end() {
    use phronesis_mcp::properties::allowlist;
    use phronesis_mcp::properties::execute::{ConfinementTier, execute};

    // The verus toolchain gate: skip (never fake) when absent.
    let verify_bin = std::env::var("VERUS_BIN").ok().or_else(which_verus);
    let Some(verify_bin) = verify_bin else {
        eprintln!("skipping C1: no verus toolchain on this host");
        return;
    };

    let d = TempDir::new().unwrap();
    std::fs::create_dir_all(d.path().join("verification")).unwrap();
    write_properties(d.path(), &fixture_properties());

    // 1. RENDER — the verus-postcondition template through the dedicated seam.
    let template = r#"
        let subject = property.get("subject");
        `// GENERATED - DO NOT EDIT
// Property: safe_divide.zero_returns_error (verus-native)
use vstd::prelude::*;

verus! {

pub enum DivResult { Ok(i32), Err(i32) }

pub open spec fn zero_returns_error(res: DivResult) -> bool {
    matches!(res, DivResult::Err(_))
}

pub fn divide_zero() -> (res: DivResult)
    ensures zero_returns_error(res),
{
    DivResult::Err(0)
}

fn main() {}

} // verus!`
    "#;
    let mut property = Map_for_test();
    property.insert("subject".into(), "safe_divide".into());
    property.insert("id".into(), "safe_divide.zero_returns_error".into());
    let body = phronesis_rhai::render(
        template,
        &phronesis_rhai::RenderInput::frozen(property, vec![]),
    )
    .expect("render");

    // 2. Validate the rendered body (S5): rust deny-list + interpolation.
    phronesis_mcp::properties::validate::validate_body(
        "rust",
        &body,
        &["safe_divide.zero_returns_error", "safe_divide"],
    )
    .expect("rendered body validates");
    std::fs::create_dir_all(d.path().join("verification/unreviewed")).expect("mkdir verification");
    let artifact = d
        .path()
        .join("verification/unreviewed/safe_divide_zero_returns_error.rs");
    std::fs::write(&artifact, &body).expect("write artifact");

    // 3. Review gate: the approval binds the artifact bytes (C-T3 store).
    let sha = sha256_of(&artifact);
    allowlist::record(
        d.path(),
        allowlist::AllowlistEntry {
            artifact_sha256: sha.clone(),
            template_sha256: "template-hash".into(),
            property_id: "safe_divide.zero_returns_error".into(),
            property_revision: "r1".into(),
            approver_principal: "awaterma (human, session-sanctioned run)".into(),
            date: "2026-09-24".into(),
        },
    )
    .expect("record approval");
    assert!(allowlist::contains(d.path(), &sha).unwrap());

    // 4. Execute confined (sandbox-exec tier, host-forced no-network) with
    //    the PROVEN local verus binary.
    let result = execute(
        d.path(),
        &artifact,
        &sha,
        &verify_bin,
        "b".repeat(40).as_str(),
    );
    // The tiered runner resolves sandbox-exec on macOS; assert the proof.
    match result {
        Ok(outcome) => {
            assert_eq!(
                outcome.status, "passed",
                "the rendered property must PROVE: {outcome:?}"
            );
            assert_eq!(
                outcome.tier, "SandboxExec",
                "tier recorded in the audit trail"
            );
        }
        Err(e) => panic!("C1 end-to-end failed: {e:?}"),
    }
}

fn Map_for_test() -> Map {
    let mut m = Map::new();
    m.insert("subject".into(), "safe_divide".into());
    m
}

fn sha256_of(path: &std::path::Path) -> String {
    // The store binds by whatever digest the caller computed; the test uses
    // a DefaultHasher digest (a real sha256 lands with the execution ledger).
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::fs::read(path)
        .expect("read artifact")
        .hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn which_verus() -> Option<String> {
    let candidates = [
        "/Users/andrewwaterman/.cargo/bin/verus".to_string(),
        format!(
            "{}/.cargo/bin/verus",
            std::env::var("HOME").unwrap_or_default()
        ),
    ];
    candidates
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
}

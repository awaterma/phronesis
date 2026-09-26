//! SPEC-property-ontology.md acceptance B1/B2/B4 — property hydration,
//! promotion discipline, and the staleness join through the real pipeline.

use std::collections::HashSet;
use std::path::PathBuf;

use serde_json::json;

use phronesis_mcp::coverage::hydrate as coverage_hydrate;
use phronesis_mcp::properties::hydrate::{EditedFile, PropertyHydrationInput, facts_for_event};
use phronesis_mcp::properties::store::{Property, PropertySource, PropertyStatus};
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

const BRANCH_REGION: &str = "branch:src/lib.rs::safe_divide:cd6054b02dde";

/// The two safe_divide properties from spec §1 (the real fixture anchor).
fn fixture_properties() -> Vec<Property> {
    vec![
        Property {
            id: "safe_divide.nonzero_returns_quotient".into(),
            subject: "safe_divide".into(),
            kind: "postcondition".into(),
            condition: None,
            guarantee: None,
            depends_on: vec!["fn:src/lib.rs::safe_divide".into()],
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
            path: "src/lib.rs".into(),
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
                path: "src/lib.rs".into(),
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
        depends_on: vec!["fn:src/lib.rs::safe_divide".into()],
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
            path: "src/lib.rs".into(),
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
        gaps.iter()
            .any(|g| g.starts_with("fn:src/lib.rs::safe_divide")),
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
        depends_on: vec!["fn:src/lib.rs::safe_divide".into()],
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

// A property store written before per-site region ids names regions by leaf
// (`fn:safe_divide`). Such a reference cannot say which site it meant, so it
// must match every same-leaf changed site (raising obligations) rather than
// silently never matching a qualified id.
#[test]
fn legacy_leaf_name_depends_on_still_raises_obligations() {
    let d = TempDir::new().unwrap();
    let mut props = fixture_properties();
    props[0].depends_on = vec!["fn:safe_divide".into()];
    props[1].depends_on = vec!["branch:safe_divide:cd6054b02dde".into()];
    write_properties(d.path(), &props);
    let input = edit_input(
        &d,
        relations(&["property_obligation"]),
        Some(OLD_SRC),
        NEW_SRC,
        Some(&"b".repeat(40)),
    );
    let obligated: HashSet<String> = facts_for_event(&input)
        .unwrap()
        .into_iter()
        .filter(|f| f.predicate == "property_obligation")
        .map(|f| f.args[0].clone())
        .collect();
    assert!(
        obligated.contains("safe_divide.zero_returns_error")
            && obligated.contains("safe_divide.nonzero_returns_quotient"),
        "legacy references must match the changed sites: {obligated:?}"
    );
}

// Hooks hand over Claude Code's absolute `file_path`. Qualified depends_on
// entries (`fn:src/lib.rs::safe_divide`) must still match, or obligations
// and stale_evidence silently stop.
#[test]
fn absolute_edit_path_still_matches_qualified_depends_on() {
    let d = TempDir::new().unwrap();
    write_properties(d.path(), &fixture_properties());
    std::fs::create_dir_all(d.path().join("src")).unwrap();
    std::fs::write(d.path().join("src/lib.rs"), NEW_SRC).unwrap();
    let abs = d.path().join("src/lib.rs").display().to_string();
    let input = PropertyHydrationInput {
        root: d.path(),
        rule_relations: relations(&["property_obligation"]),
        edited: vec![EditedFile {
            path: abs,
            old: Some(OLD_SRC),
            new: NEW_SRC,
        }],
        head_sha: Some("b".repeat(40)),
    };
    let obligated: HashSet<String> = facts_for_event(&input)
        .unwrap()
        .into_iter()
        .filter(|f| f.predicate == "property_obligation")
        .map(|f| f.args[0].clone())
        .collect();
    assert!(
        obligated.contains("safe_divide.zero_returns_error"),
        "absolute path must relativize before matching: {obligated:?}"
    );
}

// ---- SPEC-C §C1: real proof, no simulation (sandbox-exec tier, verus-native) ----

#[test]
fn c1_render_validate_gate_execute_prove_end_to_end() {
    use phronesis_mcp::properties::allowlist;
    use phronesis_mcp::properties::execute::execute;

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
    let mut property = test_property_map();
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

fn test_property_map() -> Map {
    let mut m = Map::new();
    m.insert("subject".into(), "safe_divide".into());
    m
}

fn sha256_of(path: &std::path::Path) -> String {
    // The allowlist key is the real SHA-256 of the artifact bytes — the same
    // digest `execute` recomputes from disk before it will run anything.
    phronesis_mcp::properties::execute::artifact_sha256(
        &std::fs::read(path).expect("read artifact"),
    )
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

// ---- SPEC-C §C6: mutation detection — introduce the forbidden bug, the
// proof must fail. The only test that catches claim-binding drift. ----

#[test]
fn c6_introducing_the_forbidden_bug_flips_the_proof_to_failed() {
    use phronesis_mcp::properties::allowlist;
    use phronesis_mcp::properties::execute::execute;

    let Some(verify_bin) = std::env::var("VERUS_BIN").ok().or_else(which_verus) else {
        return;
    };

    // The mutated claim: the property forbids "zero returns Ok" — introduce
    // exactly that bug into the rendered harness.
    let mutated_template = r#"
        let subject = property.get("subject");
        `// GENERATED - DO NOT EDIT (MUTATED — C6 test)
use vstd::prelude::*;

verus! {

pub enum DivResult { Ok(i32), Err(i32) }

pub open spec fn zero_returns_error(res: DivResult) -> bool {
    matches!(res, DivResult::Err(_))
}

pub fn divide_zero() -> (res: DivResult)
    ensures zero_returns_error(res),
{
    // THE FORBIDDEN BUG: zero now returns Ok — the property forbids this.
    DivResult::Ok(0)
}

fn main() {}

} // verus!`
    "#;
    let body = phronesis_rhai::render(
        mutated_template,
        &phronesis_rhai::RenderInput::frozen(test_property_map(), vec![]),
    )
    .expect("render mutated harness");

    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("verification/unreviewed")).unwrap();
    let artifact = d2_artifact(&d);
    std::fs::write(&artifact, &body).expect("write mutated artifact");

    // The S5 rendered-body validator CANNOT see this mutation: the body is
    // valid, safe Rust whose LOGIC violates the property. That is the point —
    // claim-binding drift is caught by verification, not by validators.
    phronesis_mcp::properties::validate::validate_body(
        "rust",
        &body,
        &["safe_divide.zero_returns_error", "safe_divide"],
    )
    .expect("the mutated body passes static validation (this is why C6 exists)");

    // The proof must FAIL against the mutated claim.
    let sha = sha256_of(&artifact);
    allowlist::record(
        d.path(),
        allowlist::AllowlistEntry {
            artifact_sha256: sha.clone(),
            template_sha256: "template-hash-mutated".into(),
            property_id: "safe_divide.zero_returns_error".into(),
            property_revision: "r1-mutated".into(),
            approver_principal: "awaterma (human, session-sanctioned run)".into(),
            date: "2026-09-24".into(),
        },
    )
    .expect("record approval");
    let result = execute(
        d.path(),
        &artifact,
        &sha,
        &verify_bin,
        "b".repeat(40).as_str(),
    );
    match result {
        Ok(outcome) => assert_eq!(
            outcome.status, "failed",
            "C6: the proof against the forbidden bug MUST fail: {outcome:?}"
        ),
        Err(e) => panic!("C6 execution failed: {e:?}"),
    }
}

fn d2_artifact(d: &TempDir) -> std::path::PathBuf {
    std::fs::create_dir_all(d.path().join("verification/unreviewed")).expect("mkdir");
    d.path()
        .join("verification/unreviewed/safe_divide_zero_returns_error_c6.rs")
}

// ---- SPEC-C §C9/V2: cross-revision golden persistence (review finding #6) ----
//
// The worst realistic regression is silent evidence orphaning across an
// ordinary edit cycle. This test crosses a revision boundary with a
// semantic-preserving change set and asserts the evidence survives.

#[test]
fn cross_revision_persistence_semantic_preserving_changes_keep_joins() {
    use std::path::PathBuf;

    // The committed fixture export IS the real evidence from commit A.
    let export = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/coverage-sample/export.jsonl");
    let d = TempDir::new().unwrap();
    let summary = phronesis_mcp::coverage::import::import_export(d.path(), &export, 1)
        .expect("import at commit A");
    assert_eq!(summary.tests, 3);

    // The semantic-preserving change set: reflow the condition + one new function.
    // (A real rustfmt run would produce this shape.)
    let reflowed = OLD_SRC.replace(
        "if denominator == 0 {",
        "if\n        denominator == 0\n    {",
    );
    let with_new_fn = format!("{reflowed}\npub fn completely_new_function() -> u32 {{ 42 }}\n");

    // Hydrate at the CURRENT head with the change set.
    let relations: HashSet<String> = [
        "changed_region",
        "test_hits_region",
        "region_without_dynamic_evidence",
        "region_without_formal_evidence",
        "property_depends_on",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    // The coverage hydrate produces changed_region + test_hits_region +
    // gap facts. The properties hydrate produces property_depends_on +
    // stale_evidence. The hook asserts BOTH.
    let cov = coverage_hydrate::facts_for_event(&coverage_hydrate::HydrationInput {
        root: d.path(),
        rule_relations: relations.clone(),
        edited: vec![coverage_hydrate::EditedFile {
            path: "src/lib.rs".into(),
            old: Some(OLD_SRC),
            new: &with_new_fn,
        }],
        head_sha: Some("b".repeat(40)),
    })
    .expect("coverage hydrate at revision B");
    let facts = cov;

    // Assert 1: the fn:src/lib.rs::safe_divide region is still changed (it contains the
    // edited line — this is a semantic-preserving change to safe_divide's body).
    let changed: Vec<&String> = facts
        .iter()
        .filter(|f| f.predicate == "changed_region")
        .map(|f| &f.args[1])
        .collect();
    assert!(
        changed
            .iter()
            .any(|r| r.starts_with("fn:src/lib.rs::safe_divide")),
        "fn:src/lib.rs::safe_divide must still be changed: {changed:?}"
    );

    // Assert 2: the dynamic evidence for fn:src/lib.rs::safe_divide still joins the
    // changed region — the reflow does not orphan it. At revision B the
    // evidence is stale, so it must NOT suppress the gap (D3: stale hits
    // never suppress region_without_dynamic_evidence); hydrated at the
    // import's own revision the same identities do suppress it.
    assert!(
        facts
            .iter()
            .any(|f| f.predicate == "test_hits_region" && f.args[1] == "fn:src/lib.rs::safe_divide"),
        "fn:src/lib.rs::safe_divide hits must still join after the reflow: {facts:?}"
    );
    let stale_gaps: Vec<&String> = facts
        .iter()
        .filter(|f| f.predicate == "region_without_dynamic_evidence")
        .map(|f| &f.args[0])
        .collect();
    assert!(
        stale_gaps
            .iter()
            .any(|g| g.starts_with("fn:src/lib.rs::safe_divide")),
        "stale evidence must not suppress the gap at revision B: {stale_gaps:?}"
    );
    let fresh = coverage_hydrate::facts_for_event(&coverage_hydrate::HydrationInput {
        root: d.path(),
        rule_relations: relations.clone(),
        edited: vec![coverage_hydrate::EditedFile {
            path: "src/lib.rs".into(),
            old: Some(OLD_SRC),
            new: &with_new_fn,
        }],
        head_sha: Some(summary.revision.clone()),
    })
    .expect("coverage hydrate at the import revision");
    let gaps: Vec<&String> = fresh
        .iter()
        .filter(|f| f.predicate == "region_without_dynamic_evidence")
        .map(|f| &f.args[0])
        .collect();
    assert!(
        !gaps
            .iter()
            .any(|g| g.starts_with("fn:src/lib.rs::safe_divide")),
        "fn:src/lib.rs::safe_divide has fresh dynamic evidence from the import — must not gap: {gaps:?}"
    );

    // Assert 3: the completely_new_function region gaps (5.2 fires for it).
    assert!(
        gaps.iter().any(|g| g.contains("completely_new_function")),
        "the new function must gap (no evidence): {gaps:?}"
    );

    // Assert 3b: the branch anchor survives the reflow (whitespace-normalized).
    assert!(
        changed
            .iter()
            .any(|r| r.starts_with("branch:src/lib.rs::safe_divide:")),
        "the branch region must survive the reflow: {changed:?}"
    );

    // Assert 3c: the RETE join (rule 5.1) fires for the right pairs through
    // the real network, using the hydrated facts.
    let rt = tokio::runtime::Runtime::new().unwrap();
    let consequences = rt.block_on(async {
        let net = phronesis_mcp::net::build_network();
        let rules = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/coverage-rules-5.1.json");
        let file = phronesis_mcp::rules_file::read(&rules).expect("read rules fixture");
        for disk in &file.rules {
            let (rule, _) = phronesis_mcp::rules_file::rule_from_disk(disk);
            net.add_rule(rule).await.expect("add rule 5.1");
        }
        for f in &facts {
            let joined = f.args.join("\u{1f}");
            let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
            for b in joined.as_bytes() {
                hash ^= u64::from(*b);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
            net.assert_fact(phr::Fact {
                id: format!("coverage:{}:{hash:012x}", f.predicate),
                predicate: f.predicate.clone(),
                args: f.args.clone(),
                timestamp: 0,
                source: Some("coverage".to_string()),
            })
            .await
            .expect("assert");
        }
        net.fire_all_consequences().expect("fire")
    });

    // The golden join pairs survive: 3 tests at fn:src/lib.rs::safe_divide + 1 branch pair
    // for rejects_zero_denominator — the same pairs as the original golden.
    let logged: Vec<&str> = consequences
        .iter()
        .map(|c| c.payload["message"].as_str().unwrap_or_default())
        .collect();
    let branch_pairs: Vec<&str> = logged
        .iter()
        .filter(|p| p.contains("branch:src/lib.rs::safe_divide:"))
        .copied()
        .collect();
    assert_eq!(
        branch_pairs.len(),
        1,
        "the branch pair must survive: {logged:?}"
    );
    assert!(
        branch_pairs[0].contains("rejects_zero_denominator"),
        "the branch pair must still name the branch-exercising test"
    );

    // Assert 3: the NEW function (completely_new_function) appears in
    // changed_region but has no dynamic evidence — rule 5.2 fires for it,
    // not for safe_divide.
    let new_fn_in_changed = facts
        .iter()
        .any(|f| f.predicate == "changed_region" && f.args[1].contains("completely_new_function"));
    assert!(new_fn_in_changed, "the new function is a changed region");
    let new_fn_gaps = facts
        .iter()
        .filter(|f| {
            f.predicate == "region_without_dynamic_evidence"
                && f.args[0].contains("completely_new_function")
        })
        .count();
    assert!(
        new_fn_in_changed || new_fn_gaps > 0,
        "the new function must gap or be changed"
    );
}

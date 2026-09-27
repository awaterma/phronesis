//! D10 agent-verified evidence (SPEC-verification-artifact-generation.md
//! "Agent-verified evidence"): reviewer records, the quorum rules, the host
//! checks (production reach, mutation, baseline, vacuity sentinel), the
//! `agent_quorum` principal kind, the `agent_verified` status, and hydration
//! that never conflates agent evidence with human `verified`.
//!
//! Tests that execute a verifier use the real `verus` and skip (never fake)
//! when it is absent — the C1 pattern.
#![cfg(feature = "rhai")]

use std::collections::HashSet;
use std::path::Path;

use phronesis_mcp::action_log;
use phronesis_mcp::properties::allowlist::{
    self, AllowlistEntry, PrincipalKind, QuorumChecks, QuorumEvidence, QuorumReviewer,
};
use phronesis_mcp::properties::hydrate::{PropertyFact, PropertyHydrationInput, facts_for_event};
use phronesis_mcp::properties::quorum::{self, QuorumError, ReviewInput};
use phronesis_mcp::properties::render::{self, RenderRequest};
use phronesis_mcp::properties::status::{SetPropertyStatusError, set_property_status_handler};
use phronesis_mcp::properties::store::{
    Encoding, Mutation, PROPERTIES_FORMAT, PropertiesFile, Property, PropertySource,
    PropertyStatus, ResultRecord, append_result, load_properties, properties_path,
};
use tempfile::TempDir;

const PROP: &str = "safe_divide.zero_returns_error";
const REVISION: &str = "dddddddddddddddddddddddddddddddddddddddd";
const TEMPLATE_NAME: &str = "verus-postcondition.rhai";

/// A trusted (test-local) template rendering a standalone Verus module that
/// defines `divide` and calls it from `check_zero` — production reach by
/// name. `REQUIRES` is spliced in to build the vacuous variant.
fn template(requires: &str) -> String {
    format!(
        r#"let regions = "";
for fact in facts {{
    regions += "// depends_on: " + fact.args[1] + "\n";
}}
`// property: ${{property.id}}
${{regions}}use vstd::prelude::*;

verus! {{

pub enum DivResult {{
    Ok(u32),
    Err,
}}

pub open spec fn claim_holds(n: u32, d: u32, res: DivResult) -> bool {{
    &&& (d == 0 ==> matches!(res, DivResult::Err))
    &&& (d != 0 ==> res == DivResult::Ok(n / d))
}}

pub fn divide(n: u32, d: u32) -> (res: DivResult)
{requires}    ensures
        claim_holds(n, d, res),
{{
    if d == 0 {{
        DivResult::Err
    }} else {{
        DivResult::Ok(n / d)
    }}
}}

fn check_zero(n: u32)
{requires}{{
    let r = divide(n, 0);
    assert(matches!(r, DivResult::Err));
}}

fn main() {{
}}

}} // verus!
`
"#
    )
}

/// The honest harness.
fn sound_template() -> String {
    template("")
}

/// Every proof site carries an unsatisfiable precondition: the harness
/// "proves" anything, and only the vacuity sentinel can tell.
fn vacuous_template() -> String {
    template("    requires\n        n < n,\n")
}

/// The mutation the property declares: flip the zero test. The proof must
/// fail on it.
fn killing_mutation() -> Mutation {
    Mutation {
        verifier: "verus".into(),
        find: "if d == 0 {".into(),
        replace: "if d == 1 {".into(),
    }
}

fn property(depends_on: &[&str], mutations: Vec<Mutation>) -> Property {
    Property {
        id: PROP.into(),
        subject: "safe_divide".into(),
        kind: "postcondition".into(),
        condition: None,
        guarantee: None,
        depends_on: depends_on.iter().map(|s| s.to_string()).collect(),
        source: PropertySource::ExplicitSpec,
        status: PropertyStatus::Accepted,
        corroborated_by: vec![],
        encodings: vec![Encoding {
            language: "rust".into(),
            verifier: "verus".into(),
            artifact: "verification/safe_divide.rs".into(),
        }],
        mutations,
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

/// An opted-in project with a trusted template and the rendered artifact on
/// disk. Returns the project and the artifact's SHA-256.
fn project_with(template_source: &str, prop: Property) -> (TempDir, String) {
    let d = TempDir::new().expect("tempdir");
    write_properties(d.path(), vec![prop]);
    std::fs::write(
        d.path().join(".phronesis/verification.json"),
        r#"{"raw_execution": true}"#,
    )
    .expect("opt in");
    let dir = d.path().join(render::TEMPLATES_DIR);
    std::fs::create_dir_all(&dir).expect("mkdir templates");
    std::fs::write(dir.join(TEMPLATE_NAME), template_source).expect("write template");
    let rendered = render::render_to_disk(d.path(), &request(), false).expect("render");
    let sha = rendered.artifact.artifact_sha256.clone();
    (d, sha)
}

fn sound_project() -> (TempDir, String) {
    project_with(
        &sound_template(),
        property(&["fn:src/lib.rs::divide"], vec![killing_mutation()]),
    )
}

fn request() -> RenderRequest {
    RenderRequest {
        property_id: PROP.into(),
        verifier: None,
        allow_drafts: false,
    }
}

fn review(root: &Path, sha: &str, model: &str, family: &str, verdict: &str) {
    quorum::record_review(
        root,
        &request(),
        &ReviewInput {
            artifact_sha256: sha.into(),
            reviewer_model: model.into(),
            reviewer_family: family.into(),
            verdict: verdict.into(),
            notes: "read the harness; the ensures matches the property".into(),
        },
    )
    .expect("record review");
}

/// Two approving reviewers from two families, neither the author's.
fn good_quorum(root: &Path, sha: &str) {
    review(root, sha, "model-one", "family-a", "approve");
    review(root, sha, "model-two", "family-b", "approve");
}

const AUTHOR: &str = "family-author";

fn approve(root: &Path) -> Result<quorum::QuorumApproval, QuorumError> {
    quorum::approve_quorum(root, &request(), AUTHOR, verus_command().as_deref())
}

fn verus_command() -> Option<String> {
    std::env::var("VERUS_BIN").ok().or_else(|| {
        let home = std::env::var("HOME").ok()?;
        let p = format!("{home}/.cargo/bin/verus");
        Path::new(&p).exists().then_some(p)
    })
}

fn allowlisted(root: &Path) -> Vec<AllowlistEntry> {
    allowlist::load(root).expect("allowlist").entries
}

fn refusal_stages(root: &Path) -> Vec<String> {
    let raw = std::fs::read_to_string(action_log::default_path(root)).unwrap_or_default();
    raw.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["event"] == "verify_quorum_refused")
        .map(|v| v["stage"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Assert a refusal: the expected stage, nothing allowlisted, journaled.
fn assert_refused(root: &Path, out: Result<quorum::QuorumApproval, QuorumError>, stage: &str) {
    let err = match out {
        Ok(a) => panic!("expected a {stage} refusal, got an approval: {:?}", a.entry),
        Err(e) => e,
    };
    assert_eq!(err.stage(), stage, "wrong refusal: {err}");
    assert!(
        allowlisted(root).is_empty(),
        "a refused quorum writes nothing"
    );
    assert_eq!(
        refusal_stages(root).last().map(String::as_str),
        Some(stage),
        "the refusal is journaled with its stage"
    );
}

// ---- quorum rules (no verifier needed: refused before any run) ----

#[test]
fn a_quorum_from_one_family_only_is_refused() {
    let (d, sha) = sound_project();
    review(d.path(), &sha, "model-one", "family-a", "approve");
    review(d.path(), &sha, "model-two", "Family-A", "approve");
    assert_refused(d.path(), approve(d.path()), "quorum_rules");
}

#[test]
fn a_reviewer_from_the_authors_family_is_refused() {
    let (d, sha) = sound_project();
    good_quorum(d.path(), &sha);
    review(d.path(), &sha, "model-three", AUTHOR, "approve");
    assert_refused(d.path(), approve(d.path()), "quorum_rules");
}

#[test]
fn a_single_reviewer_is_refused() {
    let (d, sha) = sound_project();
    review(d.path(), &sha, "model-one", "family-a", "approve");
    assert_refused(d.path(), approve(d.path()), "quorum_rules");
}

#[test]
fn one_reject_vetoes_and_reviews_of_other_bytes_do_not_count() {
    let (d, sha) = sound_project();
    good_quorum(d.path(), &sha);
    review(d.path(), &sha, "model-three", "family-c", "reject");
    assert_refused(d.path(), approve(d.path()), "quorum_rules");
}

#[test]
fn a_review_must_name_the_current_rendered_bytes() {
    let (d, sha) = sound_project();
    let wrong = "0".repeat(64);
    assert_ne!(wrong, sha);
    let err = quorum::record_review(
        d.path(),
        &request(),
        &ReviewInput {
            artifact_sha256: wrong,
            reviewer_model: "model-one".into(),
            reviewer_family: "family-a".into(),
            verdict: "approve".into(),
            notes: String::new(),
        },
    )
    .expect_err("a review of bytes that do not exist is refused");
    assert!(matches!(err, QuorumError::ShaMismatch { .. }), "{err}");
    assert!(quorum::load_reviews(d.path()).expect("reviews").is_empty());
}

// ---- host checks refused before any run ----

#[test]
fn no_production_reach_is_refused() {
    // The artifact calls `divide`; the property depends on `safe_divide`,
    // which it only names in a comment (the depends_on header).
    let (d, sha) = project_with(
        &sound_template(),
        property(&["fn:src/lib.rs::safe_divide"], vec![killing_mutation()]),
    );
    good_quorum(d.path(), &sha);
    assert_refused(d.path(), approve(d.path()), "reach");
}

#[test]
fn no_mutation_is_refused() {
    let (d, sha) = project_with(
        &sound_template(),
        property(&["fn:src/lib.rs::divide"], vec![]),
    );
    good_quorum(d.path(), &sha);
    assert_refused(d.path(), approve(d.path()), "mutation");
}

// ---- host checks that execute the real verifier (skipped without verus) ----

#[test]
fn a_mutant_that_still_proves_is_refused() {
    let Some(_) = verus_command() else {
        eprintln!("skipping: no verus toolchain on this host");
        return;
    };
    // A semantics-neutral mutation: the proof survives it.
    let neutral = Mutation {
        verifier: "verus".into(),
        find: "fn main() {".into(),
        replace: "fn main() { let _unused: u32 = 0;".into(),
    };
    let (d, sha) = project_with(
        &sound_template(),
        property(&["fn:src/lib.rs::divide"], vec![neutral]),
    );
    good_quorum(d.path(), &sha);
    let out = approve(d.path());
    assert!(
        matches!(&out, Err(QuorumError::MutantSurvived { status, .. }) if status == "passed"),
        "{out:?}"
    );
    assert_refused(d.path(), out, "mutation");
}

#[test]
fn a_vacuity_sentinel_that_passes_is_refused() {
    let Some(_) = verus_command() else {
        eprintln!("skipping: no verus toolchain on this host");
        return;
    };
    // Every proof site requires `n < n`. The mutant (an always-false
    // assertion in `main`, the one satisfiable site) fails and the baseline
    // proves — only the sentinel exposes the vacuous proof.
    let loud = Mutation {
        verifier: "verus".into(),
        find: "fn main() {".into(),
        replace: "fn main() { assert(1 + 1 == 3);".into(),
    };
    let (d, sha) = project_with(
        &vacuous_template(),
        property(&["fn:src/lib.rs::divide"], vec![loud]),
    );
    good_quorum(d.path(), &sha);
    let out = approve(d.path());
    assert!(
        matches!(&out, Err(QuorumError::SentinelPassed { site, status }) if site == "divide" && status == "passed"),
        "{out:?}"
    );
    assert_refused(d.path(), out, "sentinel");
}

/// The whole D10 path with the real verifier: quorum → host checks →
/// `agent_quorum` entry → `verify run` → agent-verified evidence that
/// hydrates distinctly and admits `agent_verified` but never `verified`.
#[test]
fn a_sound_quorum_is_admitted_as_agent_evidence_never_human() {
    let Some(verus) = verus_command() else {
        eprintln!("skipping: no verus toolchain on this host");
        return;
    };
    let (d, sha) = sound_project();
    good_quorum(d.path(), &sha);
    let approval = approve(d.path()).expect("a sound quorum is admitted");
    assert_eq!(approval.disposition, "recorded");
    let entries = allowlisted(d.path());
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry.principal_kind, PrincipalKind::AgentQuorum);
    assert_eq!(entry.artifact_sha256, sha);
    assert_eq!(entry.approver_principal, "agent_quorum:family-a+family-b");
    let q = entry.quorum.as_ref().expect("quorum evidence");
    assert_eq!(q.checks.reach, vec!["divide".to_string()]);
    assert_eq!(q.checks.baseline, "passed");
    assert_eq!(q.checks.mutation, "failed");
    assert_eq!(q.checks.sentinel_sites, 3, "divide, check_zero, main");

    // Re-approving the same bytes is a no-op.
    assert_eq!(
        approve(d.path()).expect("idempotent").disposition,
        "already_approved"
    );

    let record =
        render::run(d.path(), &request(), Some(&verus), Some(REVISION)).expect("verus run");
    assert_eq!(record.status, "passed");

    let facts = hydrate(d.path());
    assert_eq!(
        with(&facts, "agent_verification_result"),
        vec![[PROP.to_string(), "verus".into(), "passed".into()].as_slice()]
    );
    assert!(
        with(&facts, "verification_result").is_empty(),
        "agent evidence never hydrates as verification_result"
    );
    let err = set_property_status_handler(d.path(), PROP, "verified", "quorum approved")
        .expect_err("agent evidence cannot make a property verified");
    assert!(
        matches!(err, SetPropertyStatusError::InsufficientEvidence { .. }),
        "{err}"
    );
    set_property_status_handler(d.path(), PROP, "agent_verified", "quorum approved")
        .expect("agent evidence admits agent_verified");
    assert_eq!(
        load_properties(d.path()).expect("props")[0].status,
        PropertyStatus::AgentVerified
    );
    // The status is part of the property revision (S2), so the transition
    // itself invalidates the rendered bytes: a new render needs a new review.
    assert!(matches!(
        approve(d.path()),
        Err(QuorumError::NotRendered { .. })
    ));
}

// ---- status and hydration without a verifier: evidence written directly ----

fn quorum_entry(sha: &str, revision: &str) -> AllowlistEntry {
    AllowlistEntry {
        artifact_sha256: sha.into(),
        template_sha256: "t".repeat(64),
        property_id: PROP.into(),
        property_revision: revision.into(),
        approver_principal: "agent_quorum:family-a+family-b".into(),
        date: "2026-09-27".into(),
        principal_kind: PrincipalKind::AgentQuorum,
        quorum: Some(QuorumEvidence {
            author_family: AUTHOR.into(),
            reviewers: vec![
                QuorumReviewer {
                    model: "model-one".into(),
                    family: "family-a".into(),
                    verdict: "approve".into(),
                    record_sha256: "1".repeat(64),
                },
                QuorumReviewer {
                    model: "model-two".into(),
                    family: "family-b".into(),
                    verdict: "approve".into(),
                    record_sha256: "2".repeat(64),
                },
            ],
            checks: QuorumChecks {
                reach: vec!["divide".into()],
                mutation: "failed".into(),
                baseline: "passed".into(),
                sentinel_sites: 3,
                tier: "sandbox_exec".into(),
            },
        }),
    }
}

fn human_entry(sha: &str) -> AllowlistEntry {
    AllowlistEntry {
        artifact_sha256: sha.into(),
        template_sha256: "t".repeat(64),
        property_id: PROP.into(),
        property_revision: "r".into(),
        approver_principal: "a human".into(),
        date: "2026-09-27".into(),
        principal_kind: PrincipalKind::Human,
        quorum: None,
    }
}

/// A project whose sidecar holds one bound passed result for `sha`.
fn evidence_project(entry: AllowlistEntry) -> TempDir {
    let d = TempDir::new().expect("tempdir");
    write_properties(
        d.path(),
        vec![property(
            &["fn:src/lib.rs::divide"],
            vec![killing_mutation()],
        )],
    );
    let sha = entry.artifact_sha256.clone();
    allowlist::record(d.path(), entry).expect("record approval");
    append_result(
        d.path(),
        &ResultRecord::sample(PROP, "verus", "passed", REVISION, &sha),
    )
    .expect("append result");
    d
}

fn hydrate(root: &Path) -> Vec<PropertyFact> {
    let relations = [
        "verification_result",
        "agent_verification_result",
        "result_principal",
        "unbound_evidence",
        "property_obligation",
    ];
    facts_for_event(&PropertyHydrationInput {
        root,
        rule_relations: relations
            .iter()
            .map(|s| s.to_string())
            .collect::<HashSet<_>>(),
        edited: vec![],
        head_sha: Some(REVISION.into()),
    })
    .expect("hydrate")
}

fn with<'a>(facts: &'a [PropertyFact], predicate: &str) -> Vec<&'a [String]> {
    facts
        .iter()
        .filter(|f| f.predicate == predicate)
        .map(|f| f.args.as_slice())
        .collect()
}

#[test]
fn agent_quorum_evidence_cannot_set_verified_but_admits_agent_verified() {
    let d = evidence_project(quorum_entry(&"a".repeat(64), "r"));
    let err = set_property_status_handler(d.path(), PROP, "verified", "a quorum approved it")
        .expect_err("agent-quorum evidence must not reach verified");
    assert!(
        matches!(err, SetPropertyStatusError::InsufficientEvidence { .. }),
        "{err}"
    );
    assert_eq!(
        load_properties(d.path()).expect("props")[0].status,
        PropertyStatus::Accepted,
        "a refused transition changes nothing"
    );
    set_property_status_handler(d.path(), PROP, "agent_verified", "a quorum approved it")
        .expect("agent_verified accepts agent-quorum evidence");
}

#[test]
fn verified_and_agent_verified_both_need_bound_passed_evidence() {
    // No evidence at all: both targets refused; other targets unchecked.
    let d = TempDir::new().expect("tempdir");
    write_properties(d.path(), vec![property(&["fn:src/lib.rs::divide"], vec![])]);
    for target in ["verified", "agent_verified"] {
        assert!(
            matches!(
                set_property_status_handler(d.path(), PROP, target, "no evidence"),
                Err(SetPropertyStatusError::InsufficientEvidence { .. })
            ),
            "{target} without evidence must be refused"
        );
    }
    set_property_status_handler(d.path(), PROP, "corroborated", "unchecked target")
        .expect("other targets stay unchecked");
    // Human evidence admits both.
    let h = evidence_project(human_entry(&"b".repeat(64)));
    set_property_status_handler(h.path(), PROP, "agent_verified", "human approved")
        .expect("human evidence admits agent_verified");
    set_property_status_handler(h.path(), PROP, "verified", "human approved")
        .expect("human evidence admits verified");
}

#[test]
fn an_agent_verified_result_hydrates_distinctly_from_human_verified() {
    let agent = evidence_project(quorum_entry(&"a".repeat(64), "r"));
    let facts = hydrate(agent.path());
    assert_eq!(
        with(&facts, "agent_verification_result"),
        vec![[PROP.to_string(), "verus".into(), "passed".into()].as_slice()]
    );
    assert!(
        with(&facts, "verification_result").is_empty(),
        "agent-quorum evidence is never verification_result: {facts:?}"
    );
    assert_eq!(
        with(&facts, "result_principal"),
        vec![[PROP.to_string(), "verus".into(), "agent_quorum".into()].as_slice()]
    );

    let human = evidence_project(human_entry(&"b".repeat(64)));
    let facts = hydrate(human.path());
    assert_eq!(
        with(&facts, "verification_result"),
        vec![[PROP.to_string(), "verus".into(), "passed".into()].as_slice()]
    );
    assert!(with(&facts, "agent_verification_result").is_empty());
    assert_eq!(
        with(&facts, "result_principal"),
        vec![[PROP.to_string(), "verus".into(), "human".into()].as_slice()]
    );
}

#[test]
fn only_human_evidence_discharges_the_first_proof_obligation() {
    let edited = |root: &Path| {
        facts_for_event(&PropertyHydrationInput {
            root,
            rule_relations: ["property_obligation".to_string()].into_iter().collect(),
            edited: vec![phronesis_mcp::properties::EditedFile {
                path: "src/lib.rs".into(),
                old: Some(""),
                new: "",
                whole_file: true,
            }],
            head_sha: Some(REVISION.into()),
        })
        .expect("hydrate")
    };
    let agent = evidence_project(quorum_entry(&"a".repeat(64), "r"));
    assert_eq!(
        with(&edited(agent.path()), "property_obligation").len(),
        1,
        "agent-quorum evidence at HEAD leaves the obligation open"
    );
    let human = evidence_project(human_entry(&"b".repeat(64)));
    assert!(
        with(&edited(human.path()), "property_obligation").is_empty(),
        "human evidence at HEAD discharges it"
    );
}

#[test]
fn verify_approve_without_quorum_refuses_to_write_a_human_entry() {
    use phronesis_mcp::properties::verify_cli::{VerifyCmd, run};
    let (d, _) = sound_project();
    let err = run(
        d.path(),
        VerifyCmd::Approve {
            property_id: PROP.into(),
            verifier: None,
            quorum: false,
            author_family: Some(AUTHOR.into()),
            verifier_command: None,
            json: false,
        },
    )
    .expect_err("no --quorum, no approval");
    assert!(err.to_string().contains("--quorum"), "{err}");
    assert!(allowlisted(d.path()).is_empty());
}

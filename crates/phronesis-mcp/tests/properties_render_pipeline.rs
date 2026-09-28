//! The production render pipeline (SPEC-verification-artifact-generation.md):
//! template lookup (trusted `verification/templates/` vs. dev-only
//! `verification/template-drafts/`), render → S5 `validate_body` → write
//! `verification/unreviewed/`, then — in a later invocation — allowlist gate
//! → confined executor → a bound result record carrying its template origin.
//!
//! The Rhai render entry itself (scope freeze, eval disabled, determinism of
//! `RenderInput::frozen`) is tested in `crates/phronesis-rhai/tests/
//! evaluator.rs`; these tests cover the host caller.
#![cfg(feature = "rhai")]

use std::collections::HashSet;
use std::path::Path;

use phronesis_mcp::action_log;
use phronesis_mcp::properties::allowlist::{self, AllowlistEntry};
use phronesis_mcp::properties::hydrate::{PropertyFact, PropertyHydrationInput, facts_for_event};
use phronesis_mcp::properties::render::{
    self, RenderPipelineError, RenderRequest, TemplateOrigin, UNREVIEWED_DIR,
};
use phronesis_mcp::properties::store::{
    Encoding, PROPERTIES_FORMAT, PropertiesFile, Property, PropertySource, PropertyStatus,
    load_results, properties_path, results_path,
};
use tempfile::TempDir;

const PROP: &str = "safe_divide.zero_returns_error";
/// The shipped draft: the seam example this pipeline must render end to end.
const DRAFT_TEMPLATE: &str =
    include_str!("../../../verification/template-drafts/verus-postcondition.rhai");
const TEMPLATE_NAME: &str = "verus-postcondition.rhai";

fn property(status: PropertyStatus, depends_on: &[&str]) -> Property {
    Property {
        mutations: vec![],
        id: PROP.into(),
        subject: "safe_divide".into(),
        kind: "postcondition".into(),
        condition: Some("denominator == 0".into()),
        guarantee: Some("result is Error".into()),
        depends_on: depends_on.iter().map(|s| s.to_string()).collect(),
        source: PropertySource::ExplicitSpec,
        status,
        corroborated_by: vec![],
        encodings: vec![Encoding {
            language: "rust".into(),
            verifier: "verus".into(),
            artifact: "verification/safe_divide.rs".into(),
        }],
    }
}

fn other_property() -> Property {
    Property {
        mutations: vec![],
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

/// An opted-in project (S1) with the accepted property. `raw_execution`
/// lets a host without sandbox-exec or a container runtime still run the
/// fake verifier; the executor prefers the strongest tier available.
fn project() -> TempDir {
    let d = TempDir::new().expect("tempdir");
    write_properties(
        d.path(),
        vec![property(
            PropertyStatus::Accepted,
            &["branch:safe_divide:cd6054b02dde", "fn:safe_divide"],
        )],
    );
    std::fs::write(
        d.path().join(".phronesis/verification.json"),
        r#"{"raw_execution": true}"#,
    )
    .expect("opt in");
    d
}

fn put_template(root: &Path, dir: &str, source: &str) {
    let dir = root.join(dir);
    std::fs::create_dir_all(&dir).expect("mkdir templates");
    std::fs::write(dir.join(TEMPLATE_NAME), source).expect("write template");
}

fn trusted(root: &Path, source: &str) {
    put_template(root, render::TEMPLATES_DIR, source);
}

fn draft(root: &Path, source: &str) {
    put_template(root, render::TEMPLATE_DRAFTS_DIR, source);
}

fn request(allow_drafts: bool) -> RenderRequest {
    RenderRequest {
        property_id: PROP.into(),
        verifier: None,
        allow_drafts,
    }
}

fn unreviewed_files(root: &Path) -> Vec<String> {
    std::fs::read_dir(root.join(UNREVIEWED_DIR))
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

fn journal_events(root: &Path) -> Vec<(String, Option<String>)> {
    let raw = std::fs::read_to_string(action_log::default_path(root)).unwrap_or_default();
    raw.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .map(|v| {
            (
                v["event"].as_str().unwrap_or_default().to_string(),
                v["reason"].as_str().map(str::to_string),
            )
        })
        .collect()
}

/// `(event, outcome)` pairs, for the `verify_render` entries an unchanged
/// re-render must also produce (unlike `reason`, only refusals carry).
fn journal_outcomes(root: &Path) -> Vec<(String, Option<String>)> {
    let raw = std::fs::read_to_string(action_log::default_path(root)).unwrap_or_default();
    raw.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .map(|v| {
            (
                v["event"].as_str().unwrap_or_default().to_string(),
                v["outcome"].as_str().map(str::to_string),
            )
        })
        .collect()
}

/// The human review act the tests stand in for (S3): approve the bytes.
fn approve(root: &Path, artifact_sha256: &str, template_sha256: &str, revision: &str) {
    allowlist::record(
        root,
        AllowlistEntry {
            principal_kind: Default::default(),
            quorum: None,
            artifact_sha256: artifact_sha256.into(),
            template_sha256: template_sha256.into(),
            property_id: PROP.into(),
            property_revision: revision.into(),
            approver_principal: "test (human stand-in)".into(),
            date: "2026-09-27".into(),
        },
    )
    .expect("record approval");
}

/// A stand-in verifier printing a passing verus summary: it proves nothing,
/// it exercises the orchestration (the real proof is `c1_*` below).
fn fake_verifier(root: &Path) -> String {
    let path = root.join("fake-verus.sh");
    std::fs::write(
        &path,
        "#!/bin/sh\necho 'verification results:: 1 verified, 0 errors'\n",
    )
    .expect("write fake verifier");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    path.display().to_string()
}

const REVISION: &str = "cccccccccccccccccccccccccccccccccccccccc";

fn hydrate(root: &Path) -> Vec<PropertyFact> {
    let relations = ["verification_result", "unbound_evidence", "result_tier"];
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

/// render → approve → run → hydrate, returning the recorded result.
fn render_approve_run(root: &Path, allow_drafts: bool) -> phronesis_mcp::properties::ResultRecord {
    let rendered = render::render_to_disk(root, &request(allow_drafts), false).expect("render");
    assert_eq!(rendered.disposition, "written");
    let a = &rendered.artifact;
    approve(
        root,
        &a.artifact_sha256,
        &a.template_sha256,
        &a.property_revision,
    );
    let verifier = fake_verifier(root);
    render::run(
        root,
        &request(allow_drafts),
        Some(&verifier),
        Some(REVISION),
    )
    .expect("run")
}

// ---- the happy path from a trusted template ----

#[test]
fn trusted_template_renders_validates_and_writes_into_unreviewed() {
    let d = project();
    trusted(d.path(), DRAFT_TEMPLATE);

    let out = render::render_to_disk(d.path(), &request(false), false).expect("render");
    let a = &out.artifact;
    assert_eq!(out.disposition, "written");
    assert_eq!(a.template_origin, TemplateOrigin::Templates);
    assert_eq!(
        a.template_path,
        format!("verification/templates/{TEMPLATE_NAME}")
    );
    assert!(a.artifact_path.starts_with("verification/unreviewed/"));

    let on_disk = std::fs::read_to_string(d.path().join(&a.artifact_path)).expect("artifact");
    assert_eq!(on_disk, a.body);
    assert!(
        on_disk.starts_with("// GENERATED — DO NOT EDIT"),
        "{on_disk}"
    );
    assert!(on_disk.contains(&format!("revision: {}", a.property_revision)));
    assert!(on_disk.contains("pub fn divide(n: u32, d: u32)"));
    assert!(
        on_disk.contains("// condition: denominator == 0"),
        "free text lands in a comment: {on_disk}"
    );
    assert!(
        on_disk.contains(
            "\n// depends_on: branch:safe_divide:cd6054b02dde\n// depends_on: fn:safe_divide\nuse vstd::prelude::*;\n"
        ),
        "one sorted comment line per dependency, and the `use` stays live code: {on_disk}"
    );
    assert!(
        journal_events(d.path()).contains(&("verify_render".into(), None)),
        "every generation is journaled (S7)"
    );

    // Re-rendering identical bytes is a no-op, not a second file.
    let again = render::render_to_disk(d.path(), &request(false), false).expect("re-render");
    assert_eq!(again.disposition, "unchanged");
    assert_eq!(unreviewed_files(d.path()).len(), 1);
}

/// An `unchanged` re-render (identical bytes already on disk) still writes
/// a `verify_render` journal entry — S7 covers every generation, not only
/// the first one that actually wrote bytes.
#[test]
fn unchanged_rerender_is_journaled() {
    let d = project();
    trusted(d.path(), DRAFT_TEMPLATE);

    render::render_to_disk(d.path(), &request(false), false).expect("first render");
    let again = render::render_to_disk(d.path(), &request(false), false).expect("second render");
    assert_eq!(again.disposition, "unchanged");

    let verify_renders: Vec<_> = journal_events(d.path())
        .into_iter()
        .filter(|(event, _)| event == "verify_render")
        .collect();
    assert_eq!(
        verify_renders.len(),
        2,
        "the unchanged re-render must be journaled too, not just the write: {verify_renders:?}"
    );
    assert!(
        journal_outcomes(d.path()).contains(&("verify_render".into(), Some("unchanged".into()))),
        "{:?}",
        journal_outcomes(d.path())
    );
}

// ---- determinism: same inputs → byte-identical output, across runs and
// across input orderings ----

#[test]
fn render_is_byte_identical_across_runs_and_input_orderings() {
    let d = project();
    trusted(d.path(), DRAFT_TEMPLATE);
    let first = render::prepare(d.path(), &request(false)).expect("first");
    let second = render::prepare(d.path(), &request(false)).expect("second");
    assert_eq!(first.body, second.body, "same inputs, same bytes");

    // Same property, depends_on and the store's property order reversed.
    write_properties(
        d.path(),
        vec![
            other_property(),
            property(
                PropertyStatus::Accepted,
                &["fn:safe_divide", "branch:safe_divide:cd6054b02dde"],
            ),
        ],
    );
    let reordered = render::prepare(d.path(), &request(false)).expect("reordered");
    assert_eq!(
        first.body, reordered.body,
        "input ordering must not change the rendered bytes"
    );
    assert_eq!(first.artifact_sha256, reordered.artifact_sha256);
    assert_eq!(first.artifact_path, reordered.artifact_path);
    assert_eq!(first.property_revision, reordered.property_revision);

    // A template that reads the record's own `depends_on` array (not only
    // the pre-sorted `facts`) is ordering-independent too.
    let array_template = "`// deps: ${property.depends_on}\nfn main() {}\n`";
    let d2 = project();
    trusted(d2.path(), array_template);
    let forward = render::prepare(d2.path(), &request(false)).expect("forward");
    write_properties(
        d2.path(),
        vec![property(
            PropertyStatus::Accepted,
            &["fn:safe_divide", "branch:safe_divide:cd6054b02dde"],
        )],
    );
    let backward = render::prepare(d2.path(), &request(false)).expect("backward");
    assert_eq!(
        forward.body, backward.body,
        "property.depends_on reaches the script sorted"
    );

    // A real edit is a new revision and new bytes.
    let mut edited = property(PropertyStatus::Accepted, &["fn:safe_divide"]);
    edited.guarantee = Some("result is Err".into());
    write_properties(d.path(), vec![edited]);
    let changed = render::prepare(d.path(), &request(false)).expect("edited");
    assert_ne!(first.property_revision, changed.property_revision);
    assert_ne!(first.artifact_sha256, changed.artifact_sha256);
}

// ---- S5: a body validate_body rejects is never written or executed ----

#[test]
fn a_body_validate_body_rejects_is_never_written_or_executed() {
    for (label, template) in [
        (
            "denied construct",
            "`fn main() { std::process::Command::new(\"touch\"); }`",
        ),
        (
            // `subject` is an identifier candidate now (it may be called in
            // live code — see `identifier_value_called_...` in validate.rs),
            // but a free-text field never is: it must stay inert wherever it
            // sits, live-code included.
            "free-text value in live code",
            "`fn main() { let _ = 1 + ${property.condition}; }`",
        ),
        ("unparseable body", "`fn main( {`"),
    ] {
        let d = project();
        trusted(d.path(), template);

        let err = render::render_to_disk(d.path(), &request(false), false)
            .expect_err(&format!("{label}: must be refused"));
        assert!(
            matches!(err, RenderPipelineError::Validation(_)),
            "{label}: {err:?}"
        );
        assert!(
            unreviewed_files(d.path()).is_empty(),
            "{label}: a refused body is never written"
        );
        assert!(
            journal_events(d.path()).contains(&(
                "verify_render_refused".into(),
                Some("validation_refused".into())
            )),
            "{label}: the refusal is journaled"
        );

        let err = render::run(d.path(), &request(false), Some("/bin/echo"), Some(REVISION))
            .expect_err(&format!("{label}: run must refuse"));
        assert!(
            matches!(err, RenderPipelineError::Validation(_)),
            "{label}: {err:?}"
        );
        assert!(
            !results_path(d.path()).exists(),
            "{label}: nothing executed, nothing recorded"
        );
    }
}

// ---- the render scope, seen from the pipeline: no emit_fact, no eval, no
// mutation of the read-only record ----

#[test]
fn render_scope_capabilities_are_absent_from_the_pipeline() {
    for probe in [
        "emit_fact(\"verification_result\", []); `fn main() {}`",
        "eval(\"`fn main() {}`\")",
        "property.status = \"verified\"; `fn main() {}`",
        "property.id = \"forged\"; `fn main() {}`",
        "facts.push(#{}); `fn main() {}`",
        "42",
    ] {
        let d = project();
        trusted(d.path(), probe);
        let err = render::render_to_disk(d.path(), &request(false), false)
            .expect_err(&format!("{probe}: must fail"));
        assert!(
            matches!(err, RenderPipelineError::Script { .. }),
            "{probe}: {err:?}"
        );
        assert!(unreviewed_files(d.path()).is_empty(), "{probe}");
    }
}

// ---- template lookup: drafts are dev-only ----

#[test]
fn a_draft_template_is_refused_without_the_flag() {
    let d = project();
    draft(d.path(), DRAFT_TEMPLATE);

    let err = render::render_to_disk(d.path(), &request(false), false)
        .expect_err("a draft without --allow-drafts is refused");
    match &err {
        RenderPipelineError::DraftRefused { path } => {
            assert_eq!(
                path,
                &format!("verification/template-drafts/{TEMPLATE_NAME}")
            )
        }
        other => panic!("expected DraftRefused, got {other:?}"),
    }
    assert!(err.to_string().contains("--allow-drafts"), "{err}");
    assert!(unreviewed_files(d.path()).is_empty());
    assert!(
        journal_events(d.path())
            .contains(&("verify_render_refused".into(), Some("draft_refused".into())))
    );

    let err = render::run(d.path(), &request(false), Some("/bin/echo"), Some(REVISION))
        .expect_err("run refuses the draft too");
    assert!(
        matches!(err, RenderPipelineError::DraftRefused { .. }),
        "{err:?}"
    );

    let out = render::render_to_disk(d.path(), &request(true), false).expect("with the flag");
    assert_eq!(out.artifact.template_origin, TemplateOrigin::TemplateDrafts);
    assert!(
        out.artifact.body.contains("origin: template_drafts"),
        "the provenance header names the draft origin"
    );
}

// `verification/templates` itself (the trust anchor's directory entry, not
// one file inside it) replaced by a symlink to `verification/template-drafts`
// must never be treated as the trust anchor: the existing per-file
// containment check alone would follow it, since the canonicalized directory
// and the canonicalized file both resolve into the same real (drafts) tree.
#[test]
fn templates_dir_itself_symlinked_to_drafts_is_refused_without_allow_drafts() {
    let d = project();
    let drafts_dir = d.path().join(render::TEMPLATE_DRAFTS_DIR);
    std::fs::create_dir_all(&drafts_dir).expect("mkdir drafts");
    std::fs::write(drafts_dir.join(TEMPLATE_NAME), DRAFT_TEMPLATE).expect("write draft");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&drafts_dir, d.path().join(render::TEMPLATES_DIR))
        .expect("symlink verification/templates -> verification/template-drafts");

    let err = render::render_to_disk(d.path(), &request(false), false)
        .expect_err("a templates/ symlink to drafts must not count as the trust anchor");
    match &err {
        RenderPipelineError::DraftRefused { path } => {
            assert_eq!(
                path,
                &format!("verification/template-drafts/{TEMPLATE_NAME}")
            )
        }
        other => panic!("expected DraftRefused, got {other:?}"),
    }
    assert!(unreviewed_files(d.path()).is_empty());

    // With the flag, the same bytes load — but honestly, as a draft, never
    // mislabeled as the trust anchor.
    let out = render::render_to_disk(d.path(), &request(true), false).expect("with the flag");
    assert_eq!(
        out.artifact.template_origin,
        TemplateOrigin::TemplateDrafts,
        "a templates/ symlink to drafts must never be recorded as the trust anchor"
    );
}

#[test]
fn a_trusted_template_wins_over_a_draft_even_with_the_flag() {
    let d = project();
    draft(d.path(), "`fn main() {}`");
    trusted(d.path(), DRAFT_TEMPLATE);
    let out = render::prepare(d.path(), &request(true)).expect("render");
    assert_eq!(out.template_origin, TemplateOrigin::Templates);
    assert!(out.body.contains("pub fn divide"));
}

// ---- D9 + drafts: a draft-derived result is recorded as such and never
// counts as verified evidence ----

#[test]
fn a_draft_derived_result_never_hydrates_as_verified() {
    let d = project();
    draft(d.path(), DRAFT_TEMPLATE);
    let record = render_approve_run(d.path(), true);
    assert_eq!(record.status, "passed");
    assert_eq!(record.template_origin.as_deref(), Some("template_drafts"));

    let stored = load_results(d.path()).expect("results load");
    assert_eq!(stored.len(), 1);
    assert_eq!(
        stored[0].template_origin.as_deref(),
        Some("template_drafts")
    );

    let facts = hydrate(d.path());
    assert!(
        with(&facts, "verification_result").is_empty(),
        "a draft-derived pass must never hydrate as verification_result: {facts:?}"
    );
    assert!(
        with(&facts, "unbound_evidence")
            .contains(&[PROP.to_string(), "verus".into(), "draft_template".into()].as_slice()),
        "{facts:?}"
    );
}

#[test]
fn a_trusted_template_result_is_bound_evidence() {
    let d = project();
    trusted(d.path(), DRAFT_TEMPLATE);
    let record = render_approve_run(d.path(), false);
    assert_eq!(record.template_origin.as_deref(), Some("templates"));
    assert_eq!(record.revision, REVISION);
    assert!(record.tier.is_some());

    let facts = hydrate(d.path());
    assert_eq!(
        with(&facts, "verification_result"),
        vec![[PROP.to_string(), "verus".into(), "passed".into()].as_slice()],
        "{facts:?}"
    );
    assert!(
        journal_events(d.path()).contains(&("verify_run".into(), None)),
        "every execution is journaled (S7)"
    );
}

// ---- S3: never executed in the invocation that wrote it; approval binds
// bytes ----

#[test]
fn run_refuses_an_artifact_that_was_not_rendered_earlier() {
    let d = project();
    trusted(d.path(), DRAFT_TEMPLATE);
    // Approve the exact bytes a render would produce, without writing them.
    let a = render::prepare(d.path(), &request(false)).expect("prepare");
    approve(
        d.path(),
        &a.artifact_sha256,
        &a.template_sha256,
        &a.property_revision,
    );

    let err = render::run(d.path(), &request(false), Some("/bin/echo"), Some(REVISION))
        .expect_err("run never writes the artifact it executes");
    assert!(
        matches!(err, RenderPipelineError::NotRendered { .. }),
        "{err:?}"
    );
    assert!(unreviewed_files(d.path()).is_empty(), "run wrote nothing");
    assert!(!results_path(d.path()).exists());
}

#[test]
fn run_refuses_an_unapproved_artifact() {
    let d = project();
    trusted(d.path(), DRAFT_TEMPLATE);
    render::render_to_disk(d.path(), &request(false), false).expect("render");
    let err = render::run(d.path(), &request(false), Some("/bin/echo"), Some(REVISION))
        .expect_err("no allowlist approval, no execution");
    assert!(
        matches!(
            err,
            RenderPipelineError::Execution(
                phronesis_mcp::properties::execute::ExecutionError::NotApproved { .. }
            )
        ),
        "{err:?}"
    );
    assert!(!results_path(d.path()).exists());
    assert!(journal_events(d.path()).contains(&(
        "verify_run_refused".into(),
        Some("execution_refused".into())
    )));
}

// ---- S1 / S2 ----

#[test]
fn render_requires_the_opt_in_and_an_accepted_record() {
    let d = project();
    trusted(d.path(), DRAFT_TEMPLATE);
    std::fs::remove_file(d.path().join(".phronesis/verification.json")).expect("opt out");
    let err = render::prepare(d.path(), &request(false)).expect_err("not opted in");
    assert!(matches!(err, RenderPipelineError::NotOptedIn), "{err:?}");

    let d = project();
    trusted(d.path(), DRAFT_TEMPLATE);
    write_properties(
        d.path(),
        vec![property(PropertyStatus::Candidate, &["fn:safe_divide"])],
    );
    let err = render::render_to_disk(d.path(), &request(false), false).expect_err("candidate");
    assert!(
        matches!(err, RenderPipelineError::NotAccepted { .. }),
        "{err:?}"
    );
    assert!(unreviewed_files(d.path()).is_empty());
}

// ---- C1 through the production pipeline: the shipped draft renders a
// Verus harness that really proves (skipped, never faked, without verus) ----

fn which_verus() -> Option<String> {
    std::env::var("VERUS_BIN").ok().or_else(|| {
        let home = std::env::var("HOME").ok()?;
        let p = format!("{home}/.cargo/bin/verus");
        Path::new(&p).exists().then_some(p)
    })
}

#[test]
fn c1_the_draft_template_renders_a_harness_verus_proves() {
    let Some(verus) = which_verus() else {
        eprintln!("skipping C1 pipeline proof: no verus toolchain on this host");
        return;
    };
    let d = project();
    draft(d.path(), DRAFT_TEMPLATE);
    let rendered = render::render_to_disk(d.path(), &request(true), false).expect("render");
    let a = &rendered.artifact;
    approve(
        d.path(),
        &a.artifact_sha256,
        &a.template_sha256,
        &a.property_revision,
    );
    let record =
        render::run(d.path(), &request(true), Some(&verus), Some(REVISION)).expect("verus run");
    assert_eq!(
        record.status, "passed",
        "the rendered harness must prove: {record:?}"
    );
    assert_eq!(
        record.artifact_sha256.as_deref(),
        Some(a.artifact_sha256.as_str())
    );
    assert_eq!(record.template_origin.as_deref(), Some("template_drafts"));
    // A draft's real proof is still not verified evidence.
    assert!(with(&hydrate(d.path()), "verification_result").is_empty());
}

// ---- `kind` is a closed vocabulary, not free text (VT1) ----
//
// `property.kind` names come from a fixed set (SPEC-property-ontology.md §2
// `property_kind`) that this repository's own properties.json exercises:
// `precondition`, `postcondition`, `invariant`, `equivalence`,
// `determinism`, `soundness`, `totality`. A closed-vocabulary word can never
// carry an injection payload, so `kind` is not one of `validate_body`'s
// inert values — before this fix it was, and a property of kind
// `invariant` whose harness legitimately used Verus's `invariant`
// loop-annotation keyword in live code was a false S5 refusal.

#[test]
fn property_of_kind_invariant_can_render_a_harness_using_the_verus_invariant_keyword() {
    let d = project();
    let mut p = property(PropertyStatus::Accepted, &["fn:safe_divide"]);
    p.kind = "invariant".into();
    write_properties(d.path(), vec![p]);

    // `resolve_template` names the file `{verifier}-{kind}.rhai`.
    std::fs::create_dir_all(d.path().join(render::TEMPLATES_DIR)).expect("mkdir templates");
    std::fs::write(
        d.path().join(render::TEMPLATES_DIR).join("verus-invariant.rhai"),
        // Verus-specific `while ... invariant ...` syntax is not valid bare
        // Rust grammar; like the shipped templates, it must sit inside a
        // macro invocation (`verus! { ... }`), whose body `syn` treats as an
        // opaque token tree rather than parsing as Rust items.
        "`verus! {\nfn h() {\n    let mut i: u32 = 0;\n    while i < 10\n        invariant\n            i <= 10,\n    {\n        i = i + 1;\n    }\n}\n} // verus!\n`",
    )
    .expect("write template");

    let out = render::render_to_disk(d.path(), &request(false), false).expect(
        "a closed-vocabulary kind must never make its own text a live-code S5 hazard \
         (the property's `kind` and the Verus `invariant` keyword are the same word)",
    );
    assert!(
        out.artifact.body.contains("invariant"),
        "the keyword must actually be live in the rendered body: {}",
        out.artifact.body
    );
}

#[test]
fn an_unrecognized_kind_is_refused_at_render_time() {
    let d = project();
    let mut p = property(PropertyStatus::Accepted, &["fn:safe_divide"]);
    p.kind = "not_a_real_kind".into();
    write_properties(d.path(), vec![p]);

    let err = render::render_to_disk(d.path(), &request(false), false)
        .expect_err("an unrecognized kind must be refused before template lookup");
    assert!(
        matches!(err, RenderPipelineError::UnknownKind { .. }),
        "{err:?}"
    );
    assert!(err.to_string().contains("not_a_real_kind"), "{err}");
    assert!(unreviewed_files(d.path()).is_empty());
    assert!(
        journal_events(d.path())
            .contains(&("verify_render_refused".into(), Some("unknown_kind".into()))),
        "{:?}",
        journal_events(d.path())
    );
}

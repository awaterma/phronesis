//! Step definitions for `tests/features/coverage-evidence.feature`.
//!
//! Every scenario runs against the committed `coverage-sample` fixture (its
//! real `src/lib.rs` and its real per-test `cargo-llvm-cov` export, whose
//! region ids are file-qualified per SPEC-coverage-evidence §3.2) and, where
//! the scenario is about hook behavior, the real `phr-mcp` binary. Rule
//! firings are read back from the hook's own `.phronesis/log.jsonl`, so a
//! "fires" assertion names the rule id and its bindings, not a stderr guess.
//!
//! Spec: `docs/specs/SPEC-coverage-evidence.md` §5.1–§5.3, §9 A2–A5.

use std::collections::{BTreeSet, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use cucumber::{given, then, when};
use phronesis_mcp::coverage::hydrate::{EditedFile, HydrationInput, RELATIONS, hydrate};
use phronesis_mcp::coverage::import::import_export;
use phronesis_mcp::coverage::store::store_paths;
use phronesis_mcp::rules_file;
use tempfile::TempDir;

use super::World;

/// The fixture's function-level region (SPEC §3.2 qualified form).
const FN_REGION: &str = "fn:src/lib.rs::safe_divide";
/// The edit the whole feature is about: the zero-denominator error message
/// changes; the condition guarding it does not.
const OLD_LINE: &str = r#"return Err("division by zero");"#;
const NEW_LINE: &str = r#"return Err("invalid denominator");"#;

/// Rules 5.2 and 5.3 verbatim from SPEC §5.2/§5.3 (the same text
/// `coverage_gap_rules.rs` proves end to end), in the v2 disk shape.
const GAP_RULES: &str = r#"[
    {
      "id":"warn-evidence-gap","phase":"post","priority":20,"audit":true,
      "when":[
        {"changed_region":["?change","?region"]},
        {"region_without_dynamic_evidence":["?region"]},
        {"region_without_formal_evidence":["?region"]}
      ],
      "then":{"warn":"changed region ?region has neither dynamic test evidence nor formal proof evidence"}
    },
    {
      "id":"warn-commit-on-stale-coverage","phase":"pre","priority":20,"audit":true,
      "when":[
        {"bash_command_matches":["git (commit|merge|rebase|cherry-pick|revert|pull)"]},
        {"coverage_stale": true}
      ],
      "then":{"warn":"coverage evidence is stale (imported revision differs from HEAD) - commits proceed, but relevance claims need re-import"}
    }
]"#;

/// A rule set that mentions no coverage relation at all (acceptance A4).
const NON_COVERAGE_RULES: &str = r#"{"rules":[
    {
      "id":"log-rust-edit","phase":"post","priority":1,"audit":false,
      "when":[{"file_path_matches":["src/"]}],
      "then":{"log":"rust source edited"}
    }
]}"#;

/// Per-scenario coverage state: a throwaway project holding the fixture.
#[derive(Debug)]
pub struct CoverageProject {
    dir: TempDir,
    /// The fixture source before the edit (the whole file, tests included).
    old_src: String,
    /// Facts the last in-process hydration produced (A4).
    hydrated: Option<Vec<(String, Vec<String>)>>,
}

impl CoverageProject {
    fn root(&self) -> &Path {
        self.dir.path()
    }
    fn new_src(&self) -> String {
        self.old_src.replacen(OLD_LINE, NEW_LINE, 1)
    }
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/coverage-sample")
}

fn fixture_export() -> PathBuf {
    fixture_dir().join("export.jsonl")
}

/// The branch region id exactly as the committed export names it — read,
/// not hand-copied, so the step tracks a regenerated export.
fn fixture_branch_region() -> String {
    let text = std::fs::read_to_string(fixture_export()).expect("read fixture export");
    let regions: BTreeSet<String> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<serde_json::Value>(l).expect("export line is JSON"))
        .filter(|v| v["hit_kind"] == "branch")
        .map(|v| v["region"].as_str().expect("region").to_string())
        .collect();
    assert_eq!(
        regions.len(),
        1,
        "fixture export must name exactly one branch region: {regions:?}"
    );
    let region = regions.into_iter().next().expect("one branch region");
    assert!(
        region.starts_with("branch:src/lib.rs::safe_divide:"),
        "fixture branch region must be file-qualified (SPEC §3.2): {region}"
    );
    region
}

fn project(world: &World) -> &CoverageProject {
    world
        .coverage
        .as_ref()
        .expect("Background must set up the coverage project")
}

fn project_mut(world: &mut World) -> &mut CoverageProject {
    world
        .coverage
        .as_mut()
        .expect("Background must set up the coverage project")
}

/// Run a hook subcommand in `root`, isolated from any root override or
/// coverage opt-out in the test runner's own environment.
fn run_in(root: &Path, subcommand: &str, payload: &str) -> (i32, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(root)
        .arg(subcommand)
        .env_remove("PHRONESIS_PROJECT_ROOT")
        .env_remove("PHRONESIS_NO_COVERAGE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn phr-mcp");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(payload.as_bytes())
        .expect("write payload");
    let out = child.wait_with_output().expect("wait for phr-mcp");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// `(rule_id, bindings)` of every consequence the most recent hook entry in
/// the project's action log recorded.
fn last_hook_firings(root: &Path) -> Vec<(String, serde_json::Value, String)> {
    let log = std::fs::read_to_string(root.join(".phronesis/log.jsonl")).unwrap_or_default();
    let entry = log
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .rfind(|v| v["kind"] == "hook")
        .unwrap_or_else(|| panic!("no hook entry in the action log:\n{log}"));
    entry["consequences"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|c| {
            (
                c["rule_id"].as_str().unwrap_or_default().to_string(),
                c["bindings"].clone(),
                c["message"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

fn binding(bindings: &serde_json::Value, var: &str) -> String {
    // Bindings are recorded by variable name; accept either spelling.
    bindings[var]
        .as_str()
        .or_else(|| bindings[var.trim_start_matches('?')].as_str())
        .unwrap_or_default()
        .to_string()
}

fn git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(root)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

// ---------------------------------------------------------------------------
// Background
// ---------------------------------------------------------------------------

#[given("a project with the safe_divide fixture and its real coverage export")]
async fn given_fixture_project(world: &mut World) {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).expect("mkdir src");
    std::fs::create_dir_all(root.join(".phronesis")).expect("mkdir .phronesis");
    std::fs::copy(fixture_dir().join("Cargo.toml"), root.join("Cargo.toml"))
        .expect("copy Cargo.toml");
    let old_src =
        std::fs::read_to_string(fixture_dir().join("src/lib.rs")).expect("read fixture source");
    assert!(
        old_src.contains(OLD_LINE),
        "fixture source must hold the zero-denominator error: {old_src}"
    );
    std::fs::write(root.join("src/lib.rs"), &old_src).expect("write source");

    // Rule 5.1 from its committed fixture, plus rules 5.2 and 5.3.
    let rule_51: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/coverage-rules-5.1.json"),
        )
        .expect("read rule 5.1 fixture"),
    )
    .expect("rule 5.1 fixture is JSON");
    let mut rules: Vec<serde_json::Value> = rule_51["rules"].as_array().expect("rules").clone();
    rules.extend(
        serde_json::from_str::<Vec<serde_json::Value>>(GAP_RULES).expect("gap rules are JSON"),
    );
    std::fs::write(
        root.join(".phronesis/rules.json"),
        serde_json::json!({ "rules": rules }).to_string(),
    )
    .expect("write rules");

    // The committed, real per-test export (acceptance A1).
    let summary = import_export(root, &fixture_export(), 1).expect("import fixture export");
    assert_eq!(summary.tests, 3, "fixture export must carry 3 tests");

    world.coverage = Some(CoverageProject {
        dir,
        old_src,
        hydrated: None,
    });
}

#[given("the coverage evidence is hydrated at hook fire")]
async fn given_hydrated_at_hook_fire(world: &mut World) {
    // Hydration is demand-gated on the loaded rules' relations: prove the
    // Background rules demand coverage facts, so every hook run below
    // hydrates (the child hook also runs with PHRONESIS_NO_COVERAGE unset).
    let p = project(world);
    let predicates = loaded_rule_predicates(p.root());
    for rel in ["changed_region", "test_hits_region", "coverage_stale"] {
        assert!(
            predicates.contains(rel),
            "loaded rules must demand `{rel}`: {predicates:?}"
        );
    }
}

fn loaded_rule_predicates(root: &Path) -> HashSet<String> {
    let file = rules_file::read(&rules_file::default_path(root)).expect("read rules.json");
    file.rules
        .iter()
        .flat_map(|disk| {
            let (rule, _phase) = rules_file::rule_from_disk(disk);
            rule.conditions
                .into_iter()
                .map(|c| c.predicate)
                .collect::<Vec<_>>()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Given
// ---------------------------------------------------------------------------

#[given("a coverage store with no imported evidence")]
async fn given_empty_store(world: &mut World) {
    let root = project(world).root().to_path_buf();
    let (records, index) = store_paths(&root);
    for path in [records, index] {
        if path.exists() {
            std::fs::remove_file(&path).expect("remove coverage store file");
        }
    }
}

#[given("no loaded rule mentions any coverage relation")]
async fn given_no_coverage_rules(world: &mut World) {
    let root = project(world).root().to_path_buf();
    std::fs::write(root.join(".phronesis/rules.json"), NON_COVERAGE_RULES).expect("write rules");
    let predicates = loaded_rule_predicates(&root);
    assert!(
        !RELATIONS.iter().any(|r| predicates.contains(*r)),
        "no loaded rule may mention a coverage relation: {predicates:?}"
    );
}

#[given("a coverage store imported at a revision other than HEAD")]
async fn given_stale_store(world: &mut World) {
    let root = project(world).root().to_path_buf();
    // A real repository whose HEAD cannot be the fixture export's revision.
    git(&root, &["init", "-q"]);
    git(&root, &["add", "src", "Cargo.toml"]);
    git(&root, &["commit", "-q", "-m", "fixture"]);
    let head = Command::new("git")
        .current_dir(&root)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("git rev-parse");
    let head = String::from_utf8_lossy(&head.stdout).trim().to_string();
    let index = phronesis_mcp::coverage::store::load_index(&root).expect("imported store index");
    assert_ne!(
        index.revision, head,
        "the imported revision must differ from HEAD for this scenario"
    );
}

// ---------------------------------------------------------------------------
// When
// ---------------------------------------------------------------------------

#[when("the zero-denominator error message is edited")]
async fn when_error_message_edited(world: &mut World) {
    let p = project(world);
    let root = p.root().to_path_buf();
    let new_src = p.new_src();
    // Post-check reads disk: the edit has already applied.
    std::fs::write(root.join("src/lib.rs"), &new_src).expect("apply edit");
    let payload = serde_json::json!({
        "session_id": "bdd",
        "cwd": root.display().to_string(),
        "hook_event_name": "PostToolUse",
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "src/lib.rs",
            "old_string": p.old_src,
            "new_string": new_src,
        },
    });
    let (code, stderr) = run_in(&root, "post-check", &payload.to_string());
    world.last_exit_code = Some(code);
    world.last_stderr = stderr;
}

#[when("an edit event hydrates")]
async fn when_edit_hydrates(world: &mut World) {
    let p = project(world);
    let root = p.root().to_path_buf();
    let old_src = p.old_src.clone();
    let new_src = p.new_src();
    let predicates = loaded_rule_predicates(&root);
    let run = |rule_relations: HashSet<String>| {
        hydrate(&HydrationInput {
            root: &root,
            rule_relations,
            edited: vec![EditedFile {
                path: "src/lib.rs".into(),
                old: Some(&old_src),
                new: &new_src,
                whole_file: false,
            }],
            head_sha: None,
        })
        .expect("hydrate")
        .facts
        .into_iter()
        .map(|f| (f.predicate, f.args))
        .collect::<Vec<_>>()
    };
    // Control: the same event and store with every coverage relation
    // demanded produces facts, so a zero below is the demand gate at work
    // and not an empty input.
    let ungated = run(RELATIONS.iter().map(|r| r.to_string()).collect());
    assert!(
        ungated.iter().any(|(p, _)| p == "test_hits_region")
            && ungated.iter().any(|(p, _)| p == "changed_region"),
        "control hydration must produce coverage facts: {ungated:?}"
    );
    project_mut(world).hydrated = Some(run(predicates));
}

#[when(expr = "a pre-check runs with {string}")]
async fn when_pre_check_runs(world: &mut World, command: String) {
    let root = project(world).root().to_path_buf();
    let payload = serde_json::json!({
        "session_id": "bdd",
        "cwd": root.display().to_string(),
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": { "command": command },
    });
    let (code, stderr) = run_in(&root, "pre-check", &payload.to_string());
    world.last_exit_code = Some(code);
    world.last_stderr = stderr;
}

// ---------------------------------------------------------------------------
// Then
// ---------------------------------------------------------------------------

/// Tests rule 5.1 paired with the branch region in the last hook run.
fn tests_paired_with_branch(world: &World) -> Vec<String> {
    let branch = fixture_branch_region();
    last_hook_firings(project(world).root())
        .into_iter()
        .filter(|(rule, b, _)| {
            rule == "log-relevant-test-for-change" && binding(b, "?region") == branch
        })
        .map(|(_, b, _)| binding(&b, "?test"))
        .collect()
}

#[then(expr = "rule log-relevant-test-for-change fires for test {string} at the branch region")]
async fn then_relevant_test_at_branch(world: &mut World, test: String) {
    let firings = last_hook_firings(project(world).root());
    let paired = tests_paired_with_branch(world);
    assert_eq!(
        paired,
        vec![test.clone()],
        "exactly {test} must pair with the branch region (exit {:?}, stderr {}): {firings:?}",
        world.last_exit_code,
        world.last_stderr
    );
    // All three tests exercise the function region itself (A2's other half).
    let fn_tests: BTreeSet<String> = firings
        .iter()
        .filter(|(rule, b, _)| {
            rule == "log-relevant-test-for-change" && binding(b, "?region") == FN_REGION
        })
        .map(|(_, b, _)| binding(b, "?test"))
        .collect();
    assert!(
        fn_tests.contains(&test) && fn_tests.len() == 3,
        "all three tests must be relevant to {FN_REGION}: {fn_tests:?}"
    );
    assert_eq!(
        world.last_exit_code,
        Some(0),
        "covered regions must not warn: {}",
        world.last_stderr
    );
}

#[then(expr = "it does not fire for {string} at the branch region")]
async fn then_not_relevant_at_branch(world: &mut World, test: String) {
    let paired = tests_paired_with_branch(world);
    assert!(
        !paired.contains(&test),
        "{test} must not pair with the branch region: {paired:?}"
    );
}

#[then("rule warn-evidence-gap fires for the changed region")]
async fn then_gap_fires(world: &mut World) {
    let regions: BTreeSet<String> = last_hook_firings(project(world).root())
        .into_iter()
        .filter(|(rule, _, _)| rule == "warn-evidence-gap")
        .map(|(_, b, _)| binding(&b, "?region"))
        .collect();
    let expected: BTreeSet<String> = [FN_REGION.to_string(), fixture_branch_region()]
        .into_iter()
        .collect();
    assert_eq!(
        regions, expected,
        "warn-evidence-gap must fire for exactly the edited function and branch regions (stderr {})",
        world.last_stderr
    );
    assert_eq!(
        world.last_exit_code,
        Some(1),
        "a post-check gap warns (exit 1): {}",
        world.last_stderr
    );
}

#[then(expr = "the warning names regions with {string}")]
async fn then_gap_warning_text(world: &mut World, text: String) {
    for region in [FN_REGION.to_string(), fixture_branch_region()] {
        let line = format!("changed region {region} has {text}");
        assert!(
            world.last_stderr.contains(&line),
            "stderr must name {region} with {text:?}: {}",
            world.last_stderr
        );
    }
}

#[then("zero coverage facts are asserted")]
async fn then_zero_coverage_facts(world: &mut World) {
    let facts = project(world)
        .hydrated
        .clone()
        .expect("an edit event must have hydrated");
    assert!(
        facts.is_empty(),
        "the demand gate must suppress every coverage fact: {facts:?}"
    );
}

#[then("rule warn-commit-on-stale-coverage fires")]
async fn then_stale_fires(world: &mut World) {
    let firings = last_hook_firings(project(world).root());
    assert!(
        firings
            .iter()
            .any(|(rule, _, _)| rule == "warn-commit-on-stale-coverage"),
        "warn-commit-on-stale-coverage must fire (stderr {}): {firings:?}",
        world.last_stderr
    );
}

#[then("the warning names the staleness without blocking the commit")]
async fn then_stale_warns_without_blocking(world: &mut World) {
    assert!(
        world.last_stderr.contains("coverage evidence is stale"),
        "stderr must name the staleness: {}",
        world.last_stderr
    );
    assert_eq!(
        world.last_exit_code,
        Some(1),
        "stale coverage warns (exit 1), never blocks (exit 2): {}",
        world.last_stderr
    );
}

#[given("a Python project with per-test lcov evidence")]
async fn given_python_lcov_project(world: &mut World) {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lcov/python-store");
    fn copy(src: &Path, dst: &Path) {
        for e in std::fs::read_dir(src).unwrap() {
            let e = e.unwrap();
            let to = dst.join(e.file_name());
            if e.path().is_dir() {
                std::fs::create_dir_all(&to).unwrap();
                copy(&e.path(), &to);
            } else {
                std::fs::copy(e.path(), to).unwrap();
            }
        }
    }
    copy(&fixture, root);
    std::fs::create_dir_all(root.join(".phronesis")).unwrap();
    std::fs::write(root.join(".phronesis/rules.json"), r#"{"rules":[]}"#).unwrap();
    git(root, &["init", "-q"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "fixture"]);
    let revision = String::from_utf8(
        Command::new("git")
            .current_dir(root)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let digest = phronesis_mcp::properties::execute::artifact_sha256(
        &std::fs::read(root.join("pkg/store.py")).unwrap(),
    );
    std::fs::write(
        root.join("cov/manifest.json"),
        serde_json::json!({"revision":revision,"files":{"pkg/store.py":digest}}).to_string(),
    )
    .unwrap();
    world.checked_file_path = Some(root.to_string_lossy().to_string());
    world.temp_dir = Some(dir);
}

#[when("the lcov evidence is imported and its load body changes")]
async fn when_import_python_lcov_and_edit(world: &mut World) {
    let root = Path::new(world.checked_file_path.as_deref().expect("Python project"));
    let import = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(root)
        .args([
            "coverage",
            "import",
            "--format",
            "lcov-dir",
            "--tool",
            "coverage.py",
            "--allow-dirty",
            "cov",
        ])
        .output()
        .unwrap();
    assert!(
        import.status.success(),
        "{}",
        String::from_utf8_lossy(&import.stderr)
    );
    let rebuild = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(root)
        .args(["graph", "rebuild"])
        .output()
        .unwrap();
    assert!(
        rebuild.status.success(),
        "{}",
        String::from_utf8_lossy(&rebuild.stderr)
    );
    let source = root.join("pkg/store.py");
    let old = std::fs::read_to_string(&source).unwrap();
    std::fs::write(
        &source,
        old.replace("return path.read_text()", "return str(path.read_text())"),
    )
    .unwrap();
    let select = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(root)
        .args(["coverage", "select", "--json"])
        .output()
        .unwrap();
    world.last_exit_code = select.status.code();
    world.last_json = String::from_utf8_lossy(&select.stdout).to_string();
    world.last_stderr = String::from_utf8_lossy(&select.stderr).to_string();
    assert!(select.status.success(), "{}", world.last_stderr);
}

#[then("coverage select lists the Python test and pytest command")]
async fn then_python_selection_command(world: &mut World) {
    assert!(
        world
            .last_json
            .contains("python:pkg::tests::test_store::test_load"),
        "{}",
        world.last_json
    );
    assert!(
        world
            .last_json
            .contains("python -m pytest tests/test_store.py::test_load"),
        "{}",
        world.last_json
    );
}

#[given("a Java Maven project with an annotated test and production method")]
async fn given_java_jacoco_project(world: &mut World) {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    let source = root.join("core/src/main/java/com/x/Store.java");
    let test = root.join("core/src/test/java/com/x/StoreTest.java");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    std::fs::create_dir_all(test.parent().unwrap()).unwrap();
    std::fs::write(
        root.join("pom.xml"),
        "<project><artifactId>root</artifactId></project>",
    )
    .unwrap();
    std::fs::write(
        root.join("core/pom.xml"),
        "<project><artifactId>core</artifactId></project>",
    )
    .unwrap();
    std::fs::write(
        &source,
        "package com.x; public class Store { public int load() { return 1; } }\n",
    )
    .unwrap();
    std::fs::write(
        &test,
        "package com.x; class StoreTest { @Test void testLoad() { new Store().load(); } }\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("cov/1")).unwrap();
    std::fs::write(root.join("cov/1/module.txt"), "core\n").unwrap();
    std::fs::write(
        root.join("cov/1/TN"),
        "java:core::com::x::StoreTest::testLoad\n",
    )
    .unwrap();
    std::fs::write(root.join("cov/1/jacoco.xml"), r#"<report><package name="com/x"><class name="com/x/Store" sourcefilename="Store.java"><method name="load"><counter type="METHOD" missed="0" covered="1"/></method></class><sourcefile name="Store.java"><line nr="1" mi="0" ci="1"/></sourcefile></package></report>"#).unwrap();
    std::fs::create_dir_all(root.join(".phronesis")).unwrap();
    std::fs::write(root.join(".phronesis/rules.json"), r#"{"rules":[]}"#).unwrap();
    git(root, &["init", "-q"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "fixture"]);
    let revision = String::from_utf8(
        Command::new("git")
            .current_dir(root)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let digest =
        phronesis_mcp::properties::execute::artifact_sha256(&std::fs::read(&source).unwrap());
    std::fs::write(root.join("cov/manifest.json"), serde_json::json!({"revision":revision,"files":{"core/src/main/java/com/x/Store.java":digest},"runner":"mvn"}).to_string()).unwrap();
    world.checked_file_path = Some(root.to_string_lossy().to_string());
    world.temp_dir = Some(dir);
}

#[when("the Java production method body changes")]
async fn when_java_jacoco_import_and_edit(world: &mut World) {
    let root = Path::new(world.checked_file_path.as_deref().expect("Java project"));
    let import = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(root)
        .args([
            "coverage",
            "import",
            "--format",
            "jacoco-dir",
            "--tool",
            "jacoco+mvn",
            "cov",
        ])
        .output()
        .unwrap();
    assert!(
        import.status.success(),
        "{}",
        String::from_utf8_lossy(&import.stderr)
    );
    let source = root.join("core/src/main/java/com/x/Store.java");
    let old = std::fs::read_to_string(&source).unwrap();
    std::fs::write(&source, old.replace("return 1", "return 2")).unwrap();
    let rebuild = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(root)
        .args(["graph", "rebuild"])
        .output()
        .unwrap();
    assert!(
        rebuild.status.success(),
        "{}",
        String::from_utf8_lossy(&rebuild.stderr)
    );
    let select = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(root)
        .args(["coverage", "select", "--json"])
        .output()
        .unwrap();
    world.last_exit_code = select.status.code();
    world.last_json = String::from_utf8_lossy(&select.stdout).to_string();
    world.last_stderr = String::from_utf8_lossy(&select.stderr).to_string();
    assert!(select.status.success(), "{}", world.last_stderr);
}

#[then("coverage select lists the Java test and Maven command")]
async fn then_java_selection_command(world: &mut World) {
    assert!(
        world
            .last_json
            .contains("java:core::com::x::StoreTest::testLoad"),
        "{}",
        world.last_json
    );
    assert!(
        world
            .last_json
            .contains("mvn -pl core -Dtest='com.x.StoreTest#testLoad' test"),
        "{}",
        world.last_json
    );
}

#[given("a Swift project with per-test lcov evidence")]
async fn given_swift_lcov_project(world: &mut World) {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lcov/swift-store");
    fn copy(src: &Path, dst: &Path) {
        for e in std::fs::read_dir(src).unwrap() {
            let e = e.unwrap();
            let to = dst.join(e.file_name());
            if e.path().is_dir() {
                std::fs::create_dir_all(&to).unwrap();
                copy(&e.path(), &to);
            } else {
                std::fs::copy(e.path(), to).unwrap();
            }
        }
    }
    copy(&fixture, root);
    std::fs::create_dir_all(root.join(".phronesis")).unwrap();
    std::fs::write(root.join(".phronesis/rules.json"), r#"{"rules":[]}"#).unwrap();
    git(root, &["init", "-q"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "fixture"]);
    let revision = String::from_utf8(
        Command::new("git")
            .current_dir(root)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let digest = phronesis_mcp::properties::execute::artifact_sha256(
        &std::fs::read(root.join("Sources/Store/Store.swift")).unwrap(),
    );
    std::fs::write(
        root.join("cov/manifest.json"),
        serde_json::json!({"revision":revision,"files":{"Sources/Store/Store.swift":digest}})
            .to_string(),
    )
    .unwrap();
    world.checked_file_path = Some(root.to_string_lossy().to_string());
    world.temp_dir = Some(dir);
}

#[when("the Swift lcov evidence is imported and its load body changes")]
async fn when_import_swift_lcov_and_edit(world: &mut World) {
    let root = Path::new(world.checked_file_path.as_deref().expect("Swift project"));
    let import = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(root)
        .args([
            "coverage",
            "import",
            "--format",
            "lcov-dir",
            "--tool",
            "swift-cov",
            "--allow-dirty",
            "cov",
        ])
        .output()
        .unwrap();
    assert!(
        import.status.success(),
        "{}",
        String::from_utf8_lossy(&import.stderr)
    );
    let rebuild = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(root)
        .args(["graph", "rebuild"])
        .output()
        .unwrap();
    assert!(
        rebuild.status.success(),
        "{}",
        String::from_utf8_lossy(&rebuild.stderr)
    );
    let source = root.join("Sources/Store/Store.swift");
    let old = std::fs::read_to_string(&source).unwrap();
    std::fs::write(
        &source,
        old.replace("return text", "return text.lowercased()"),
    )
    .unwrap();
    let select = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(root)
        .args(["coverage", "select", "--json"])
        .output()
        .unwrap();
    world.last_exit_code = select.status.code();
    world.last_json = String::from_utf8_lossy(&select.stdout).to_string();
    world.last_stderr = String::from_utf8_lossy(&select.stderr).to_string();
    assert!(select.status.success(), "{}", world.last_stderr);
}

#[then("coverage select lists the Swift test and swift test command")]
async fn then_swift_selection_command(world: &mut World) {
    assert!(
        world
            .last_json
            .contains("swift:StoreTests::StoreTests::StoreTests::testLoad"),
        "{}",
        world.last_json
    );
    assert!(
        world
            .last_json
            .contains("swift test --filter '^StoreTests\\\\.StoreTests/testLoad$'"),
        "{}",
        world.last_json
    );
}

#[given("a TypeScript project with per-test lcov evidence")]
async fn given_typescript_lcov_project(world: &mut World) {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lcov/ts-store");
    fn copy(src: &Path, dst: &Path) {
        for e in std::fs::read_dir(src).unwrap() {
            let e = e.unwrap();
            let to = dst.join(e.file_name());
            if e.path().is_dir() {
                std::fs::create_dir_all(&to).unwrap();
                copy(&e.path(), &to);
            } else {
                std::fs::copy(e.path(), to).unwrap();
            }
        }
    }
    copy(&fixture, root);
    std::fs::create_dir_all(root.join(".phronesis")).unwrap();
    std::fs::write(root.join(".phronesis/rules.json"), r#"{"rules":[]}"#).unwrap();
    git(root, &["init", "-q"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "fixture"]);
    let revision = String::from_utf8(
        Command::new("git")
            .current_dir(root)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let digest = phronesis_mcp::properties::execute::artifact_sha256(
        &std::fs::read(root.join("src/store.ts")).unwrap(),
    );
    std::fs::write(
        root.join("cov/manifest.json"),
        serde_json::json!({"revision":revision,"files":{"src/store.ts":digest}}).to_string(),
    )
    .unwrap();
    world.checked_file_path = Some(root.to_string_lossy().to_string());
    world.temp_dir = Some(dir);
}

#[when("the TypeScript lcov evidence is imported and its load body changes")]
async fn when_import_typescript_lcov_and_edit(world: &mut World) {
    let root = Path::new(world.checked_file_path.as_deref().expect("TS project"));
    let import = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(root)
        .args([
            "coverage",
            "import",
            "--format",
            "lcov-dir",
            "--tool",
            "c8+vitest",
            "--allow-dirty",
            "cov",
        ])
        .output()
        .unwrap();
    assert!(
        import.status.success(),
        "{}",
        String::from_utf8_lossy(&import.stderr)
    );
    let rebuild = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(root)
        .args(["graph", "rebuild"])
        .output()
        .unwrap();
    assert!(
        rebuild.status.success(),
        "{}",
        String::from_utf8_lossy(&rebuild.stderr)
    );
    let source = root.join("src/store.ts");
    let old = std::fs::read_to_string(&source).unwrap();
    std::fs::write(
        &source,
        old.replace("return \"value\";", "return \"value!\";"),
    )
    .unwrap();
    let select = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(root)
        .args(["coverage", "select", "--json"])
        .output()
        .unwrap();
    world.last_exit_code = select.status.code();
    world.last_json = String::from_utf8_lossy(&select.stdout).to_string();
    world.last_stderr = String::from_utf8_lossy(&select.stderr).to_string();
    assert!(select.status.success(), "{}", world.last_stderr);
}

#[then("coverage select lists the TypeScript test and vitest command")]
async fn then_typescript_selection_command(world: &mut World) {
    // The graph's real `defines_test` id: the `#test:` target infix for a
    // file under `tests/`, the module segment keeping `.test`, and the raw
    // `it()` title at the end.
    assert!(
        world
            .last_json
            .contains("typescript:ts-store#test:store.test.ts::tests::store.test::Store loads"),
        "{}",
        world.last_json
    );
    // The command is chosen by the imported record's tool string
    // (`c8+vitest`), with the title extracted after the file's module
    // marker; JSON-escaped, the inner quotes read `\"`.
    assert!(
        world
            .last_json
            .contains(r#"npx vitest run tests/store.test.ts -t \"Store loads\""#),
        "{}",
        world.last_json
    );
}

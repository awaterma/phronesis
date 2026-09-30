//! Language-pack toolchain definitions: `cue` and `helm` shipped by
//! `phr-mcp init --packs cue` / `--packs helm3` with merge-if-absent
//! semantics (the language-pack toolchains writer adds missing toolchain
//! ids and never touches user-edited ones).

use phronesis_mcp::outcomes::facts::OutcomeFact;
use phronesis_mcp::outcomes::toolchain::{CompiledDef, DefSource, load_project_defs};
use std::process::Command;

fn run_init(dir: &std::path::Path, packs: &str) {
    let out = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(["init", "--packs", packs])
        .current_dir(dir)
        .output()
        .expect("spawn phr-mcp init");
    assert!(
        out.status.success(),
        "init --packs {packs} failed: stderr={:?}",
        out.stderr
    );
}

fn load_def(dir: &std::path::Path, id: &str) -> phronesis_mcp::outcomes::toolchain::ToolchainDef {
    let defs = load_project_defs(dir);
    defs.iter()
        .find(|d| d.id == id)
        .unwrap_or_else(|| panic!("{id} def in toolchains.json"))
        .clone()
}

fn compile(dir: &std::path::Path, id: &str) -> CompiledDef {
    let def = load_def(dir, id);
    CompiledDef::compile(def, DefSource::Project).expect("compile def")
}

fn build_outcome(facts: &[OutcomeFact]) -> &str {
    facts
        .iter()
        .find(|f| f.predicate == "build_outcome")
        .map(|f| f.args[1].as_str())
        .expect("build_outcome fact")
}

// ── cue ─────────────────────────────────────────────────────────────

#[test]
fn cue_pack_writes_cue_toolchain_def_when_no_file_exists() {
    let dir = tempfile::tempdir().unwrap();
    run_init(dir.path(), "cue");
    let compiled = compile(dir.path(), "cue");

    // Recognition: cue vet, cue eval, cue export, cue fmt --check
    assert!(compiled.handles("cue vet config/"), "cue vet recognized");
    assert!(compiled.handles("cue eval config/"), "cue eval recognized");
    assert!(
        compiled.handles("cue export config/"),
        "cue export recognized"
    );
    assert!(
        compiled.handles("cue fmt --check config/"),
        "cue fmt --check recognized"
    );
    assert!(!compiled.handles("echo cue vet"), "echo must not match");
}

/// Live-probed `cue vet` failure output on this machine (cue v0.16.1; the
/// patterns these captures pin are the toolchain def's `compile_fail`):
///
/// conflicting values (`cue vet conflict.cue`, file: `a: int` + `a: "hello"`):
///   `a: conflicting values int and "hello" (mismatched types int and string):`
///   `    ./conflict.cue:1:4`
///   `    ./conflict.cue:2:4`
///
/// reference not found (`cue vet unresolved.cue`, file: `value: missingRef`):
///   `value: reference "missingRef" not found:`
///   `    ./unresolved.cue:2:8`
///
/// incomplete value (`cue vet -c`, file: `instance: {age: int}`):
///   `instance.age: incomplete value int:`
///   `    ./incomplete.cue:2:17`
///
/// cannot use (`cue vet`, file: `result: strings.HasPrefix(42, "x")`):
///   `result: cannot use 42 (type int) as string in argument 1 to strings.HasPrefix:`
///   `    ./cannotuse.cue:3:27`
///
/// All four patterns are pinned from the actual `cue vet` output above.
#[test]
fn cue_compile_fail_patterns_match_observed_cue_vet_output() {
    let dir = tempfile::tempdir().unwrap();
    run_init(dir.path(), "cue");
    let compiled = compile(dir.path(), "cue");

    // conflicting values — from `cue vet` on a file with `a: int` and `a: "hello"`
    let conflict_output = "a: conflicting values int and \"hello\" (mismatched types int and string):\n    ./conflict.cue:1:4\n    ./conflict.cue:2:4";
    let facts = compiled.parse("s", "cue vet", conflict_output, Some(1));
    assert_eq!(
        build_outcome(&facts),
        "fail",
        "conflicting values pattern must ground a compile fail"
    );

    // reference not found — from `cue vet` on a file with `value: missingRef`
    let ref_output = "value: reference \"missingRef\" not found:\n    ./unresolved.cue:2:8";
    let facts = compiled.parse("s", "cue vet", ref_output, Some(1));
    assert_eq!(
        build_outcome(&facts),
        "fail",
        "reference not found pattern must ground a compile fail"
    );

    // incomplete value — from `cue vet -c` on a partial struct
    let incomplete_output = "instance.age: incomplete value int:\n    ./incomplete.cue:2:17";
    let facts = compiled.parse("s", "cue vet -c", incomplete_output, Some(1));
    assert_eq!(
        build_outcome(&facts),
        "fail",
        "incomplete value pattern must ground a compile fail"
    );

    // cannot use — from `cue vet -c` with a type mismatch in a builtin call
    let cannot_output = "result: cannot use 42 (type int) as string in argument 1 to strings.HasPrefix:\n    ./cannotuse.cue:3:27";
    let facts = compiled.parse("s", "cue vet -c", cannot_output, Some(1));
    assert_eq!(
        build_outcome(&facts),
        "fail",
        "cannot use pattern must ground a compile fail"
    );

    // A passing cue vet (exit 0, no error text) should ground a pass
    let facts = compiled.parse("s", "cue vet", "all good\n", Some(0));
    assert_eq!(
        build_outcome(&facts),
        "pass",
        "clean output with exit 0 must ground a compile pass"
    );

    // A file:line:col pattern alone (no exit code) must ground a fail
    let facts = compiled.parse("s", "cue vet", "./conflict.cue:4:8", None);
    assert_eq!(
        build_outcome(&facts),
        "fail",
        "file:line:col pattern must ground a compile fail even without an exit code"
    );
}

// ── helm ────────────────────────────────────────────────────────────
// NOTE: `helm` is NOT installed on this machine. `brew install helm` is
// blocked by the sandbox, so the helm `compile_fail` and `test_summary`
// regexes below are UNVERIFIED (probe-first debt). The patterns follow
// the plan's documented shapes:
//   compile_fail: `[ERROR]` and `Error:`
//   test_summary:  `(?P<total>\d+) chart\(s\) linted, (?P<failed>\d+) chart\(s\) failed`
// A reviewer with helm installed should confirm them against a real
// `helm lint` run before relying on them.

#[test]
fn helm3_pack_writes_helm_toolchain_def_when_no_file_exists() {
    let dir = tempfile::tempdir().unwrap();
    run_init(dir.path(), "helm3");
    let compiled = compile(dir.path(), "helm");

    // Recognition: helm lint, helm template
    assert!(
        compiled.handles("helm lint mychart/"),
        "helm lint recognized"
    );
    assert!(
        compiled.handles("helm template mychart/"),
        "helm template recognized"
    );
    assert!(!compiled.handles("echo helm lint"), "echo must not match");
}

/// UNVERIFIED: helm is not installed; patterns are pinned from the plan,
/// not from a live `helm lint` run.
#[test]
fn helm_compile_fail_patterns_match_plan_shapes() {
    let dir = tempfile::tempdir().unwrap();
    run_init(dir.path(), "helm3");
    let compiled = compile(dir.path(), "helm");

    // [ERROR] — plan-documented pattern
    let error_output = "[ERROR] templates/: manifest is not valid YAML";
    let facts = compiled.parse("s", "helm lint", error_output, Some(1));
    assert_eq!(
        build_outcome(&facts),
        "fail",
        "[ERROR] pattern must ground a compile fail"
    );

    // Error: — plan-documented pattern
    let error_output = "Error: Chart.yaml file not found";
    let facts = compiled.parse("s", "helm lint", error_output, Some(1));
    assert_eq!(
        build_outcome(&facts),
        "fail",
        "Error: pattern must ground a compile fail"
    );

    // A clean summary (no error text) with exit 0 must ground a pass
    let clean_output = "1 chart(s) linted, 0 chart(s) failed";
    let facts = compiled.parse("s", "helm lint", clean_output, Some(0));
    assert_eq!(
        build_outcome(&facts),
        "pass",
        "clean summary with exit 0 must ground a compile pass"
    );
}

// ── merge-if-absent semantics ──────────────────────────────────────

#[test]
fn language_pack_toolchains_merge_into_existing_file_without_clobbering() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".phronesis")).unwrap();

    // Pre-existing toolchains.json with a user-defined toolchain
    std::fs::write(
        dir.path().join(".phronesis/toolchains.json"),
        r#"[{"id":"my-custom","matches":"^my-tool$","compile_fail":["boom"]}]"#,
    )
    .unwrap();

    run_init(dir.path(), "cue");

    let defs = load_project_defs(dir.path());
    let ids: Vec<&str> = defs.iter().map(|d| d.id.as_str()).collect();

    // The user's custom def must survive
    assert!(
        ids.contains(&"my-custom"),
        "user's custom toolchain must survive merge: {ids:?}"
    );
    // The cue def must be added
    assert!(ids.contains(&"cue"), "cue def must be merged in: {ids:?}");
}

#[test]
fn language_pack_toolchains_do_not_overwrite_existing_def_with_same_id() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".phronesis")).unwrap();

    // Pre-existing toolchains.json with a user-customized `cue` def
    let user_matches = "^my-cue$";
    std::fs::write(
        dir.path().join(".phronesis/toolchains.json"),
        format!(r#"[{{"id":"cue","matches":"{user_matches}"}}]"#),
    )
    .unwrap();

    run_init(dir.path(), "cue");

    let defs = load_project_defs(dir.path());
    let cue_def = defs.iter().find(|d| d.id == "cue").expect("cue def");

    // The user's matches regex must be preserved, not overwritten
    assert_eq!(
        cue_def.matches, user_matches,
        "existing cue def must not be overwritten"
    );
}

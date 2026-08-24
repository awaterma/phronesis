//! End-to-end tests for `phr-mcp init`. Each test owns a tempdir and
//! exercises the real binary so CLI argument parsing, file writes, and
//! exit codes are all under test.

use std::path::Path;
use std::process::{Command, Output};

fn run_init(args: &[&str], cwd: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(["init"])
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("spawn init")
}

#[test]
fn init_creates_all_five_files_in_fresh_project() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_init(&[], dir.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(dir.path().join(".claude/settings.local.json").exists());
    assert!(dir.path().join(".mcp.json").exists());
    assert!(dir.path().join(".phronesis/rules.json").exists());
    assert!(dir.path().join(".phronesis/durable.md").exists());
    assert!(dir.path().join(".gitignore").exists());

    // Verify the rules file has the minimal starter pack (3 deflection rules)
    let rules: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".phronesis/rules.json")).unwrap(),
    )
    .unwrap();
    let ids: Vec<&str> = rules["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"enforce-no-pre-existing-issue"));

    // Verify the emitted rules use v2 shape (when/then, no action_type)
    let raw = std::fs::read_to_string(dir.path().join(".phronesis/rules.json")).unwrap();
    assert!(raw.contains("\"when\""), "init must emit v2 `when`");
    assert!(raw.contains("\"then\""), "init must emit v2 `then`");
    assert!(
        !raw.contains("\"action_type\""),
        "init must not emit v1 `action_type`"
    );

    // Verify the durable.md template nudges the model toward the drift tools.
    let durable = std::fs::read_to_string(dir.path().join(".phronesis/durable.md")).unwrap();
    assert!(
        durable.contains("get_drift"),
        "durable.md must nudge the model toward the consolidated drift tool"
    );
    for gone in ["get_claude_md_drift", "get_memory_drift", "get_wiki_drift"] {
        assert!(
            !durable.contains(gone),
            "durable.md still names the removed tool {gone} — it is re-injected every session"
        );
    }
}

#[test]
fn init_preserves_existing_durable_md() {
    let dir = tempfile::tempdir().unwrap();
    // Pre-populate durable.md with the user's own content. init must not
    // clobber it.
    std::fs::create_dir_all(dir.path().join(".phronesis")).unwrap();
    let original = "# my team's directives\n\n- do X\n- don't Y\n";
    std::fs::write(dir.path().join(".phronesis/durable.md"), original).unwrap();

    let out = run_init(&[], dir.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let after = std::fs::read_to_string(dir.path().join(".phronesis/durable.md")).unwrap();
    assert_eq!(
        after, original,
        "init must not overwrite existing durable.md"
    );
}

#[test]
fn init_with_language_rust_uses_rust_pack() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_init(&["--language", "rust"], dir.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let rules: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".phronesis/rules.json")).unwrap(),
    )
    .unwrap();
    let ids: Vec<&str> = rules["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"enforce-no-unwrap-in-src"));
    assert!(ids.contains(&"enforce-no-result-string-error"));
    assert!(ids.contains(&"warn-dbg-in-src"));
    // Deflection still carried
    assert!(ids.contains(&"enforce-no-pre-existing-issue"));
}

#[test]
fn init_dry_run_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_init(&["--dry-run"], dir.path());
    assert!(out.status.success());
    assert!(!dir.path().join(".claude/settings.local.json").exists());
    assert!(!dir.path().join(".mcp.json").exists());
    assert!(!dir.path().join(".phronesis/rules.json").exists());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("dry-run"),
        "stdout should announce dry-run: {}",
        stdout
    );
}

#[test]
fn init_preserves_existing_settings_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let settings = dir.path().join(".claude/settings.local.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    std::fs::write(
        &settings,
        r#"{"permissions":{"allow":["Bash(ls:*)","Bash(cargo:*)"]}}"#,
    )
    .unwrap();

    let out = run_init(&[], dir.path());
    assert!(out.status.success());

    let content: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(content["permissions"]["allow"][0], "Bash(ls:*)");
    assert_eq!(content["permissions"]["allow"][1], "Bash(cargo:*)");
    // And hooks were added
    assert!(content["hooks"]["PreToolUse"].is_array());
    assert!(content["hooks"]["PostToolUse"].is_array());
}

#[test]
fn init_idempotent_on_second_run() {
    let dir = tempfile::tempdir().unwrap();
    run_init(&["--language", "rust"], dir.path());
    let first_settings =
        std::fs::read_to_string(dir.path().join(".claude/settings.local.json")).unwrap();
    let first_rules = std::fs::read_to_string(dir.path().join(".phronesis/rules.json")).unwrap();

    run_init(&["--language", "rust"], dir.path());
    let second_settings =
        std::fs::read_to_string(dir.path().join(".claude/settings.local.json")).unwrap();
    let second_rules = std::fs::read_to_string(dir.path().join(".phronesis/rules.json")).unwrap();

    assert_eq!(first_settings, second_settings);
    assert_eq!(first_rules, second_rules);
}

#[test]
fn init_force_overwrites_rules_and_creates_backup() {
    let dir = tempfile::tempdir().unwrap();
    let rules = dir.path().join(".phronesis/rules.json");
    std::fs::create_dir_all(rules.parent().unwrap()).unwrap();
    std::fs::write(
        &rules,
        r#"{"rules":[{"id":"my-rule","phase":"pre","priority":1,"conditions":[],"actions":[]}]}"#,
    )
    .unwrap();

    let out = run_init(&["--language", "rust", "--force"], dir.path());
    assert!(out.status.success());

    let content: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&rules).unwrap()).unwrap();
    let ids: Vec<&str> = content["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"enforce-no-unwrap-in-src"));
    assert!(!ids.contains(&"my-rule"));
    // Backup preserved
    let bak: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".phronesis/rules.json.bak")).unwrap(),
    )
    .unwrap();
    assert_eq!(bak["rules"][0]["id"], "my-rule");
}

#[test]
fn init_appends_to_existing_gitignore() {
    let dir = tempfile::tempdir().unwrap();
    let gi = dir.path().join(".gitignore");
    std::fs::write(&gi, "/target\nCargo.lock\n").unwrap();

    run_init(&[], dir.path());

    let content = std::fs::read_to_string(&gi).unwrap();
    assert!(content.contains("/target"));
    assert!(content.contains("Cargo.lock"));
    assert!(content.contains(".phronesis/log.jsonl"));
    assert!(content.contains(".phronesis/log.jsonl.1"));
    assert!(content.contains(".phronesis/rules.json.bak"));
}

#[test]
fn init_unignores_extensible_predicate_providers() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_init(&[], dir.path());
    assert!(out.status.success());
    let gitignore = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert!(gitignore.contains("!.phronesis/predicates/"));
    assert!(gitignore.contains("!.phronesis/predicates/**"));
}

#[test]
fn init_rejects_unknown_language() {
    // --language is a thin backward-compat wrapper around --packs; an unknown
    // value still errors out, just via the pack parser's message wording.
    let dir = tempfile::tempdir().unwrap();
    let out = run_init(&["--language", "haskell"], dir.path());
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unknown pack"), "stderr: {}", stderr);
}

#[test]
fn init_none_language_writes_empty_rules() {
    let dir = tempfile::tempdir().unwrap();
    run_init(&["--language", "none"], dir.path());
    let content: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".phronesis/rules.json")).unwrap(),
    )
    .unwrap();
    assert!(content["rules"].as_array().unwrap().is_empty());
}

#[test]
fn init_rejects_none_combined_with_another_pack() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_init(&["--packs", "none,rust"], dir.path());
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("cannot be combined"), "stderr: {stderr}");
}

#[test]
fn init_writes_correct_mcp_server_registration() {
    let dir = tempfile::tempdir().unwrap();
    run_init(&[], dir.path());
    let mcp: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join(".mcp.json")).unwrap())
            .unwrap();
    assert_eq!(mcp["mcpServers"]["phronesis"]["command"], "phr-mcp");
    assert_eq!(mcp["mcpServers"]["phronesis"]["args"][0], "serve");
}

#[test]
fn init_writes_correct_hook_matchers_including_bash() {
    let dir = tempfile::tempdir().unwrap();
    run_init(&[], dir.path());
    let settings: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".claude/settings.local.json")).unwrap(),
    )
    .unwrap();
    let pre = &settings["hooks"]["PreToolUse"];
    let entry = pre
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["matcher"] == "Edit|Write|MultiEdit|Bash")
        .expect("expected an Edit|Write|MultiEdit|Bash matcher");
    assert_eq!(entry["hooks"][0]["command"], "phr-mcp pre-check");
}

// ─────────────────────────────────────────────────────────────────────────
// Pack composition (new --packs CLI surface)
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn init_packs_llm_only_omits_language_rules() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_init(&["--packs", "llm"], dir.path());
    assert!(out.status.success());
    let rules: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".phronesis/rules.json")).unwrap(),
    )
    .unwrap();
    let ids: Vec<&str> = rules["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"enforce-no-pre-existing-issue"));
    assert!(!ids.contains(&"enforce-no-unwrap-in-src"));
}

#[test]
fn init_packs_rust_adds_language_rules_to_the_base() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_init(&["--packs", "rust"], dir.path());
    assert!(out.status.success());
    let rules: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".phronesis/rules.json")).unwrap(),
    )
    .unwrap();
    let ids: Vec<&str> = rules["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"enforce-no-unwrap-in-src"));
    assert!(ids.contains(&"enforce-no-pre-existing-issue"));
    assert!(!ids.contains(&"block-await-on-sync-execute-all-agenda-items"));
    assert!(!ids.contains(&"block-await-on-sync-fire-all-consequences"));
}

#[test]
fn init_typescript_pack_uses_only_the_structural_explicit_any_rule() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_init(&["--packs", "typescript"], dir.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rules: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".phronesis/rules.json")).unwrap(),
    )
    .unwrap();
    let ids: Vec<&str> = rules["rules"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|rule| rule["id"].as_str())
        .collect();

    assert!(ids.contains(&"warn-ts-explicit-any-ast"));
    assert!(
        !ids.contains(&"warn-any-in-src"),
        "the lexical duplicate must not ship alongside its structural replacement: {ids:?}"
    );
}

#[test]
fn init_packs_llm_rust_composes_both() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_init(&["--packs", "llm,rust"], dir.path());
    assert!(out.status.success());
    let rules: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".phronesis/rules.json")).unwrap(),
    )
    .unwrap();
    let ids: Vec<&str> = rules["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"enforce-no-pre-existing-issue"));
    assert!(ids.contains(&"enforce-no-unwrap-in-src"));
}

#[test]
fn init_packs_confidence_writes_gate_rules_and_scaffold() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_init(&["--packs", "confidence"], dir.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Gate rules land in rules.json.
    let rules: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".phronesis/rules.json")).unwrap(),
    )
    .unwrap();
    let ids: Vec<&str> = rules["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"confidence-low-blocks-commit"));
    assert!(ids.contains(&"confidence-medium-warns-commit"));

    // Opt-in marker + registry are scaffolded.
    assert!(dir.path().join(".phronesis/confidence.json").exists());
    assert!(dir.path().join(".phronesis/bugs.json").exists());
    let bugs: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".phronesis/bugs.json")).unwrap(),
    )
    .unwrap();
    assert!(
        bugs.as_array().unwrap().is_empty(),
        "bugs.json starts empty"
    );

    // confidence config is carved back in (tracked, not ignored).
    let gitignore = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert!(gitignore.contains("!.phronesis/confidence.json"));
    assert!(gitignore.contains("!.phronesis/bugs.json"));
}

#[test]
fn explicit_llm_selection_still_writes_default_confidence_scaffold() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_init(&["--packs", "llm"], dir.path());
    assert!(out.status.success());
    assert!(dir.path().join(".phronesis/confidence.json").exists());
    assert!(dir.path().join(".phronesis/bugs.json").exists());
    let gitignore = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert!(gitignore.contains("!.phronesis/confidence.json"));
}

#[test]
fn confidence_pack_writes_toolchains_example() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_init(&["--packs", "confidence"], dir.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let raw = std::fs::read_to_string(dir.path().join(".phronesis/toolchains.json"))
        .expect("toolchains.json written");
    let defs: serde_json::Value =
        serde_json::from_str(&raw).expect("toolchains.json is valid JSON array");
    let ids: Vec<&str> = defs
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d["id"].as_str())
        .collect();
    assert_eq!(ids, vec!["pytest", "tsc"]);
}

#[test]
fn toolchains_example_left_alone_on_rerun() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_init(&["--packs", "confidence"], dir.path());
    assert!(out.status.success());
    let custom = r#"[{"id":"mine","matches":"mine"}]"#;
    std::fs::write(dir.path().join(".phronesis/toolchains.json"), custom).unwrap();
    let out = run_init(&["--packs", "confidence"], dir.path());
    assert!(out.status.success());
    let raw = std::fs::read_to_string(dir.path().join(".phronesis/toolchains.json")).unwrap();
    assert_eq!(raw, custom, "existing file must be left unchanged");
}

#[test]
fn confidence_pack_unignores_toolchains_json() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_init(&["--packs", "confidence"], dir.path());
    assert!(out.status.success());
    let gitignore = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert!(gitignore.contains("!.phronesis/toolchains.json"));
}

#[test]
fn init_confidence_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        run_init(&["--packs", "confidence"], dir.path())
            .status
            .success()
    );
    // Hand-edit the registry, then re-run: must be left untouched.
    let bugs_path = dir.path().join(".phronesis/bugs.json");
    std::fs::write(
        &bugs_path,
        r#"[{"bug_id":"1","test":"a::b","status":"open"}]"#,
    )
    .unwrap();
    let out = run_init(&["--packs", "confidence"], dir.path());
    assert!(out.status.success());
    assert!(
        std::fs::read_to_string(&bugs_path)
            .unwrap()
            .contains("\"bug_id\":\"1\""),
        "re-run must not clobber an existing bugs.json"
    );
}

#[test]
fn init_default_is_the_complete_language_agnostic_platform() {
    // No --packs flag → every language-agnostic subsystem is ready.
    let dir = tempfile::tempdir().unwrap();
    let out = run_init(&[], dir.path());
    assert!(out.status.success());
    let rules: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".phronesis/rules.json")).unwrap(),
    )
    .unwrap();
    let ids: Vec<&str> = rules["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"enforce-no-pre-existing-issue"));
    assert!(ids.contains(&"confidence-low-blocks-commit"));
    // No language-specific rules in the default
    assert!(!ids.contains(&"enforce-no-unwrap-in-src"));
    for path in [
        ".phronesis/graph.jsonl",
        ".phronesis/context.json",
        ".phronesis/kernel.md",
        ".phronesis/confidence.json",
        ".phronesis/journey.json",
    ] {
        assert!(
            dir.path().join(path).exists(),
            "default init omitted {path}"
        );
    }
}

#[test]
fn init_rejects_unknown_pack() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_init(&["--packs", "haskell"], dir.path());
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unknown pack"), "stderr: {}", stderr);
}

// ─────────────────────────────────────────────────────────────────────────
// Subcommand aliases
// ─────────────────────────────────────────────────────────────────────────

fn run_aliased(alias: &str, cwd: &std::path::Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args([alias])
        .current_dir(cwd)
        .output()
        .expect("spawn alias")
}

#[test]
fn setup_alias_works() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_aliased("setup", dir.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(dir.path().join(".phronesis/rules.json").exists());
}

#[test]
fn configure_alias_works() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_aliased("configure", dir.path());
    assert!(out.status.success());
    assert!(dir.path().join(".phronesis/rules.json").exists());
}

// ─────────────────────────────────────────────────────────────────────────
// Backward compat: --language flag still works (auto-composes with llm)
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn deprecated_language_flag_still_works() {
    // Old CLI: --language rust = the bundled "deflection + rust" behavior.
    let dir = tempfile::tempdir().unwrap();
    let out = run_init(&["--language", "rust"], dir.path());
    assert!(out.status.success());
    let rules: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".phronesis/rules.json")).unwrap(),
    )
    .unwrap();
    let ids: Vec<&str> = rules["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    // Both the LLM pack and the rust pack should be present (legacy behavior)
    assert!(ids.contains(&"enforce-no-pre-existing-issue"));
    assert!(ids.contains(&"enforce-no-unwrap-in-src"));
}

// ─────────────────────────────────────────────────────────────────────────
// --rules-only: refresh just the rules pack
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn rules_only_skips_other_files() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_init(&["--rules-only", "--packs", "llm,rust"], dir.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Rules file is written
    assert!(dir.path().join(".phronesis/rules.json").exists());
    // None of the others were touched
    assert!(!dir.path().join(".claude/settings.local.json").exists());
    assert!(!dir.path().join(".mcp.json").exists());
    assert!(!dir.path().join(".gitignore").exists());
}

#[test]
fn rules_only_with_force_refreshes_existing_rules() {
    // The intended workflow: an old rules.json exists, the user wants to
    // refresh it with a newer pack composition without touching anything else.
    let dir = tempfile::tempdir().unwrap();
    // Pre-existing project with hooks and a stale rules file
    std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
    std::fs::write(
        dir.path().join(".claude/settings.local.json"),
        r#"{"permissions":{"allow":["custom"]},"hooks":{"PreToolUse":[{"matcher":"Edit","hooks":[{"type":"command","command":"custom-hook"}]}]}}"#,
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join(".phronesis")).unwrap();
    std::fs::write(
        dir.path().join(".phronesis/rules.json"),
        r#"{"rules":[{"id":"old-only","phase":"pre","priority":1,"conditions":[],"actions":[]}]}"#,
    )
    .unwrap();

    let out = run_init(
        &["--rules-only", "--force", "--packs", "llm,rust"],
        dir.path(),
    );
    assert!(out.status.success());

    // Rules file refreshed
    let rules: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".phronesis/rules.json")).unwrap(),
    )
    .unwrap();
    let ids: Vec<&str> = rules["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"enforce-no-unwrap-in-src"));
    assert!(!ids.contains(&"old-only"));

    // Backup of prior rules was made
    assert!(dir.path().join(".phronesis/rules.json.bak").exists());

    // Settings file untouched (custom hook still there, no phronesis entries added)
    let settings: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".claude/settings.local.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(settings["permissions"]["allow"][0], "custom");
    let pre = settings["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(pre.len(), 1);
    assert_eq!(pre[0]["matcher"], "Edit");
    assert_eq!(pre[0]["hooks"][0]["command"], "custom-hook");
}

#[test]
fn rules_only_dry_run_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_init(
        &["--rules-only", "--dry-run", "--packs", "rust"],
        dir.path(),
    );
    assert!(out.status.success());
    assert!(!dir.path().join(".phronesis/rules.json").exists());
    assert!(!dir.path().join(".claude/settings.local.json").exists());
}

#[test]
fn rules_only_without_force_respects_existing_rules() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".phronesis")).unwrap();
    let custom =
        r#"{"rules":[{"id":"mine","phase":"pre","priority":1,"conditions":[],"actions":[]}]}"#;
    std::fs::write(dir.path().join(".phronesis/rules.json"), custom).unwrap();

    let out = run_init(&["--rules-only", "--packs", "llm,rust"], dir.path());
    assert!(out.status.success());

    // Without --force, the existing file is preserved verbatim.
    let content = std::fs::read_to_string(dir.path().join(".phronesis/rules.json")).unwrap();
    assert!(content.contains("\"mine\""));
    assert!(!content.contains("enforce-no-unwrap-in-src"));
}

// ─────────────────────────────────────────────────────────────────────────
// install / uninstall — user-level MCP server registration
//
// Each test points HOME at a tempdir so the subprocess writes to a fresh
// `<tempdir>/.claude.json` instead of the real one.
// ─────────────────────────────────────────────────────────────────────────

fn run_with_fake_home(args: &[&str], fake_home: &std::path::Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .args(args)
        .env("HOME", fake_home)
        .output()
        .expect("spawn subcommand")
}

fn fake_claude_json(home: &std::path::Path) -> std::path::PathBuf {
    home.join(".claude.json")
}

#[test]
fn install_creates_user_claude_json_with_mcp_entry() {
    let home = tempfile::tempdir().unwrap();
    let out = run_with_fake_home(&["install"], home.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fake_claude_json(home.path())).unwrap())
            .unwrap();
    assert_eq!(config["mcpServers"]["phronesis"]["command"], "phr-mcp");
    assert_eq!(config["mcpServers"]["phronesis"]["args"][0], "serve");
}

#[test]
fn install_merges_into_existing_claude_json() {
    let home = tempfile::tempdir().unwrap();
    // Pre-existing config with other MCP servers AND unrelated top-level keys
    std::fs::write(
        fake_claude_json(home.path()),
        r#"{
            "theme": "dark",
            "mcpServers": {
                "other": {"command":"other-mcp","args":[]}
            }
        }"#,
    )
    .unwrap();

    let out = run_with_fake_home(&["install"], home.path());
    assert!(out.status.success());

    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fake_claude_json(home.path())).unwrap())
            .unwrap();
    // Other MCP server preserved
    assert_eq!(config["mcpServers"]["other"]["command"], "other-mcp");
    // Our entry added
    assert_eq!(config["mcpServers"]["phronesis"]["command"], "phr-mcp");
    // Unrelated top-level keys preserved
    assert_eq!(config["theme"], "dark");
}

#[test]
fn install_is_idempotent() {
    let home = tempfile::tempdir().unwrap();
    run_with_fake_home(&["install"], home.path());
    let first = std::fs::read_to_string(fake_claude_json(home.path())).unwrap();
    run_with_fake_home(&["install"], home.path());
    let second = std::fs::read_to_string(fake_claude_json(home.path())).unwrap();
    assert_eq!(first, second);
}

#[test]
fn install_dry_run_writes_nothing() {
    let home = tempfile::tempdir().unwrap();
    let out = run_with_fake_home(&["install", "--dry-run"], home.path());
    assert!(out.status.success());
    assert!(!fake_claude_json(home.path()).exists());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("dry-run"), "stdout: {}", stdout);
}

#[test]
fn uninstall_removes_only_phronesis_entry() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        fake_claude_json(home.path()),
        r#"{
            "theme": "dark",
            "mcpServers": {
                "phronesis": {"command":"phr-mcp","args":["serve"]},
                "other": {"command":"other-mcp","args":[]}
            }
        }"#,
    )
    .unwrap();

    let out = run_with_fake_home(&["uninstall"], home.path());
    assert!(out.status.success());

    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fake_claude_json(home.path())).unwrap())
            .unwrap();
    assert!(config["mcpServers"].get("phronesis").is_none());
    assert_eq!(config["mcpServers"]["other"]["command"], "other-mcp");
    assert_eq!(config["theme"], "dark");
}

#[test]
fn uninstall_is_idempotent_when_already_absent() {
    let home = tempfile::tempdir().unwrap();
    // No file at all
    let out = run_with_fake_home(&["uninstall"], home.path());
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("doesn't exist") || stdout.contains("nothing to"));
}

#[test]
fn install_uninstall_round_trip_preserves_other_state() {
    let home = tempfile::tempdir().unwrap();
    let original = r#"{
  "theme": "dark",
  "mcpServers": {
    "other": {
      "command": "other-mcp",
      "args": []
    }
  }
}"#;
    std::fs::write(fake_claude_json(home.path()), original).unwrap();

    run_with_fake_home(&["install"], home.path());
    run_with_fake_home(&["uninstall"], home.path());

    let after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fake_claude_json(home.path())).unwrap())
            .unwrap();
    // Other server still there
    assert_eq!(after["mcpServers"]["other"]["command"], "other-mcp");
    // Theme still there
    assert_eq!(after["theme"], "dark");
    // No phronesis entry
    assert!(after["mcpServers"].get("phronesis").is_none());
}

#[test]
fn init_creates_wiki_decisions_directory_and_readme() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_init(&[], dir.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let dec = dir.path().join(".phronesis/wiki/decisions");
    assert!(dec.is_dir(), "decisions dir should be created");
    let readme = dec.join("README.md");
    assert!(readme.is_file(), "README.md should be created");
    let body = std::fs::read_to_string(&readme).unwrap();
    assert!(body.to_lowercase().contains("decision"));
    assert!(body.contains("frontmatter") || body.contains("frontmatter"));
}

#[test]
fn init_preserves_existing_wiki_readme() {
    let dir = tempfile::tempdir().unwrap();
    let dec = dir.path().join(".phronesis/wiki/decisions");
    std::fs::create_dir_all(&dec).unwrap();
    let original = "# my custom README\n\nproject-specific notes\n";
    std::fs::write(dec.join("README.md"), original).unwrap();

    let out = run_init(&[], dir.path());
    assert!(out.status.success());

    let after = std::fs::read_to_string(dec.join("README.md")).unwrap();
    assert_eq!(
        after, original,
        "init must not overwrite an existing README"
    );
}

#[test]
fn init_gitignore_carves_out_wiki_exception() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_init(&[], dir.path());
    assert!(out.status.success());

    let gi = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    let lines: Vec<&str> = gi.lines().map(str::trim).collect();

    // Broad ignore must be `.phronesis/*` (with the `*`) — NOT `.phronesis/`,
    // which would prevent git from listing the dir at all, making the
    // un-ignore inert. Verified empirically: `.phronesis/` keeps wiki/
    // ignored; `.phronesis/*` lets the un-ignore land.
    let broad_idx = lines
        .iter()
        .position(|l| *l == ".phronesis/*")
        .expect("init must write a standalone `.phronesis/*` broad-ignore line");
    let un_dir_idx = lines
        .iter()
        .position(|l| *l == "!.phronesis/wiki/")
        .expect("init must write `!.phronesis/wiki/` un-ignore line");
    let un_glob_idx = lines
        .iter()
        .position(|l| *l == "!.phronesis/wiki/**")
        .expect("init must write `!.phronesis/wiki/**` un-ignore line");

    // Gitignore semantics: un-ignores only take effect when they follow
    // the broad ignore. Order must be: broad, then both un-ignores.
    assert!(
        broad_idx < un_dir_idx,
        "broad ignore must precede !.phronesis/wiki/ ({} vs {})",
        broad_idx,
        un_dir_idx
    );
    assert!(
        broad_idx < un_glob_idx,
        "broad ignore must precede !.phronesis/wiki/** ({} vs {})",
        broad_idx,
        un_glob_idx
    );
}

#[test]
fn init_gitignore_migrates_legacy_bare_phronesis_line() {
    // Pre-0.9.0 init wrote a bare `.phronesis/` (no trailing `*`). That form
    // tells git not to descend into the directory at all, so any later
    // `!.phronesis/wiki/**` un-ignore is inert. Re-running init on such a
    // project must rewrite the legacy line to `.phronesis/*` so the carveout
    // takes effect.
    let dir = tempfile::tempdir().unwrap();
    let gi_path = dir.path().join(".gitignore");
    std::fs::write(&gi_path, "/target\n.phronesis/\n.phronesis/log.jsonl\n").unwrap();

    let out = run_init(&[], dir.path());
    assert!(out.status.success(), "init must succeed: {out:?}");

    let gi = std::fs::read_to_string(&gi_path).unwrap();
    let lines: Vec<&str> = gi.lines().map(str::trim).collect();

    assert!(
        !lines.contains(&".phronesis/"),
        "legacy bare `.phronesis/` must be rewritten; got:\n{gi}"
    );
    assert!(
        lines.contains(&".phronesis/*"),
        "migration must produce `.phronesis/*`; got:\n{gi}"
    );

    // Pre-existing unrelated content is preserved.
    assert!(lines.contains(&"/target"));
    // Specific log entry that was already present stays put.
    assert!(lines.contains(&".phronesis/log.jsonl"));
}

#[test]
fn init_gitignore_migration_dedupes_when_target_already_present() {
    // Mixed state: legacy bare `.phronesis/` AND modern `.phronesis/*` both
    // present (a hand-edited partial-migration). After init, exactly one
    // `.phronesis/*` line — the migration must not produce a duplicate.
    let dir = tempfile::tempdir().unwrap();
    let gi_path = dir.path().join(".gitignore");
    std::fs::write(
        &gi_path,
        "/target\n.phronesis/\n.phronesis/log.jsonl\n.phronesis/*\n",
    )
    .unwrap();

    let out = run_init(&[], dir.path());
    assert!(out.status.success(), "init must succeed: {out:?}");

    let gi = std::fs::read_to_string(&gi_path).unwrap();
    let count = gi.lines().filter(|l| l.trim() == ".phronesis/*").count();
    assert_eq!(
        count, 1,
        "exactly one `.phronesis/*` line expected; got:\n{gi}"
    );
    assert!(
        !gi.lines().any(|l| l.trim() == ".phronesis/"),
        "legacy bare line must be gone; got:\n{gi}"
    );
}

#[test]
fn init_gitignore_migration_only_no_missing_entries() {
    // Covers the `migrated > 0 && missing.empty()` write-anyway branch.
    // The file already contains every modern entry plus the legacy bare
    // line — only the migration runs, but the file MUST be re-written.
    let dir = tempfile::tempdir().unwrap();
    let gi_path = dir.path().join(".gitignore");
    std::fs::write(
        &gi_path,
        "/target\n\
         .phronesis/\n\
         .phronesis/log.jsonl\n\
         .phronesis/log.jsonl.1\n\
         .phronesis/rules.json.bak\n\
         .phronesis/*\n\
         !.phronesis/wiki/\n\
         !.phronesis/wiki/**\n",
    )
    .unwrap();

    let out = run_init(&[], dir.path());
    assert!(out.status.success(), "init must succeed: {out:?}");

    let gi = std::fs::read_to_string(&gi_path).unwrap();
    assert!(
        !gi.lines().any(|l| l.trim() == ".phronesis/"),
        "legacy bare line must be removed even when no entries are missing"
    );
}

#[test]
fn init_gitignore_preserves_no_trailing_newline_state_when_migration_only() {
    // Original file has no trailing newline. After a migration-only run
    // (no appended entries), the file must still not have a spurious
    // trailing newline introduced.
    let dir = tempfile::tempdir().unwrap();
    let gi_path = dir.path().join(".gitignore");
    // No trailing newline.
    std::fs::write(
        &gi_path,
        "/target\n\
         .phronesis/\n\
         .phronesis/log.jsonl\n\
         .phronesis/log.jsonl.1\n\
         .phronesis/rules.json.bak\n\
         .phronesis/*\n\
         !.phronesis/wiki/\n\
         !.phronesis/wiki/**",
    )
    .unwrap();

    let out = run_init(&[], dir.path());
    assert!(out.status.success(), "init must succeed: {out:?}");

    let gi = std::fs::read_to_string(&gi_path).unwrap();
    assert!(
        !gi.ends_with("\n\n"),
        "must not introduce blank trailing newline"
    );
    // Migration still happened.
    assert!(
        !gi.lines().any(|l| l.trim() == ".phronesis/"),
        "legacy bare line must still be migrated"
    );
}

#[test]
fn init_gitignore_dry_run_reports_migration_distinctly() {
    let dir = tempfile::tempdir().unwrap();
    let gi_path = dir.path().join(".gitignore");
    std::fs::write(&gi_path, ".phronesis/\n").unwrap();

    let out = run_init(&["--dry-run"], dir.path());
    assert!(out.status.success(), "dry-run init must succeed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("would migrate"),
        "dry-run must surface the migration distinctly:\n{stdout}"
    );

    // And no actual write happened.
    let after = std::fs::read_to_string(&gi_path).unwrap();
    assert_eq!(after, ".phronesis/\n", "dry-run must not write");
}

#[test]
fn init_gitignore_idempotent_on_second_run() {
    let dir = tempfile::tempdir().unwrap();
    run_init(&[], dir.path());
    let first = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    run_init(&[], dir.path());
    let second = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert_eq!(
        first, second,
        "gitignore must not duplicate entries on re-run"
    );
}

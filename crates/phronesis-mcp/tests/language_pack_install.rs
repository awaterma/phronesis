//! Task 3 of the language-pack plan: installation into
//! `.phronesis/packs/<id>/`, no-op reinstall, collision refusal, dry-run
//! writes nothing, and `state`/`clean` preserve installed packs.

use std::fs;
use std::path::Path;

use phronesis_mcp::language_packs::install::{InstallAction, install, validate_directory};
use phronesis_mcp::language_packs::registry::{self, PackStatus};
use serde_json::{Value, json};

fn manifest(id: &str, ext: &str) -> Value {
    json!({
        "schema_version": 1,
        "adapter_api": 1,
        "id": id,
        "version": "0.1.0",
        "language": id,
        "extensions": [ext],
        "grammar": "python",
        "capabilities": ["structural_facts"],
        "queries": [{"id": "decls", "path": "queries/decls.scm", "emit": [
            {"kind": "declaration", "name": "${name}", "scope": "", "span_capture": "def"}
        ]}],
        "context_files": [],
        "fixtures": []
    })
}

fn write(dir: &Path, rel: &str, bytes: &[u8]) {
    let path = dir.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn write_pack(dir: &Path, manifest: &Value) {
    for q in manifest["queries"].as_array().into_iter().flatten() {
        let p = q["path"].as_str().unwrap();
        if !dir.join(p).exists() {
            write(dir, p, b"(function_definition) @def");
        }
    }
    write(dir, "pack.json", manifest.to_string().as_bytes());
}

fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    if !dir.exists() {
        return out;
    }
    for entry in walkdir(dir) {
        let bytes = fs::read(&entry).unwrap();
        out.push((
            entry.strip_prefix(dir).unwrap().display().to_string(),
            bytes,
        ));
    }
    out.sort();
    out
}

fn walkdir(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(walkdir(&path));
            } else {
                out.push(path);
            }
        }
    }
    out
}

#[test]
fn validate_directory_writes_nothing() {
    let src = tempfile::tempdir().unwrap();
    write_pack(src.path(), &manifest("demo_a", "demoa"));
    let before = snapshot(src.path());
    let report = validate_directory(src.path()).unwrap();
    assert!(report.ok, "{:?}", report.diagnostics);
    assert_eq!(
        before,
        snapshot(src.path()),
        "validate_directory must not write"
    );
}

#[test]
fn install_is_atomic_and_idempotent() {
    let project = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    write_pack(src.path(), &manifest("demo_b", "demob"));

    let report = install(project.path(), src.path(), false).unwrap();
    assert_eq!(report.action, InstallAction::Installed);
    let installed_dir = project.path().join(".phronesis/packs/demo_b");
    assert!(installed_dir.join("pack.json").exists());
    assert!(installed_dir.join("installed.json").exists());
    assert!(installed_dir.join("queries/decls.scm").exists());

    // Reinstalling identical content is a no-op.
    let before = snapshot(&installed_dir);
    let report2 = install(project.path(), src.path(), false).unwrap();
    assert_eq!(report2.action, InstallAction::Unchanged);
    assert_eq!(
        before,
        snapshot(&installed_dir),
        "unchanged reinstall must not rewrite files"
    );
}

#[test]
fn install_refuses_collision_under_same_id() {
    let project = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    write_pack(src.path(), &manifest("demo_c", "democ"));
    install(project.path(), src.path(), false).unwrap();

    // Different content (bumped version) under the same id must be refused,
    // not silently overwritten.
    let other = tempfile::tempdir().unwrap();
    let mut m = manifest("demo_c", "democ");
    m["version"] = json!("0.2.0");
    write_pack(other.path(), &m);
    let err = install(project.path(), other.path(), false).unwrap_err();
    assert!(err.to_string().contains("demo_c"), "{err}");

    // The original installation must be untouched.
    let installed =
        fs::read_to_string(project.path().join(".phronesis/packs/demo_c/pack.json")).unwrap();
    assert!(installed.contains("\"0.1.0\""), "{installed}");
}

#[test]
fn dry_run_install_writes_nothing() {
    let project = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    write_pack(src.path(), &manifest("demo_d", "demod"));

    let packs_dir = project.path().join(".phronesis/packs");
    let report = install(project.path(), src.path(), true).unwrap();
    assert_eq!(report.action, InstallAction::Installed);
    assert!(report.dry_run);
    assert!(
        !packs_dir.join("demo_d").exists(),
        "dry run must not create the pack directory"
    );
}

#[test]
fn install_refuses_invalid_pack() {
    let project = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let mut m = manifest("demo_e", "demoe");
    m["schema_version"] = json!(99);
    write_pack(src.path(), &m);
    let err = install(project.path(), src.path(), false).unwrap_err();
    assert!(err.to_string().contains("schema_version"), "{err}");
    assert!(!project.path().join(".phronesis/packs/demo_e").exists());
}

/// Installed packs are authored configuration: `phr-mcp state` reports
/// `.phronesis/packs` as `authored`, and `clean --cache` (which only removes
/// a fixed list of rebuildable graph files) never touches it.
#[test]
fn installed_packs_survive_state_classification_and_clean() {
    let project = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    write_pack(src.path(), &manifest("demo_f", "demof"));
    install(project.path(), src.path(), false).unwrap();

    let entries = phronesis_mcp::state::inspect(project.path());
    let packs_entry = entries
        .iter()
        .find(|e| e.path == ".phronesis/packs")
        .unwrap_or_else(|| panic!("installed packs dir should be classified: {entries:?}"));
    assert_eq!(packs_entry.class, "authored");

    phronesis_mcp::state::clean_cache(project.path()).unwrap();
    assert!(
        project
            .path()
            .join(".phronesis/packs/demo_f/pack.json")
            .exists(),
        "clean --cache must never remove installed packs"
    );
}

/// A dry run must be a pure read: it must not create `.phronesis` at all,
/// not even the lock file or the packs directory, on a project that has none
/// yet (repairs a bug where dry-run install still created
/// `.phronesis/packs/.install.lock`).
#[test]
fn dry_run_install_creates_no_phronesis_directory() {
    let project = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    write_pack(src.path(), &manifest("demo_g", "demog"));

    let report = install(project.path(), src.path(), true).unwrap();
    assert!(report.dry_run);
    assert!(
        !project.path().join(".phronesis").exists(),
        ".phronesis must not be created by a dry run"
    );
}

/// Content modified inside an installed pack directory after installation
/// must be rejected, not silently accepted as the new "current" content:
/// both strict `load` and diagnostic `inspect` must surface it.
#[test]
fn tampered_installed_bytes_are_rejected() {
    let project = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    write_pack(src.path(), &manifest("demo_h", "demoh"));
    install(project.path(), src.path(), false).unwrap();

    let query_path = project
        .path()
        .join(".phronesis/packs/demo_h/queries/decls.scm");
    fs::write(&query_path, b"(call_expression) @call\n").unwrap();

    let err = match registry::load(project.path()) {
        Ok(_) => panic!("tampered pack must not load"),
        Err(e) => e,
    };
    assert!(err.to_string().contains("digest"), "{err}");

    let inventory = registry::inspect(project.path());
    let pack = inventory
        .packs
        .iter()
        .find(|p| p.id.as_deref() == Some("demo_h"))
        .expect("tampered pack still listed");
    assert_eq!(pack.status, PackStatus::Invalid);
    assert!(
        pack.diagnostics
            .iter()
            .any(|d| d.message.contains("digest")),
        "{:?}",
        pack.diagnostics
    );
}

/// An installed pack with an extra, undeclared file in its directory is
/// rejected the same way as tampered bytes — the installed tree may contain
/// exactly the declared assets plus `installed.json`.
#[test]
fn undeclared_file_in_installed_dir_is_rejected() {
    let project = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    write_pack(src.path(), &manifest("demo_i", "demoi"));
    install(project.path(), src.path(), false).unwrap();

    let extra = project
        .path()
        .join(".phronesis/packs/demo_i/queries/extra.scm");
    fs::write(&extra, b"(identifier) @id\n").unwrap();

    let err = match registry::load(project.path()) {
        Ok(_) => panic!("pack with an undeclared file must not load"),
        Err(e) => e,
    };
    assert!(err.to_string().contains("not declared"), "{err}");
}

/// Two different packs cannot both claim the same extension: install must
/// check ownership against every other installed pack, not only against
/// native extensions.
#[test]
fn install_refuses_second_pack_claiming_same_extension() {
    let project = tempfile::tempdir().unwrap();
    let src_a = tempfile::tempdir().unwrap();
    write_pack(src_a.path(), &manifest("demo_j", "sharedext"));
    install(project.path(), src_a.path(), false).unwrap();

    let src_b = tempfile::tempdir().unwrap();
    write_pack(src_b.path(), &manifest("demo_k", "sharedext"));
    let err = install(project.path(), src_b.path(), false).unwrap_err();
    assert!(err.to_string().contains("sharedext"), "{err}");
    assert!(
        !project.path().join(".phronesis/packs/demo_k").exists(),
        "a pack refused for ownership conflict must not be installed"
    );
}

#[cfg(unix)]
#[test]
fn install_refuses_symlinked_packs_directory() {
    use std::os::unix::fs::symlink;

    let project = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::create_dir_all(project.path().join(".phronesis")).unwrap();
    symlink(outside.path(), project.path().join(".phronesis/packs")).unwrap();

    let src = tempfile::tempdir().unwrap();
    write_pack(src.path(), &manifest("demo_l", "demol"));

    let err = install(project.path(), src.path(), false).unwrap_err();
    assert!(err.to_string().contains("symlink"), "{err}");
    assert!(
        !outside.path().join("demo_l").exists(),
        "install must never write through a symlinked packs directory"
    );
}

#[test]
fn dry_run_rejects_extension_collision_without_writing() {
    let project = tempfile::tempdir().unwrap();
    let first = tempfile::tempdir().unwrap();
    write_pack(first.path(), &manifest("owner_a", "sharedext"));
    install(project.path(), first.path(), false).unwrap();
    let second = tempfile::tempdir().unwrap();
    write_pack(second.path(), &manifest("owner_b", "sharedext"));
    let before = snapshot(project.path());
    let dry = install(project.path(), second.path(), true).unwrap_err();
    assert!(dry.to_string().contains("sharedext"), "{dry}");
    assert_eq!(before, snapshot(project.path()));
    let real = install(project.path(), second.path(), false).unwrap_err();
    assert!(real.to_string().contains("sharedext"), "{real}");
}

#[test]
fn validation_rejects_wrong_toolchain_namespace_before_install() {
    let project = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let mut m = manifest("paced", "pacedext");
    m["capabilities"] = json!(["structural_facts", "toolchains"]);
    m["toolchains"] = json!("toolchains.json");
    write_pack(src.path(), &m);
    write(
        src.path(),
        "toolchains.json",
        br#"[{"id":"other.test","matches":"^go test"}]"#,
    );
    let report = validate_directory(src.path()).unwrap();
    assert!(!report.ok);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.message.contains("paced.")),
        "{:?}",
        report.diagnostics
    );
    assert!(install(project.path(), src.path(), true).is_err());
    assert!(!project.path().join(".phronesis").exists());
}

#[test]
fn validation_rejects_wrong_rule_namespace_before_install() {
    let project = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let mut m = manifest("policy_pack", "policyext");
    m["rules"] = json!("rules.json");
    write_pack(src.path(), &m);
    write(src.path(), "rules.json", br#"{"rules":[{"id":"wrong.rule","phase":"post","when":[{"file_extension_is":"policyext"}],"then":{"warn":"warning"}}]}"#);
    let report = validate_directory(src.path()).unwrap();
    assert!(!report.ok);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.message.contains("policy_pack.")),
        "{:?}",
        report.diagnostics
    );
    assert!(install(project.path(), src.path(), true).is_err());
    assert!(!project.path().join(".phronesis").exists());
}

#[test]
fn sixty_fifth_install_is_refused_in_dry_run_and_real_mode() {
    let project = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    for n in 0..64 {
        write_pack(
            src.path(),
            &manifest(&format!("bounded_{n:02}"), &format!("ext{n:02}")),
        );
        install(project.path(), src.path(), false).unwrap();
    }
    let packs = project.path().join(".phronesis/packs");
    let before = snapshot(&packs);
    write_pack(src.path(), &manifest("bounded_64", "ext64"));
    for dry_run in [true, false] {
        let err = install(project.path(), src.path(), dry_run).unwrap_err();
        assert!(err.to_string().contains("limit of 64"), "{err}");
        assert_eq!(before, snapshot(&packs), "limit refusal must not publish");
    }
    write_pack(src.path(), &manifest("bounded_00", "ext00"));
    let unchanged = install(project.path(), src.path(), true).unwrap();
    assert_eq!(unchanged.action, InstallAction::Unchanged);
}

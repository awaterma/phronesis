//! Task 3 of the language-pack plan: CLI `pack list --json` must report
//! exactly `language_packs::inspect(root)` — the same data the MCP
//! `list_language_packs` tool returns (`server::list_language_packs_tool_tests`
//! pins the MCP side of that same parity in-crate, since MCP tools are not
//! reachable from an external integration-test crate). `pack list` must
//! never write to the project, and a broken or tampered pack must stay
//! visible rather than vanish.

use std::fs;
use std::path::Path;
use std::process::Command;

use phronesis_mcp::language_packs::install::install;
use phronesis_mcp::language_packs::registry::inspect;

fn bin() -> std::path::PathBuf {
    std::env::var_os("PHRONESIS_TEST_BIN")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_BIN_EXE_phr-mcp")))
}

fn cli_list_json(root: &Path) -> serde_json::Value {
    let output = Command::new(bin())
        .args(["pack", "list", "--json", "--path"])
        .arg(root)
        .output()
        .expect("run phr-mcp pack list");
    assert!(
        output.status.success(),
        "pack list exited non-zero: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("pack list prints valid JSON")
}

fn manifest(id: &str, ext: &str) -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1, "adapter_api": 1, "id": id, "version": "0.1.0",
        "language": id, "extensions": [ext], "grammar": "python",
        "capabilities": ["structural_facts"],
        "queries": [{"id": "decls", "path": "queries/decls.scm", "emit": [
            {"kind": "declaration", "name": "${name}", "scope": "", "span_capture": "def"}
        ]}],
        "context_files": [], "fixtures": []
    })
}

fn write_pack(dir: &Path, m: &serde_json::Value) {
    fs::create_dir_all(dir.join("queries")).unwrap();
    fs::write(dir.join("queries/decls.scm"), "(function_definition) @def").unwrap();
    fs::write(dir.join("pack.json"), m.to_string()).unwrap();
}

fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    fn walk(base: &Path, cur: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let Ok(entries) = fs::read_dir(cur) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                out.push((
                    path.strip_prefix(base).unwrap().display().to_string(),
                    fs::read(&path).unwrap(),
                ));
            }
        }
    }
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// CLI JSON must equal the library inventory call field-for-field, for a
/// project with one installed pack. The MCP tool wraps the identical
/// `inspect` call (`server::language_pack_inventory_payload`), so this pins
/// CLI/MCP parity transitively.
#[test]
fn cli_json_matches_library_inspect() {
    let project = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    write_pack(src.path(), &manifest("demo_disc_a", "discoa"));
    install(project.path(), src.path(), false).unwrap();

    let cli = cli_list_json(project.path());
    let lib = serde_json::to_value(inspect(project.path())).unwrap();
    assert_eq!(cli, lib);
}

/// `pack list` is read-only: it must not write to the project, whether or
/// not any pack is installed.
#[test]
fn pack_list_writes_nothing() {
    let project = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    write_pack(src.path(), &manifest("demo_disc_b", "discob"));
    install(project.path(), src.path(), false).unwrap();

    let before = snapshot(project.path());
    cli_list_json(project.path());
    assert_eq!(before, snapshot(project.path()), "pack list must not write");
}

/// A pack with a manifest that fails to parse remains a visible,
/// identifiable, diagnosable entry over the CLI — never a silently empty
/// list.
#[test]
fn missing_manifest_pack_remains_visible_over_cli() {
    let project = tempfile::tempdir().unwrap();
    let broken = project.path().join(".phronesis/packs/broken_disc");
    fs::create_dir_all(&broken).unwrap();
    fs::write(broken.join("pack.json"), "{ not valid json").unwrap();

    let cli = cli_list_json(project.path());
    let packs = cli["packs"].as_array().unwrap();
    let broken = packs
        .iter()
        .find(|p| {
            p["location"]
                .as_str()
                .is_some_and(|path| path.ends_with("broken_disc"))
        })
        .expect("broken pack remains listed");
    assert_eq!(broken["status"], "invalid");
    assert!(!broken["diagnostics"].as_array().unwrap().is_empty());
    assert_eq!(cli["complete"], false);
}

/// Tampering with an installed pack's bytes after installation remains
/// visible as an invalid entry over the CLI, rather than silently loading
/// the modified content as the pack's new current state.
#[test]
fn tampered_pack_remains_visible_over_cli() {
    let project = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    write_pack(src.path(), &manifest("demo_disc_c", "discoc"));
    install(project.path(), src.path(), false).unwrap();

    fs::write(
        project
            .path()
            .join(".phronesis/packs/demo_disc_c/queries/decls.scm"),
        "(call_expression) @call",
    )
    .unwrap();

    let cli = cli_list_json(project.path());
    let pack = cli["packs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "demo_disc_c")
        .expect("tampered pack still listed");
    assert_eq!(pack["status"], "invalid");
    assert!(
        pack["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains("digest")
    );
}

/// Installing a second pack is visible on the very next `pack list` call —
/// there is no cross-invocation cache to go stale.
#[test]
fn install_between_two_list_calls_is_visible() {
    let project = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    write_pack(src.path(), &manifest("demo_disc_d", "discod"));

    let before = cli_list_json(project.path());
    assert!(
        before["packs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["id"] != "demo_disc_d")
    );

    install(project.path(), src.path(), false).unwrap();

    let after = cli_list_json(project.path());
    assert!(
        after["packs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == "demo_disc_d")
    );
}

#[test]
fn invalid_nondirectory_and_dangling_entries_stay_visible_and_fail_strict_load() {
    let project = tempfile::tempdir().unwrap();
    let packs = project.path().join(".phronesis/packs");
    fs::create_dir_all(&packs).unwrap();
    fs::write(packs.join("regular_entry"), b"not a directory").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("missing_target", packs.join("dangling_entry")).unwrap();
    }
    let inventory = inspect(project.path());
    assert!(!inventory.complete);
    assert!(
        inventory
            .packs
            .iter()
            .any(|p| p.location.ends_with("regular_entry")
                && p.status == phronesis_mcp::language_packs::registry::PackStatus::Invalid)
    );
    #[cfg(unix)]
    assert!(
        inventory
            .packs
            .iter()
            .any(|p| p.location.ends_with("dangling_entry")
                && p.status == phronesis_mcp::language_packs::registry::PackStatus::Invalid)
    );
    assert!(phronesis_mcp::language_packs::registry::load(project.path()).is_err());
}

#[test]
fn more_than_64_installed_entries_reports_limit_before_reading_them() {
    let project = tempfile::tempdir().unwrap();
    let packs = project.path().join(".phronesis/packs");
    fs::create_dir_all(&packs).unwrap();
    for n in 0..65 {
        fs::create_dir(packs.join(format!("pack_{n:02}"))).unwrap();
    }
    let inventory = inspect(project.path());
    assert!(!inventory.complete);
    assert!(
        inventory
            .diagnostics
            .iter()
            .any(|d| d.message.contains("limit of 64")),
        "{:?}",
        inventory.diagnostics
    );
    assert!(
        inventory.packs.len() <= 65,
        "one built-in plus 64 installed"
    );
    assert!(phronesis_mcp::language_packs::registry::load(project.path()).is_err());
}

#[cfg(unix)]
#[test]
fn installed_metadata_fifo_and_oversize_are_bounded_and_diagnostic() {
    let project = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    write_pack(src.path(), &manifest("bounded_meta", "boundext"));
    install(project.path(), src.path(), false).unwrap();
    let meta = project
        .path()
        .join(".phronesis/packs/bounded_meta/installed.json");
    fs::write(&meta, vec![b' '; 65 * 1024]).unwrap();
    let oversize = inspect(project.path());
    assert!(!oversize.complete);
    assert!(
        oversize
            .packs
            .iter()
            .any(|p| p.location.ends_with("bounded_meta")
                && p.diagnostics.iter().any(|d| d.message.contains("limit")))
    );
    fs::remove_file(&meta).unwrap();
    let status = Command::new("mkfifo").arg(&meta).status().unwrap();
    assert!(status.success());
    let fifo = inspect(project.path());
    assert!(!fifo.complete);
    assert!(
        fifo.packs
            .iter()
            .any(|p| p.location.ends_with("bounded_meta")
                && p.status == phronesis_mcp::language_packs::registry::PackStatus::Invalid)
    );
    assert!(phronesis_mcp::language_packs::registry::load(project.path()).is_err());
}

#[test]
fn bundled_go_pack_is_discoverable_without_a_source_checkout() {
    let project = tempfile::tempdir().unwrap();
    let inventory = inspect(project.path());
    let go = inventory
        .packs
        .iter()
        .find(|p| p.id.as_deref() == Some("go"))
        .expect("embedded Go descriptor must be listed");
    assert_eq!(
        go.origin,
        phronesis_mcp::language_packs::registry::PackOrigin::BuiltIn
    );
    assert!(go.extensions.iter().any(|ext| ext == "go"));
    #[cfg(all(feature = "go-grammar", feature = "rhai"))]
    assert_eq!(
        go.status,
        phronesis_mcp::language_packs::registry::PackStatus::Valid,
        "{:?}",
        go.diagnostics
    );
    #[cfg(not(all(feature = "go-grammar", feature = "rhai")))]
    assert_eq!(
        go.status,
        phronesis_mcp::language_packs::registry::PackStatus::Unsupported
    );
}

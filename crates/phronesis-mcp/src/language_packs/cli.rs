//! `phr-mcp pack list|validate|install` rendering glue. The MCP
//! `list_language_packs` tool shares `registry::inspect` directly so CLI
//! JSON and the MCP report are the same data, not two implementations.

use std::path::Path;

use super::install::{InstallAction, install, validate_directory};
use super::registry::{PACKS_DIR, inspect};

/// `phr-mcp pack list [--json]`.
pub fn run_list(root: &Path, json_output: bool) {
    let inventory = inspect(root);
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&inventory).expect("PackInventory serializes")
        );
        return;
    }

    if inventory.packs.is_empty() {
        println!("No language packs found (built-in or installed).");
    }
    for pack in &inventory.packs {
        let id = pack.id.as_deref().unwrap_or("<invalid manifest>");
        let version = pack.version.as_deref().unwrap_or("?");
        println!(
            "{id}  {version}  origin={:?} status={:?}  extensions={}",
            pack.origin,
            pack.status,
            pack.extensions.join(",")
        );
        for d in &pack.diagnostics {
            println!("  ! {}", d.message);
        }
    }
    for d in &inventory.diagnostics {
        println!("! {}", d.message);
    }
    if !inventory.complete {
        println!("(enumeration incomplete — see diagnostics above)");
    }
}

/// `phr-mcp pack validate <directory> [--json]`. Returns whether the pack
/// is valid, for the caller to decide an exit code.
pub fn run_validate(dir: &Path, json_output: bool) -> bool {
    match validate_directory(dir) {
        Ok(report) => {
            if json_output {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).expect("ValidationReport serializes")
                );
            } else {
                println!(
                    "{}: {}",
                    report.id.as_deref().unwrap_or("<invalid manifest>"),
                    if report.ok { "valid" } else { "invalid" }
                );
                for d in &report.diagnostics {
                    println!("  ! {}", d.message);
                }
                for f in &report.fixtures {
                    println!("  fixture {}: {}", f.path, if f.ok { "ok" } else { "FAIL" });
                    if let Some(m) = &f.message {
                        println!("    {m}");
                    }
                }
            }
            report.ok
        }
        Err(e) => {
            if json_output {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&e.to_diagnostic()).unwrap()
                );
            } else {
                eprintln!("error: {e}");
            }
            false
        }
    }
}

/// `phr-mcp pack install <directory> [--dry-run]`. Returns whether the
/// install (or dry-run preview) succeeded.
pub fn run_install(root: &Path, dir: &Path, dry_run: bool, json_output: bool) -> bool {
    match install(root, dir, dry_run) {
        Ok(report) => {
            if json_output {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).expect("InstallReport serializes")
                );
                return true;
            }
            let verb = match (report.action, report.dry_run) {
                (_, true) => "would install",
                (InstallAction::Unchanged, false) => "already installed (unchanged)",
                (InstallAction::Installed, false) => "installed",
            };
            println!(
                "{} {} -> {} ({verb})",
                report.id,
                report.digest,
                root.join(PACKS_DIR).join(&report.id).display()
            );
            true
        }
        Err(e) => {
            if json_output {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&e.to_diagnostic()).unwrap()
                );
            } else {
                eprintln!("error: {e}");
            }
            false
        }
    }
}

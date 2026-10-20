//! Repairing a broken installed pack through the hooks.
//!
//! A pack that fails to load fails closed project-wide, like a malformed
//! `rules.json`: its rule layers can apply to any file, so nothing can be
//! evaluated without it. Unlike `rules.json`, editing the pack in place
//! cannot repair it, because the digest recorded at install time treats new
//! bytes as tampering. The repair is the pack CLI, so a load error names the
//! broken pack and admits exactly one kind of command: a lone
//! `phr-mcp pack install|remove|list|validate` invocation.

use std::path::{Path, PathBuf};

use super::PackError;
use super::registry::{PACKS_DIR, PackOrigin, enumerate};

/// The installed pack directory whose load failed, when one can be named.
/// `None` for a failure that is not an installed pack's (a built-in pack
/// failing is a defect in this binary, not something to remove).
pub fn locate_broken_pack(root: &Path, error: &PackError) -> Option<PathBuf> {
    let packs_dir = root.join(PACKS_DIR);
    if let Some(id) = error.pack() {
        let dir = packs_dir.join(id);
        if dir.is_dir() {
            return Some(dir);
        }
    }
    // An unparseable manifest carries no pack id: name the first installed
    // entry that does not load.
    let (entries, _) = enumerate(root);
    entries
        .into_iter()
        .find(|entry| entry.origin == PackOrigin::Installed && entry.assets.is_err())
        .map(|entry| PathBuf::from(entry.location))
}

/// The sentence a load error ends with, naming the repair command.
pub fn repair_hint(location: &Path) -> String {
    let id = location
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!(
        " Installed packs cannot be repaired by editing them (the install digest rejects changed bytes): remove it with `phr-mcp pack remove {id}`, then reinstall from its source with `phr-mcp pack install <dir>`. Until then only the pack CLI is allowed."
    )
}

/// Whether `command` is a lone pack-CLI invocation a pack load error may
/// admit. Deliberately narrow: one command, no shell syntax at all (no
/// chaining, pipes, redirects, substitution, or quoting), the `phr-mcp`
/// binary by name or path, and a pack subcommand that only touches the
/// pack store. Anything else stays blocked.
pub fn is_pack_repair_command(command: &str) -> bool {
    let command = command.trim();
    let safe = |c: char| c.is_ascii_alphanumeric() || " -_./=~:+@,".contains(c);
    if command.is_empty() || !command.chars().all(safe) {
        return false;
    }
    let mut words = command.split_whitespace();
    let binary_ok = words
        .next()
        .and_then(|binary| Path::new(binary).file_name())
        .is_some_and(|name| name == "phr-mcp");
    binary_ok
        && words.next() == Some("pack")
        && matches!(
            words.next(),
            Some("install" | "remove" | "list" | "validate")
        )
}

#[cfg(test)]
mod tests {
    use super::is_pack_repair_command;

    #[test]
    fn only_a_lone_pack_cli_invocation_is_a_repair() {
        for ok in [
            "phr-mcp pack remove demo_a",
            "  phr-mcp pack list --json ",
            "/Users/me/.cargo/bin/phr-mcp pack install ../packs/demo --path .",
            "phr-mcp pack validate ./src",
        ] {
            assert!(is_pack_repair_command(ok), "{ok}");
        }
        for blocked in [
            "",
            "rm -rf .phronesis/packs/demo_a",
            "phr-mcp pack remove demo_a && rm -rf src",
            "phr-mcp pack remove demo_a; rm -rf src",
            "phr-mcp pack remove demo_a | tee x",
            "phr-mcp pack remove $(whoami)",
            "phr-mcp pack remove `id`",
            "phr-mcp pack remove demo_a > out",
            "phr-mcp pack remove 'demo a'",
            "echo phr-mcp pack remove demo_a",
            "phr-mcp-evil pack remove demo_a",
            "phr-mcp init --force",
            "phr-mcp pack",
            "phr-mcp pack remove demo_a\nrm -rf src",
            "env X=1 phr-mcp pack remove demo_a",
        ] {
            assert!(!is_pack_repair_command(blocked), "{blocked:?}");
        }
    }
}

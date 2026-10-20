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
/// admit for the project at `project_root`. Deliberately narrow, because
/// governance is down while it applies:
///
/// - one command and no shell syntax (no chaining, pipes, redirects,
///   substitution, or quoting);
/// - the executable is this hook's own binary: an explicit path, or a bare
///   `phr-mcp` found on an absolute `PATH` entry, must canonicalize to the
///   running executable, so a project-local file named `phr-mcp` is not it;
/// - a pack subcommand with exactly its own arguments, and any `--path`
///   naming this project, so the repair cannot reach another pack store.
pub fn is_pack_repair_command(command: &str, project_root: &Path) -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let path_env = std::env::var_os("PATH").unwrap_or_default();
    is_repair_for(command, project_root, &exe, &path_env)
}

fn is_repair_for(command: &str, root: &Path, exe: &Path, path_env: &std::ffi::OsStr) -> bool {
    let command = command.trim();
    let safe = |c: char| c.is_ascii_alphanumeric() || " -_./=~:+@,".contains(c);
    if command.is_empty() || !command.chars().all(safe) {
        return false;
    }
    let words: Vec<&str> = command.split_whitespace().collect();
    let [binary, "pack", subcommand, args @ ..] = words.as_slice() else {
        return false;
    };
    let Some(trusted) = canonical(exe) else {
        return false;
    };
    let resolved = if binary.contains('/') {
        canonical(&root.join(binary))
    } else if *binary == "phr-mcp" {
        std::env::split_paths(path_env)
            .filter(|dir| dir.is_absolute())
            .map(|dir| dir.join(binary))
            .find(|candidate| candidate.is_file())
            .and_then(|candidate| canonical(&candidate))
    } else {
        None
    };
    if resolved.as_ref() != Some(&trusted) {
        return false;
    }
    let (positionals, flags): (usize, &[&str]) = match *subcommand {
        "list" => (0, &["--json", "--path"]),
        "validate" => (1, &["--json"]),
        "install" => (1, &["--json", "--dry-run", "--path"]),
        "remove" => (1, &["--path"]),
        _ => return false,
    };
    let Some(project) = canonical(root) else {
        return false;
    };
    let mut seen_positionals = 0;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        if !arg.starts_with('-') {
            seen_positionals += 1;
            continue;
        }
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) => (flag, Some(value)),
            None => (*arg, None),
        };
        if !flags.contains(&flag) {
            return false;
        }
        if flag == "--path" {
            let Some(value) = inline.or_else(|| rest.next().copied()) else {
                return false;
            };
            if canonical(&root.join(value)).as_ref() != Some(&project) {
                return false;
            }
        } else if inline.is_some() {
            return false;
        }
    }
    seen_positionals == positionals
}

fn canonical(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok()
}

#[cfg(test)]
mod tests {
    use super::is_repair_for;

    /// A project with a fake "trusted" binary at `<bin>/phr-mcp`, standing in
    /// for the hook's own executable.
    fn setup() -> (tempfile::TempDir, tempfile::TempDir, std::path::PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let exe = bin.path().join("phr-mcp");
        std::fs::write(&exe, b"").unwrap();
        (root, bin, exe)
    }

    #[test]
    fn only_this_binary_running_a_pack_subcommand_on_this_project_is_a_repair() {
        let (root, bin, exe) = setup();
        let root = root.path();
        let path_env = std::ffi::OsString::from(bin.path());
        let exe_s = exe.display().to_string();
        let other = tempfile::tempdir().unwrap();
        let ok = |c: &str| is_repair_for(c, root, &exe, &path_env);
        for c in [
            "phr-mcp pack remove demo_a".to_string(),
            "  phr-mcp pack list --json ".to_string(),
            format!("{exe_s} pack install ../packs/demo --path ."),
            format!("{exe_s} pack remove demo_a --path={}", root.display()),
            "phr-mcp pack validate ./src --json".to_string(),
        ] {
            assert!(ok(&c), "{c}");
        }
        // A project-local file named `phr-mcp` is not this binary.
        std::fs::write(root.join("phr-mcp"), b"#!/bin/sh\necho pwned\n").unwrap();
        for c in [
            "./phr-mcp pack list".to_string(),
            format!("{}/phr-mcp pack list", root.display()),
            // Another project's pack store is out of reach.
            format!(
                "phr-mcp pack remove victim --path {}",
                other.path().display()
            ),
            format!("phr-mcp pack list --path={}", other.path().display()),
            // Wrong arity, unknown or valued boolean flags.
            "phr-mcp pack remove".to_string(),
            "phr-mcp pack remove a b".to_string(),
            "phr-mcp pack remove demo_a --force".to_string(),
            "phr-mcp pack list --json=1".to_string(),
            "phr-mcp pack list --path".to_string(),
            // Shell syntax and other commands.
            "".to_string(),
            "rm -rf .phronesis/packs/demo_a".to_string(),
            "phr-mcp pack remove demo_a && rm -rf src".to_string(),
            "phr-mcp pack remove demo_a; rm -rf src".to_string(),
            "phr-mcp pack remove demo_a | tee x".to_string(),
            "phr-mcp pack remove $(whoami)".to_string(),
            "phr-mcp pack remove `id`".to_string(),
            "phr-mcp pack remove demo_a > out".to_string(),
            "phr-mcp pack remove 'demo a'".to_string(),
            "echo phr-mcp pack remove demo_a".to_string(),
            "phr-mcp-evil pack remove demo_a".to_string(),
            "phr-mcp init --force".to_string(),
            "phr-mcp pack".to_string(),
            "phr-mcp pack remove demo_a\nrm -rf src".to_string(),
            "env X=1 phr-mcp pack remove demo_a".to_string(),
        ] {
            assert!(!ok(&c), "{c:?}");
        }
        // A relative PATH entry never resolves the bare name.
        let relative = std::ffi::OsString::from(".");
        assert!(!is_repair_for("phr-mcp pack list", root, &exe, &relative));
    }
}

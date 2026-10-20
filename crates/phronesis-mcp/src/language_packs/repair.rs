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

/// The sentence a load error ends with, naming the exact repair command:
/// this binary by absolute path and the project by absolute `--path`,
/// which is the only form the hooks admit (see [`is_pack_repair_command`]).
pub fn repair_hint(location: &Path) -> String {
    let id = location
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let exe = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.canonicalize().ok())
        .map(|exe| shell_word(&exe.display().to_string()))
        .unwrap_or_else(|| "phr-mcp".to_string());
    let project = location
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .and_then(|root| root.canonicalize().ok())
        .map(|root| shell_word(&root.display().to_string()))
        .unwrap_or_else(|| ".".to_string());
    format!(
        " Installed packs cannot be repaired by editing them (the install digest rejects changed bytes): remove it with `{exe} pack remove {id} --path {project}`, then reinstall from its source with `{exe} pack install <dir> --path {project}`. Until then only that pack CLI is allowed, with the binary and `--path` written as absolute paths."
    )
}

/// Characters a repair-command word may contain unquoted: none is special
/// to a POSIX shell.
fn is_plain(c: char) -> bool {
    c.is_ascii_alphanumeric() || "-_./=:+@,".contains(c)
}

/// `word` as one shell word: as-is when every character is plain, else
/// single-quoted (inside single quotes the shell expands nothing). A word
/// containing a single quote is left as-is; the check will refuse it, and
/// the human runs the repair from a terminal instead.
fn shell_word(word: &str) -> String {
    if word.chars().all(is_plain) || word.contains('\'') {
        word.to_string()
    } else {
        format!("'{word}'")
    }
}

/// Split `command` into shell words, admitting only plain characters and
/// single-quoted literal text. Anything else a shell would interpret
/// (operators, substitutions, expansions, double quotes, escapes, an
/// unterminated quote, control characters) makes the whole command
/// unparseable: `None`.
fn literal_words(command: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '\'' => break,
                        c if c.is_control() => return None,
                        c => word.push(c),
                    }
                }
            }
            c if is_plain(c) => {
                in_word = true;
                word.push(c);
            }
            _ => return None,
        }
    }
    if in_word {
        words.push(word);
    }
    Some(words)
}

/// Whether `command` is a lone pack-CLI invocation a pack load error may
/// admit for the project at `project_root`. Deliberately narrow, because
/// governance is down while it applies:
///
/// - one command and no shell syntax (no chaining, pipes, redirects,
///   substitution, or quoting);
/// - the executable is written as an absolute path that canonicalizes to
///   this hook's own binary. A bare name or relative path would be resolved
///   by the shell (through `PATH`, from its own working directory), which
///   the hook cannot see, so neither is admitted;
/// - a pack subcommand with exactly its own arguments. The two that write
///   the pack store, `install` and `remove`, must name it with an absolute
///   `--path` that canonicalizes to this project; a relative or omitted one
///   would resolve against the shell's working directory. `list` and
///   `validate` only read.
pub fn is_pack_repair_command(command: &str, project_root: &Path) -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    is_repair_for(command, project_root, &exe)
}

fn is_repair_for(command: &str, root: &Path, exe: &Path) -> bool {
    let Some(words) = literal_words(command) else {
        return false;
    };
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let [binary, "pack", subcommand, args @ ..] = words.as_slice() else {
        return false;
    };
    let Some(trusted) = canonical(exe) else {
        return false;
    };
    let binary = Path::new(binary);
    if !binary.is_absolute() || canonical(binary).as_ref() != Some(&trusted) {
        return false;
    }
    let (positionals, flags, writes): (usize, &[&str], bool) = match *subcommand {
        "list" => (0, &["--json", "--path"], false),
        "validate" => (1, &["--json"], false),
        "install" => (1, &["--json", "--dry-run", "--path"], true),
        "remove" => (1, &["--path"], true),
        _ => return false,
    };
    let Some(project) = canonical(root) else {
        return false;
    };
    let mut seen_positionals = 0;
    let mut named_project = false;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        if !arg.starts_with('-') {
            // `remove` deletes by id, so the id must be a pack identifier.
            if *subcommand == "remove" && !super::is_identifier(arg) {
                return false;
            }
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
            let value = Path::new(value);
            if !value.is_absolute() || canonical(value).as_ref() != Some(&project) {
                return false;
            }
            named_project = true;
        } else if inline.is_some() {
            return false;
        }
    }
    seen_positionals == positionals && (named_project || !writes)
}

fn canonical(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok()
}

#[cfg(test)]
mod tests {
    use super::is_repair_for;

    #[test]
    fn only_this_binary_by_absolute_path_on_this_project_is_a_repair() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let bin = tempfile::tempdir().unwrap();
        let exe = bin.path().join("phr-mcp");
        std::fs::write(&exe, b"").unwrap();
        let exe_s = exe.display().to_string();
        let project = root.canonicalize().unwrap().display().to_string();
        let other = tempfile::tempdir().unwrap();
        let ok = |c: &str| is_repair_for(c, root, &exe);
        for c in [
            format!("{exe_s} pack remove demo_a --path {project}"),
            format!("{exe_s} pack remove demo_a --path={project}"),
            format!("  {exe_s} pack install ../packs/demo --path {project} --json "),
            format!("{exe_s} pack list --json"),
            format!("{exe_s} pack validate ./src --json"),
            // Single-quoted literal words: a path with a space or non-ASCII.
            format!("'{exe_s}' pack list --path '{project}'"),
            format!("{exe_s} pack validate '/tmp/my packs/é'"),
        ] {
            assert!(ok(&c), "{c}");
        }
        for c in [
            // Quoting never hides shell syntax outside the quotes, and an
            // unterminated or double quote is refused.
            format!("{exe_s} pack list '--path' x; rm -rf src"),
            format!("{exe_s} pack validate 'src"),
            format!("{exe_s} pack validate \"src\""),
            format!("{exe_s} pack validate src\\ x"),
            format!("{exe_s} pack remove 'demo_a' --path ~"),
        ] {
            assert!(!ok(&c), "{c:?}");
        }
        std::fs::write(root.join("phr-mcp"), b"#!/bin/sh\necho pwned\n").unwrap();
        for c in [
            // Resolved by the shell, not by the hook: a bare name, relative paths.
            format!("phr-mcp pack remove demo_a --path {project}"),
            format!("./phr-mcp pack remove demo_a --path {project}"),
            format!("{}/phr-mcp pack list", root.display()),
            // A store write must name this project, absolutely.
            format!("{exe_s} pack remove demo_a"),
            format!("{exe_s} pack remove demo_a --path ."),
            format!("{exe_s} pack install ./src --path ."),
            format!(
                "{exe_s} pack remove victim --path {}",
                other.path().display()
            ),
            format!("{exe_s} pack list --path={}", other.path().display()),
            // Wrong arity, unknown or valued boolean flags.
            format!("{exe_s} pack remove --path {project}"),
            format!("{exe_s} pack remove a b --path {project}"),
            format!("{exe_s} pack remove demo_a --force --path {project}"),
            format!("{exe_s} pack list --json=1"),
            format!("{exe_s} pack list --path"),
            // Shell syntax and other commands.
            String::new(),
            "rm -rf .phronesis/packs/demo_a".to_string(),
            format!("{exe_s} pack remove demo_a --path {project} && rm -rf src"),
            format!("{exe_s} pack remove demo_a --path {project}; rm -rf src"),
            format!("{exe_s} pack list | tee x"),
            format!("{exe_s} pack remove $(whoami) --path {project}"),
            format!("{exe_s} pack remove `id` --path {project}"),
            format!("{exe_s} pack list > out"),
            format!("{exe_s} pack remove 'demo a' --path {project}"),
            format!("echo {exe_s} pack list"),
            format!("{exe_s} init --force"),
            format!("{exe_s} pack"),
            format!("{exe_s} pack list\nrm -rf src"),
            format!("env X=1 {exe_s} pack list"),
        ] {
            assert!(!ok(&c), "{c:?}");
        }
    }
}

//! `pyproject.toml` parsing and the on-disk layout probe that names the
//! import packages a Python distribution provides.

use super::Manifest;
use super::cargo::{strip_comment, unquote};
use std::path::Path;

/// Parse the subset of `pyproject.toml` that bears on identity: the
/// distribution name, under PEP 621's `[project]` or Poetry's `[tool.poetry]`.
///
/// No dependency aliases. Python has no rename-on-import at the distribution
/// level — `import x` names the import package directly — so the alias map
/// that Cargo needs has no Python counterpart.
pub fn parse_pyproject_manifest(text: &str) -> Manifest {
    let mut out = Manifest::default();
    let mut section = String::new();
    for raw in text.lines() {
        let line = strip_comment(raw).trim();
        if line.starts_with('[') {
            section = line.trim_matches(['[', ']']).trim().to_string();
            continue;
        }
        if section != "project" && section != "tool.poetry" {
            continue;
        }
        if let Some((key, value)) = line.split_once('=')
            && unquote(key) == "name"
        {
            out.package = Some(unquote(value).to_string());
            // PEP 621 wins over Poetry when a file carries both.
            if section == "project" {
                return out;
            }
        }
    }
    out
}

/// Import packages a Python distribution provides, read from its import
/// root: a directory holding `__init__.py`, or a bare top-level module.
///
/// Read from disk rather than declared, because neither layout states it in
/// the manifest and the distribution name is frequently not the package name.
pub(super) fn top_level_packages(import_root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(import_root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if path.is_dir() && path.join("__init__.py").is_file() {
            out.push(name.to_string());
        } else if let Some(stem) = name.strip_suffix(".py")
            && stem != "__init__"
        {
            out.push(stem.to_string());
        }
    }
    out.sort();
    out
}

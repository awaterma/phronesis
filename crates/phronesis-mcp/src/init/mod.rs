//! `phr-mcp init` — one-command setup for a project.
//!
//! Writes (or merges) the config files phr-mcp needs:
//!
//! - `.claude/settings.local.json`             — hook registrations
//! - `.mcp.json`                                — MCP server registration
//! - `.gemini/settings.json`                    — Gemini CLI MCP + hooks
//! - `.phronesis/rules.json`                    — starter rule pack
//! - `.phronesis/durable.md`                    — re-injected directives
//! - `.phronesis/wiki/decisions/README.md`     — ADR scaffold
//! - `.gitignore`                                — log/backup paths + wiki carveout
//!
//! Idempotent and non-destructive by default. Existing permissions, hooks,
//! and MCP servers are preserved; only our entries are added or refreshed.
//! Existing rules files are left alone unless `--rules-only` or `--force` is set.

use std::path::{Path, PathBuf};

mod rule_sync;
use rule_sync::write_rules_file;

mod types;
pub use types::{BASE_PACKS, InitError, InitOpts, InitReport, Pack, compose_packs, parse_packs};

mod global_install;
pub use global_install::{
    install_globally, install_globally_with_home, uninstall_globally, uninstall_globally_with_home,
    user_claude_config_path, user_gemini_config_path,
};

mod json_helpers;
mod rules_core;
mod rules_other;
mod rules_python;
mod rules_rust;
mod writers_hooks;
mod writers_scaffold;

pub(crate) use writers_scaffold::DEFAULT_DURABLE_MD;

use writers_hooks::*;
use writers_scaffold::*;

/// Run the installer. Returns a structured report; the CLI layer prints it.
pub fn run(opts: InitOpts) -> Result<InitReport, InitError> {
    let root = canonicalize_root(&opts.project_root)?;
    let mut report = InitReport::default();

    if !binary_on_path("phr-mcp") {
        report.warnings.push(
            "`phr-mcp` not found on PATH. Hooks won't function until it is. \
             Install via `cargo install --path .` from the phr-mcp repo."
                .to_string(),
        );
    }

    if !opts.rules_only {
        write_settings(&root, &opts, &mut report)?;
        write_mcp_json(&root, &opts, &mut report)?;
        write_gemini_settings(&root, &opts, &mut report)?;
        write_codex_hooks(&root, &opts, &mut report)?;
        write_codex_config(&root, &opts, &mut report)?;
    }
    if !opts.hooks_only {
        write_rules_file(&root, &opts, &mut report)?;
        write_durable_md(&root, &opts, &mut report)?;
        write_context_scaffold(&root, &opts, &mut report)?;
        write_wiki_scaffold(&root, &opts, &mut report)?;
        write_confidence_scaffold(&root, &opts, &mut report)?;
        write_language_pack_toolchains(&root, &opts, &mut report)?;
        write_journey_scaffold(&root, &opts, &mut report)?;
        build_structural_graph(&root, &opts, &mut report);
    }
    if !opts.rules_only && !opts.hooks_only {
        update_gitignore(&root, &opts, &mut report)?;
    }

    Ok(report)
}

// ─────────────────────────────────────────────────────────────────────
// Path + binary checks
// ─────────────────────────────────────────────────────────────────────

fn canonicalize_root(p: &Path) -> Result<PathBuf, InitError> {
    if !p.exists() {
        return Err(InitError::NoSuchPath(p.display().to_string()));
    }
    if !p.is_dir() {
        return Err(InitError::NotADirectory(p.display().to_string()));
    }
    p.canonicalize().map_err(|e| InitError::Io {
        path: p.display().to_string(),
        source: e,
    })
}

pub(super) fn binary_on_path(name: &str) -> bool {
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    path.split(if cfg!(windows) { ';' } else { ':' })
        .any(|dir| Path::new(dir).join(name).is_file())
}

#[cfg(test)]
mod tests;

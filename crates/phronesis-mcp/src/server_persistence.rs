//! Persistence helpers for `EpistemeMcp`: autoload on startup and
//! autosave after mutating tool calls.
//!
//! Lives in a separate `impl EpistemeMcp { ... }` block so `server.rs`
//! stays focused on the MCP tool surface itself. The split is purely
//! organizational — these methods could be in either file; we put them
//! here so the file-LOC audit isn't dominated by serialization plumbing.

use std::collections::{HashMap, HashSet};

use phr::{ReteError, ReteNetwork};
use rmcp::ErrorData as McpError;

use crate::rules_file::{self, DiskRule};
use crate::security;
use crate::server::EpistemeMcp;

/// Load rules from a disk slice into `network` + `phase_map`, skipping any
/// whose ID already appears in `existing_ids`.  Returns `(loaded, skipped)`.
///
/// Callers that want best-effort semantics (e.g. `autoload`) should pass an
/// empty `existing_ids` set and ignore the `Result` with `let _ = ...`.
/// Callers that need per-rule error propagation (e.g. `load_rules_file`)
/// should propagate via `?`.
pub(crate) async fn hydrate_rules(
    network: &ReteNetwork,
    phase_map: &mut HashMap<String, String>,
    rules: &[DiskRule],
    existing_ids: &HashSet<String>,
) -> Result<(usize, usize), ReteError> {
    let mut loaded = 0usize;
    let mut skipped = 0usize;
    for disk in rules {
        if existing_ids.contains(&disk.id) {
            skipped += 1;
            continue;
        }
        let (rule, phase) = rules_file::rule_from_disk(disk);
        let id = rule.id.clone();
        network.add_rule(rule).await?;
        phase_map.insert(id, phase);
        loaded += 1;
    }
    Ok((loaded, skipped))
}

impl EpistemeMcp {
    /// Hydrate the in-memory network from `.phronesis/rules.json` at startup.
    ///
    /// Best-effort: silently returns when no rules file exists, or if it
    /// exists but is malformed (the hook surfaces malformed-rules errors
    /// with the right exit code; failing startup here would prevent the
    /// user from ever inspecting the broken file via MCP tools).
    pub async fn autoload(&self) {
        if Self::autopersist_disabled() {
            return;
        }
        let path = {
            let root = security::project_root();
            rules_file::default_path(&root)
        };
        let file = match rules_file::read(&path) {
            Ok(f) => f,
            Err(_) => return,
        };
        let network = self.network.lock().await;
        let mut phase_map = self.phase_map.lock().await;
        // Best-effort: bail on the first add_rule error and discard it;
        // rules before the failure stay loaded. (Unreachable in practice —
        // save_rules dedups ids before writing.)
        let _ = hydrate_rules(&network, &mut phase_map, &file.rules, &HashSet::new()).await;
    }

    /// Persist the current in-memory rules to `.phronesis/rules.json` as
    /// a full replace. Called automatically at the end of `add_rule`,
    /// `extract_rules`, and `remove_rule` so the hook (which reads disk
    /// on every invocation) sees changes within milliseconds.
    ///
    /// Replaces rather than merges: with `autoload` at startup, the
    /// in-memory network already contains everything that was on disk,
    /// so in-memory is authoritative. This also makes `remove_rule`
    /// actually remove from disk. The explicit `save_rules` tool still
    /// supports merge semantics for callers who want them.
    ///
    /// Honors `PHRONESIS_NO_AUTOPERSIST=1` for tests and
    /// explicit-control workflows.
    pub(crate) async fn autosave(&self) -> Result<(), McpError> {
        if Self::autopersist_disabled() {
            return Ok(());
        }
        let root = security::project_root();
        let path = rules_file::default_path(&root);

        let network = self.network.lock().await;
        let in_memory = network.get_all_rules().map_err(Self::err)?;
        drop(network);

        let phase_map = self.phase_map.lock().await.clone();
        let disk_rules: Vec<rules_file::DiskRule> = in_memory
            .iter()
            .map(|rule| {
                let phase = phase_map
                    .get(&rule.id)
                    .cloned()
                    .unwrap_or_else(|| "pre".to_string());
                rules_file::rule_to_disk(rule, &phase)
            })
            .collect();

        rules_file::write_atomic(&path, &rules_file::RulesFile { rules: disk_rules })
            .map_err(|e| Self::err(e.to_string()))?;
        Ok(())
    }
}

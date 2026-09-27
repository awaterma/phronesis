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

/// Reconcile an explicit disk reload by replacing same-ID rules in memory.
pub(crate) async fn reconcile_rules(
    network: &ReteNetwork,
    phase_map: &mut HashMap<String, String>,
    rules: &[DiskRule],
) -> Result<(usize, usize), ReteError> {
    let mut loaded = 0;
    let mut replaced = 0;
    for disk in rules {
        let (rule, phase) = rules_file::rule_from_disk(disk);
        if network.get_rule_by_id(&rule.id)?.is_some() {
            network.remove_rule(&rule.id)?;
            replaced += 1;
        }
        let id = rule.id.clone();
        network.add_rule(rule).await?;
        phase_map.insert(id, phase);
        loaded += 1;
    }
    Ok((loaded, replaced))
}

impl EpistemeMcp {
    /// Hydrate the in-memory network from `.phronesis/rules.json` at startup.
    ///
    /// Never fails startup: a server that refused to start could not be used
    /// to inspect the broken file. When the rules on disk do not load, the
    /// error is recorded instead; `list_rules` reports it and every
    /// rule-writing tool refuses until the file loads (see
    /// [`Self::ensure_disk_rules_load`]). Without that, autosave would
    /// replace the unloadable file with only the rules added since startup.
    pub async fn autoload(&self) {
        let root = security::project_root();
        match crate::rule_layers::resolve(&root) {
            // With autopersist disabled nothing is loaded, but a load error
            // is still recorded so `list_rules` can explain it.
            Ok(_) if Self::autopersist_disabled() => {}
            Ok(resolved) => {
                // Best-effort: an add_rule error leaves the rules before it
                // loaded. (Unreachable in practice — the loader rejects
                // duplicate ids.)
                let _ = self.hydrate_from(&root, resolved).await;
            }
            Err(error) => *self.disk_load_error.lock().await = Some(error.to_string()),
        }
    }

    /// Add the resolved on-disk rules the network does not hold yet, and
    /// record which ids autosave owns and which project rules are shadowed.
    async fn hydrate_from(
        &self,
        root: &std::path::Path,
        resolved: crate::rule_layers::ResolvedRules,
    ) -> Result<(), ReteError> {
        let project_path = rules_file::default_path(root);
        let network = self.network.lock().await;
        let existing: HashSet<String> = network
            .get_all_rules()?
            .into_iter()
            .map(|rule| rule.id)
            .collect();
        let mut phase_map = self.phase_map.lock().await;
        hydrate_rules(&network, &mut phase_map, &resolved.rules, &existing).await?;
        for fact in crate::rule_layers::override_facts(&resolved.overrides) {
            network.assert_fact(fact).await?;
        }
        drop(phase_map);
        drop(network);
        self.persistent_rule_ids.lock().await.extend(
            resolved
                .origins
                .iter()
                .filter_map(|(id, origin)| (origin.path == project_path).then_some(id.clone())),
        );
        // `resolve` succeeded, so the project file loads too.
        let project_file =
            rules_file::read(&project_path).unwrap_or(rules_file::RulesFile { rules: Vec::new() });
        let shadowed = project_file.rules.into_iter().filter(|rule| {
            resolved
                .origins
                .get(&rule.id)
                .is_some_and(|origin| origin.path != project_path)
        });
        self.shadowed_project_rules
            .lock()
            .await
            .extend(shadowed.map(|rule| (rule.id.clone(), rule)));
        Ok(())
    }

    /// Replace every rule the server holds with the repaired file's rules.
    ///
    /// While the file did not load, every rule-writing tool was refused, so
    /// each rule in the network came from disk (at startup, or before the
    /// file broke) and may be stale. The repaired file is authoritative:
    /// changed rules come from it and rules deleted from it stay deleted.
    pub(crate) async fn reload_from(
        &self,
        root: &std::path::Path,
        resolved: crate::rule_layers::ResolvedRules,
    ) -> Result<(), ReteError> {
        {
            let network = self.network.lock().await;
            for rule in network.get_all_rules()? {
                network.remove_rule(&rule.id)?;
            }
            for fact in
                network.facts_matching_predicates(&[crate::rule_layers::OVERRIDE_PREDICATE])?
            {
                network.retract_fact(&fact.id).await?;
            }
        }
        self.phase_map.lock().await.clear();
        self.persistent_rule_ids.lock().await.clear();
        self.shadowed_project_rules.lock().await.clear();
        self.hydrate_from(root, resolved).await
    }

    /// Refuse a rule-writing call while the rules on disk do not load.
    ///
    /// The network then does not hold the user's rules, and every write path
    /// (autosave's full replace, `save_rules`) would overwrite the file with a
    /// partial set — and rotate the last good copy out of `.bak`. When a file
    /// that failed earlier loads again, its rules are hydrated first so the
    /// next write keeps them.
    pub(crate) async fn ensure_disk_rules_load(&self) -> Result<(), McpError> {
        let root = security::project_root();
        match crate::rule_layers::resolve(&root) {
            Err(error) => {
                let message = error.to_string();
                *self.disk_load_error.lock().await = Some(message.clone());
                Err(Self::err(format!(
                    "refusing to change rules: the rules on disk do not load, and writing now \
                     would replace them with the partial set this server holds. Fix {} first \
                     (the hooks block every tool call until it loads, but allow edits to it): \
                     {message}",
                    error.failing_file(&root).display()
                )))
            }
            Ok(resolved) => {
                let recovered = self.disk_load_error.lock().await.take().is_some();
                if recovered && !Self::autopersist_disabled() {
                    self.reload_from(&root, resolved).await.map_err(Self::err)?;
                }
                Ok(())
            }
        }
    }

    /// [`Self::ensure_disk_rules_load`] for tools that write only through
    /// autosave: with autopersist disabled they never touch disk.
    pub(crate) async fn ensure_autosave_safe(&self) -> Result<(), McpError> {
        if Self::autopersist_disabled() {
            return Ok(());
        }
        self.ensure_disk_rules_load().await
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
        let persistent_rule_ids = self.persistent_rule_ids.lock().await.clone();
        let mut disk_rules: Vec<rules_file::DiskRule> = self
            .shadowed_project_rules
            .lock()
            .await
            .values()
            .cloned()
            .collect();
        disk_rules.sort_by(|left, right| left.id.cmp(&right.id));
        disk_rules.extend(
            in_memory
                .iter()
                .filter(|rule| persistent_rule_ids.contains(&rule.id))
                .map(|rule| {
                    let phase = phase_map
                        .get(&rule.id)
                        .cloned()
                        .unwrap_or_else(|| "pre".to_string());
                    rules_file::rule_to_disk(rule, &phase)
                }),
        );

        rules_file::write_atomic(&path, &rules_file::RulesFile { rules: disk_rules })
            .map_err(|e| Self::err(e.to_string()))?;
        Ok(())
    }
}

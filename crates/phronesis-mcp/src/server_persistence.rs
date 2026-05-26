//! Persistence helpers for `EpistemeMcp`: autoload on startup and
//! autosave after mutating tool calls.
//!
//! Lives in a separate `impl EpistemeMcp { ... }` block so `server.rs`
//! stays focused on the MCP tool surface itself. The split is purely
//! organizational — these methods could be in either file; we put them
//! here so the file-LOC audit isn't dominated by serialization plumbing.

use rmcp::ErrorData as McpError;

use crate::rules_file;
use crate::security;
use crate::server::EpistemeMcp;

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
        let root = security::project_root();
        let path = rules_file::default_path(&root);
        let file = match rules_file::read(&path) {
            Ok(f) => f,
            Err(_) => return,
        };
        let network = self.network.lock().await;
        let mut phase_map = self.phase_map.lock().await;
        for disk in &file.rules {
            let (rule, phase) = rules_file::rule_from_disk(disk);
            let id = rule.id.clone();
            // Ignore duplicate-id errors etc. — best-effort hydration.
            let _ = network.add_rule(rule).await;
            phase_map.insert(id, phase);
        }
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

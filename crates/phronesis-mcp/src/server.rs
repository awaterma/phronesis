//! MCP server: the full tool surface phronesis exposes to MCP clients
//! (Claude Code, Gemini CLI, and any other MCP-capable host).
//!
//! This file is intentionally large. The `#[tool_router]` macro from
//! `rmcp` requires the `#[tool]` methods to live in a single `impl`
//! block, and splitting that block would scatter ~25 related tool
//! implementations across files for no real cohesion gain — the
//! grouping that matters here is "MCP tool surface", and grouping by
//! that lives in one place. Parameter types live in `server_params.rs`
//! and persistence helpers in `server_persistence.rs`.
//!
//! phronesis-allow: audit-file-loc-high (coherent MCP tool surface)

use std::collections::HashMap;
use std::sync::Arc;

use phr::{
    Action, Condition, Consequence, ConsequenceKind, Fact, LookupRegistry, ReteNetwork, Rule,
    rule_firing_to_consequences,
};
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::{ErrorData as McpError, ServerHandler, tool, tool_handler, tool_router};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::action_log::{self, LogEntry};
use crate::rules_file;
use crate::security::{
    self, MAX_CONSEQUENCES, MAX_FACTS, MAX_RULES, require_extension, resolve_safe_path,
    validate_args, validate_string,
};

#[derive(Clone)]
pub struct EpistemeMcp {
    pub(crate) network: Arc<Mutex<ReteNetwork>>,
    /// The lookup-tool registry is plumbed through the server but not yet
    /// exposed via an MCP tool — the current MCP surface only addresses rules
    /// and facts. Kept here so a future `invoke_tool` MCP method can dispatch
    /// against it without restructuring the server's state.
    #[allow(dead_code)]
    pub(crate) registry: Arc<Mutex<LookupRegistry>>,
    pub(crate) consequences: Arc<Mutex<Vec<Consequence>>>,
    /// Side-channel map from rule_id → phase ("pre"/"post"). Populated when
    /// rules are added or loaded; consulted on save so the round-trip is
    /// lossless. The phronesis `Rule` struct itself has no phase concept.
    pub(crate) phase_map: Arc<Mutex<HashMap<String, String>>>,
    tool_router: ToolRouter<Self>,
}

impl Default for EpistemeMcp {
    fn default() -> Self {
        Self::new()
    }
}

impl EpistemeMcp {
    pub fn new() -> Self {
        Self {
            network: Arc::new(Mutex::new(ReteNetwork::new())),
            registry: Arc::new(Mutex::new(LookupRegistry::new())),
            consequences: Arc::new(Mutex::new(Vec::new())),
            phase_map: Arc::new(Mutex::new(HashMap::new())),
            tool_router: Self::tool_router(),
        }
    }

    fn ok_text(text: String) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    pub(crate) fn err(msg: String) -> McpError {
        McpError::new(ErrorCode(-1), msg, None::<serde_json::Value>)
    }

    fn validate_rule_params(params: &AddRuleParams) -> Result<(), security::SecurityError> {
        validate_string(&params.id, "rule.id")?;
        for c in &params.conditions {
            validate_string(&c.predicate, "condition.predicate")?;
            validate_args(&c.args, "condition.args")?;
            if let Some(s) = &c.script {
                validate_string(s, "condition.script")?;
            }
        }
        for a in &params.actions {
            validate_string(&a.action_type, "action.action_type")?;
            validate_args(&a.params, "action.params")?;
        }
        Ok(())
    }

    fn validate_fact_params(params: &AssertFactParams) -> Result<(), security::SecurityError> {
        validate_string(&params.id, "fact.id")?;
        validate_string(&params.predicate, "fact.predicate")?;
        validate_args(&params.args, "fact.args")?;
        Ok(())
    }

    /// Returns true when `PHRONESIS_NO_AUTOPERSIST=1` is set. Tests of the
    /// explicit `save_rules` / `load_rules_file` tools set this to keep
    /// autoload/autosave from interfering with their isolation assertions.
    pub(crate) fn autopersist_disabled() -> bool {
        std::env::var("PHRONESIS_NO_AUTOPERSIST").is_ok()
    }

    /// Append an MCP-side event to `.phronesis/log.jsonl`. Best-effort: log
    /// failures are intentionally swallowed because we never want logging
    /// to alter the MCP tool's return value.
    fn log_event(event: &str, build: impl FnOnce(LogEntry) -> LogEntry) {
        let entry = build(LogEntry::new("mcp", event));
        let path = action_log::default_path(&security::project_root());
        let _ = action_log::append(&path, &entry);
    }
}

// Persistence helpers (autoload, autosave) live in server_persistence.rs.

// --- Input types for tools ---

// Parameter types extracted to server_params.rs to keep server.rs
// focused on the tool surface itself. Re-exported here so callers
// referencing crate::server::AddRuleParams etc. still resolve.
pub use crate::server_params::*;

// --- Tool implementations ---

#[tool_router]
impl EpistemeMcp {
    // ── Rules Management ──

    #[tool(description = "Add a rule to the RETE network")]
    async fn add_rule(
        &self,
        Parameters(params): Parameters<AddRuleParams>,
    ) -> Result<CallToolResult, McpError> {
        Self::validate_rule_params(&params).map_err(|e| Self::err(e.to_string()))?;

        // Only record a phase when the caller explicitly set one. Leaving the
        // entry absent lets `save_rules`'s `phase` argument supply the default.
        let explicit_phase = match params.phase.as_deref() {
            None => None,
            Some("pre") | Some("post") => params.phase.clone(),
            Some(other) => {
                return Err(Self::err(format!(
                    "phase must be \"pre\" or \"post\", got: {}",
                    other
                )));
            }
        };

        let network = self.network.lock().await;
        let existing = network.get_all_rules().map_err(Self::err)?.len();
        if existing >= MAX_RULES {
            return Err(Self::err(format!(
                "rule limit reached: {} (max {})",
                existing, MAX_RULES
            )));
        }

        let rule = Rule {
            id: params.id.clone(),
            priority: params.priority,
            conditions: params
                .conditions
                .into_iter()
                .map(|c| Condition {
                    predicate: c.predicate,
                    args: c.args,
                    script: c.script,
                })
                .collect(),
            actions: params
                .actions
                .into_iter()
                .map(|a| Action {
                    action_type: a.action_type,
                    params: a.params,
                })
                .collect(),
        };

        network.add_rule(rule).await.map_err(Self::err)?;
        drop(network);
        let phase_for_log = explicit_phase.clone().unwrap_or_else(|| "pre".to_string());
        if let Some(phase) = explicit_phase {
            self.phase_map.lock().await.insert(params.id.clone(), phase);
        }
        self.autosave().await?;
        Self::log_event("add_rule", |e| {
            e.with("rule_id", params.id.clone())
                .with("priority", params.priority)
                .with("phase", phase_for_log)
        });
        Self::ok_text(format!("Rule '{}' added", params.id))
    }

    #[tool(description = "List all rules currently loaded in the RETE network")]
    async fn list_rules(&self) -> Result<CallToolResult, McpError> {
        let network = self.network.lock().await;
        let rules = network.get_all_rules().map_err(Self::err)?;
        let json = serde_json::to_string_pretty(&rules).map_err(|e| Self::err(e.to_string()))?;
        Self::ok_text(json)
    }

    #[tool(description = "Get a specific rule by its ID")]
    async fn get_rule(
        &self,
        Parameters(params): Parameters<RuleIdParam>,
    ) -> Result<CallToolResult, McpError> {
        let network = self.network.lock().await;
        match network
            .get_rule_by_id(params.rule_id.as_str())
            .map_err(Self::err)?
        {
            Some(rule) => {
                let json =
                    serde_json::to_string_pretty(&rule).map_err(|e| Self::err(e.to_string()))?;
                Self::ok_text(json)
            }
            None => Self::ok_text(format!("Rule '{}' not found", params.rule_id)),
        }
    }

    #[tool(description = "Remove a rule from the RETE network by its ID")]
    async fn remove_rule(
        &self,
        Parameters(params): Parameters<RuleIdParam>,
    ) -> Result<CallToolResult, McpError> {
        let network = self.network.lock().await;
        network
            .remove_rule(params.rule_id.as_str())
            .map_err(Self::err)?;
        drop(network);
        self.phase_map.lock().await.remove(params.rule_id.as_str());
        self.autosave().await?;
        Self::log_event("remove_rule", |e| {
            e.with("rule_id", params.rule_id.as_str().to_string())
        });
        Self::ok_text(format!("Rule '{}' removed", params.rule_id))
    }

    // ── Facts Management ──

    #[tool(description = "Assert a fact into working memory")]
    async fn assert_fact(
        &self,
        Parameters(params): Parameters<AssertFactParams>,
    ) -> Result<CallToolResult, McpError> {
        Self::validate_fact_params(&params).map_err(|e| Self::err(e.to_string()))?;

        let network = self.network.lock().await;
        let existing = network.get_all_wmes().await.map_err(Self::err)?.len();
        if existing >= MAX_FACTS {
            return Err(Self::err(format!(
                "fact limit reached: {} (max {})",
                existing, MAX_FACTS
            )));
        }

        let fact = Fact {
            id: params.id.clone(),
            predicate: params.predicate,
            args: params.args,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        };
        network.assert_fact(fact).await.map_err(Self::err)?;
        Self::ok_text(format!("Fact '{}' asserted", params.id))
    }

    #[tool(description = "Retract a fact from working memory")]
    async fn retract_fact(
        &self,
        Parameters(params): Parameters<FactIdParam>,
    ) -> Result<CallToolResult, McpError> {
        let network = self.network.lock().await;
        let wme = network
            .retract_fact(params.fact_id.as_str())
            .await
            .map_err(Self::err)?;
        let json = serde_json::to_string_pretty(&wme.fact).map_err(|e| Self::err(e.to_string()))?;
        Self::ok_text(format!("Retracted: {}", json))
    }

    #[tool(description = "List all facts in working memory, optionally filtered by predicate")]
    async fn list_facts(
        &self,
        Parameters(params): Parameters<PredicateFilter>,
    ) -> Result<CallToolResult, McpError> {
        let network = self.network.lock().await;
        let wmes = network.get_all_wmes().await.map_err(Self::err)?;
        let facts: Vec<_> = wmes
            .iter()
            .filter(|wme| {
                params
                    .predicate
                    .as_ref()
                    .is_none_or(|p| wme.fact.predicate == *p)
            })
            .map(|wme| &wme.fact)
            .collect();
        let json = serde_json::to_string_pretty(&facts).map_err(|e| Self::err(e.to_string()))?;
        Self::ok_text(json)
    }

    #[tool(description = "Get a specific fact by its ID")]
    async fn get_fact(
        &self,
        Parameters(params): Parameters<FactIdParam>,
    ) -> Result<CallToolResult, McpError> {
        let network = self.network.lock().await;
        let wmes = network.get_all_wmes().await.map_err(Self::err)?;
        match wmes
            .iter()
            .find(|wme| wme.fact.id == params.fact_id.as_str())
        {
            Some(wme) => {
                let json = serde_json::to_string_pretty(&wme.fact)
                    .map_err(|e| Self::err(e.to_string()))?;
                Self::ok_text(json)
            }
            None => Self::ok_text(format!("Fact '{}' not found", params.fact_id)),
        }
    }

    // ── Execution ──

    #[tool(
        description = "Execute all pending agenda items, returning fired actions and generated consequences"
    )]
    async fn fire_rules(&self) -> Result<CallToolResult, McpError> {
        let network = self.network.lock().await;
        let actions = network.execute_all_agenda_items().map_err(Self::err)?;

        let new_consequences =
            rule_firing_to_consequences("fire_rules", &[], ConsequenceKind::Event, actions.clone());

        let mut consequences = self.consequences.lock().await;
        consequences.extend(new_consequences.clone());
        // FIFO eviction to discard against unbounded accumulation
        if consequences.len() > MAX_CONSEQUENCES {
            let excess = consequences.len() - MAX_CONSEQUENCES;
            consequences.drain(0..excess);
        }

        let actions_fired = actions.len();
        let consequences_generated = new_consequences.len();
        let action_types: Vec<String> = {
            let mut seen = std::collections::HashSet::new();
            actions
                .iter()
                .filter_map(|a| {
                    if seen.insert(a.action_type.clone()) {
                        Some(a.action_type.clone())
                    } else {
                        None
                    }
                })
                .collect()
        };

        let result = serde_json::json!({
            "actions_fired": actions_fired,
            "actions": actions,
            "consequences_generated": consequences_generated,
            "consequences": new_consequences,
        });
        let json = serde_json::to_string_pretty(&result).map_err(|e| Self::err(e.to_string()))?;
        Self::log_event("fire_rules", |e| {
            e.with("actions_fired", actions_fired)
                .with("consequences_generated", consequences_generated)
                .with("action_types", action_types)
        });
        Self::ok_text(json)
    }

    #[tool(description = "Peek at the current agenda (pending rule activations)")]
    async fn get_agenda(&self) -> Result<CallToolResult, McpError> {
        let network = self.network.lock().await;
        let agenda = network
            .agenda
            .lock()
            .map_err(|e| Self::err(e.to_string()))?;
        let items = agenda.get_all_items();
        let summaries: Vec<_> = items
            .iter()
            .map(|item| {
                serde_json::json!({
                    "rule_id": item.rule.id,
                    "salience": item.salience,
                    "wme_count": item.wme_list.len(),
                    "bindings": item.bindings.bindings,
                })
            })
            .collect();
        let json =
            serde_json::to_string_pretty(&summaries).map_err(|e| Self::err(e.to_string()))?;
        Self::ok_text(json)
    }

    #[tool(
        description = "Check for active Constraint-type consequences, optionally filtered by predicate"
    )]
    async fn check_constraints(
        &self,
        Parameters(params): Parameters<PredicateFilter>,
    ) -> Result<CallToolResult, McpError> {
        let consequences = self.consequences.lock().await;
        let constraints: Vec<_> = consequences
            .iter()
            .filter(|c| c.kind == ConsequenceKind::Constraint)
            .filter(|c| {
                params
                    .predicate
                    .as_ref()
                    .is_none_or(|p| c.predicate.contains(p.as_str()))
            })
            .collect();
        if constraints.is_empty() {
            Self::ok_text("No constraint violations".to_string())
        } else {
            let json =
                serde_json::to_string_pretty(&constraints).map_err(|e| Self::err(e.to_string()))?;
            Self::ok_text(format!(
                "{} constraint(s) active:\n{}",
                constraints.len(),
                json
            ))
        }
    }

    // ── Consequence Query ──

    #[tool(
        description = "Get accumulated consequences, optionally filtered by kind (event, snapshot, constraint, affordance)"
    )]
    async fn get_consequences(
        &self,
        Parameters(params): Parameters<KindFilter>,
    ) -> Result<CallToolResult, McpError> {
        let consequences = self.consequences.lock().await;
        let filtered: Vec<_> = consequences
            .iter()
            .filter(|c| {
                params.kind.as_ref().is_none_or(|k| {
                    let k_lower = k.to_lowercase();
                    match c.kind {
                        ConsequenceKind::Event => k_lower == "event",
                        ConsequenceKind::Snapshot => k_lower == "snapshot",
                        ConsequenceKind::Constraint => k_lower == "constraint",
                        ConsequenceKind::Affordance => k_lower == "affordance",
                    }
                })
            })
            .collect();
        let json = serde_json::to_string_pretty(&filtered).map_err(|e| Self::err(e.to_string()))?;
        Self::ok_text(json)
    }

    // ── Rules Extraction ──

    #[tool(
        description = "Submit a markdown file from the project for rules extraction. Parses the file for enforceable patterns and constraints, converts them to phronesis Rules, and loads them into the RETE network. Path must resolve inside the project root."
    )]
    async fn extract_rules(
        &self,
        Parameters(params): Parameters<FilePathParam>,
    ) -> Result<CallToolResult, McpError> {
        validate_string(&params.file_path, "file_path").map_err(|e| Self::err(e.to_string()))?;

        let root = security::project_root();
        let safe_path = resolve_safe_path(&params.file_path, &root)
            .map_err(|e| Self::err(format!("path rejected: {}", e)))?;
        require_extension(&safe_path, "md").map_err(|e| Self::err(e.to_string()))?;

        let content =
            security::read_file_capped(&safe_path).map_err(|e| Self::err(e.to_string()))?;

        let rules = extract_rules_from_markdown(&content, &params.file_path);
        let count = rules.len();

        let network = self.network.lock().await;
        let existing = network.get_all_rules().map_err(Self::err)?.len();
        if existing + count > MAX_RULES {
            return Err(Self::err(format!(
                "extracting would exceed rule limit: {} + {} > {}",
                existing, count, MAX_RULES
            )));
        }

        for rule in &rules {
            network.add_rule(rule.clone()).await.map_err(Self::err)?;
        }
        drop(network);
        // Extracted rules have no inherent phase; autosave defaults to "pre".
        self.autosave().await?;
        Self::log_event("extract_rules", |e| {
            e.with("source_file", params.file_path.clone())
                .with("rules_count", count)
        });

        let json = serde_json::to_string_pretty(&rules).map_err(|e| Self::err(e.to_string()))?;
        Self::ok_text(format!(
            "Extracted {} rule(s) from '{}':\n{}",
            count, params.file_path, json
        ))
    }

    #[tool(
        description = "Persist the current in-memory rules to .phronesis/rules.json. By default merges with the existing file (rules with matching IDs are updated; new rules append; disk-only rules are preserved). Use dry_run to preview without writing."
    )]
    async fn save_rules(
        &self,
        Parameters(params): Parameters<SaveRulesParams>,
    ) -> Result<CallToolResult, McpError> {
        let default_phase = params.phase.as_deref().unwrap_or("pre");
        if default_phase != "pre" && default_phase != "post" {
            return Err(Self::err(format!(
                "phase must be \"pre\" or \"post\", got: {}",
                default_phase
            )));
        }

        let root = security::project_root();
        let path = rules_file::default_path(&root);

        let existing = if params.merge {
            rules_file::read(&path).map_err(|e| Self::err(e.to_string()))?
        } else {
            rules_file::RulesFile { rules: vec![] }
        };

        let network = self.network.lock().await;
        let in_memory = network.get_all_rules().map_err(Self::err)?;
        drop(network);

        let phase_map = self.phase_map.lock().await.clone();
        let result = rules_file::merge(&existing, &in_memory, &phase_map, default_phase);

        let summary = serde_json::json!({
            "path": path.display().to_string(),
            "added": result.added,
            "updated": result.updated,
            "preserved": result.preserved,
            "total": result.merged.rules.len(),
            "dry_run": params.dry_run,
        });

        if !params.dry_run {
            rules_file::write_atomic(&path, &result.merged)
                .map_err(|e| Self::err(e.to_string()))?;
        }

        let json = serde_json::to_string_pretty(&summary).map_err(|e| Self::err(e.to_string()))?;
        Self::ok_text(json)
    }

    #[tool(
        description = "Hydrate the in-memory network from .phronesis/rules.json. Adds each rule (preserving its phase) and skips rules whose ID already exists."
    )]
    async fn load_rules_file(
        &self,
        Parameters(_): Parameters<LoadRulesFileParams>,
    ) -> Result<CallToolResult, McpError> {
        let root = security::project_root();
        let path = rules_file::default_path(&root);
        let file = rules_file::read(&path).map_err(|e| Self::err(e.to_string()))?;

        let network = self.network.lock().await;
        let existing_ids: std::collections::HashSet<String> = network
            .get_all_rules()
            .map_err(Self::err)?
            .into_iter()
            .map(|r| r.id)
            .collect();

        let mut loaded = 0usize;
        let mut skipped = 0usize;
        let mut phase_map = self.phase_map.lock().await;
        for disk in &file.rules {
            if existing_ids.contains(&disk.id) {
                skipped += 1;
                continue;
            }
            let (rule, phase) = rules_file::rule_from_disk(disk);
            let id = rule.id.clone();
            network.add_rule(rule).await.map_err(Self::err)?;
            phase_map.insert(id, phase);
            loaded += 1;
        }
        drop(phase_map);
        drop(network);

        let summary = serde_json::json!({
            "path": path.display().to_string(),
            "loaded": loaded,
            "skipped_duplicate_ids": skipped,
        });
        let json = serde_json::to_string_pretty(&summary).map_err(|e| Self::err(e.to_string()))?;
        Self::ok_text(json)
    }

    #[tool(
        description = "Declare which section of a patterns-guide document the agent is currently working in. Retracts any previous section-context fact and asserts `markdown_rule(file, section)`. Patterns-guide rules with matching conditions will fire on the next `fire_rules`."
    )]
    async fn set_section_context(
        &self,
        Parameters(params): Parameters<SetSectionContextParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_string(&params.file, "file").map_err(|e| Self::err(e.to_string()))?;
        validate_string(&params.section, "section").map_err(|e| Self::err(e.to_string()))?;

        let network = self.network.lock().await;

        // Retract any existing context fact. The well-known ID lets us replace
        // it deterministically; if there's none, ignore the error.
        let _ = network.retract_fact("__section_context__").await;

        network
            .assert_fact(Fact {
                id: "__section_context__".to_string(),
                predicate: "markdown_rule".to_string(),
                args: vec![params.file.clone(), params.section.clone()],
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            })
            .await
            .map_err(Self::err)?;

        Self::log_event("set_section_context", |e| {
            e.with("file", params.file.clone())
                .with("section", params.section.clone())
        });
        Self::ok_text(format!(
            "Section context set: {} / {}",
            params.file, params.section
        ))
    }

    #[tool(
        description = "Retract the current section context fact. Patterns-guide rules will stop firing until `set_section_context` is called again."
    )]
    async fn clear_section_context(
        &self,
        Parameters(_): Parameters<ClearSectionContextParams>,
    ) -> Result<CallToolResult, McpError> {
        let network = self.network.lock().await;
        let result = network.retract_fact("__section_context__").await;
        let cleared = result.is_ok();
        Self::log_event("clear_section_context", |e| e.with("cleared", cleared));
        match result {
            Ok(_) => Self::ok_text("Section context cleared".to_string()),
            Err(_) => Self::ok_text("No section context was set".to_string()),
        }
    }

    #[tool(
        description = "Read entries from the action log at .phronesis/log.jsonl. Default returns the most recent 100 entries across both hook and MCP events. Filter by `kind` (hook|mcp), `event` name, `since` timestamp, or `only_nonzero_exit: true` to find blocks/warnings."
    )]
    async fn get_action_log(
        &self,
        Parameters(params): Parameters<GetActionLogParams>,
    ) -> Result<CallToolResult, McpError> {
        let opts = action_log::ReadOpts {
            limit: Some(params.limit.unwrap_or(100)),
            since: params.since,
            kind: params.kind,
            event: params.event,
            only_nonzero_exit: params.only_nonzero_exit,
        };
        let path = action_log::default_path(&security::project_root());
        let entries =
            action_log::read_recent(&path, &opts).map_err(|e| Self::err(e.to_string()))?;
        let json = serde_json::to_string_pretty(&entries).map_err(|e| Self::err(e.to_string()))?;
        Self::ok_text(json)
    }

    #[tool(description = "Clear all accumulated consequences from memory")]
    async fn clear_consequences(&self) -> Result<CallToolResult, McpError> {
        let mut consequences = self.consequences.lock().await;
        let count = consequences.len();
        consequences.clear();
        Self::ok_text(format!("Cleared {} consequence(s)", count))
    }

    #[tool(
        description = "Aggregate rule-firing statistics from the action log (per-rule blocked/warned counts, last-fired timestamp, window label). Mirrors the `phr-mcp values` CLI. Optional filters: `since` (e.g. \"7d\"), `rule` (single rule id), `format` (\"json\" default, or \"table\")."
    )]
    async fn get_values(
        &self,
        Parameters(params): Parameters<GetValuesParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::stats::{StatsOpts, aggregate, parse_since, render_json, render_table};

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        // Mirror CLI behaviour: unrecognized window falls back to all-time.
        // We can't print to stderr from inside the MCP handler without polluting
        // the JSON-RPC stream, so the action_log record is the only signal.
        let since_secs = params.since.as_deref().and_then(parse_since);

        let opts_log = action_log::ReadOpts {
            kind: Some("hook".to_string()),
            ..action_log::ReadOpts::default()
        };
        let path = action_log::default_path(&security::project_root());
        let entries =
            action_log::read_recent(&path, &opts_log).map_err(|e| Self::err(e.to_string()))?;

        let values_opts = StatsOpts {
            since_secs,
            rule_filter: params.rule.clone(),
            now_secs: now,
        };
        let values = aggregate(&entries, &values_opts);

        Self::log_event("get_values", |e| {
            e.with("since", params.since.clone().unwrap_or_default())
                .with("rule", params.rule.clone().unwrap_or_default())
                .with("per_rule_count", values.per_rule.len() as u64)
        });

        let format = params.format.as_deref().unwrap_or("json");
        match format {
            "table" => Self::ok_text(render_table(&values)),
            // Default + any other value: JSON.
            _ => Self::ok_text(render_json(&values)),
        }
    }

    #[tool(
        description = "Audit the project tree for violations of phronesis rules. Unlike the pre-check hook (which only sees diffs), this runs each opted-in rule's predicates against every matching file's current contents and reports per-rule violation counts plus the specific files and lines. Use this to find grandfathered tech debt, prioritize cleanup, or measure debt shrinkage over time. Optional filters: `rule` (single rule id), `path` (subdirectory), `format` (\"json\" default, or \"table\"). Each call writes a snapshot to the action log so `get_debt_trend` can show shrinkage over time."
    )]
    async fn audit_codebase(
        &self,
        Parameters(params): Parameters<AuditCodebaseParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::audit::{AuditOpts, Level, render_json, render_table, run};
        use crate::rules_file;

        let project_root = crate::security::project_root();
        let rules_path = rules_file::default_path(&project_root);
        let rules = match rules_file::read(&rules_path) {
            Ok(r) => r,
            Err(e) => {
                return Self::ok_text(format!("phronesis: cannot read rules file: {}", e));
            }
        };
        if rules.rules.is_empty() {
            return Self::ok_text("no rules configured; run `phr-mcp init` first".to_string());
        }

        let scan_root = match params.path.as_deref() {
            Some(p) => {
                let pb = std::path::PathBuf::from(p);
                if pb.is_absolute() {
                    pb
                } else {
                    project_root.join(pb)
                }
            }
            None => project_root.clone(),
        };

        let opts = AuditOpts {
            project_root: project_root.clone(),
            scan_root,
            rule_filter: params.rule.clone(),
        };
        let report = run(&rules, &opts);

        // Write the audit snapshot to the log so get_debt_trend can read it.
        Self::log_event("audit_codebase", |e| {
            let mut per_rule = serde_json::Map::new();
            for r in &report.per_rule {
                per_rule.insert(
                    r.rule_id.as_str().to_string(),
                    serde_json::json!({
                        "level": r.level.as_str(),
                        "hits": r.hits,
                    }),
                );
            }
            let blocked: u32 = report
                .per_rule
                .iter()
                .filter(|r| r.level == Level::Block)
                .map(|r| r.hits)
                .sum();
            let warned: u32 = report
                .per_rule
                .iter()
                .filter(|r| r.level == Level::Warn)
                .map(|r| r.hits)
                .sum();
            e.with("files_scanned", report.files_scanned as u64)
                .with("blocked_total", blocked as u64)
                .with("warned_total", warned as u64)
                .with("per_rule", serde_json::Value::Object(per_rule))
        });

        match params.format.as_deref() {
            Some("table") => {
                let expand = params.rule.is_some();
                Self::ok_text(render_table(&report, expand))
            }
            _ => Self::ok_text(render_json(&report)),
        }
    }

    #[tool(
        description = "Show debt-over-time by diffing audit snapshots from the action log. Each `audit_codebase` call writes a snapshot; this tool reads them back and reports per-rule hit counts across snapshots plus net change (negative = improvement). Use after running audit_codebase a few times to see whether cleanup is making progress. Optional filters: `last` (default 5), `since` (e.g. \"7d\"), `rule`, `format` (\"json\" default, or \"table\")."
    )]
    async fn get_debt_trend(
        &self,
        Parameters(params): Parameters<GetDebtTrendParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::action_log::{self, ReadOpts};
        use crate::audit::{TrendOpts, compute_trend, render_trend_json, render_trend_table};
        use crate::stats::parse_since;

        let path = action_log::default_path(&crate::security::project_root());
        let opts_log = ReadOpts {
            event: Some("audit_codebase".to_string()),
            ..ReadOpts::default()
        };
        let entries = action_log::read_recent(&path, &opts_log).unwrap_or_default();

        let since_secs = params.since.as_deref().and_then(parse_since);
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let opts = TrendOpts {
            last: params.last.map(|n| n as usize).or(Some(5)),
            since_secs,
            rule_filter: params.rule.clone(),
            now_secs,
        };
        let trend = compute_trend(&entries, &opts);

        Self::log_event("get_debt_trend", |e| {
            e.with("snapshots_considered", trend.snapshots_considered as u64)
                .with("rules_count", trend.rules.len() as u64)
        });

        match params.format.as_deref() {
            Some("table") => Self::ok_text(render_trend_table(&trend)),
            _ => Self::ok_text(render_trend_json(&trend)),
        }
    }

    #[tool(
        description = "Detect drift between CLAUDE.md imperatives and the current rule pack. Heuristic: extracts bullets like \"Don't X\" / \"Always Y\" / \"Prefer Z\" from CLAUDE.md and matches each against rule contents by token overlap. Returns each bullet with its best-match rule (if any) and a Jaccard similarity score; bullets below the coverage threshold are candidates that should become enforced rules. Read-only. Use this when the user mentions CLAUDE.md, project conventions, or asks whether guidance is enforced. Optional `format` param: \"json\" (default) or \"table\"."
    )]
    async fn get_claude_md_drift(
        &self,
        Parameters(params): Parameters<GetClaudeMdDriftParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::claude_md_drift::{DriftError, render_json, render_table, run};

        let root = security::project_root();
        let report = match run(&root) {
            Ok(r) => r,
            Err(DriftError::ClaudeMdMissing(p)) => {
                return Err(Self::err(format!("CLAUDE.md not found at {}", p)));
            }
            Err(e) => return Err(Self::err(e.to_string())),
        };

        let uncovered = report
            .items
            .iter()
            .filter(|i| i.similarity < report.coverage_threshold)
            .count();
        Self::log_event("get_claude_md_drift", |e| {
            e.with("items_total", report.items.len() as u64)
                .with("items_uncovered", uncovered as u64)
        });

        match params.format.as_deref() {
            Some("table") => Self::ok_text(render_table(&report)),
            _ => Self::ok_text(render_json(&report)),
        }
    }

    #[tool(
        description = "Detect drift between Claude Code's auto-memory store and the phronesis rule pack / durable directives. Walks `~/.claude/projects/<encoded-cwd>/memory/`, parses frontmatter on each `.md` file, and classifies each entry into one of three buckets: `actionable` (port to a rule), `ambient` (port to durable.md), or `personal` (stays in MEMORY.md). Non-personal entries are scored by token overlap against rules.json and durable.md; uncovered entries are candidates for porting. Read-only, heuristic, no LLM call. Use this when the user mentions memory, durable guidance, or asks whether a saved preference is enforced. Optional `memory_dir` overrides the default location. Optional `format` param: \"json\" (default) or \"table\"."
    )]
    async fn get_memory_drift(
        &self,
        Parameters(params): Parameters<GetMemoryDriftParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::memory_drift::{
            DriftError, default_memory_dir, render_json, render_table, run_with_dir,
        };

        let root = security::project_root();
        let memory_dir = match params.memory_dir.as_deref() {
            Some(p) => std::path::PathBuf::from(p),
            None => default_memory_dir(&root),
        };

        let report = match run_with_dir(&root, &memory_dir) {
            Ok(r) => r,
            Err(DriftError::MemoryDirMissing(p)) => {
                return Err(Self::err(format!(
                    "memory directory not found at {} — Claude Code creates this directory on first save; pass `memory_dir` to point elsewhere",
                    p
                )));
            }
            Err(e) => return Err(Self::err(e.to_string())),
        };

        let actionable_uncovered = report
            .items
            .iter()
            .filter(|i| {
                matches!(i.bucket, crate::memory_drift::Bucket::Actionable)
                    && i.similarity < report.coverage_threshold
            })
            .count();
        Self::log_event("get_memory_drift", |e| {
            e.with("items_total", report.items.len() as u64)
                .with("actionable_uncovered", actionable_uncovered as u64)
        });

        match params.format.as_deref() {
            Some("table") => Self::ok_text(render_table(&report)),
            _ => Self::ok_text(render_json(&report)),
        }
    }
}

#[tool_handler]
impl ServerHandler for EpistemeMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .build(),
            server_info: Implementation {
                name: "phr-mcp".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                ..Default::default()
            },
            instructions: Some(
                "RETE rules engine for rules-bounded LLM interaction. Use tools to manage rules, facts, fire the engine, and query consequences."
                    .into(),
            ),
            ..Default::default()
        }
    }
}

/// Extract enforceable rules from a markdown document.
///
/// Recognizes:
/// - Directive bullets: `- Avoid X`, `- Never Y`, `- Prefer Z`, `- ❌ ...`, etc.
/// - Callout lines: `**Problem**: ...`, `**Pattern**: ...`, `**Anti-Pattern**: ...`
/// - Subsection titles inside a `## Anti-Patterns` section (e.g., `### 3. Overusing unwrap()`)
///
/// Skips content inside fenced code blocks to avoid false positives from code comments.
/// Tracks the enclosing `##` section so each rule is tagged with its category.
pub fn extract_rules_from_markdown(content: &str, source_file: &str) -> Vec<Rule> {
    let mut rules = Vec::new();
    let mut rule_idx = 0;
    let mut in_code_fence = false;
    let mut current_section: Option<String> = None;

    let source_slug = slugify(
        source_file
            .rsplit('/')
            .next()
            .unwrap_or(source_file)
            .trim_end_matches(".md"),
    );

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_code_fence = !in_code_fence;
            continue;
        }
        if in_code_fence {
            continue;
        }

        if let Some(section) = trimmed.strip_prefix("## ") {
            current_section = Some(section.trim().to_string());
            continue;
        }

        if let Some(sub) = trimmed.strip_prefix("### ") {
            if section_is_anti_patterns(current_section.as_deref()) {
                let title = strip_numbered_prefix(sub.trim());
                if title.len() >= 5 {
                    rule_idx += 1;
                    rules.push(make_rule(
                        &source_slug,
                        rule_idx,
                        source_file,
                        current_section.as_deref(),
                        "anti_pattern",
                        &format!("Avoid: {}", title),
                    ));
                }
            }
            continue;
        }

        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if let Some((kind, text)) = parse_callout(trimmed) {
            if text.len() >= 10 {
                rule_idx += 1;
                rules.push(make_rule(
                    &source_slug,
                    rule_idx,
                    source_file,
                    current_section.as_deref(),
                    kind,
                    text,
                ));
            }
            continue;
        }

        if let Some(constraint_text) = strip_directive_prefix(trimmed) {
            if constraint_text.len() >= 10 {
                rule_idx += 1;
                rules.push(make_rule(
                    &source_slug,
                    rule_idx,
                    source_file,
                    current_section.as_deref(),
                    "directive",
                    constraint_text,
                ));
            }
        }
    }

    rules
}

fn make_rule(
    source_slug: &str,
    idx: usize,
    source_file: &str,
    section: Option<&str>,
    kind: &str,
    text: &str,
) -> Rule {
    let id = match section {
        Some(s) => format!("{}-{}-{}", source_slug, slugify(s), idx),
        None => format!("{}-{}", source_slug, idx),
    };
    let mut condition_args = vec![source_file.to_string()];
    if let Some(s) = section {
        condition_args.push(s.to_string());
    }
    Rule {
        id,
        priority: 5,
        conditions: vec![Condition {
            predicate: "markdown_rule".to_string(),
            args: condition_args,
            script: None,
        }],
        actions: vec![Action {
            action_type: "constraint_violation".to_string(),
            params: vec![format!("[{}] {}", kind, text)],
        }],
    }
}

fn slugify(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'A'..='Z' => c.to_ascii_lowercase(),
            'a'..='z' | '0'..='9' => c,
            _ => '-',
        })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn section_is_anti_patterns(section: Option<&str>) -> bool {
    section
        .map(|s| {
            let lower = s.to_ascii_lowercase();
            lower.contains("anti-pattern") || lower.contains("antipattern")
        })
        .unwrap_or(false)
}

fn strip_numbered_prefix(s: &str) -> &str {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i > 0 && i < bytes.len() && (bytes[i] == b'.' || bytes[i] == b')') {
        i += 1;
        while i < bytes.len() && bytes[i] == b' ' {
            i += 1;
        }
        &s[i..]
    } else {
        s
    }
}

/// Recognize callout-style lines used in coding standards docs.
/// Returns (kind, text) where kind tags the rule for downstream consumers.
pub fn parse_callout(line: &str) -> Option<(&'static str, &str)> {
    let stripped = line.trim_start_matches(['-', ' ']).trim_start();
    let callouts: &[(&str, &str)] = &[
        ("**Problem**:", "problem"),
        ("**Anti-Pattern**:", "anti_pattern"),
        ("**Anti-pattern**:", "anti_pattern"),
        ("**Pattern**:", "pattern"),
        ("**Recommendation**:", "pattern"),
        ("**Use Case**:", "context"),
    ];
    for (prefix, kind) in callouts {
        if let Some(rest) = stripped.strip_prefix(prefix) {
            let text = rest.trim_start_matches([' ', '*']).trim();
            if !text.is_empty() {
                return Some((kind, text));
            }
        }
    }
    None
}

/// Strip a leading directive marker (Avoid, Don't, Never, Prefer, Always, Use, ❌) from a line.
///
/// Requires a non-alphanumeric character to follow the directive word so that
/// `Use` does not match `Useful` and `Never` does not match `Nevertheless`.
pub fn strip_directive_prefix(line: &str) -> Option<&str> {
    let prefixes = [
        "- \u{274c}",
        "\u{274c}",
        "- **Avoid",
        "- Avoid",
        "Avoid",
        "- **Don't",
        "- Don't",
        "Don't",
        "- **Never",
        "- Never",
        "Never",
        "- **Prefer",
        "- Prefer",
        "Prefer",
        "- **Always",
        "- Always",
        "Always",
        "- **Use",
        "- Use",
    ];

    for prefix in &prefixes {
        if let Some(rest) = line.strip_prefix(prefix) {
            // Require a word boundary after alphabetic directive words to avoid
            // matching e.g. "Useful" via the "Use" prefix.
            let last_alpha = prefix.chars().last().is_some_and(|c| c.is_alphanumeric());
            if last_alpha {
                let next_alpha = rest.chars().next().is_some_and(|c| c.is_alphanumeric());
                if next_alpha {
                    continue;
                }
            }
            let rest = rest
                .trim_start_matches([' ', '-', ':', '\u{2014}', '*'])
                .trim();
            if !rest.is_empty() {
                return Some(rest);
            }
        }
    }

    None
}

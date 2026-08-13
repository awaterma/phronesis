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
            network: Arc::new(Mutex::new(crate::net::build_network())),
            registry: Arc::new(Mutex::new(LookupRegistry::new())),
            consequences: Arc::new(Mutex::new(Vec::new())),
            phase_map: Arc::new(Mutex::new(HashMap::new())),
            tool_router: Self::tool_router(),
        }
    }

    fn ok_text(text: String) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Return a machine-readable MCP result while retaining JSON text for
    /// clients that do not yet consume `structuredContent`.
    ///
    /// MCP structured results must be objects. Keeping that invariant here
    /// prevents collection tools from accidentally exposing top-level arrays,
    /// which several SDKs reject even when the same array is valid JSON text.
    fn ok_json(value: serde_json::Value) -> Result<CallToolResult, McpError> {
        if !value.is_object() {
            return Err(Self::err("structured MCP results must be JSON objects"));
        }
        Ok(CallToolResult::structured(value))
    }

    fn ok_collection(
        key: &'static str,
        values: impl serde::Serialize,
    ) -> Result<CallToolResult, McpError> {
        let values = serde_json::to_value(values).map_err(|e| Self::err(e.to_string()))?;
        let mut envelope = serde_json::Map::new();
        envelope.insert(key.to_string(), values);
        Self::ok_json(serde_json::Value::Object(envelope))
    }

    fn code_graph_status(root: &std::path::Path) -> Result<serde_json::Value, McpError> {
        use crate::graph::{bindings, store, sync};

        let graph_path = store::graph_path(root);
        let index_path = sync::index_path(root);
        let available = graph_path.exists() && index_path.exists();
        let index = sync::load_index(&index_path).map_err(|e| Self::err(e.to_string()))?;
        let edges = store::load(&graph_path).map_err(|e| Self::err(e.to_string()))?;
        let base_edges = edges.iter().filter(|edge| !edge.d).count();
        let derived_edges = edges.len().saturating_sub(base_edges);

        let (status, drifted_files, outdated_format) = if !available {
            ("missing", Vec::new(), false)
        } else {
            match sync::check_freshness(root, &index) {
                sync::Freshness::Fresh => ("fresh", Vec::new(), false),
                sync::Freshness::Stale(files) => ("stale", files, false),
                sync::Freshness::Outdated { .. } => ("outdated", Vec::new(), true),
            }
        };

        let binding_set =
            bindings::load(&bindings::bindings_path(root)).map_err(|e| Self::err(e.to_string()))?;
        let bindings_available = binding_set.is_some();
        let mut bound = 0;
        let mut moved = 0;
        let mut stale = 0;
        if let Some(set) = binding_set {
            for binding in set.bindings {
                match binding.state {
                    bindings::BindingState::Bound => bound += 1,
                    bindings::BindingState::Moved => moved += 1,
                    bindings::BindingState::Stale => stale += 1,
                }
            }
        }

        Ok(serde_json::json!({
            "status": status,
            "available": available,
            "fresh": status == "fresh",
            "generation": index.generation,
            "graph_format": index.format,
            "expected_format": sync::GRAPH_FORMAT,
            "outdated_format": outdated_format,
            "files_indexed": index.entries.len(),
            "drifted_files": drifted_files,
            "base_edges": base_edges,
            "derived_edges": derived_edges,
            "bindings": {
                "available": bindings_available,
                "bound": bound,
                "moved": moved,
                "stale": stale,
            },
        }))
    }

    pub(crate) fn err(msg: impl ToString) -> McpError {
        McpError::new(ErrorCode(-1), msg.to_string(), None::<serde_json::Value>)
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

    /// Validate that `phase` is `"pre"` or `"post"` and return it unchanged,
    /// or return an [`McpError`] if it is neither.
    fn validate_default_phase(phase: &str) -> Result<&str, McpError> {
        if phase == "pre" || phase == "post" {
            Ok(phase)
        } else {
            Err(Self::err(format!(
                "phase must be \"pre\" or \"post\", got: {}",
                phase
            )))
        }
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

    #[tool(
        description = "List all rules currently loaded in the RETE network. Returns {rules: [...]} as structured JSON."
    )]
    async fn list_rules(&self) -> Result<CallToolResult, McpError> {
        let network = self.network.lock().await;
        let rules = network.get_all_rules().map_err(Self::err)?;
        Self::ok_collection("rules", rules)
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

    // ── Extensible Predicate Providers ──

    #[tool(
        description = "Create a sandboxed Rhai fact provider under `.phronesis/predicates/`. Providers receive a normalized read-only `event` and call `emit_fact(predicate, args)` to add project-defined LHS predicates before RETE matching. The script is validated before writing. Existing providers are never replaced unless `replace=true`."
    )]
    async fn add_predicate_provider(
        &self,
        Parameters(params): Parameters<AddPredicateProviderParams>,
    ) -> Result<CallToolResult, McpError> {
        let root = security::project_root();
        let path =
            crate::predicate_provider::add(&root, &params.name, &params.script, params.replace)
                .map_err(Self::err)?;
        Self::log_event("add_predicate_provider", |entry| {
            entry
                .with("provider", params.name.clone())
                .with("path", path.display().to_string())
                .with("replace", params.replace)
        });
        Self::ok_text(format!(
            "Predicate provider '{}' written to {}",
            params.name,
            path.display()
        ))
    }

    #[tool(description = "List project Rhai predicate providers in deterministic load order")]
    async fn list_predicate_providers(
        &self,
        Parameters(_params): Parameters<ListPredicateProvidersParams>,
    ) -> Result<CallToolResult, McpError> {
        let providers =
            crate::predicate_provider::list(&security::project_root()).map_err(Self::err)?;
        Self::ok_text(
            serde_json::to_string_pretty(&serde_json::json!({"providers": providers}))
                .map_err(Self::err)?,
        )
    }

    #[tool(description = "Read one project Rhai predicate provider by name")]
    async fn get_predicate_provider(
        &self,
        Parameters(params): Parameters<PredicateProviderNameParam>,
    ) -> Result<CallToolResult, McpError> {
        let script = crate::predicate_provider::get(&security::project_root(), &params.name)
            .map_err(Self::err)?;
        Self::ok_text(
            serde_json::to_string_pretty(
                &serde_json::json!({"name": params.name, "script": script}),
            )
            .map_err(Self::err)?,
        )
    }

    #[tool(
        description = "Evaluate a Rhai predicate-provider script against a supplied normalized event without writing it. Returns the facts it would emit; use this before add_predicate_provider."
    )]
    async fn test_predicate_provider(
        &self,
        Parameters(params): Parameters<TestPredicateProviderParams>,
    ) -> Result<CallToolResult, McpError> {
        let event = crate::predicate_provider::ProviderEvent {
            phase: params.event.phase,
            tool_name: params.event.tool_name,
            // Derived, not supplied: the test surface must relativize exactly
            // as the real hook does, or a script that joins graph facts would
            // pass here and silently never fire in production.
            file_rel: crate::graph::hydrate::repo_relative(
                &crate::security::project_root(),
                &params.event.file_path,
            )
            .unwrap_or_default(),
            file_path: params.event.file_path,
            files: params.event.files,
            old_content: params.event.old_content,
            new_content: params.event.new_content,
            command: params.event.command,
            output: params.event.output,
        };
        let facts =
            crate::predicate_provider::test_script(&params.script, &event).map_err(Self::err)?;
        Self::ok_text(
            serde_json::to_string_pretty(&serde_json::json!({"facts": facts}))
                .map_err(Self::err)?,
        )
    }

    #[tool(description = "Remove one project Rhai predicate provider by name")]
    async fn remove_predicate_provider(
        &self,
        Parameters(params): Parameters<PredicateProviderNameParam>,
    ) -> Result<CallToolResult, McpError> {
        crate::predicate_provider::remove(&security::project_root(), &params.name)
            .map_err(Self::err)?;
        Self::log_event("remove_predicate_provider", |entry| {
            entry.with("provider", params.name.clone())
        });
        Self::ok_text(format!("Predicate provider '{}' removed", params.name))
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

    #[tool(
        description = "List or query facts in working memory. No params → all facts. `predicate` → facts with that predicate, optionally narrowed by `arg_filters` (positional arg = value constraints). `predicates` → facts whose predicate is in the set. Results are sorted by fact id and returned as {facts: [...]} structured JSON."
    )]
    async fn list_facts(
        &self,
        Parameters(params): Parameters<PredicateFilter>,
    ) -> Result<CallToolResult, McpError> {
        let network = self.network.lock().await;
        let facts = select_facts(&network, &params).map_err(Self::err)?;
        Self::ok_collection("facts", facts)
    }

    #[tool(description = "Get a specific fact by its ID")]
    async fn get_fact(
        &self,
        Parameters(params): Parameters<FactIdParam>,
    ) -> Result<CallToolResult, McpError> {
        let network = self.network.lock().await;
        match network
            .get_fact_by_id(params.fact_id.as_str())
            .map_err(Self::err)?
        {
            Some(fact) => {
                let json =
                    serde_json::to_string_pretty(&fact).map_err(|e| Self::err(e.to_string()))?;
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
        let actions = {
            let network = self.network.lock().await;
            network.execute_all_agenda_items().map_err(Self::err)?
        };

        let new_consequences =
            rule_firing_to_consequences("fire_rules", &[], ConsequenceKind::Event, actions.clone());

        // Extend the shared consequence store and FIFO-evict against the cap.
        {
            let mut consequences = self.consequences.lock().await;
            consequences.extend(new_consequences.clone());
            if consequences.len() > MAX_CONSEQUENCES {
                let excess = consequences.len() - MAX_CONSEQUENCES;
                consequences.drain(0..excess);
            }
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

    #[tool(
        description = "Peek at the current agenda (pending rule activations). Returns {agenda: [...]} as structured JSON."
    )]
    async fn get_agenda(&self) -> Result<CallToolResult, McpError> {
        let network = self.network.lock().await;
        let items = network
            .agenda_snapshot()
            .map_err(|e| Self::err(e.to_string()))?;
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
        Self::ok_collection("agenda", summaries)
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
        description = "Get accumulated consequences, optionally filtered by kind (event, snapshot, constraint, affordance). Returns {consequences: [...]} as structured JSON."
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
        Self::ok_collection("consequences", filtered)
    }

    // ── Rules Extraction ──

    #[tool(
        description = "Submit a markdown file from the project for rules extraction. Parses the file for enforceable patterns and constraints, converts them to phronesis Rules, and loads them into the RETE network. Path must resolve inside the project root."
    )]
    async fn extract_rules(
        &self,
        Parameters(params): Parameters<FilePathParam>,
    ) -> Result<CallToolResult, McpError> {
        let content = {
            validate_string(&params.file_path, "file_path")
                .map_err(|e| Self::err(e.to_string()))?;
            let root = security::project_root();
            let safe_path = resolve_safe_path(&params.file_path, &root)
                .map_err(|e| Self::err(format!("path rejected: {}", e)))?;
            require_extension(&safe_path, "md").map_err(|e| Self::err(e.to_string()))?;
            security::read_file_capped(&safe_path).map_err(|e| Self::err(e.to_string()))?
        };

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
        let default_phase = Self::validate_default_phase(params.phase.as_deref().unwrap_or("pre"))?;

        let path = {
            let root = security::project_root();
            rules_file::default_path(&root)
        };

        let existing = if params.merge {
            rules_file::read(&path).map_err(|e| Self::err(e.to_string()))?
        } else {
            rules_file::RulesFile { rules: vec![] }
        };

        let in_memory = {
            let network = self.network.lock().await;
            network.get_all_rules().map_err(Self::err)?
        };

        let phase_map = self.phase_map.lock().await.clone();

        let json = {
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
            serde_json::to_string_pretty(&summary).map_err(|e| Self::err(e.to_string()))?
        };

        Self::ok_text(json)
    }

    #[tool(
        description = "Hydrate the in-memory network from .phronesis/rules.json. Adds each rule (preserving its phase) and skips rules whose ID already exists."
    )]
    async fn load_rules_file(
        &self,
        Parameters(_): Parameters<LoadRulesFileParams>,
    ) -> Result<CallToolResult, McpError> {
        let path = {
            let root = security::project_root();
            rules_file::default_path(&root)
        };
        let file = rules_file::read(&path).map_err(|e| Self::err(e.to_string()))?;

        let (loaded, skipped) = {
            let network = self.network.lock().await;
            let existing_ids: std::collections::HashSet<String> = network
                .get_all_rules()
                .map_err(Self::err)?
                .into_iter()
                .map(|r| r.id)
                .collect();
            let mut phase_map = self.phase_map.lock().await;
            crate::server_persistence::hydrate_rules(
                &network,
                &mut phase_map,
                &file.rules,
                &existing_ids,
            )
            .await
            .map_err(Self::err)?
        };

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
        description = "Read entries from the action log at .phronesis/log.jsonl. Default returns the most recent 100 entries across both hook and MCP events in {entries: [...]} structured JSON. Filter by `kind` (hook|mcp), `event` name, `since` timestamp, or `only_nonzero_exit: true` to find blocks/warnings."
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
        Self::ok_collection("entries", entries)
    }

    #[tool(description = "Clear all accumulated consequences from memory")]
    async fn clear_consequences(&self) -> Result<CallToolResult, McpError> {
        let mut consequences = self.consequences.lock().await;
        let count = consequences.len();
        consequences.clear();
        Self::ok_text(format!("Cleared {} consequence(s)", count))
    }

    #[tool(
        description = "Aggregate rule-firing statistics from the action log (per-rule blocked/warned counts, last-fired timestamp, window label). Mirrors the `phr-mcp stats` CLI. Optional filters: `since` (e.g. \"7d\"), `rule` (single rule id), `format` (\"json\" default, or \"table\")."
    )]
    async fn get_stats(
        &self,
        Parameters(params): Parameters<GetStatsParams>,
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

        let entries = {
            let opts_log = action_log::ReadOpts {
                kind: Some("hook".to_string()),
                ..action_log::ReadOpts::default()
            };
            let path = action_log::default_path(&security::project_root());
            action_log::read_recent(&path, &opts_log).map_err(|e| Self::err(e.to_string()))?
        };

        let stats_opts = StatsOpts {
            since_secs,
            rule_filter: params.rule.clone(),
            now_secs: now,
        };
        let stats = aggregate(&entries, &stats_opts);

        Self::log_event("get_stats", |e| {
            e.with("since", params.since.clone().unwrap_or_default())
                .with("rule", params.rule.clone().unwrap_or_default())
                .with("per_rule_count", stats.per_rule.len() as u64)
        });

        let format = params.format.as_deref().unwrap_or("json");
        match format {
            "table" => Self::ok_text(render_table(&stats)),
            // Default + any other value: JSON.
            _ => Self::ok_text(render_json(&stats)),
        }
    }

    #[tool(
        description = "Audit the project tree for violations of phronesis rules. Unlike the pre-check hook (which only sees diffs), this runs each opted-in rule's predicates against every matching file's current contents and reports per-rule violation counts plus the specific files and lines. Use this to find grandfathered tech debt, prioritize cleanup, or measure debt shrinkage over time. Optional filters: `rule` (single rule id), `path` (subdirectory), `format` (\"json\" default, or \"table\"). Each call writes a snapshot to the action log so `get_debt_trend` can show shrinkage over time."
    )]
    async fn audit_codebase(
        &self,
        Parameters(params): Parameters<AuditCodebaseParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::audit::{AuditOpts, render_json, render_table, run};
        use crate::rules_file;

        let project_root = crate::security::project_root();
        let rules = {
            let rules_path = rules_file::default_path(&project_root);
            match rules_file::read(&rules_path) {
                Ok(r) => r,
                Err(e) => {
                    return Self::ok_text(format!("phronesis: cannot read rules file: {}", e));
                }
            }
        };
        if rules.rules.is_empty() {
            return Self::ok_text("no rules configured; run `phr-mcp init` first".to_string());
        }

        let scan_root = crate::audit::resolve_scan_root(params.path.as_deref(), &project_root);

        let (report, diag) = {
            let opts = AuditOpts {
                project_root: project_root.clone(),
                scan_root,
                rule_filter: params.rule.clone(),
            };
            let mut report = run(&rules, &opts);
            // Structural rules join relations across the whole repository, so
            // the file-scan loop skips them. Mirror the CLI and fold in the
            // graph findings — otherwise this tool reports zero structural
            // debt however much the graph holds, and the snapshot below
            // teaches `get_debt_trend` the same falsehood permanently.
            {
                let engine_rules: Vec<phr::Rule> = rules
                    .rules
                    .iter()
                    .filter(|r| r.audit == Some(true))
                    .map(|r| crate::rules_file::rule_from_disk(r).0)
                    .collect();
                let hits =
                    crate::graph::audit::audit_graph_rules(&project_root, &engine_rules).await;
                let scope = crate::audit::graph_scope_prefix(&project_root, &opts.scan_root);
                crate::audit::merge_graph_hits(
                    &mut report,
                    &hits,
                    params.rule.as_deref(),
                    scope.as_deref(),
                );
            }
            let report = report;
            let audit_tagged_count = rules.rules.iter().filter(|r| r.audit == Some(true)).count();
            let diag =
                crate::audit::empty_result_diagnostic(&report, audit_tagged_count, &opts.scan_root);
            (report, diag)
        };

        // Write the audit snapshot to the log so get_debt_trend can read it.
        Self::log_event("audit_codebase", |e| {
            crate::audit::audit_snapshot_entry(e, &report)
        });

        let body = match params.format.as_deref() {
            Some("table") => {
                let expand = params.rule.is_some();
                render_table(&report, expand)
            }
            _ => render_json(&report),
        };
        let response = match diag {
            Some(msg) => format!("{}\n\n{}", msg, body),
            None => body,
        };
        Self::ok_text(response)
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
        description = "Detect drift between written guidance and enforced rules across every corpus: root and package-level CLAUDE.md/AGENTS.md imperatives, Claude Code auto-memory entries, ADR decisions under .phronesis/wiki/decisions/, and rules naming code the graph no longer defines. Read-only, heuristic, no LLM call — output is a triage list, not ground truth. `source` selects one of \"claude_md\", \"memory\", \"wiki\", \"code\", or \"all\" (default). With \"all\" the response is a bounded summary: use a single source plus a higher `limit` for detail. A corpus that does not exist is reported as unavailable rather than failing the call. Optional: `limit` (default 5, max 50), `format` (\"json\" default or \"table\"), `suggest` (default false — set true to get a draft rule per item, which is large), `memory_dir`, `wiki_dir`."
    )]
    async fn get_drift(
        &self,
        Parameters(params): Parameters<GetDriftParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::drift::{self, Source, SourceInputs};

        let root = security::project_root();

        let sources: Vec<Source> = match params.source.as_deref().unwrap_or("all") {
            "all" => Source::ALL.to_vec(),
            "claude_md" => vec![Source::ClaudeMd],
            "memory" => vec![Source::Memory],
            "wiki" => vec![Source::Wiki],
            "code" => vec![Source::Code],
            other => {
                return Err(Self::err(format!(
                    "unknown source {other:?} — expected one of: claude_md, memory, wiki, code, all"
                )));
            }
        };

        let memory_dir = params.memory_dir.as_deref().map(std::path::PathBuf::from);
        let wiki_dir = params.wiki_dir.as_deref().map(std::path::PathBuf::from);
        let inputs = SourceInputs {
            project_root: &root,
            claude_md: None,
            memory_dir: memory_dir.as_deref(),
            wiki_dir: wiki_dir.as_deref(),
            suggest: params.suggest.unwrap_or(false),
        };

        let limit = params.limit.unwrap_or(drift::DEFAULT_LIMIT);
        let agg = drift::run_all(&sources, &inputs, limit);

        Self::log_event("get_drift", |e| {
            e.with("sources_present", agg.totals.sources_present as u64)
                .with("sources_missing", agg.totals.sources_missing as u64)
                .with("sources_errored", agg.totals.sources_errored as u64)
                .with("uncovered_total", agg.totals.uncovered_total as u64)
        });

        let body = if params.format.as_deref() == Some("table") {
            drift::render_table(&agg)
        } else {
            drift::render_json(&agg)
        };
        Ok(CallToolResult::success(vec![Content::text(body)]))
    }

    #[tool(
        description = "Query the structural code graph at `.phronesis/graph.jsonl` — a map of the codebase built by the PostToolUse sensor. Answers questions about code structure without reading source files. Relations include `defines_fn` [file, function], `tested_by` [function, test], `untested` [function], `calls_api` [function, api], `imports` [module, module], `in_cycle` [module, cycle_id], `file_type` [file, kind], `declares_module` [file, module], `generates` [producer, artifact], `consumes_data` [consumer, artifact], `deserializes` [Rust type, artifact], and `data_flows_to` [artifact, consumer]. Use `\"*\"` for an unconstrained position; embedded `*` and `?` are globs. Worked examples: tests covering a function -> `tested_by my_fn *`; modules depending on another -> `imports * rust:phronesis::wme`; Config flowing into consumers -> `data_flows_to yaml:* *`. Omit `relation` to list the vocabulary. Entities are language-qualified. `tested_by` is a direct-call heuristic, so transitively-covered code can still appear untested. Call `rebuild_code_graph` if the graph has never been built or `get_code_graph_status` reports stale or outdated state."
    )]
    async fn query_code_graph(
        &self,
        Parameters(params): Parameters<QueryCodeGraphParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::graph::{query as q, store};

        let root = security::project_root();
        let edges = store::load(&store::graph_path(&root)).unwrap_or_default();
        if edges.is_empty() {
            return Self::ok_text(
                serde_json::json!({
                    "error": "no code graph found",
                    "fix": "run `phr-mcp graph rebuild` in the project root",
                })
                .to_string(),
            );
        }

        // No relation is a discovery request, not an empty result.
        let Some(relation) = params.relation.as_deref() else {
            let summary = q::relation_summary(&edges);
            return Self::ok_text(
                serde_json::json!({
                    "relations": summary
                        .iter()
                        .map(|(r, n)| serde_json::json!({"relation": r, "edges": n}))
                        .collect::<Vec<_>>(),
                })
                .to_string(),
            );
        };

        let mut tokens = vec![relation.to_string()];
        tokens.extend(params.args.clone().unwrap_or_default());
        let pattern = q::Pattern::parse(&tokens);
        let limit = params.limit.unwrap_or(50);
        let total = q::count(&edges, &pattern);
        let rows = q::query(&edges, &pattern, limit);

        Self::log_event("query_code_graph", |e| {
            e.with("relation", relation.to_string())
                .with("matches", total as u64)
        });

        Self::ok_text(
            serde_json::json!({
                "total": total,
                "returned": rows.len(),
                "truncated": rows.len() < total,
                "results": rows
                    .iter()
                    .map(|e| serde_json::json!({"relation": e.p, "args": e.a, "derived": e.d}))
                    .collect::<Vec<_>>(),
            })
            .to_string(),
        )
    }

    #[tool(
        description = "Report whether the current project's derived code graph is missing, fresh, stale, or built with an outdated format. Returns generation, indexed-file and edge counts, drifted files, and rule-binding state as structured JSON."
    )]
    async fn get_code_graph_status(&self) -> Result<CallToolResult, McpError> {
        let root = security::project_root();
        Self::ok_json(Self::code_graph_status(&root)?)
    }

    #[tool(
        description = "Rebuild the current project's derived code graph from tracked Rust, Python, and TypeScript source, reconcile rule bindings, and return the resulting fresh status as structured JSON. The project root is server-controlled; this tool does not accept a filesystem path."
    )]
    async fn rebuild_code_graph(&self) -> Result<CallToolResult, McpError> {
        let root = security::project_root();
        let generation_before =
            crate::graph::sync::load_index(&crate::graph::sync::index_path(&root))
                .map_err(|e| Self::err(e.to_string()))?
                .generation;
        let outcome = crate::graph::sync::rebuild(&root).map_err(|e| Self::err(e.to_string()))?;
        let mut status = Self::code_graph_status(&root)?;
        let generation_after = status["generation"].as_u64().unwrap_or(0);
        let object = status
            .as_object_mut()
            .ok_or_else(|| Self::err("code graph status was not an object"))?;
        object.insert(
            "generation_before".to_string(),
            serde_json::json!(generation_before),
        );
        object.insert(
            "generation_after".to_string(),
            serde_json::json!(generation_after),
        );
        object.insert(
            "skipped_items".to_string(),
            serde_json::json!(outcome.skipped),
        );

        Self::log_event("rebuild_code_graph", |entry| {
            entry
                .with("generation_before", generation_before)
                .with("generation_after", generation_after)
                .with("base_edges", outcome.base as u64)
                .with("derived_edges", outcome.derived as u64)
                .with("skipped_items", outcome.skipped as u64)
        });
        Self::ok_json(status)
    }

    #[tool(
        description = "Report the confidence band (high/medium/low) and the grounded signals (compile / tests / known-bug) for the open work unit, or `subject` if given. Confidence scoring gates `git commit` on whether the suggested code compiles, its tests pass, and any known-bug test went green. Read-only; reflects `.phronesis/outcomes/`. Returns JSON `{subject, band, signals}`, or `{subject: null}` when no work unit is open. Opt-in per project via `.phronesis/confidence.json`."
    )]
    async fn get_confidence(
        &self,
        Parameters(params): Parameters<GetConfidenceParams>,
    ) -> Result<CallToolResult, McpError> {
        let root = security::project_root();
        match crate::outcomes::report(&root, params.subject.as_deref()) {
            Some(r) => {
                Self::log_event("get_confidence", |e| {
                    e.with("subject", r.subject.clone())
                        .with("band", r.band.as_str())
                });
                let out = serde_json::json!({
                    "subject": r.subject,
                    "band": r.band.as_str(),
                    "signals": r.signals,
                });
                Self::ok_text(
                    serde_json::to_string_pretty(&out).map_err(|e| Self::err(e.to_string()))?,
                )
            }
            None => Self::ok_text(
                serde_json::json!({ "subject": null, "message": "no open work unit" }).to_string(),
            ),
        }
    }

    #[tool(
        description = "Return the journey_* facts that would be asserted right now against `.phronesis/journey/events.jsonl` and the loaded rules — the agent's trajectory at a glance. Optionally pass `explain_rule` to filter to a single rule's referenced facts. Mirrors the `phr-mcp journey` CLI; reads the journey journal + journey.json + rules.json. JSON array of `{predicate, selector, window, extra, rules}` rows."
    )]
    async fn get_journey(
        &self,
        Parameters(params): Parameters<GetJourneyParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::journey_cli;
        let root = security::project_root();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let sid = crate::journey::current_sid(&root);
        let rows = journey_cli::compute(&root, params.explain_rule.as_deref(), now, &sid)
            .await
            .map_err(|e| Self::err(e.to_string()))?;
        Self::log_event("get_journey", |e| {
            e.with("rows", rows.len() as u64).with(
                "explain_rule",
                params.explain_rule.clone().unwrap_or_default(),
            )
        });
        let json = journey_cli::render_json(&rows).map_err(|e| Self::err(e.to_string()))?;
        Self::ok_text(json)
    }

    #[tool(
        description = "Declare a confidence work unit ('subject') — e.g. a cross-language translation or a discrete suggestion — and return its current confidence report. Sets the open subject so subsequent build/test runs accrue grounded signals to it (the explicit-subject path; the implicit path mints a unit automatically). Returns JSON `{subject, summary, band, signals}`. Confidence is opt-in per project via `.phronesis/confidence.json`."
    )]
    async fn submit_suggestion(
        &self,
        Parameters(params): Parameters<SubmitSuggestionParams>,
    ) -> Result<CallToolResult, McpError> {
        let root = security::project_root();
        crate::outcomes::subject::set(&root, &params.subject)
            .map_err(|e| Self::err(e.to_string()))?;
        let report = crate::outcomes::report(&root, Some(&params.subject));
        let band = report.as_ref().map(|r| r.band.as_str()).unwrap_or("low");
        let signals = report.map(|r| r.signals).unwrap_or_default();
        Self::log_event("submit_suggestion", |e| {
            e.with("subject", params.subject.clone()).with("band", band)
        });
        let out = serde_json::json!({
            "subject": params.subject,
            "summary": params.summary,
            "band": band,
            "signals": signals,
        });
        Self::ok_text(serde_json::to_string_pretty(&out).map_err(|e| Self::err(e.to_string()))?)
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

/// Per-line classification produced by [`classify_md_line`].
enum MdLineKind {
    /// A `## <section>` heading — caller should update the current-section state.
    Section(String),
    /// A `### <title>` heading inside an anti-patterns section with a long enough title.
    /// The text has already been formatted as `"Avoid: <title>"`.
    AntiPattern(String),
    /// A recognized callout (`**Problem**:`, `**Pattern**:`, etc.) with text ≥ 10 chars.
    Callout(String),
    /// A directive bullet (`Avoid`, `Never`, `Prefer`, `❌`, etc.) with text ≥ 10 chars.
    Directive(String),
}

/// Classify a single trimmed markdown line (outside a code fence) into an
/// [`MdLineKind`], or return `None` for lines that produce no rule.
///
/// The `in_anti_patterns` flag indicates whether the enclosing `##` section
/// is an anti-patterns section; it governs `### ` sub-heading handling.
/// Fence and current-section state are the caller's responsibility.
fn classify_md_line(trimmed: &str, in_anti_patterns: bool) -> Option<MdLineKind> {
    if let Some(section) = trimmed.strip_prefix("## ") {
        return Some(MdLineKind::Section(section.trim().to_string()));
    }

    if let Some(sub) = trimmed.strip_prefix("### ") {
        if in_anti_patterns {
            let title = strip_numbered_prefix(sub.trim());
            if title.len() >= 5 {
                return Some(MdLineKind::AntiPattern(format!("Avoid: {}", title)));
            }
        }
        return None;
    }

    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }

    if let Some((_kind, text)) = parse_callout(trimmed) {
        return if text.len() >= 10 {
            Some(MdLineKind::Callout(text.to_string()))
        } else {
            None
        };
    }

    if let Some(constraint_text) = strip_directive_prefix(trimmed)
        && constraint_text.len() >= 10
    {
        return Some(MdLineKind::Directive(constraint_text.to_string()));
    }

    None
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
    let source_slug = slugify(
        source_file
            .rsplit('/')
            .next()
            .unwrap_or(source_file)
            .trim_end_matches(".md"),
    );

    let rules: Vec<Rule> = {
        let mut rules = Vec::new();
        let mut in_code_fence = false;
        let mut current_section: Option<String> = None;

        for line in content.lines() {
            let trimmed = line.trim();

            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                in_code_fence = !in_code_fence;
                continue;
            }
            if in_code_fence {
                continue;
            }

            if let Some(kind) = classify_md_line(
                trimmed,
                section_is_anti_patterns(current_section.as_deref()),
            ) {
                match kind {
                    MdLineKind::Section(s) => current_section = Some(s),
                    MdLineKind::AntiPattern(text) => {
                        rules.push(make_rule(ExtractedRuleInput {
                            source_slug: &source_slug,
                            idx: rules.len() + 1,
                            source_file,
                            section: current_section.as_deref(),
                            text: &text,
                        }));
                    }
                    MdLineKind::Callout(text) => {
                        rules.push(make_rule(ExtractedRuleInput {
                            source_slug: &source_slug,
                            idx: rules.len() + 1,
                            source_file,
                            section: current_section.as_deref(),
                            text: &text,
                        }));
                    }
                    MdLineKind::Directive(text) => {
                        rules.push(make_rule(ExtractedRuleInput {
                            source_slug: &source_slug,
                            idx: rules.len() + 1,
                            source_file,
                            section: current_section.as_deref(),
                            text: &text,
                        }));
                    }
                }
            }
        }

        rules
    };

    rules
}

struct ExtractedRuleInput<'a> {
    source_slug: &'a str,
    idx: usize,
    source_file: &'a str,
    section: Option<&'a str>,
    text: &'a str,
}

fn make_rule(input: ExtractedRuleInput<'_>) -> Rule {
    let id = match input.section {
        Some(s) => format!("{}-{}-{}", input.source_slug, slugify(s), input.idx),
        None => format!("{}-{}", input.source_slug, input.idx),
    };
    let mut condition_args = vec![input.source_file.to_string()];
    if let Some(s) = input.section {
        condition_args.push(s.to_string());
    }
    // SPEC-extract-rules-defaults (scoped slice for 0.14.0):
    // - Default action is `warn` (constraint_warning), not `block`. Block was
    //   too sharp for advisory pattern reminders — every pre-check fired every
    //   rule in a section context simultaneously, blocking the model.
    // - The bracketed `[<kind>]` prefix is stripped from the user-facing
    //   message; it was an extraction-time discriminator that leaked into
    //   the sentence the model reads at hook time.
    Rule {
        id,
        priority: 5,
        conditions: vec![Condition {
            predicate: "markdown_rule".to_string(),
            args: condition_args,
            script: None,
        }],
        actions: vec![Action {
            action_type: "constraint_warning".to_string(),
            params: vec![input.text.to_string()],
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

/// Dispatch a `list_facts` query to the engine's 0.11 fact-query API.
/// Precedence: predicate set → single predicate (+ optional positional arg
/// filters) → all facts. Pulled out as a pure function so the dispatch is
/// unit-testable without the MCP `CallToolResult` envelope.
fn select_facts(
    network: &phr::ReteNetwork,
    params: &PredicateFilter,
) -> Result<Vec<phr::Fact>, phr::ReteError> {
    if let Some(predicates) = &params.predicates {
        let refs: Vec<&str> = predicates.iter().map(String::as_str).collect();
        network.facts_matching_predicates(&refs)
    } else if let Some(predicate) = &params.predicate {
        let filters: Vec<(usize, &str)> = params
            .arg_filters
            .iter()
            .flatten()
            .map(|f| (f.index, f.value.as_str()))
            .collect();
        network.facts_matching(predicate, &filters)
    } else {
        network.facts_snapshot()
    }
}

#[cfg(test)]
mod list_facts_query_tests {
    use super::*;

    fn fact(id: &str, predicate: &str, args: &[&str]) -> phr::Fact {
        phr::Fact {
            id: id.into(),
            predicate: predicate.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            timestamp: 0,
        }
    }

    #[tokio::test]
    async fn select_facts_dispatches_all_three_query_shapes() {
        let net = phr::ReteNetwork::new();
        for f in [
            fact("e1", "equipped", &["alice", "head"]),
            fact("e2", "equipped", &["bob", "head"]),
            fact("g1", "gold", &["alice", "30"]),
        ] {
            net.assert_fact(f).await.unwrap();
        }

        // no params → every fact
        let all = select_facts(
            &net,
            &PredicateFilter {
                predicate: None,
                predicates: None,
                arg_filters: None,
            },
        )
        .unwrap();
        assert_eq!(all.len(), 3);

        // single predicate + positional arg filter
        let alice_equipped = select_facts(
            &net,
            &PredicateFilter {
                predicate: Some("equipped".into()),
                predicates: None,
                arg_filters: Some(vec![ArgFilter {
                    index: 0,
                    value: "alice".into(),
                }]),
            },
        )
        .unwrap();
        assert_eq!(alice_equipped.len(), 1);
        assert_eq!(alice_equipped[0].id, "e1");

        // predicate set membership
        let set = select_facts(
            &net,
            &PredicateFilter {
                predicate: None,
                predicates: Some(vec!["equipped".into(), "gold".into()]),
                arg_filters: None,
            },
        )
        .unwrap();
        assert_eq!(set.len(), 3);
    }
}

#[cfg(test)]
mod tool_registration_tests {
    use super::*;

    /// Regression: the stats tool was once registered as `get_values` (a
    /// leftover from a codebase-wide `Stats` → `Values` rename). The CLI
    /// subcommand was correctly reverted to `phr-mcp stats`, but the MCP
    /// method name was missed in the same pass, leaving the user-facing
    /// surface inconsistent. This test guards against the mismatch
    /// reappearing — the MCP tool name must match the CLI subcommand.
    #[test]
    fn stats_tool_registered_as_get_stats_not_get_values() {
        let mcp = EpistemeMcp::new();
        assert!(
            mcp.tool_router.has_route("get_stats"),
            "get_stats tool must be registered (matches `phr-mcp stats` CLI). Registered tools: {:?}",
            mcp.tool_router
                .list_all()
                .iter()
                .map(|t| t.name.to_string())
                .collect::<Vec<_>>()
        );
        assert!(
            !mcp.tool_router.has_route("get_values"),
            "get_values must NOT be registered — the surface uses `stats`, not `values`"
        );
    }

    /// One drift tool, not three. The three removed names must NOT be
    /// registered — an incomplete removal is the failure this catches, and
    /// it is the same class of SPEC-vs-code gap the previous versions of
    /// these assertions were written to guard against.
    #[test]
    fn drift_is_one_tool_and_the_three_old_names_are_gone() {
        let mcp = EpistemeMcp::new();
        assert!(
            mcp.tool_router.has_route("get_drift"),
            "get_drift tool must be registered (matches `phr-mcp drift` CLI)"
        );
        for gone in ["get_claude_md_drift", "get_memory_drift", "get_wiki_drift"] {
            assert!(
                !mcp.tool_router.has_route(gone),
                "{gone} must be removed — superseded by get_drift(source)"
            );
        }
    }

    /// The confidence-scoring MCP tools are registered.
    #[test]
    fn confidence_tools_are_registered() {
        let mcp = EpistemeMcp::new();
        assert!(
            mcp.tool_router.has_route("get_confidence"),
            "get_confidence tool must be registered"
        );
        assert!(
            mcp.tool_router.has_route("submit_suggestion"),
            "submit_suggestion tool must be registered"
        );
    }

    /// Broader regression: no MCP tool should carry the `values` naming
    /// the broken sweep introduced. Catches future drift in either
    /// direction.
    #[test]
    fn no_registered_tool_uses_values_naming() {
        let mcp = EpistemeMcp::new();
        let stragglers: Vec<String> = mcp
            .tool_router
            .list_all()
            .iter()
            .map(|t| t.name.to_string())
            .filter(|n| n.contains("values"))
            .collect();
        assert!(
            stragglers.is_empty(),
            "no MCP tool should carry 'values' in its name — the surface uses 'stats'. Found: {:?}",
            stragglers
        );
    }

    /// Regression: `get_journey` (0.13.0) is registered.
    #[test]
    fn journey_tool_is_registered() {
        let mcp = EpistemeMcp::new();
        assert!(
            mcp.tool_router.has_route("get_journey"),
            "get_journey tool must be registered (matches `phr-mcp journey` CLI)"
        );
    }

    /// Regression: `query_code_graph` (0.9.0) is registered.
    #[test]
    fn code_graph_query_tool_is_registered() {
        let mcp = EpistemeMcp::new();
        assert!(
            mcp.tool_router.has_route("query_code_graph"),
            "query_code_graph tool must be registered (matches `phr-mcp graph query` CLI)"
        );
        assert!(
            mcp.tool_router.has_route("get_code_graph_status"),
            "get_code_graph_status must expose `phr-mcp graph status` over MCP"
        );
        assert!(
            mcp.tool_router.has_route("rebuild_code_graph"),
            "rebuild_code_graph must expose `phr-mcp graph rebuild` over MCP"
        );
    }
}

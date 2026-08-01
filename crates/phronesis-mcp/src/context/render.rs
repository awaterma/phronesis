//! Structured context rendering.
//!
//! One function builds the complete picture of a context payload: which items
//! were candidates, which were admitted, what each cost, and why the rest were
//! dropped. The live hook path and `phr-mcp context inspect` are both
//! projections of that single result, so what `inspect` reports is by
//! construction what the hook would emit.
//!
//! Rendering writes nothing. Metric recording is the caller's step, which is
//! what keeps `inspect` from contaminating the log it exists to explain.

use std::path::Path;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::action_log::{self, LogEntry, ReadOpts};
use crate::rules_file::{self, RulesFile};

use super::capsule;
use super::config::{self, ConfigError, ContextConfig};
use super::packing::{self, ContextItem, ItemKind, PackedContext, Severity};

/// The host lifecycle events Phronesis renders for. Unsupported events are
/// never synthesized — an adapter that lacks a capability simply never
/// constructs the corresponding variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextEvent {
    Session,
    Interaction,
    PostCompact,
}

impl ContextEvent {
    /// Default `event` label for the observation record.
    pub fn metric_event(self) -> &'static str {
        match self {
            Self::Session => "session_context",
            Self::Interaction => "interaction_context",
            Self::PostCompact => "post_compact_context",
        }
    }

    /// The event name echoed in the hook envelope. Claude Code validates it;
    /// Gemini reads only `additionalContext` and ignores it.
    pub fn hook_event_name(self) -> &'static str {
        match self {
            Self::Interaction => "UserPromptSubmit",
            Self::Session | Self::PostCompact => "SessionStart",
        }
    }

    /// Session and PostCompact both render the charter.
    fn renders_charter(self) -> bool {
        !matches!(self, Self::Interaction)
    }
}

/// How the configuration that governed this render was obtained.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ConfigStatus {
    /// `.phronesis/context.json` parsed and validated.
    Loaded,
    /// The file exists but is malformed or invalid; bounded defaults were used
    /// and the reason is reported rather than silently absorbed.
    Defaulted { reason: String },
}

/// An item that was offered to the packer, with the cost it would carry on its
/// own. The packer measures the *increment* in context (headings, separators),
/// which can exceed this; `body_bytes` is the item's own contribution.
#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    pub kind: ItemKind,
    pub stable_id: String,
    pub body_bytes: usize,
    pub estimated_tokens: usize,
    pub priority: i32,
    pub severity: Severity,
}

impl From<&ContextItem> for Candidate {
    fn from(item: &ContextItem) -> Self {
        Self {
            kind: item.kind,
            stable_id: item.stable_id.clone(),
            body_bytes: item.body.len(),
            estimated_tokens: packing::estimate_tokens(item.body.len()),
            priority: item.priority,
            severity: item.severity,
        }
    }
}

/// Everything one context construction produced. Side-effect free.
#[derive(Debug)]
pub struct RenderResult {
    pub event: ContextEvent,
    /// Observation label. Defaults to `event.metric_event()`; a host adapter
    /// may override it to distinguish an event that renders the charter but
    /// sits outside the durability contract (e.g. Codex `SubagentStart`).
    pub metric_event: String,
    pub config: ContextConfig,
    pub config_status: ConfigStatus,
    pub candidates: Vec<Candidate>,
    pub packed: PackedContext,
    /// Ids of capsules that loaded and validated.
    pub capsules_loaded: Vec<String>,
    /// Per-file capsule load failures, path-scoped.
    pub capsule_diagnostics: Vec<String>,
    /// Capsules whose conditions matched the hydrated facts.
    pub matched_capsules: Vec<String>,
    /// Why a demanded fact could not be produced, when that happened.
    pub hydration_diagnostics: Vec<String>,
    pub latency: Duration,
}

impl RenderResult {
    /// Would the envelope's last-resort guard cut the body?
    ///
    /// Normal packing must never require this. A `true` here is a renderer
    /// bug, which is exactly why it is measured against the same limit the
    /// envelope enforces rather than assumed away.
    pub fn raw_truncation(&self) -> bool {
        self.packed.body.len() > self.config.hard_max_bytes
    }

    pub fn bytes(&self) -> usize {
        self.packed.body.len()
    }

    pub fn estimated_tokens(&self) -> usize {
        packing::estimate_tokens(self.packed.body.len())
    }

    /// The wrapped payload a hook would print, or an empty string when there
    /// is nothing worth injecting.
    pub fn envelope(&self) -> String {
        if self.packed.body.is_empty() {
            return String::new();
        }
        super::wrap_additional_context(
            self.event.hook_event_name(),
            &self.packed.body,
            self.config.hard_max_bytes,
        )
    }

    /// All diagnostics, in report order. Never enters the prompt body.
    pub fn diagnostics(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let ConfigStatus::Defaulted { reason } = &self.config_status {
            out.push(format!("{reason}; using bounded defaults"));
        }
        out.extend(self.capsule_diagnostics.iter().cloned());
        out.extend(self.hydration_diagnostics.iter().cloned());
        if self.raw_truncation() {
            out.push(format!(
                "internal context error: packed body is {} bytes, over the {} byte hard limit; \
                 the envelope guard will truncate",
                self.packed.body.len(),
                self.config.hard_max_bytes
            ));
        }
        out
    }
}

/// A dry-run view of one render. Both `--json` and the human table are
/// projections of this single value, so they cannot disagree.
#[derive(Debug, Serialize)]
pub struct InspectReport {
    pub event: ContextEvent,
    pub config_status: ConfigStatus,
    pub config: ContextConfig,
    pub bytes: usize,
    pub estimated_tokens: usize,
    /// Whether the envelope's last-resort guard would cut the body. Always
    /// false unless the packer has a bug.
    pub raw_truncation: bool,
    pub candidates: Vec<Candidate>,
    pub selected: Vec<String>,
    pub omitted: Vec<packing::OmittedItem>,
    pub capsules_loaded: Vec<String>,
    pub matched_capsules: Vec<String>,
    pub capsule_diagnostics: Vec<String>,
    pub hydration_diagnostics: Vec<String>,
    pub latency_micros: u64,
    pub body: String,
}

impl RenderResult {
    pub fn report(&self) -> InspectReport {
        InspectReport {
            event: self.event,
            config_status: self.config_status.clone(),
            config: self.config.clone(),
            bytes: self.bytes(),
            estimated_tokens: self.estimated_tokens(),
            raw_truncation: self.raw_truncation(),
            candidates: self.candidates.clone(),
            selected: self.packed.selected.clone(),
            omitted: self.packed.omitted.clone(),
            capsules_loaded: self.capsules_loaded.clone(),
            matched_capsules: self.matched_capsules.clone(),
            capsule_diagnostics: self.capsule_diagnostics.clone(),
            hydration_diagnostics: self.hydration_diagnostics.clone(),
            latency_micros: u64::try_from(self.latency.as_micros()).unwrap_or(u64::MAX),
            body: self.packed.body.clone(),
        }
    }
}

impl InspectReport {
    /// Human projection. Written to a `String` rather than printed so the
    /// caller owns the output stream.
    pub fn to_text(&self) -> String {
        use std::fmt::Write;
        let mut out = String::new();
        let event = serde_json::to_value(self.event)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        let _ = writeln!(out, "event:  {event}");
        match &self.config_status {
            ConfigStatus::Loaded => {
                let _ = writeln!(out, "config: .phronesis/context.json (loaded)");
            }
            ConfigStatus::Defaulted { reason } => {
                let _ = writeln!(out, "config: bounded defaults — {reason}");
            }
        }
        let _ = writeln!(
            out,
            "budget: {} / {} bytes, {} / {} estimated tokens",
            self.bytes,
            self.config.hard_max_bytes,
            self.estimated_tokens,
            self.config
                .estimated_max_tokens
                .map(|t| t.to_string())
                .unwrap_or_else(|| "unlimited".to_string()),
        );
        let _ = writeln!(out, "latency: {}µs", self.latency_micros);
        if self.raw_truncation {
            let _ = writeln!(
                out,
                "WARNING: body exceeds the hard limit; the envelope guard would truncate"
            );
        }

        let _ = writeln!(out, "\ncandidates ({}):", self.candidates.len());
        for candidate in &self.candidates {
            let kind = serde_json::to_value(candidate.kind)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            let verdict = if self.selected.contains(&candidate.stable_id) {
                "selected".to_string()
            } else {
                self.omitted
                    .iter()
                    .find(|o| o.stable_id == candidate.stable_id)
                    .map(|o| {
                        serde_json::to_value(o.reason)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_string))
                            .unwrap_or_default()
                    })
                    .unwrap_or_else(|| "not offered".to_string())
            };
            let _ = writeln!(
                out,
                "  [{kind:<11}] {:<40} {:>5}B {:>5}t  {verdict}",
                candidate.stable_id, candidate.body_bytes, candidate.estimated_tokens,
            );
        }

        let _ = writeln!(
            out,
            "\nkind ceilings: kernel={} activity_reserve={} nudges={} \
             state_reserve={} charter={} rules={}",
            self.config.interaction.kernel_max_bytes,
            self.config.interaction.activity_reserve_bytes,
            self.config.interaction.nudges_max_bytes,
            self.config.session.state_reserve_bytes,
            self.config.session.charter_max_bytes,
            self.config.session.rules_max_bytes,
        );

        let _ = writeln!(
            out,
            "\ncapsules: {} loaded, {} matched",
            self.capsules_loaded.len(),
            self.matched_capsules.len()
        );
        for id in &self.capsules_loaded {
            let mark = if self.matched_capsules.contains(id) {
                "match"
            } else {
                "no match"
            };
            let _ = writeln!(out, "  {id} — {mark}");
        }
        for diagnostic in self
            .capsule_diagnostics
            .iter()
            .chain(&self.hydration_diagnostics)
        {
            let _ = writeln!(out, "  ! {diagnostic}");
        }

        if !self.body.is_empty() {
            let _ = writeln!(out, "\n--- rendered body ---\n{}", self.body);
        }
        out
    }
}

/// Build the context payload for one event.
///
/// Returns `None` when `.phronesis/context.json` is absent — the project has
/// not opted in, and the caller must fall back to the byte-for-byte legacy
/// renderer.
pub async fn render(root: &Path, event: ContextEvent, last_n: usize) -> Option<RenderResult> {
    let started = Instant::now();
    let (config, config_status) = match config::load(root) {
        Ok(config) => (config, ConfigStatus::Loaded),
        Err(ConfigError::NotFound(_)) => return None,
        Err(error) => (
            ContextConfig::default(),
            ConfigStatus::Defaulted {
                reason: error.to_string(),
            },
        ),
    };

    // The kernel comes from `kernel.md` only. An existing `durable.md` is a
    // session-level document and is never squeezed into the per-turn budget.
    let kernel = kernel_items(&super::read_durable_kernel(root));
    let mut candidates: Vec<Candidate> = kernel.iter().map(Candidate::from).collect();
    let mut capsules_loaded = Vec::new();
    let mut capsule_diagnostics = Vec::new();
    let mut matched_capsules = Vec::new();
    let mut hydration_diagnostics = Vec::new();

    let packed = if event.renders_charter() {
        let state = state_items(root);
        let charter = charter_items(&super::read_durable_directives(root));
        let rules = rules_file::read(&rules_file::default_path(root))
            .map(|rules| rule_items(&rules))
            .unwrap_or_default();
        let orientation = orientation_item();
        candidates.extend(state.iter().map(Candidate::from));
        candidates.extend(charter.iter().map(Candidate::from));
        candidates.extend(rules.iter().map(Candidate::from));
        candidates.push(Candidate::from(&orientation));
        packing::pack_session(
            &config,
            &state,
            &kernel,
            &charter,
            &rules,
            Some(&orientation),
        )
    } else {
        let now = unix_now();
        let activity = activity_items(&recent_hook_entries(root, last_n), now);
        // No capsule evaluation at a charter event: capsules are selected by
        // current-interaction facts, and session construction must not invent
        // an event merely to trigger them.
        let loaded = capsule::load(root);
        capsules_loaded = loaded.capsules.iter().map(|c| c.id.clone()).collect();
        capsule_diagnostics = loaded.diagnostics.clone();
        // An undeclared journey selector surfaces through the derivation
        // error below (`validate_selectors` names the rule and the selector),
        // so there is no separate pre-check to duplicate it here.
        let outcome = capsule::matched(root, &loaded.capsules, now).await;
        matched_capsules = outcome.matched_ids.clone();
        hydration_diagnostics.extend(outcome.diagnostics);
        let nudges = outcome
            .items
            .into_iter()
            .map(|mut item| {
                item.stable_id = format!("nudge:{}", item.stable_id);
                item
            })
            .collect::<Vec<_>>();
        candidates.extend(activity.iter().map(Candidate::from));
        candidates.extend(nudges.iter().map(Candidate::from));
        packing::pack_interaction(&config, &activity, &kernel, &nudges)
    };

    Some(RenderResult {
        event,
        metric_event: event.metric_event().to_string(),
        config,
        config_status,
        candidates,
        packed,
        capsules_loaded,
        capsule_diagnostics,
        matched_capsules,
        hydration_diagnostics,
        latency: started.elapsed(),
    })
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn recent_hook_entries(project_root: &Path, last_n: usize) -> Vec<LogEntry> {
    action_log::read_recent(
        &action_log::default_path(project_root),
        &ReadOpts {
            limit: Some(last_n),
            kind: Some("hook".to_string()),
            ..ReadOpts::default()
        },
    )
    .unwrap_or_default()
}

/// Split Markdown into indivisible `##` sections, in file order.
///
/// The section — heading, lead-in, and the list or steps under it — is the
/// unit, not the blank-line paragraph. Splitting on blank lines lets a
/// too-large list be dropped while its lead-in survives, which produces text
/// like "Three heuristic tools ...:" followed by nothing: a document with
/// holes that still reads as complete. A section drops whole or not at all.
///
/// Content before the first `##` (a title, a preamble) is its own leading
/// section. `###` and deeper stay inside their parent section, which is what
/// keeps a procedure attached to the sentence introducing it.
///
/// Phronesis never rewrites the source file.
pub(crate) fn section_items(content: &str, kind: ItemKind, prefix: &str) -> Vec<ContextItem> {
    let mut sections: Vec<String> = Vec::new();
    let mut current = String::new();
    for line in content.lines() {
        let is_section_heading = line.starts_with("## ") && !line.starts_with("### ");
        if is_section_heading && !current.trim().is_empty() {
            sections.push(current.trim_end().to_string());
            current = String::new();
        }
        current.push_str(line);
        current.push('\n');
    }
    if !current.trim().is_empty() {
        sections.push(current.trim_end().to_string());
    }
    sections
        .into_iter()
        .filter(|s| !s.trim().is_empty())
        .enumerate()
        .map(|(i, body)| ContextItem::new(kind, format!("{prefix}:{i}"), body))
        .collect()
}

/// The always-on kernel, from `.phronesis/kernel.md`.
pub(crate) fn kernel_items(content: &str) -> Vec<ContextItem> {
    section_items(content, ItemKind::Kernel, "kernel")
}

/// The session-level project document, from `.phronesis/durable.md`.
pub(crate) fn charter_items(content: &str) -> Vec<ContextItem> {
    section_items(content, ItemKind::Charter, "charter")
}

fn orientation_item() -> ContextItem {
    ContextItem::new(
        ItemKind::Orientation,
        "orientation:mcp",
        "Detailed rules, decisions, graph facts, journey, and confidence are available \
         through the Phronesis MCP tools.",
    )
}

/// Activity in the normative order: current-event block, current-event
/// warning, older blocks newest-first, older warnings newest-first, with rule
/// id then file path as stable tie-breakers.
///
/// `entries` arrives oldest-first from `read_recent`, so the last entry is the
/// most recent hook decision — the closest thing the log offers to "the
/// current event".
pub(crate) fn activity_items(entries: &[LogEntry], now: u64) -> Vec<ContextItem> {
    let newest_index = entries.len().checked_sub(1);
    let mut records: Vec<(ActivityKey, ContextItem)> = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        for (ordinal, decision) in super::entry_decisions(entry, now).into_iter().enumerate() {
            let mut item = ContextItem::new(
                ItemKind::Activity,
                format!("activity:{}:{}:{ordinal}", entry.ts, decision.rule_id),
                decision.bullet,
            );
            item.severity = decision.severity;
            item.priority = match decision.severity {
                Severity::Block => 2,
                Severity::Warning => 1,
                Severity::None => 0,
            };
            records.push((
                ActivityKey {
                    group: u8::from(Some(index) != newest_index),
                    severity: decision.severity,
                    ts: entry.ts,
                    rule_id: decision.rule_id,
                    file: decision.file,
                },
                item,
            ));
        }
    }
    records.sort_by(|a, b| {
        a.0.group
            .cmp(&b.0.group)
            .then_with(|| b.0.severity.cmp(&a.0.severity))
            .then_with(|| b.0.ts.cmp(&a.0.ts))
            .then_with(|| a.0.rule_id.cmp(&b.0.rule_id))
            .then_with(|| a.0.file.cmp(&b.0.file))
    });
    records.into_iter().map(|(_, item)| item).collect()
}

struct ActivityKey {
    /// 0 for the current event, 1 for everything older.
    group: u8,
    severity: Severity,
    ts: u64,
    rule_id: String,
    file: String,
}

pub(crate) fn rule_items(rules: &RulesFile) -> Vec<ContextItem> {
    let mut items = rules
        .rules
        .iter()
        .filter(|r| r.silent != Some(true))
        .map(|r| {
            let mut item = ContextItem::new(
                ItemKind::Rule,
                format!("rule:{}", r.id),
                format!("- {} — {}", r.id, super::rule_intent(r)),
            );
            item.priority = r.priority;
            item.severity = if r
                .actions
                .iter()
                .any(|a| a.action_type == "constraint_violation")
            {
                Severity::Block
            } else if r
                .actions
                .iter()
                .any(|a| a.action_type == "constraint_warning")
            {
                Severity::Warning
            } else {
                Severity::None
            };
            item
        })
        .collect::<Vec<_>>();
    items.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then_with(|| b.priority.cmp(&a.priority))
            .then_with(|| a.stable_id.cmp(&b.stable_id))
    });
    items
}

/// State lines in fixed order: open subject, confidence band, freshness
/// diagnostic.
///
/// Confidence is a projection of the outcomes report and appears only when the
/// project configured confidence *and* has an open subject. Without both there
/// is no band to report and no substitute is invented.
pub(crate) fn state_items(root: &Path) -> Vec<ContextItem> {
    let mut items = Vec::new();
    if crate::outcomes::enabled(root)
        && let Some(report) = crate::outcomes::report(root, None)
    {
        items.push(ContextItem::new(
            ItemKind::State,
            "state:subject",
            format!("- Open subject: {}", report.subject),
        ));
        items.push(ContextItem::new(
            ItemKind::State,
            "state:confidence",
            format!("- Confidence: {}", report.band.as_str()),
        ));
    }
    if let Some(line) = graph_freshness_line(root) {
        items.push(ContextItem::new(ItemKind::State, "state:graph", line));
    }
    items
}

/// A one-line graph-freshness diagnostic, or `None` when the project has no
/// graph index — absence of the graph is not a state worth a line.
fn graph_freshness_line(root: &Path) -> Option<String> {
    use crate::graph::sync::{Freshness, check_freshness, index_path, load_index};
    let path = index_path(root);
    if !path.exists() {
        return None;
    }
    let index = load_index(&path).ok()?;
    Some(match check_freshness(root, &index) {
        Freshness::Fresh => "- Code graph: current".to_string(),
        Freshness::Stale(files) => format!(
            "- Code graph: stale ({} file(s) changed outside the hook); run `phr-mcp graph rebuild`",
            files.len()
        ),
        Freshness::Outdated { .. } => {
            "- Code graph: built under an older identity scheme; run `phr-mcp graph rebuild`"
                .to_string()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: u64 = 1_700_000_000;

    fn hook_entry(ts: u64, file: &str, decisions: &[(&str, &str)]) -> LogEntry {
        let consequences = decisions
            .iter()
            .map(|(rule_id, action_type)| {
                json!({
                    "rule_id": rule_id,
                    "action_type": action_type,
                    "message": "m",
                    "bindings": {},
                })
            })
            .collect::<Vec<_>>();
        let mut entry = LogEntry::new("hook", "pre_check")
            .with("file", file.to_string())
            .with("consequences", json!(consequences));
        entry.ts = ts;
        entry
    }

    fn bodies(items: &[ContextItem]) -> Vec<&str> {
        items.iter().map(|i| i.body.as_str()).collect()
    }

    fn opted_in(dir: &Path) {
        std::fs::create_dir_all(dir.join(".phronesis")).expect("mkdir .phronesis");
        let config = serde_json::to_string(&ContextConfig::default()).expect("serialize config");
        std::fs::write(dir.join(".phronesis/context.json"), config).expect("write config");
    }

    #[test]
    fn activity_puts_the_current_event_first_regardless_of_severity_elsewhere() {
        // The current event only warned; an older event blocked. The current
        // event still leads, because it describes what just happened.
        let entries = [
            hook_entry(
                NOW - 500,
                "src/old.rs",
                &[("old-block", "constraint_violation")],
            ),
            hook_entry(NOW, "src/now.rs", &[("now-warn", "constraint_warning")]),
        ];
        let items = activity_items(&entries, NOW);
        assert_eq!(
            bodies(&items),
            [
                "- WARNED 0s ago: now-warn in src/now.rs",
                "- BLOCKED 8m ago: old-block in src/old.rs",
            ]
        );
    }

    #[test]
    fn within_the_current_event_blocks_precede_warnings() {
        let entries = [hook_entry(
            NOW,
            "src/a.rs",
            &[("w", "constraint_warning"), ("b", "constraint_violation")],
        )];
        let items = activity_items(&entries, NOW);
        assert_eq!(
            bodies(&items),
            [
                "- BLOCKED 0s ago: b in src/a.rs",
                "- WARNED 0s ago: w in src/a.rs",
            ]
        );
    }

    #[test]
    fn older_items_are_all_blocks_newest_first_then_all_warnings_newest_first() {
        let entries = [
            hook_entry(
                NOW - 3000,
                "src/c.rs",
                &[("old-warn", "constraint_warning")],
            ),
            hook_entry(
                NOW - 2000,
                "src/b.rs",
                &[("mid-block", "constraint_violation")],
            ),
            hook_entry(
                NOW - 1000,
                "src/a.rs",
                &[("new-warn", "constraint_warning")],
            ),
            hook_entry(NOW, "src/current.rs", &[("cur", "constraint_violation")]),
        ];
        let items = activity_items(&entries, NOW);
        assert_eq!(
            bodies(&items),
            [
                "- BLOCKED 0s ago: cur in src/current.rs",
                "- BLOCKED 33m ago: mid-block in src/b.rs",
                "- WARNED 16m ago: new-warn in src/a.rs",
                "- WARNED 50m ago: old-warn in src/c.rs",
            ],
            "older blocks come before older warnings, each newest-first"
        );
    }

    #[test]
    fn equal_timestamps_break_ties_on_rule_id_then_file() {
        let entries = [
            hook_entry(NOW - 10, "src/z.rs", &[("same", "constraint_violation")]),
            hook_entry(NOW - 10, "src/a.rs", &[("same", "constraint_violation")]),
            hook_entry(NOW - 10, "src/m.rs", &[("aaa", "constraint_violation")]),
            hook_entry(NOW, "src/current.rs", &[("cur", "constraint_violation")]),
        ];
        let items = activity_items(&entries, NOW);
        assert_eq!(
            bodies(&items),
            [
                "- BLOCKED 0s ago: cur in src/current.rs",
                "- BLOCKED 10s ago: aaa in src/m.rs",
                "- BLOCKED 10s ago: same in src/a.rs",
                "- BLOCKED 10s ago: same in src/z.rs",
            ]
        );
    }

    #[test]
    fn activity_ordering_is_stable_across_calls() {
        let entries = [
            hook_entry(NOW - 5, "src/b.rs", &[("r", "constraint_warning")]),
            hook_entry(NOW - 5, "src/a.rs", &[("r", "constraint_warning")]),
            hook_entry(NOW, "src/c.rs", &[("r", "constraint_violation")]),
        ];
        assert_eq!(
            bodies(&activity_items(&entries, NOW)),
            bodies(&activity_items(&entries, NOW))
        );
    }

    #[test]
    fn entries_without_surfacing_consequences_produce_no_items() {
        let entries = [hook_entry(NOW, "src/a.rs", &[("r", "log")])];
        assert!(activity_items(&entries, NOW).is_empty());
    }

    #[test]
    fn sections_split_on_h2_headings_in_file_order() {
        let items =
            kernel_items("# Title\n\nPreamble.\n\n## One\n\nBody one.\n\n## Two\n\nBody two.");
        assert_eq!(
            bodies(&items),
            [
                "# Title\n\nPreamble.",
                "## One\n\nBody one.",
                "## Two\n\nBody two."
            ]
        );
        assert_eq!(items[0].stable_id, "kernel:0");
    }

    #[test]
    fn a_section_keeps_its_lead_in_and_list_together() {
        // The regression this split exists to prevent: a lead-in sentence
        // ending in a colon surviving while the list it introduces is dropped,
        // leaving text that promises content it does not deliver.
        let content = "## Drift discipline\n\nThree tools surface the gap:\n\n\
                       - one\n- two\n- three\n\n## Next\n\nUnrelated.";
        let items = kernel_items(content);
        assert_eq!(items.len(), 2, "one item per H2 section");
        assert!(items[0].body.contains("Three tools surface the gap:"));
        assert!(
            items[0].body.contains("- three"),
            "the list must travel with its lead-in"
        );
    }

    #[test]
    fn h3_subsections_stay_inside_their_parent_section() {
        let content =
            "## Governance\n\nIntro.\n\n### Step list\n\nWhen X:\n\n1. do this\n2. do that";
        let items = kernel_items(content);
        assert_eq!(items.len(), 1);
        assert!(items[0].body.contains("1. do this"));
    }

    #[test]
    fn an_oversized_section_drops_whole_rather_than_leaving_a_lead_in() {
        let content = format!(
            "## Small\n\nok.\n\n## Big\n\nLead-in promising a list:\n\n{}",
            "- an item\n".repeat(80)
        );
        let items = charter_items(&content);
        let config = ContextConfig {
            hard_max_bytes: 4096,
            estimated_max_tokens: None,
            session: crate::context::config::SessionConfig {
                kernel_max_bytes: 4096,
                state_reserve_bytes: 0,
                charter_max_bytes: 120,
                rules_max_bytes: 4096,
            },
            ..ContextConfig::default()
        };
        let packed = packing::pack_session(&config, &[], &[], &items, &[], None);
        assert!(packed.body.contains("## Small"));
        assert!(
            !packed.body.contains("Lead-in promising a list:"),
            "a dropped section must not leave its lead-in behind:\n{}",
            packed.body
        );
    }

    #[test]
    fn state_is_empty_when_confidence_is_not_configured() {
        let d = tempfile::tempdir().expect("tempdir");
        crate::outcomes::subject::set(d.path(), "unit").expect("set subject");
        assert!(
            state_items(d.path()).is_empty(),
            "a subject alone must not produce a confidence line"
        );
    }

    #[test]
    fn state_reports_subject_and_band_once_confidence_is_configured() {
        let d = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(d.path().join(".phronesis")).expect("mkdir");
        std::fs::write(d.path().join(".phronesis/confidence.json"), "{}").expect("write marker");
        crate::outcomes::subject::set(d.path(), "unit").expect("set subject");
        let items = state_items(d.path());
        assert_eq!(
            items
                .iter()
                .map(|i| i.stable_id.as_str())
                .collect::<Vec<_>>(),
            ["state:subject", "state:confidence"]
        );
    }

    #[tokio::test]
    async fn render_returns_none_when_the_project_has_not_opted_in() {
        let d = tempfile::tempdir().expect("tempdir");
        assert!(
            render(d.path(), ContextEvent::Interaction, 5)
                .await
                .is_none(),
            "no context.json means the legacy renderer, not a packed one"
        );
    }

    #[tokio::test]
    async fn malformed_config_falls_back_to_defaults_and_says_so() {
        let d = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(d.path().join(".phronesis")).expect("mkdir");
        std::fs::write(d.path().join(".phronesis/context.json"), "{ not json").expect("write");
        std::fs::write(d.path().join(".phronesis/kernel.md"), "Be careful.").expect("write");

        let result = render(d.path(), ContextEvent::Interaction, 5)
            .await
            .expect("a malformed config must still render");
        assert!(matches!(
            result.config_status,
            ConfigStatus::Defaulted { .. }
        ));
        assert_eq!(result.config, ContextConfig::default());
        assert!(result.packed.body.contains("Be careful."));
        assert!(
            result.diagnostics().iter().any(|d| d.contains("malformed")),
            "the operator must see why defaults were used: {:?}",
            result.diagnostics()
        );
    }

    #[tokio::test]
    async fn rendering_writes_nothing() {
        // This is what makes `context inspect` a real dry run.
        let d = tempfile::tempdir().expect("tempdir");
        opted_in(d.path());
        std::fs::write(d.path().join(".phronesis/kernel.md"), "Be careful.").expect("write");

        let log = crate::action_log::default_path(d.path());
        let _ = render(d.path(), ContextEvent::Interaction, 5)
            .await
            .expect("opted in");
        let _ = render(d.path(), ContextEvent::Session, 5)
            .await
            .expect("opted in");
        assert!(
            !log.exists(),
            "render must not append a context observation"
        );
    }

    #[tokio::test]
    async fn a_normal_render_never_needs_the_last_resort_truncator() {
        let d = tempfile::tempdir().expect("tempdir");
        opted_in(d.path());
        std::fs::write(
            d.path().join(".phronesis/kernel.md"),
            "## Section\n\nA paragraph.\n\n".repeat(60),
        )
        .expect("write");
        let result = render(d.path(), ContextEvent::Interaction, 5)
            .await
            .expect("opted in");
        assert!(!result.raw_truncation());
        assert!(result.bytes() <= result.config.hard_max_bytes);
        assert!(
            !result.packed.omitted.is_empty(),
            "and it did omit material"
        );
    }

    #[tokio::test]
    async fn session_and_post_compact_render_the_same_charter() {
        let d = tempfile::tempdir().expect("tempdir");
        opted_in(d.path());
        std::fs::write(d.path().join(".phronesis/kernel.md"), "Kernel text.").expect("write");
        let session = render(d.path(), ContextEvent::Session, 5)
            .await
            .expect("opted in");
        let post = render(d.path(), ContextEvent::PostCompact, 5)
            .await
            .expect("opted in");
        assert_eq!(session.packed.body, post.packed.body);
        assert_eq!(session.event.hook_event_name(), "SessionStart");
        assert_ne!(
            session.metric_event, post.metric_event,
            "but they must remain distinguishable in the observations"
        );
    }

    #[tokio::test]
    async fn the_project_document_reaches_the_session_but_never_a_turn() {
        // The whole point of the split: an existing durable file is session
        // orientation, not per-turn cost.
        let d = tempfile::tempdir().expect("tempdir");
        opted_in(d.path());
        std::fs::write(d.path().join(".phronesis/kernel.md"), "Always-on core.")
            .expect("write kernel");
        std::fs::write(
            d.path().join(".phronesis/durable.md"),
            "# House rules\n\n## Review\n\nAlways review before merging.",
        )
        .expect("write durable");

        let interaction = render(d.path(), ContextEvent::Interaction, 5)
            .await
            .expect("opted in");
        assert!(interaction.packed.body.contains("Always-on core."));
        assert!(
            !interaction
                .packed
                .body
                .contains("Always review before merging."),
            "the session document must not ride along every turn:\n{}",
            interaction.packed.body
        );

        let session = render(d.path(), ContextEvent::Session, 5)
            .await
            .expect("opted in");
        assert!(session.packed.body.contains("Always-on core."));
        assert!(
            session
                .packed
                .body
                .contains("Always review before merging."),
            "but it must still be delivered once per session:\n{}",
            session.packed.body
        );
    }

    #[tokio::test]
    async fn an_absent_kernel_file_invents_no_substitute() {
        // Without `kernel.md` there is no always-on core. Falling back to
        // `durable.md` would silently restore the conflation this undoes.
        let d = tempfile::tempdir().expect("tempdir");
        opted_in(d.path());
        std::fs::write(d.path().join(".phronesis/durable.md"), "Session guidance.")
            .expect("write durable");
        let interaction = render(d.path(), ContextEvent::Interaction, 5)
            .await
            .expect("opted in");
        assert!(!interaction.packed.body.contains("Session guidance."));
    }

    #[test]
    fn an_oversized_source_file_is_ignored_rather_than_read_every_turn() {
        let d = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(d.path().join(".phronesis")).expect("mkdir");
        std::fs::write(d.path().join(".phronesis/kernel.md"), "x".repeat(70 * 1024))
            .expect("write kernel");
        assert_eq!(super::super::read_durable_kernel(d.path()), "");
    }

    #[tokio::test]
    async fn no_capsule_is_selected_at_a_charter_event() {
        let d = tempfile::tempdir().expect("tempdir");
        opted_in(d.path());
        std::fs::create_dir_all(d.path().join(".phronesis/nudges")).expect("mkdir");
        std::fs::write(
            d.path().join(".phronesis/nudges/x.md"),
            "---json\n{\"id\":\"x\",\"priority\":50,\"max_bytes\":128,\
             \"when\":{\"predicate\":\"context_confidence_band\",\"args\":[\"low\"]}}\n---\nnudge body\n",
        )
        .expect("write capsule");
        std::fs::write(d.path().join(".phronesis/confidence.json"), "{}").expect("write marker");
        crate::outcomes::subject::set(d.path(), "unit").expect("set subject");

        let session = render(d.path(), ContextEvent::Session, 5)
            .await
            .expect("opted in");
        assert!(
            session.matched_capsules.is_empty(),
            "session construction must not invent an interaction to trigger capsules"
        );
        assert!(!session.packed.body.contains("nudge body"));

        let interaction = render(d.path(), ContextEvent::Interaction, 5)
            .await
            .expect("opted in");
        assert_eq!(interaction.matched_capsules, ["x"]);
        assert!(interaction.packed.body.contains("nudge body"));
    }

    #[tokio::test]
    async fn the_report_agrees_with_what_the_hook_would_emit() {
        let d = tempfile::tempdir().expect("tempdir");
        opted_in(d.path());
        std::fs::write(d.path().join(".phronesis/kernel.md"), "Kernel text.").expect("write");
        let result = render(d.path(), ContextEvent::Interaction, 5)
            .await
            .expect("opted in");
        let report = result.report();
        assert_eq!(report.body, result.packed.body);
        assert_eq!(report.bytes, result.bytes());
        assert!(result.envelope().contains("Kernel text."));
        // Every selected id must have been offered as a candidate, apart from
        // the footer the packer synthesizes.
        for id in &report.selected {
            assert!(
                id == "omission-footer" || report.candidates.iter().any(|c| &c.stable_id == id),
                "selected `{id}` was never listed as a candidate"
            );
        }
    }
}

//! Derivation of Prometheus metric families from the action log.
//!
//! Pure: takes parsed log records, returns a populated [`Registry`]. No I/O,
//! no clock, no globals — which is what makes the whole surface unit-testable
//! against synthetic records.
//!
//! Why derive from the log rather than count in-process: the interesting half
//! of phronesis runs as short-lived hook processes (`pre-check`/`post-check`)
//! that exit long before any scrape could reach them. The log is the one sink
//! every writer already shares, so deriving from it gives complete coverage
//! for free.
//!
//! One hard rule the whole module obeys: a file path never appears as a label
//! value. Only `phase`, `tool`, `event`, `decision`, `rule_id`, `outcome`, and
//! omission-kind names are ever emitted.

use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::registry::Registry;
use std::collections::HashMap;

use crate::log::{LogRead, LogRecord};

/// The literal `rule_id` that over-cap rules are folded into.
pub const OTHER_RULE: &str = "__other__";

/// Knobs for a derivation pass.
#[derive(Debug, Clone)]
pub struct Options {
    /// Cap on distinct `rule_id` label values. Rule ids are author-defined and
    /// therefore unbounded; without a cap a project with generated rules would
    /// blow up Prometheus' series count. The top-N rules by fire count keep
    /// their identity, the rest are summed into [`OTHER_RULE`].
    pub max_rule_series: usize,
    /// When `Some(cutoff)`, ignore records with `ts < cutoff`.
    pub since: Option<u64>,
    /// Unused by the derivation: the time window is an absolute `since`, not a
    /// window relative to `now`. Present so the same struct can drive both the
    /// log-derived and the live families.
    pub now: u64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            max_rule_series: 100,
            since: None,
            now: 0,
        }
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct HookLabels {
    phase: String,
    tool: String,
    decision: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct RuleLabels {
    rule_id: String,
    outcome: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct RuleIdLabels {
    rule_id: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct ToolLabels {
    tool: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct EventLabels {
    event: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct OmitKindLabels {
    kind: String,
}

/// One rule's tallies, accumulated before the cardinality cap is applied.
#[derive(Default, Clone)]
struct RuleTally {
    blocked: u64,
    warned: u64,
    last_ts: u64,
}

impl RuleTally {
    fn total(&self) -> u64 {
        self.blocked + self.warned
    }

    fn merge(&mut self, other: &RuleTally) {
        self.blocked += other.blocked;
        self.warned += other.warned;
        self.last_ts = self.last_ts.max(other.last_ts);
    }
}

/// Classify a hook record into `block` / `warn` / `allow`.
///
/// A non-zero `exit` is the hook telling the harness to reject the edit, so it
/// dominates. Otherwise a `constraint_warning` consequence means the edit went
/// through but the author was told something.
fn hook_decision(rec: &LogRecord) -> &'static str {
    if rec.num("exit").unwrap_or(0) != 0 {
        return "block";
    }
    let warned = rec
        .consequences()
        .iter()
        .any(|c| c.get("action_type").and_then(|v| v.as_str()) == Some("constraint_warning"));
    if warned { "warn" } else { "allow" }
}

/// Walk `consequences` and fold them into per-rule tallies.
///
/// Mirrors the classification in `phronesis-mcp`'s `stats::aggregate` so the
/// `phr-mcp stats` table and the Prometheus counters can never disagree about
/// what "blocked" means. A `constraint_violation` is "blocked"; a
/// `constraint_warning` is "warned"; any other `action_type` is skipped
/// entirely.
fn tally_consequences(rec: &LogRecord, tallies: &mut HashMap<String, RuleTally>) {
    for c in rec.consequences() {
        let Some(rule_id) = c.get("rule_id").and_then(|v| v.as_str()) else {
            continue;
        };
        let entry = match c.get("action_type").and_then(|v| v.as_str()) {
            Some("constraint_violation") => {
                let e = tallies.entry(rule_id.to_string()).or_default();
                e.blocked += 1;
                e
            }
            Some("constraint_warning") => {
                let e = tallies.entry(rule_id.to_string()).or_default();
                e.warned += 1;
                e
            }
            // Unknown action types are not rule *enforcement* and would only
            // add noise; skip without creating an empty series.
            _ => continue,
        };
        entry.last_ts = entry.last_ts.max(rec.ts);
    }
}

/// Apply the cardinality cap: keep the top-N rules by total fires (ties broken
/// by `rule_id` ascending for determinism), fold the remainder into a single
/// [`OTHER_RULE`] series with its counts summed.
fn cap_rules(tallies: HashMap<String, RuleTally>, max: usize) -> Vec<(String, RuleTally)> {
    let mut rows: Vec<(String, RuleTally)> = tallies.into_iter().collect();
    rows.sort_by(|a, b| b.1.total().cmp(&a.1.total()).then_with(|| a.0.cmp(&b.0)));
    if rows.len() <= max {
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        return rows;
    }
    let overflow = rows.split_off(max);
    let mut other = RuleTally::default();
    for (_, tally) in &overflow {
        other.merge(tally);
    }
    rows.push((OTHER_RULE.to_string(), other));
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

/// Build a registry from one read of the log.
///
/// Counter names are registered *without* the `_total` suffix:
/// `prometheus-client` appends it during encoding, and registering it here
/// would render `phronesis_hook_checks_total_total`.
pub fn build(read: &LogRead, opts: &Options) -> Registry {
    let mut registry = Registry::default();

    let hook_checks = Family::<HookLabels, Counter>::default();
    let rule_fires = Family::<RuleLabels, Counter>::default();
    let rule_last_fired = Family::<RuleIdLabels, Gauge>::default();
    let mcp_tool_calls = Family::<ToolLabels, Counter>::default();
    let context_renders = Family::<EventLabels, Counter>::default();
    let context_tokens = Family::<EventLabels, Counter>::default();
    let context_bytes = Family::<EventLabels, Counter>::default();
    let context_latency = Family::<EventLabels, Counter>::default();
    let context_omitted = Family::<OmitKindLabels, Counter>::default();
    let log_entries = Counter::<u64>::default();
    let log_malformed = Counter::<u64>::default();
    let log_size = Gauge::<i64>::default();

    let mut rule_tallies: HashMap<String, RuleTally> = HashMap::new();
    let mut counted: u64 = 0;

    for rec in &read.records {
        if let Some(cutoff) = opts.since
            && rec.ts < cutoff
        {
            continue;
        }
        counted += 1;

        // Consequences can ride on any record kind, so tally them first.
        tally_consequences(rec, &mut rule_tallies);

        match rec.kind.as_str() {
            "hook" => {
                // NOTE: only `phase`, `tool`, and the derived decision become
                // labels. The record's `file` field is deliberately never read
                // here — file paths are both a cardinality bomb and a
                // disclosure risk on a shared dashboard.
                let tool = match rec.str_field("tool") {
                    Some(t) => t.to_string(),
                    None => "unknown".to_string(),
                };
                hook_checks
                    .get_or_create(&HookLabels {
                        phase: rec
                            .str_field("phase")
                            .map(str::to_string)
                            .unwrap_or_default(),
                        tool,
                        decision: hook_decision(rec).to_string(),
                    })
                    .inc();
            }
            "mcp" => {
                // The MCP tool name is the event name; the vocabulary is
                // closed (one variant per `#[tool]` method), so it is safe as
                // a label.
                mcp_tool_calls
                    .get_or_create(&ToolLabels {
                        tool: rec.event.clone(),
                    })
                    .inc();
            }
            "context" => {
                let labels = EventLabels {
                    event: rec.event.clone(),
                };
                context_renders.get_or_create(&labels).inc();
                context_tokens
                    .get_or_create(&labels)
                    .inc_by(rec.num("estimated_tokens").unwrap_or(0));
                context_bytes
                    .get_or_create(&labels)
                    .inc_by(rec.num("bytes").unwrap_or(0));
                context_latency
                    .get_or_create(&labels)
                    .inc_by(rec.num("latency_micros").unwrap_or(0));
                if let Some(omitted) = rec.data.get("omitted").and_then(|v| v.as_object()) {
                    for (kind, count) in omitted {
                        context_omitted
                            .get_or_create(&OmitKindLabels { kind: kind.clone() })
                            .inc_by(count.as_u64().unwrap_or(0));
                    }
                }
            }
            _ => {}
        }
    }

    for (rule_id, tally) in cap_rules(rule_tallies, opts.max_rule_series) {
        if tally.blocked > 0 {
            rule_fires
                .get_or_create(&RuleLabels {
                    rule_id: rule_id.clone(),
                    outcome: "blocked".to_string(),
                })
                .inc_by(tally.blocked);
        }
        if tally.warned > 0 {
            rule_fires
                .get_or_create(&RuleLabels {
                    rule_id: rule_id.clone(),
                    outcome: "warned".to_string(),
                })
                .inc_by(tally.warned);
        }
        rule_last_fired
            .get_or_create(&RuleIdLabels {
                rule_id: rule_id.clone(),
            })
            .set(tally.last_ts as i64);
    }

    log_entries.inc_by(counted);
    log_malformed.inc_by(read.malformed);
    log_size.set(read.bytes as i64);

    registry.register(
        "phronesis_hook_checks",
        "Edit/Write decisions made by the pre- and post-check hooks",
        hook_checks,
    );
    registry.register(
        "phronesis_rule_fires",
        "Rule consequences recorded in the action log, by rule and outcome",
        rule_fires,
    );
    registry.register(
        "phronesis_rule_last_fired_timestamp_seconds",
        "Unix timestamp of the most recent consequence for each rule",
        rule_last_fired,
    );
    registry.register(
        "phronesis_mcp_tool_calls",
        "MCP tool invocations recorded by the server",
        mcp_tool_calls,
    );
    registry.register(
        "phronesis_context_renders",
        "Context constructions, by injection event",
        context_renders,
    );
    registry.register(
        "phronesis_context_estimated_tokens",
        "Cumulative estimated tokens of injected context, by event",
        context_tokens,
    );
    registry.register(
        "phronesis_context_bytes",
        "Cumulative bytes of injected context, by event",
        context_bytes,
    );
    registry.register(
        "phronesis_context_render_latency_micros",
        "Cumulative context render latency in microseconds, by event",
        context_latency,
    );
    registry.register(
        "phronesis_context_omitted",
        "Context items dropped during packing, by item kind",
        context_omitted,
    );
    registry.register(
        "phronesis_log_entries",
        "Action log entries considered by this scrape",
        log_entries,
    );
    registry.register(
        "phronesis_log_malformed_lines",
        "Action log lines that failed to parse",
        log_malformed,
    );
    registry.register(
        "phronesis_log_size_bytes",
        "Size of .phronesis/log.jsonl on disk",
        log_size,
    );

    registry
}

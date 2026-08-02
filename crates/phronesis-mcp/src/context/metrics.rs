use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

use crate::action_log::{self, LogEntry, ReadOpts};

use super::packing::{ItemKind, PackedContext};
use super::render::RenderResult;

/// The bounded observation written for one context construction.
///
/// Cost and selection only: no body, no fact arguments, no user content, and
/// no claim about whether the model read or followed anything.
#[derive(Debug, Clone, Serialize)]
pub struct ContextMetric {
    pub event: String,
    pub bytes: usize,
    pub estimated_tokens: usize,
    pub kernel_paragraphs: usize,
    pub capsules: Vec<String>,
    pub activity_items: usize,
    pub state_items: usize,
    pub rule_items: usize,
    pub omitted: BTreeMap<String, usize>,
    pub raw_truncation: bool,
    pub latency_micros: u64,
}

/// Build the observation for a render result.
pub fn metric(result: &RenderResult) -> ContextMetric {
    let selected_count = |prefix: &str| {
        result
            .packed
            .selected
            .iter()
            .filter(|id| id.starts_with(prefix))
            .count()
    };
    ContextMetric {
        event: result.metric_event.clone(),
        bytes: result.bytes(),
        estimated_tokens: result.estimated_tokens(),
        kernel_paragraphs: selected_count("kernel:"),
        capsules: result
            .packed
            .selected
            .iter()
            .filter_map(|id| id.strip_prefix("nudge:").map(str::to_string))
            .collect(),
        activity_items: selected_count("activity:"),
        state_items: selected_count("state:"),
        rule_items: selected_count("rule:"),
        omitted: omitted_by_kind(&result.packed),
        raw_truncation: result.raw_truncation(),
        latency_micros: u64::try_from(result.latency.as_micros()).unwrap_or(u64::MAX),
    }
}

/// Omission counts keyed by item kind, matching the spec's `omitted` object.
fn omitted_by_kind(packed: &PackedContext) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for omitted in &packed.omitted {
        let key = serde_json::to_value(omitted.kind)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| "unknown".to_string());
        *counts.entry(key).or_default() += 1;
    }
    counts
}

/// Append the observation. Failures are ignored: metrics must never fail a
/// model turn.
pub fn record(root: &Path, result: &RenderResult) {
    let metric = metric(result);
    let total_omitted: usize = metric.omitted.values().sum();
    let entry = LogEntry::new("context", &metric.event)
        .with("bytes", metric.bytes as u64)
        .with("estimated_tokens", metric.estimated_tokens as u64)
        .with("kernel_paragraphs", metric.kernel_paragraphs as u64)
        .with("capsules", serde_json::json!(metric.capsules))
        .with("activity_items", metric.activity_items as u64)
        .with("state_items", metric.state_items as u64)
        .with("rule_items", metric.rule_items as u64)
        .with("omitted", serde_json::json!(metric.omitted))
        .with("omitted_total", total_omitted as u64)
        .with("raw_truncation", metric.raw_truncation)
        .with("latency_micros", metric.latency_micros);
    let _ = action_log::append(&action_log::default_path(root), &entry);
}

/// Directly observed properties only. No compliance, effectiveness, or
/// subsequent-block correlation is reported, because none is measured.
#[derive(Debug, Clone, Serialize, Default)]
pub struct ContextStats {
    pub payloads: usize,
    pub average_bytes: u64,
    pub p95_bytes: u64,
    pub median_bytes: u64,
    pub average_estimated_tokens: u64,
    pub p95_estimated_tokens: u64,
    pub median_estimated_tokens: u64,
    pub omissions: u64,
    /// Omission counts keyed by item kind.
    pub omissions_by_kind: BTreeMap<String, u64>,
    /// How many payloads selected each capsule id.
    pub capsule_selections: BTreeMap<String, u64>,
    pub raw_truncations: u64,
    pub average_latency_micros: u64,
    pub p95_latency_micros: u64,
}

/// Nearest-rank percentile: the smallest value at or above which `p` percent
/// of the sample falls. Sorts in place.
fn percentile(values: &mut [u64], p: usize) -> u64 {
    if values.is_empty() {
        return 0;
    }
    values.sort_unstable();
    let rank = (values.len() * p).div_ceil(100).max(1);
    values.get(rank - 1).copied().unwrap_or(0)
}

/// One numeric field of a logged observation, defaulting to 0 when the field
/// is absent or is not an integer (an object under `omitted`, for instance).
fn field(entry: &LogEntry, name: &str) -> u64 {
    entry.data.get(name).and_then(|v| v.as_u64()).unwrap_or(0)
}

/// Summary of one numeric column across every observation in the window.
struct Distribution {
    average: u64,
    median: u64,
    p95: u64,
}

/// Average, median, and 95th percentile of `name` across `entries`.
fn distribution(entries: &[LogEntry], name: &str) -> Distribution {
    let mut values = entries.iter().map(|e| field(e, name)).collect::<Vec<u64>>();
    let count = values.len() as u64;
    Distribution {
        average: values.iter().sum::<u64>().checked_div(count).unwrap_or(0),
        median: percentile(&mut values, 50),
        p95: percentile(&mut values, 95),
    }
}

/// The two per-key tallies that need a pass over every observation.
#[derive(Default)]
struct Tallies {
    omissions_by_kind: BTreeMap<String, u64>,
    capsule_selections: BTreeMap<String, u64>,
}

/// Omissions per item kind, and how many payloads selected each capsule.
fn tallies(entries: &[LogEntry]) -> Tallies {
    let mut tallies = Tallies::default();
    for entry in entries {
        if let Some(map) = entry.data.get("omitted").and_then(|v| v.as_object()) {
            for (kind, value) in map {
                *tallies.omissions_by_kind.entry(kind.clone()).or_default() +=
                    value.as_u64().unwrap_or(0);
            }
        }
        for id in entry
            .data
            .get("capsules")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
        {
            *tallies
                .capsule_selections
                .entry(id.to_string())
                .or_default() += 1;
        }
    }
    tallies
}

/// The `kind:"context"` observations at or after `since`, or none if the log
/// cannot be read.
fn observations(root: &Path, since: Option<u64>) -> Vec<LogEntry> {
    action_log::read_recent(
        &action_log::default_path(root),
        &ReadOpts {
            since,
            kind: Some("context".to_string()),
            ..ReadOpts::default()
        },
    )
    .unwrap_or_default()
}

pub fn stats(root: &Path, since: Option<u64>) -> ContextStats {
    let entries = observations(root, since);
    let bytes = distribution(&entries, "bytes");
    let tokens = distribution(&entries, "estimated_tokens");
    let latency = distribution(&entries, "latency_micros");
    let tallies = tallies(&entries);

    ContextStats {
        payloads: entries.len(),
        average_bytes: bytes.average,
        p95_bytes: bytes.p95,
        median_bytes: bytes.median,
        average_estimated_tokens: tokens.average,
        p95_estimated_tokens: tokens.p95,
        median_estimated_tokens: tokens.median,
        // `omitted_total` is written alongside the per-kind object; older
        // records carried a bare integer under `omitted`, which `field`
        // still reads correctly because an object yields 0 there.
        omissions: entries
            .iter()
            .map(|e| field(e, "omitted_total") + field(e, "omitted"))
            .sum(),
        omissions_by_kind: tallies.omissions_by_kind,
        capsule_selections: tallies.capsule_selections,
        raw_truncations: entries
            .iter()
            .filter(|e| e.data.get("raw_truncation").and_then(|v| v.as_bool()) == Some(true))
            .count() as u64,
        average_latency_micros: latency.average,
        p95_latency_micros: latency.p95,
    }
}

/// Omission counts by kind for a single packed payload.
pub fn counts_by_kind(packed: &PackedContext) -> BTreeMap<ItemKind, usize> {
    let mut counts = BTreeMap::new();
    for omitted in &packed.omitted {
        *counts.entry(omitted.kind).or_default() += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::render::{ContextEvent, render};

    /// An opted-in project whose kernel is far over its ceiling, so an
    /// interaction render produces both selections and omissions.
    fn opted_in_project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let ep = dir.path().join(".phronesis");
        std::fs::create_dir_all(&ep).expect("mkdir .phronesis");
        std::fs::write(
            ep.join("context.json"),
            serde_json::to_string(&crate::context::config::ContextConfig::default())
                .expect("serialize"),
        )
        .expect("write config");
        std::fs::write(
            ep.join("kernel.md"),
            "## Section\n\nA guidance paragraph.\n\n".repeat(40),
        )
        .expect("write kernel");
        dir
    }

    async fn record_one(root: &std::path::Path) {
        let result = render(root, ContextEvent::Interaction, 5)
            .await
            .expect("opted in");
        record(root, &result);
    }

    #[tokio::test]
    async fn the_observation_carries_costs_but_no_content() {
        let dir = opted_in_project();
        record_one(dir.path()).await;
        let raw = std::fs::read_to_string(action_log::default_path(dir.path())).expect("log");
        assert!(raw.contains("\"kind\":\"context\""));
        assert!(raw.contains("\"bytes\""));
        assert!(raw.contains("\"estimated_tokens\""));
        assert!(raw.contains("\"latency_micros\""));
        assert!(
            !raw.contains("A guidance paragraph."),
            "no body may reach the log: {raw}"
        );
    }

    #[tokio::test]
    async fn omissions_are_recorded_per_kind() {
        let dir = opted_in_project();
        let result = render(dir.path(), ContextEvent::Interaction, 5)
            .await
            .expect("opted in");
        let metric = metric(&result);
        assert!(!metric.omitted.is_empty(), "the fixture must omit kernel");
        assert!(metric.omitted.contains_key("kernel"));
        assert!(!metric.raw_truncation);
    }

    #[tokio::test]
    async fn stats_aggregate_across_payloads() {
        let dir = opted_in_project();
        for _ in 0..4 {
            record_one(dir.path()).await;
        }
        let stats = stats(dir.path(), None);
        assert_eq!(stats.payloads, 4);
        assert!(stats.average_bytes > 0);
        assert_eq!(stats.p95_bytes, stats.median_bytes, "identical payloads");
        assert!(stats.omissions > 0);
        assert!(stats.omissions_by_kind.contains_key("kernel"));
        assert_eq!(stats.raw_truncations, 0);
    }

    #[test]
    fn stats_on_an_empty_log_are_zeroed_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let stats = stats(dir.path(), None);
        assert_eq!(stats.payloads, 0);
        assert_eq!(stats.average_bytes, 0);
        assert_eq!(stats.p95_bytes, 0);
        assert!(stats.capsule_selections.is_empty());
    }

    #[tokio::test]
    async fn a_cutoff_excludes_older_observations() {
        let dir = opted_in_project();
        record_one(dir.path()).await;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        assert_eq!(
            stats(dir.path(), Some(now.saturating_sub(3600))).payloads,
            1
        );
        assert_eq!(
            stats(dir.path(), Some(now + 3600)).payloads,
            0,
            "a future cutoff must exclude everything"
        );
    }

    #[test]
    fn percentiles_use_nearest_rank() {
        let mut values = (1..=100).collect::<Vec<u64>>();
        assert_eq!(percentile(&mut values, 95), 95);
        assert_eq!(percentile(&mut values, 50), 50);
        let mut single = vec![7];
        assert_eq!(percentile(&mut single, 95), 7);
        assert_eq!(percentile(&mut [], 95), 0);
    }

    #[tokio::test]
    async fn rule_statistics_ignore_context_observations() {
        let dir = opted_in_project();
        // One real hook decision plus several context observations.
        let entry = crate::action_log::LogEntry::new("hook", "pre_check")
            .with("file", "src/x.rs")
            .with(
                "consequences",
                serde_json::json!([{"rule_id": "r1", "action_type": "constraint_violation", "message": "m", "bindings": {}}]),
            );
        action_log::append(&action_log::default_path(dir.path()), &entry).expect("append");
        for _ in 0..3 {
            record_one(dir.path()).await;
        }
        let entries =
            action_log::read_recent(&action_log::default_path(dir.path()), &ReadOpts::default())
                .expect("read log");
        let summary = crate::stats::aggregate(
            &entries,
            &crate::stats::StatsOpts {
                since_secs: None,
                rule_filter: None,
                now_secs: 0,
            },
        );
        let total: u32 = summary.per_rule.iter().map(|r| r.blocked + r.warned).sum();
        assert_eq!(
            total, 1,
            "context observations must not be counted as rule firings"
        );
    }
}

//! Live, in-process metrics for the long-lived MCP server.
//!
//! Scope is deliberately narrow: only what the action log *cannot* express.
//! Anything that is already journalled — rule fires, hook decisions, context
//! cost — is derived in [`crate::families`] instead, so there is exactly one
//! source of truth per number.
//!
//! All handles are Arc-backed and `Clone` shares state, so [`Live`] is cheap
//! to clone across tasks and its clones can be registered into a fresh
//! per-scrape registry.

use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::{Histogram, exponential_buckets};
use prometheus_client::registry::Registry;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Instant;

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct ToolLabels {
    tool: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct BuildLabels {
    version: String,
    rhai: String,
}

/// Histogram family type, spelled out because `Histogram` has no `Default`
/// and therefore needs an explicit constructor function.
type LatencyFamily = Family<ToolLabels, Histogram, fn() -> Histogram>;

fn latency_histogram() -> Histogram {
    // 1ms doubling to ~4s: rule firing is sub-millisecond, a graph rebuild is
    // seconds, and this spans both without a long tail of empty buckets.
    Histogram::new(exponential_buckets(0.001, 2.0, 12))
}

/// Live server metrics. Clone freely — clones share the same counters.
#[derive(Clone)]
pub struct Live {
    start: Instant,
    version: String,
    rhai: bool,
    tool_latency: LatencyFamily,
    tool_errors: Family<ToolLabels, Counter>,
    rules_loaded: Gauge,
}

impl std::fmt::Debug for Live {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Live")
            .field("version", &self.version)
            .field("rhai", &self.rhai)
            .field("uptime_secs", &self.start.elapsed().as_secs())
            .finish()
    }
}

impl Live {
    pub fn new(version: impl Into<String>, rhai: bool) -> Self {
        Self {
            start: Instant::now(),
            version: version.into(),
            rhai,
            tool_latency: Family::new_with_constructor(latency_histogram as fn() -> Histogram),
            tool_errors: Family::default(),
            rules_loaded: Gauge::default(),
        }
    }

    /// Record how long one MCP tool call took.
    pub fn observe_tool(&self, tool: &str, seconds: f64) {
        self.tool_latency
            .get_or_create(&ToolLabels {
                tool: tool.to_string(),
            })
            .observe(seconds);
    }

    /// Record that an MCP tool call returned an error.
    pub fn record_tool_error(&self, tool: &str) {
        self.tool_errors
            .get_or_create(&ToolLabels {
                tool: tool.to_string(),
            })
            .inc();
    }

    /// Publish how many rules the network currently holds.
    pub fn set_rules_loaded(&self, n: i64) {
        self.rules_loaded.set(n);
    }

    /// Seconds since this `Live` was constructed.
    pub fn uptime_seconds(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// Register clones of every live handle into a scrape-scoped registry.
    pub(crate) fn register(&self, registry: &mut Registry) {
        // Sampled at register time rather than tracked continuously — uptime
        // is a pure function of the clock, so there is nothing to maintain.
        let uptime = Gauge::<f64, AtomicU64>::default();
        uptime.set(self.uptime_seconds());
        registry.register(
            "phronesis_server_uptime_seconds",
            "Seconds since the MCP server started",
            uptime,
        );

        let build_info = Family::<BuildLabels, Gauge>::default();
        build_info
            .get_or_create(&BuildLabels {
                version: self.version.clone(),
                rhai: if self.rhai { "on" } else { "off" }.to_string(),
            })
            .set(1);
        registry.register(
            "phronesis_build_info",
            "Build metadata for the running server; value is always 1",
            build_info,
        );

        registry.register(
            "phronesis_server_tool_latency_seconds",
            "MCP tool call latency",
            self.tool_latency.clone(),
        );
        registry.register(
            "phronesis_server_tool_errors",
            "MCP tool calls that returned an error",
            self.tool_errors.clone(),
        );
        registry.register(
            "phronesis_server_rules_loaded",
            "Rules currently loaded in the RETE network",
            self.rules_loaded.clone(),
        );
    }
}

/// Shared handle used by the serve layer.
pub type SharedLive = Arc<Live>;

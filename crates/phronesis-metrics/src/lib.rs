//! Prometheus metrics for phronesis.
//!
//! # Why this is a separate, optional crate
//!
//! An OpenMetrics endpoint needs an HTTP stack, and `phronesis-mcp` otherwise
//! has none. Keeping the exporter here means the default `phr-mcp` build stays
//! free of hyper and its transitive tree; a user who wants metrics opts in with
//! `--features metrics`.
//!
//! This crate deliberately does **not** depend on `phronesis-mcp`. The action
//! log is a stable JSON Lines format, so the exporter parses it directly and
//! the dependency arrow points one way, with no cycle.
//!
//! # How the two halves fit together
//!
//! phronesis runs as two very different kinds of process:
//!
//! * short-lived hook invocations (`phr-mcp pre-check` / `post-check`), which
//!   exit in milliseconds and can never be scraped; and
//! * one long-lived MCP server (`phr-mcp serve`).
//!
//! Rather than instrument each separately, everything already journalled to
//! `.phronesis/log.jsonl` is *derived* from that file at scrape time
//! ([`families`]), which covers both halves uniformly. Only what the log cannot
//! express — uptime, in-process latency histograms — lives in a real
//! in-process registry ([`live`]). Both encode into a single registry per
//! scrape, so the output has exactly one `# EOF`.
//!
//! # Counter semantics
//!
//! Log-derived counters are absolute totals over the current log file. When the
//! log is truncated (`append_with_max`), those totals drop. Prometheus treats
//! that as an ordinary counter reset and `rate()` handles it correctly, but it
//! does mean `phronesis_*_total` reads as "since the last truncation" rather
//! than "since the beginning of time".
//!
//! ```no_run
//! use std::path::Path;
//! let text = phronesis_metrics::scrape(
//!     Path::new("."),
//!     &phronesis_metrics::families::Options::default(),
//!     None,
//! )?;
//! assert!(text.ends_with("# EOF\n"));
//! # Ok::<(), phronesis_metrics::Error>(())
//! ```

pub mod families;
pub mod live;
pub mod log;
pub mod serve;

pub use families::Options;
pub use live::Live;

use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("reading action log: {0}")]
    Io(#[from] std::io::Error),
    #[error("encoding metrics: {0}")]
    Fmt(#[from] std::fmt::Error),
    #[error(
        "refusing to bind metrics endpoint to non-loopback address {0}: \
         the endpoint exposes project-internal rule ids"
    )]
    NonLoopbackBind(std::net::SocketAddr),
}

/// Render one scrape: read the log, derive families, merge in live metrics,
/// encode as OpenMetrics text.
pub fn scrape(
    project_root: &Path,
    opts: &families::Options,
    live: Option<&Live>,
) -> Result<String, Error> {
    let read = log::read(&log::default_path(project_root))?;
    let mut registry = families::build(&read, opts);
    if let Some(live) = live {
        live.register(&mut registry);
    }
    let mut buf = String::new();
    prometheus_client::encoding::text::encode(&mut buf, &registry)?;
    Ok(buf)
}

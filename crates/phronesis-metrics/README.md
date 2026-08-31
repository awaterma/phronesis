# phronesis-metrics

Prometheus metrics for [phronesis](https://github.com/awaterma/phronesis).

Derives rule, hook, and context-cost metric families from the
`.phronesis/log.jsonl` action log and exposes them over an OpenMetrics
endpoint.

## Why derive from the log

phronesis runs as two very different kinds of process. The MCP server
(`phr-mcp serve`) is long-lived and scrapeable; the pre/post-check hooks are
short-lived, exiting in milliseconds — long before any scrape could reach
them. Since every writer already appends to `.phronesis/log.jsonl` under an
exclusive lock, deriving metrics from that file at scrape time covers both
halves uniformly, with no instrumentation in the hook hot path.

Only what the log cannot express — server uptime, in-process tool latency —
lives in a real in-process registry. Both encode into a single registry per
scrape.

## Usage

Metrics are off by default. Build `phr-mcp` with the `metrics` feature:

```sh
cargo install phronesis-mcp --features metrics
```

One scrape to stdout:

```sh
phr-mcp metrics
```

For node_exporter's textfile collector (written temp-then-rename, so the
collector never reads a partial file):

```sh
phr-mcp metrics --out /var/lib/node_exporter/textfile/phronesis.prom
```

As a standalone exporter:

```sh
phr-mcp metrics --listen 127.0.0.1:9464
```

## Metrics

| Metric | Type | Labels |
|---|---|---|
| `phronesis_hook_checks_total` | counter | `phase`, `tool`, `decision` |
| `phronesis_rule_fires_total` | counter | `rule_id`, `outcome` |
| `phronesis_rule_last_fired_timestamp_seconds` | gauge | `rule_id` |
| `phronesis_mcp_tool_calls_total` | counter | `tool` |
| `phronesis_context_renders_total` | counter | `event` |
| `phronesis_context_estimated_tokens_total` | counter | `event` |
| `phronesis_context_bytes_total` | counter | `event` |
| `phronesis_context_render_latency_micros_total` | counter | `event` |
| `phronesis_context_omitted_total` | counter | `kind` |
| `phronesis_log_entries_total` | counter | — |
| `phronesis_log_malformed_lines_total` | counter | — |
| `phronesis_log_size_bytes` | gauge | — |
| `phronesis_server_uptime_seconds` | gauge | — |
| `phronesis_server_tool_latency_seconds` | histogram | `tool` |
| `phronesis_server_tool_errors_total` | counter | `tool` |
| `phronesis_server_rules_loaded` | gauge | — |
| `phronesis_build_info` | gauge | `version`, `rhai` |

`phronesis_rule_fires_total` uses the same blocked/warned classification as
`phr-mcp stats`, so the table and the counters cannot disagree.

## Cardinality and disclosure

Rule ids are author-defined and unbounded, so `--max-rule-series` (default
100) keeps the busiest rules and folds the remainder into a single
`rule_id="__other__"` series. Ties break on rule id so the series set is
stable across scrapes.

**File paths never become label values.** The action log records the edited
file, but nothing in the exposition output carries it — that would be both a
cardinality bomb and a disclosure risk on a shared dashboard.

For the same reason the endpoint refuses to bind a non-loopback address:
the output carries project-internal rule ids. Use a local collector or a
separately authenticated tunnel or proxy for remote monitoring.

## Counter semantics

Log-derived counters are absolute totals over the *current* log file. When
the log is truncated, the totals drop. Prometheus treats this as an ordinary
counter reset and `rate()` handles it correctly, but `phronesis_*_total`
reads as "since the last truncation" rather than "since the beginning of
time".

## License

MIT

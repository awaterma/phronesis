# governance-consumer: standalone out-of-workspace consumer

Proves that a project outside this workspace can depend on the **packaged**
`phronesis-governance` crate (from a `cargo package` artifact, not a path
into workspace source), build fully offline, and exercise the public policy
surface: strict parse → `CompiledPolicy::compile` → explicit-fact
`evaluate` with allow/deny/warn verdicts, provenance, demotion, and
per-call isolation.

Nothing here touches the MCP crate, `.phronesis`, or project discovery, and
nothing is ever published; this is offline validation only.

## Layout

| Path | Purpose |
|------|---------|
| `consumer/src/main.rs` | Consumer program source. Compiled inside the temp consumer, never in this workspace. |
| `run.sh` | Reusable acceptance script (package → extract → assemble → offline build/run → proof → cleanup). |

## What `run.sh` does

1. `cargo package -p phronesis --no-verify --offline` and
   `cargo package -p phronesis-governance --no-verify --offline` (the
   governance packaging injects a `[patch.crates-io] phronesis` via
   `--config` so the strict dependency resolution succeeds against the local
   crate instead of the offline crates.io index, which predates 0.40.0).
2. Extracts both `.crate` tarballs into a fresh temp dir under
   `$TMPDIR/governance-consumer-<stamp>`.
3. Writes a standalone consumer manifest with its own empty `[workspace]`
   table, a path dependency on the extracted governance crate, and
   `[patch.crates-io] phronesis = { path = <extracted phronesis> }` so the
   packaged governance dependency on `phronesis = "0.40.0"` resolves to the
   extracted workspace crate.
4. Builds and runs the consumer with `--offline` using an isolated
   `CARGO_TARGET_DIR` under the same temp dir.
5. Prints the run output as result proof and greps it for the expected
   verdict markers (`deny/allow/warn/demoted/isolated`).
6. On success removes only the temp dirs this script created under the
   `governance-consumer-` prefix; on failure it keeps them and prints the
   path for inspection.

## Run

```sh
sh examples/governance-consumer/run.sh
```

Exit 0 with a `PASS:` line means the packaged crate works for an external
consumer; any failure exits nonzero naming the log to read.

## In-workspace equivalents

- Coding example: `cargo run -p phronesis-governance --example consumer_flow`
- Noncoding operator workflow (JSON in, verdict + exit code out, explicit
  paths only): `cargo run -p phronesis-governance --example operator_flow -- rules.json events.json [--phase pre|post|audit]`
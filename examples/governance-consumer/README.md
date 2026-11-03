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
| `run.sh` | Reusable acceptance script (package → extract → assemble → offline build/run → proof, then the expense-approval operator-flow checks → cleanup). |
| `fixtures/` | Expense-approval policy and events fixtures for the noncoding operator flow. |

## What `run.sh` does

1. Derives the cargo target directory and both crate versions from
   `cargo metadata --offline --no-deps`, so the script works whether
   `CARGO_TARGET_DIR` is unset, absolute, or relative.
2. `cargo package -p phronesis --no-verify --offline` and
   `cargo package -p phronesis-governance --no-verify --offline` (the
   governance packaging injects a `[patch.crates-io] phronesis` via
   `--config` so the strict dependency resolution succeeds against the local
   crate instead of the offline crates.io index, which predates the
   workspace version).
3. Extracts both `.crate` tarballs into a fresh temp dir under
   `$TMPDIR/governance-consumer-<stamp>`.
4. Writes a standalone consumer manifest with its own empty `[workspace]`
   table, a path dependency on the extracted governance crate, and
   `[patch.crates-io] phronesis = { path = <extracted phronesis> }` so the
   packaged governance dependency on `phronesis` resolves to the
   extracted workspace crate.
5. Builds and runs the consumer with `--offline` using an isolated
   `CARGO_TARGET_DIR` under the same temp dir.
6. Prints the run output as result proof and greps it for the expected
   verdict markers (`deny/allow/warn/demoted/isolated`).
7. Builds the `operator_flow` example and runs it **from an empty cwd**
   with explicit absolute fixture paths (see below), asserting verdict text
   and exact exit codes for approve, warn, deny, `--phase audit`, and a
   rejected reservation spoof.
8. On success removes only the temp dirs this script created under the
   `governance-consumer-` prefix; on ANY failure (exit status nonzero) it
   keeps the temp dir with every evidence log and prints the path for
   inspection.

## Expense-approval fixtures (a non-software domain)

The operator flow is for noncoding domains too: these fixtures model an
expense-approval policy, not code rules. The expense system (the host)
asserts facts; the policy classifies them.

| Fixture | Events | Expected |
|---------|--------|----------|
| `expense-events-approve.json` | a submitted expense with a receipt, under the limit | `verdict: Allow`, exit 0 |
| `expense-events-warn.json` | a submitted expense missing its receipt | `verdict: Warn`, exit 1 |
| `expense-events-deny.json` | a submitted expense flagged over the approval limit | `verdict: Deny`, exit 2 |
| `expense-events-approve.json` + `--phase audit` | same clean expense | `Selection::Audit` fires the `audit: true` rule `expense-audit-trail` (disk phase `post`), exit 0 |
| `expense-events-spoof.json` | an extension fact asserting `rule_overridden`, a predicate the configuration reserves but **no host fact currently carries**, claiming `source: "host"` | rejected before anything is asserted: `evaluation failed closed: extension fact … asserts reserved predicate …`, exit 3 |

Two semantics are pinned by these fixtures:

- **Reservations are explicit host configuration.** The events file carries
  a required `reserved_predicates` list naming the predicates only host code
  may assert. The reserved set is never derived from the host facts that
  happen to be present, so spoofing a reserved predicate that is currently
  absent from the snapshot is still rejected, whatever `source` the fact
  claims (source is attribution, never authorization). Host facts may use
  reserved predicates freely — the host owns them by definition.
- **`--phase audit` is the audit engine, not a hook phase.** It selects
  `audit: true` rules regardless of their disk phase (`Selection::Audit`),
  matching `filter_audit_rules` in the MCP audit engine. A `Hook(Audit)`
  selection would match only disk-phase `audit` rules and would fire
  nothing here. `pre` and `post` select by disk phase exactly
  (`Selection::Hook`), where the audit flag is ignored.

The same behaviors are characterized in
`crates/phronesis-governance/tests/consumer_examples.rs`, which reads these
fixture files directly.

## Run

```sh
sh examples/governance-consumer/run.sh
```

Exit 0 with a `PASS:` line means the packaged crate works for an external
consumer and the expense-approval operator flow behaves as documented; any
failure exits nonzero naming the log to read, and the temp dir is kept.

## In-workspace equivalents

- Coding example: `cargo run -p phronesis-governance --example consumer_flow`
- Noncoding operator workflow (JSON in, verdict + exit code out, explicit
  paths only): `cargo run -p phronesis-governance --example operator_flow -- rules.json events.json [--phase pre|post|audit]`
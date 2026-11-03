# Governance compatibility differential runner

A stdlib-only Python runner that pins `phr-mcp` observable behavior between
two binaries (baseline vs candidate). It runs an identical, isolated set of
explicit fixture inputs against each binary in fresh temp project roots and
compares every observable: exit codes, stdout, stderr, and on-disk state
(`rules.json`, `log.jsonl`, `emitted-capsules.json`). Exit codes are
compared for **every** call, even calls that carry no explicit `expect`.
The disk state of the three `.phronesis/` files is captured for **every**
case (raw + normalized, absent files recorded as `<absent>`) and compared
byte-for-byte, and an intermediate disk snapshot is taken at every
`expect_disk` step so a later removal or recovery cannot hide a difference.
Regression tests for these comparison guarantees live in
`test_diff_runner.py` (`python3 -m unittest test_diff_runner`), including
negative controls proving the normalizer preserves literal UUIDs, dates,
rule ids, and message wording.

## Usage

```bash
python3 scripts/governance-compat/diff_runner.py \
  --baseline-bin /path/to/baseline/phr-mcp \
  --candidate-bin /path/to/candidate/phr-mcp \
  --output-dir /tmp/compat-out \
  [--baseline-src /path/to/baseline/checkout] \
  [--candidate-src /path/to/candidate/checkout]
```

Exit codes:

- `0` — every case ran, all compared artifacts match, all per-step
  expectations held
- `1` — at least one normalized artifact differs (see `diffs/`)
- `3` — a required case failed to run or was omitted, or a per-step
  expectation failed (so two binaries agreeing on the same accidental
  error cannot pass the run)
- `2` — operational error (missing binary, subprocess failure)

The runner's scope is differential parity plus the positive expectations
listed per case; it does not claim to pin every possible external behavior
of `phr-mcp` (undocumented flags, host-specific locale/path handling, and
timing-dependent output are out of scope).

Each call and MCP step carries positive expectations (exit codes, stderr
needles, JSONRPC success/error shape, required disk state transitions
before/after the step), so the runner asserts real governance behavior,
not just equality between the two sides.

`--baseline-src` / `--candidate-src` record the git `HEAD` revision of each
checkout into `report.json` alongside the binary SHA-256 hashes and the
SHA-256 of every fixture file.

## Output layout

```
output-dir/
  report.json            # per-case per-artifact match/MISMATCH + hashes
  report.txt             # human summary (also printed to stdout)
  baseline/<case>/...    # verbatim raw artifacts (unnormalized)
  candidate/<case>/...   # verbatim raw artifacts
  normalized/<side>/<case>/...  # normalized copies used for comparison
  diffs/<case>/...       # unified diffs of normalized text, mismatches only
```

## What is compared

23 required cases cover:

- CLI help/version/subcommand dispatch (including unknown subcommand)
- pre/post hook exits for block (2), warn (1), allow (0)
- Malformed v1/v2 rules files: mis-cased verb, mis-cased phase-free v1 arg,
  empty `when`, duplicate id — fail-closed block/warn/deny surfaces
- Mixed phases (pre/post/audit/none) including `audit --json` findings
- Silent rules, OR clauses
- Codex adapter decisions: clean `{}`, warn `additionalContext`, deny
  `permissionDecision` with zero process exit, lifecycle fail-open
- Capsule disk output (`emitted-capsules.json` + `context list --json`)
- Layered loader overrides over absolute temp paths, missing required layer,
  unsupported version, optional-missing layer, action-log recording
- Rhai provider emitting reserved host-owned predicates (`signal_pass`) —
  fail closed with the provider named
- MCP persistent single-connection sessions: initialize, tools/list,
  add_rule (v1 accepted, v2 disk shape refused), assert_fact/fire_rules,
  remove_rule, list_rules
- Broken rules file at startup: `add_rule` refusal error, recovery after
  the file is repaired, `list_rules` load-error report
- `load_rules_file` external-edit flow
- Autopersist write, autoload across a server restart (fresh `serve`),
  `PHRONESIS_NO_AUTOPERSIST` with dry-run and explicit `save_rules`
- MCP capsule emission, persistence to disk, idempotent retraction

## Normalization (deliberately minimal)

Normalization is deliberately narrow — only identified generated
nondeterminism in *named* fields:

- the per-run fixture project root → `<ROOT>`
- quoted (or JSON-escaped-quoted) named numeric fields: `ts`, `ts_ms`,
  `generated_at`, `scan_duration_ms`, `created_at_ms`, `updated_at_ms`,
  `expires_at`, `emitted_at` → value `<TS>`
- the generated `session_id` value shaped `s-YYYY-MM-DD-<hex>` → `<SESSION>`

There are no generic ISO/UUID regexes: literal user-facing messages,
decision/rule ids, scope, error wording, and ordering are compared
verbatim. JSON responses are dumped with `sort_keys` so key order is
stable; MCP tool responses are matched with needles against both the
JSONRPC envelope and the unescaped inner tool text.

## Determinism self-check

Running the runner twice with the same binary as baseline and candidate
must exit 0. Example:

```bash
python3 scripts/governance-compat/diff_runner.py \
  --baseline-bin "$BIN" --candidate-bin "$BIN" \
  --baseline-src "$SRC" --candidate-src "$SRC" \
  --output-dir /tmp/compat-self
```

A self-run that reports mismatches means the runner is over-sensitive, not
that the binaries disagree.

## Adding cases

Cases are plain dicts in `CASES` inside `diff_runner.py`; fixtures live in
`fixtures/` (rules files, hook payloads, provider scripts, loader layers).
A case is either a list of one-shot hook/CLI `calls` or a persistent `mcp`
step list (`{"tool": ..., "args": ...}`, `{"method": ..., "params": ...}`,
`{"write_disk": ...}` for external file edits between calls). Any new case
must be added to `REQUIRED_CASES` — the runner fails with exit 3 when a
required case is missing.

## MCP transport evidence

Each MCP session saves and compares `mcp-session.err` and
`mcp-session.exit` (with the case variant suffix where applicable).
Stderr is drained alongside stdout during RPCs and shutdown. Closing stdin
allows a natural server exit; a nonzero exit fails the case expectation,
and a forced kill or timeout is an operational problem even when both sides
behave identically. RPC responses retain their existing canonical JSON comparison.

Each RPC has one 60-second deadline including unrelated notifications.
Shutdown has a 10-second deadline. Nonblocking reads cap a response line at
8 MiB and each captured stream at 32 MiB; exceeding a cap fails the case.
The timestamp prefix of recognized `rmcp::` tracing lines is normalized in
MCP stderr only. Log level, target, message, and timestamps inside messages
remain significant. Raw stderr retains the original timestamp prefix.

Use the same executable basename (normally `phr-mcp`) in separate baseline
and candidate directories: CLI usage output legitimately includes argv[0].
A missing disk file may satisfy a negative-only `not_contains` assertion,
but never a simultaneous positive `contains` assertion.

Focused regression suite:

```bash
python3 -m unittest discover -s scripts/governance-compat -p test_diff_runner.py -v
```

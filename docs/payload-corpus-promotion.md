# Payload-corpus promotion workflow

How a live-captured hook payload becomes a committed contract fixture under
`crates/phronesis-mcp/tests/fixtures/payloads/<cli>/<name>.json`.

**Do not fabricate live captures. Never relabel authored data as captured.**
Promotion always goes through capture, scrubbing, and mandatory human review.
There is no automated path that writes a captured fixture.

## Provenance vocabulary

The contract runner (`crates/phronesis-mcp/tests/payload_contract.rs`) accepts
exactly three `source.provenance` values and fails fixture loading on anything
else:

| Value | Meaning |
|-------|---------|
| `authored` | Hand-written approximation of a host envelope. Validates internal assumptions only — it is NOT evidence of the live host payload contract. |
| `captured` | Recorded verbatim from a live host via `PHRONESIS_CAPTURE_DIR`, committed without scrubbing (only acceptable if it provably contains nothing private). |
| `captured-and-scrubbed` | Recorded from a live host, then anonymized with `phr-mcp scrub-payload` and human-reviewed. This is the normal provenance for promoted fixtures. |

Captured provenance (`captured` or `captured-and-scrubbed`) additionally
requires a `source.capture` metadata object (see the skeleton below). A
captured fixture missing that metadata fails to load. `host_version` uses an
explicit-null policy: the key must always be present; write `null` when the
host version was unknown at capture time.

## Workflow

### 1. Capture

Set the capture directory in the shell that launches the host CLI, then work
normally. Hook payloads are appended to `<dir>/payloads.jsonl`:

```bash
PHRONESIS_CAPTURE_DIR=/safe/private/path phr-mcp pre-check
PHRONESIS_CAPTURE_DIR=/safe/private/path phr-mcp post-check
```

Use a private path outside any repository. Raw captures may contain secrets,
usernames, and absolute paths — never commit them.

### 2. Scrub

```bash
phr-mcp scrub-payload /safe/private/path/payloads.jsonl \
  --project-root /path/to/project
```

Scrubbing performs deterministic anonymization and detects several common
leak classes. It is not a proof that arbitrary source or command content
contains no secrets.

### 3. Mandatory human review

A human MUST read every scrubbed record end to end before promotion, checking
for semantic leaks the scrubber cannot detect (secrets embedded in source
text, private hostnames, identifying prose). No record is promoted without
this review. This step cannot be delegated to tooling.

### 4. Promote into a fixture envelope

Wrap each reviewed record in the fixture envelope and place it under
`crates/phronesis-mcp/tests/fixtures/payloads/<cli>/`. Skeleton:

```json
{
  "schema": 1,
  "source": {
    "cli": "claude-code",
    "event": "PostToolUse",
    "provenance": "captured-and-scrubbed",
    "capture": {
      "host": "claude-code",
      "host_version": "2.1.0",
      "capture_date": "2026-07-12",
      "scrubber_version": "1"
    },
    "description": "What this fixture pins, and why."
  },
  "subcommand": "post-check",
  "packs": "llm,rust",
  "payload": { "...": "the scrubbed host payload, verbatim" },
  "expect": {
    "exit": 0,
    "stdout_json": true,
    "log_rule_fired": null,
    "journal_tag_new": [],
    "journal_tag_from_output": [],
    "stderr_contains": []
  }
}
```

Notes on the `capture` block:

- `host` — host CLI name (`claude-code` or `gemini`); non-empty string.
- `host_version` — the host CLI version if known; explicit `null` if unknown.
  The key itself is always required.
- `capture_date` — ISO date the payload was recorded; non-empty string.
- `scrubber_version` — the scrubber schema/version used (for
  `captured-and-scrubbed`), or the version that would have applied (for
  `captured`); non-empty string.

### 5. Expectations are supplied by a human

The `expect` block (exit code, fired rules, journal tags, stderr fragments)
must be written or reviewed by a human who understands what the payload should
do. Nothing infers expectations from observed behavior silently — an inferred
expectation would just re-assert whatever the code currently does, including
its bugs.

### 6. Verify

```bash
cargo test -p phronesis-mcp --test payload_contract
```

The `provenance_coverage_report` test prints per-host counts distinguishing
internal contract coverage (authored) from host-observed coverage (captured),
and notes any host that still has zero captured fixtures. That note is
informational: the corpus stays green when live fixtures are unavailable.

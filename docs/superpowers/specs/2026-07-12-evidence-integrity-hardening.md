# SPEC: v0.19.0 Evidence Integrity and Payload Safety Hardening

## Status

Accepted (authored by Codex; adopted 2026-07-12)

## Context

Phronesis `v0.18.0` introduced toolchain-neutral outcome grading, command-exit capture, and journey-journal compaction.

The current `0.19.0` branch adds:

- `PHRONESIS_CAPTURE_DIR`
- `phr-mcp scrub-payload`
- Claude Code and Gemini payload fixtures
- A payload-contract test runner
- A local registry of supported hook event names

The implementation and tests are generally strong, but several claims are currently stronger than the guarantees provided:

1. The payload scrubber is not a complete privacy boundary.
2. All payload fixtures currently have authored provenance, so they do not prove current host payload compatibility.
3. Missing command exit codes can generate optimistic `compile_ok` evidence.
4. Toolchain recognition can match incidental command substrings.
5. Journal compaction has a bounded stale-file retry path that can theoretically lose an append.
6. Empty scrubber roots are not rejected and can cause pathological behavior.

This work must resolve those issues before `v0.19.0` is tagged.

## Goal

Make payload handling and confidence evidence conservative, testable, and accurately documented.

After this work:

- Scrubbing must fail on known high-risk residuals.
- Scrubbing must never hang on empty or adversarial configuration.
- Authored fixtures must not be presented as evidence of live host compatibility.
- Missing execution evidence must not become a successful build outcome.
- Toolchain recognition must distinguish actual invocations from incidental text.
- Journal appends must not be lost during concurrent compaction.
- Documentation must state exactly what is and is not guaranteed.

## Required workflow

Use Phronesis itself throughout the implementation.

Before editing:

1. Run the current audit and record the baseline:

   ```bash
   phr-mcp audit
   phr-mcp confidence
   ```

2. Inspect active project rules and durable directives.
3. Create or identify an open journey subject for this work.

During implementation:

- Use pre-check and post-check hooks normally.
- Do not disable journey or confidence recording.
- Run targeted tests after each task.
- Record any discovered architectural decision in the appropriate spec or ADR.

Before completion:

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace -- -D warnings
phr-mcp audit
phr-mcp confidence
```

Do not claim completion unless these pass. Report any skipped test.

## Non-goals

- Do not build a general-purpose data-loss-prevention engine.
- Do not claim automated scrubbing can guarantee that arbitrary source text contains no secrets.
- Do not add network calls to Claude Code or Gemini during normal tests.
- Do not redesign the RETE engine.
- Do not decompose unrelated large files as part of this change.
- Do not add support for additional host CLIs or toolchains.
- Do not fabricate "live" payload fixtures.

---

# Task 1: Make scrubber construction safe

## Problem

`Scrubber::new(home, project_root)` accepts empty roots. Empty strings interact badly with substring replacement and repeated search.

## Requirements

1. Replace infallible construction with validated construction:

   ```rust
   pub fn new(home: &str, project_root: &str) -> Result<Self, ScrubError>
   ```

   An equivalent named constructor is acceptable if changing `new` would unnecessarily disrupt internal callers.

2. Reject:
    - Empty values
    - Whitespace-only values
    - Filesystem-root project paths such as `/`
    - A project root outside the configured home only if the implementation cannot safely distinguish it; otherwise support it explicitly
    - Ambiguous relative roots

3. Normalize trailing separators without changing the filesystem identity.
4. The CLI must return a nonzero exit and a clear diagnostic for invalid roots.

## Tests

Add tests proving:

- Empty home is rejected.
- Empty project_root is rejected.
- Whitespace-only values are rejected.
- `/` is rejected as a project root.
- Normal roots still work.
- Already-scrubbed input remains a fixpoint.
- No adversarial input causes non-termination.

A regression test must exercise the actual `phr-mcp scrub-payload` binary.

---

# Task 2: Define an honest payload-scrubbing safety contract

## Problem

The scrubber currently replaces home paths, usernames, session IDs, and transcript paths. It does not detect arbitrary credentials or all out-of-project paths.

## Required safety levels

Implement and document two distinct concepts:

### Deterministic anonymization

The tool must anonymize:

- Project-root paths to `/home/dev/project`
- Home-rooted paths outside the project to deterministic placeholders
- Session identifiers under recognized key variants
- Transcript paths under recognized key variants
- Username path components
- Other identity fields explicitly enumerated in the implementation

### Residual-risk detection

The tool must reject or warn about:

- Absolute paths outside the canonical project root
- Credential-bearing URLs
- Common private-key headers
- Common token/secret assignments
- Environment keys whose names strongly indicate secrets
- Email addresses or host-specific identity fields, according to documented policy

Use conservative, bounded detection. Avoid printing the full suspected secret in diagnostics.

## Failure policy

Classify findings:

- Error: clear credential, private key, residual home path, or unapproved absolute path.
- Warning: possible identity token or free-text username.
- Allowed: project content that merely contains words such as token, secret, or password without a value.

Warnings must be visible on stderr. Under `--write`, errors must abort before backup or output modification.

## Shape validation

The scrubber must distinguish:

1. A capture JSONL record produced by `PHRONESIS_CAPTURE_DIR`.
2. A single fixture envelope.
3. A raw host payload intentionally passed as a single JSON object.

Do not claim that an "unrecognized shape" aborts unless shape rejection is actually implemented.

Choose one of these policies and document it:

- Strict mode by default with an explicit `--allow-raw` option, or
- Raw JSON accepted by default, with documentation that shape recognition is not a safety guarantee.

Prefer the smallest compatible change.

## Tests

Cover:

- API key assignment in a shell command.
- Bearer token.
- Credential-bearing URL.
- PEM private-key header.
- `/private/tmp/...` or another absolute path outside `$HOME`.
- In-project absolute path.
- Relative project path.
- Benign source containing the word password.
- Error diagnostics do not echo full secret values.
- `--write` leaves the original untouched on every failure.
- Backup behavior remains correct after successful validation.
- Re-scrubbing remains idempotent.

## Documentation language

The documentation must say:

> scrub-payload performs deterministic anonymization and detects several common leak classes. It is not a proof that arbitrary source or command content contains no secrets. Review scrubbed fixtures before committing them.

Do not call the scrubber a complete privacy boundary.

---

# Task 3: Make fixture provenance enforceable

## Problem

All current fixtures use authored approximations. They protect internal behavior but do not validate current Claude Code or Gemini payloads.

## Requirements

1. Define allowed provenance values, for example:
    - `authored`
    - `captured`
    - `captured-and-scrubbed`

2. Reject unknown provenance values when loading fixtures.
3. Require captured provenance to include non-sensitive metadata:
    - Host name
    - Host version, when known
    - Capture date
    - Scrubber schema/version

4. The contract-test output must distinguish:
    - Internal contract coverage from authored fixtures
    - Host-observed coverage from captured fixtures

5. Tests must not fail merely because live fixtures are unavailable.
6. Add a separate test or report that clearly states when a host has zero captured fixtures.
7. Do not relabel authored data as captured.

## Promotion workflow

Document the exact workflow:

```bash
PHRONESIS_CAPTURE_DIR=/safe/private/path phr-mcp pre-check
PHRONESIS_CAPTURE_DIR=/safe/private/path phr-mcp post-check

phr-mcp scrub-payload /safe/private/path/payloads.jsonl \
  --project-root /path/to/project

# Human review is mandatory.
# Promote selected records into fixture envelopes.
cargo test -p phronesis-mcp --test payload_contract
```

If practical, add a helper command or script that converts a scrubbed capture record into a fixture skeleton. It must not infer expected results silently; expectations must be supplied or reviewed by a human.

## Acceptance criteria

- Authored fixtures continue to protect hook liveness.
- Reports no longer imply those fixtures prove current host payload shapes.
- Captured fixtures, when added later, can be identified and audited.

---

# Task 4: Introduce unknown outcome evidence

## Problem

When `command_exit` is absent and no compile-failure regex matches, the current parser records a successful build. Absence of failure evidence is not proof of success.

## Required model

Represent build outcome as:

- pass
- fail
- unknown

Minimum semantics:

| Exit code | Failure regex | Explicit success evidence | Outcome |
|-----------|---------------|---------------------------|---------|
| 0         | no            | any                       | pass |
| nonzero   | any           | any                       | fail, except documented test-failure split |
| absent    | yes           | any                       | fail |
| absent    | no            | yes                       | pass |
| absent    | no            | no                        | unknown |

"Explicit success evidence" must come from a configured success regex or another strong, documented signal. A test summary may qualify as compile success if it proves tests executed.

## Schema

Extend `ToolchainDef` with an optional success matcher if necessary:

```json
{
  "compile_success": ["Finished .* profile"]
}
```

The exact field shape may differ, but it must remain declarative.

Existing project definitions without success matchers must remain loadable.

## Confidence behavior

- unknown must not emit `outcome:compile_ok`.
- unknown must not increase confidence as if a build passed.
- It may emit `outcome:compile_unknown` if useful for transparency.
- Existing explicit pass and fail behavior must remain stable.
- A test failure must still distinguish "compiled, tests failed" from "compilation failed" where evidence supports that distinction.

## Tests

Cover:

- Exit 0 with empty output → pass.
- Exit nonzero with empty output → fail.
- No exit and empty output → unknown.
- No exit with compile-failure text → fail.
- No exit with explicit compile-success text → pass.
- No exit with a valid test summary → compiled, with test result.
- Piped/truncated output does not produce false compile_ok.
- Confidence scoring does not count unknown as pass.
- Journal tags accurately represent all three states.
- Existing cargo, pytest, and tsc examples behave correctly.

Document any on-disk or rule-level compatibility impact.

---

# Task 5: Tighten toolchain command recognition

## Problem

Toolchain `matches` regexes are applied anywhere in the full shell command. Incidental text such as `echo cargo test` can be recognized as a real invocation.

## Requirements

1. Built-in and scaffolded matchers must recognize shell command positions, including common leading forms such as:
    - Direct invocation
    - `env NAME=value command`
    - `cd dir && command`
    - A command following `&&`, `||`, `;`, or a pipeline where appropriate

2. They must not recognize:
    - `echo cargo test`
    - A quoted prose string
    - A filename merely containing a tool name
    - Comments containing the command

3. Do not write a full shell parser unless required.
4. If regex-only recognition cannot satisfy the cases reliably, introduce a small command-segmentation helper with clear limitations.
5. Preserve support for Claude Code `Bash` and Gemini `run_shell_command`.

## Tests

Add table-driven tests for positive and negative command forms.

At minimum:

```
cargo test                         => yes
cd repo && cargo test              => yes
FOO=1 cargo test                   => yes
env FOO=1 cargo test               => yes
echo cargo test                    => no
printf 'cargo test'                => no
touch cargo-test.log               => no
# cargo test                       => no
```

---

# Task 6: Use a stable journal lock

## Problem

Compaction locks the journal inode and then atomically replaces it. Appenders revalidate the inode and retry, but after a bounded number of retries they may write through a stale descriptor.

## Required design

Use a stable lock file, for example:

```
.phronesis/journey/events.lock
```

All operations that mutate `events.jsonl` must:

1. Open/create the stable lock file.
2. Acquire the exclusive lock.
3. Open or read the current `events.jsonl`.
4. Append or compact.
5. Flush the required data.
6. Release the stable lock.

The lock inode must not be replaced during compaction.

## Atomicity

Compaction must:

1. Write the complete compacted journal to a temporary file in the same directory.
2. Flush the temporary file.
3. Rename it over `events.jsonl`.
4. Preserve valid append order.
5. Clean up temporary files on recoverable failures where possible.

Consider syncing the parent directory where platform support makes that meaningful. Document durability limitations.

## Compatibility

- Existing `events.jsonl` files must continue to work.
- No migration command should be required.
- Readers may remain lock-free if atomic rename gives them a coherent old or new snapshot.
- Non-Unix behavior must be explicit and tested where practical.

## Tests

Add deterministic concurrency tests proving:

- Concurrent appenders do not interleave JSON.
- Appends racing with compaction remain present.
- Repeated compaction and append loops lose no uniquely numbered record.
- A reader sees valid JSON lines during replacement.
- Malformed historical lines are handled according to the existing policy.
- Latest outcome records per subject survive compaction.
- Temporary-file or lock errors fail according to the documented policy.

Avoid timing-only tests where possible. Use barriers or controlled synchronization seams.

---

# Task 7: Correct documentation and release claims

Update:

- `CHANGELOG.md`
- `crates/phronesis-mcp/CLAUDE.md`
- Relevant README sections
- Payload-corpus design/spec
- Neutral-toolchain design/spec
- `AGENTS.md` if file roles, commands, or behavior changed

Required corrections:

- Authored fixtures validate internal assumptions, not live host contracts.
- Captured provenance identifies host-observed payloads.
- Scrubbing is not a proof of secret absence.
- Missing execution evidence produces unknown, not pass.
- Journal concurrency guarantees match the implementation precisely.
- Document any new toolchain definition fields.
- Document new journal tags or fact values.

Do not bump beyond 0.19.0; this work hardens the unreleased branch.

---

# Recommended agent division

## Agent A: Payload safety

Own:

- Tasks 1 and 2
- Scrubber unit and CLI integration tests
- Scrubbing documentation

Must not edit outcome or journal modules.

## Agent B: Outcome integrity

Own:

- Tasks 4 and 5
- Toolchain schema and parsing tests
- Confidence/journey integration tests
- Toolchain documentation

Must preserve backward deserialization compatibility.

## Agent C: Journal concurrency

Own:

- Task 6
- Locking implementation
- Deterministic concurrency tests

Must avoid changing journey semantics unrelated to storage.

## Agent D: Contract corpus and provenance

Own:

- Task 3
- Fixture schema and contract-runner reporting
- Capture-promotion documentation

Must not fabricate captured fixtures.

## Foreman/integration agent

Own:

- Baseline Phronesis evidence
- Task 7
- Cross-agent review
- Conflict resolution
- Full quality gates
- Final Phronesis audit and confidence report

The Foreman must review all agent changes before merging them into the working branch.

---

# Required final report

The final report must include:

1. Files changed, grouped by task.
2. Behavioral changes.
3. Compatibility impact.
4. Tests added.
5. Exact quality-gate results.
6. Initial and final Phronesis audit results.
7. Initial and final confidence/journey evidence.
8. Any remaining limitations.
9. Confirmation that no fixture provenance was falsified.
10. Confirmation that no user-owned unrelated files were modified.

## Completion criteria

This specification is complete only when:

- All required behavior is implemented.
- All new behavior has integration coverage.
- Full workspace tests pass.
- Clippy passes with warnings denied.
- Formatting passes.
- Phronesis audit results are reported.
- Documentation makes no stronger guarantee than the implementation.
- No known path converts missing evidence into a successful confidence signal.
- No known compaction race can discard a successfully completed append.

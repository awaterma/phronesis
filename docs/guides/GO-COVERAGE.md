# Go runtime coverage evidence

Go coverage import uses the bundled Go language pack's declarations and module/
receiver identities. `coverage select` and the MCP `select_tests` tool can join
per-test runtime observations to changed Go functions and static graph reach.
An observation means the test process executed a function; it does not establish
assertion quality, branch coverage, or that the changed behavior is correct.

## Import per-test LCOV

Run tests separately under coverage. Each report must represent one isolated test
(or one explicitly named test target). Do not label a combined suite report as an
individual test. Bazel's per-test-target report is target-level evidence: retain
that target identity rather than guessing which Go test ran a function.

The importer reads ordinary `.lcov`, `.info`, and `coverage.dat` files at the top
level of the supplied directory. It does not recursively traverse Bazel output
symlinks. Copy the report into an ordinary directory before import.

A report with `TN:<test-id>` supplies its identity. For a single report with empty
`TN:`, provide `--test-id`; multiple reports or a conflicting `TN:` are rejected.
Use the exact Go test identity from `defines_test` in the rebuilt graph when
attributing an individually isolated Go test. That permits static and dynamic
entries to refer to the same test and supplies its runnable command.

```sh
phr-mcp graph rebuild
phr-mcp coverage import --format lcov-dir --tool bazel-go \
  --test-id '//pkg:unit_tests' --workspace-prefix my_workspace ./per-target-coverage
phr-mcp coverage select --json
```

`--workspace-prefix` explicitly strips the given leading relative workspace path
from `SF:` paths. It does not guess a prefix from a basename. Dot components,
absolute prefixes, and paths traversing source symlinks are refused. The existing
relocated-absolute-path fallback remains ambiguity-aware and no longer traverses
symlink directories, including Bazel links.

A Bazel target identity usually has no corresponding Go `defines_test` entry.
Its dynamic observations still appear in selection, but a command may be absent;
run that target with the original Bazel command. No Go test command is fabricated.

## Manifest and revision

The coverage directory must contain `manifest.json` with the source revision and
SHA-256 digests of every resolved covered source:

```json
{
  "revision": "<40-character Git commit>",
  "files": {"pkg/store.go": "<SHA-256 of the tested source>"}
}
```

Record these values when producing coverage, using the exact tested source.
Import before editing, then make the change, rebuild the graph and select tests.
The manifest is an integrity check, not a cryptographic attestation of test
execution. `--allow-dirty` permits a dirty tree but does not relax digest checks.
`--no-manifest` is an explicit escape hatch that labels evidence unverified;
unverified evidence does not remove functions from the fresh-evidence gap list.

A report containing only zero hits currently produces no hit records and is
rejected by the existing importer. Missing evidence remains missing; this is not
reported as a successful coverage run.

## Inspect coverage gaps

`coverage select --json` and `select_tests` include
`functions_without_dynamic_evidence`: changed function regions with no fresh,
verified dynamic observation. A static `tested_by` edge does not fill this gap.
The text command prints the same gap list. Stale observations remain separately
labeled and do not satisfy it. Check `coverage_note` to distinguish absent,
corrupt, busy, and stale stores where reported.

Receiver names and module scopes remain part of Go function identities. Graph
percent escapes are encoded into the coverage identifier alphabet, consistently
for import, change detection, and static joins. Same-named methods on different
receivers are not joined by leaf name. Line-only evidence shared by two function
declarations on one source line is conservatively left unattributed. A body-opening
line shared with another top-level declaration is also excluded.

This support does not expand Go method-call resolution, implement arbitrary
third-party pack coverage semantics, or add Go to the native `coverage collect`
command. `go test -coverprofile` produces Go's block format, not LCOV: convert it
per isolated test before importing. The acceptance script below demonstrates the
conversion for its controlled fixture; it is not a general-purpose converter.

## Real-runner CI acceptance

```sh
cargo build -p phronesis-mcp
python3 scripts/verify-go-coverage.py \
  --binary "$PWD/target/debug/phr-mcp" --go "$(command -v go)" \
  --artifacts /tmp/phronesis-go-coverage
```

The verifier runs a real isolated Go test, preserves its coverage profile,
converts fixture block counters to LCOV, and imports an empty-`TN` `coverage.dat`
with an explicit identity and workspace prefix. It checks missing attribution
and incorrect digests are rejected, edits covered and uncovered functions,
rebuilds the graph, verifies dynamic selection and gaps (including two
same-named receiver methods), executes the selected test command, and compares the actual MCP response with CLI
selection.
Repository CI runs this alongside the existing real Go language-pack acceptance
job and retains evidence even when verification fails.

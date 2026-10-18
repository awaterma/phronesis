# Go and starter language packs

Status: Approved by the user on 2026-10-17, including MCP pack discovery;
implementation has not started.
Baseline inspected: `01513a7`, workspace version `0.38.0`.

## Intent

Make Go and authorable starter language packs the headline feature of the
next Phronesis feature release. The user wants to add languages as needed
and make that work easier for other contributors. Share the common
machinery rather than claiming a percentage of a language's semantics.

A successful release lets a Go project initialize structural rules, inspect
declarations and conservative call relationships, and recognize build/test
results. Contributors can publish packs containing a manifest, structural
queries, optional Rhai, rules, and toolchain definitions. Projects can
validate and install these without editing hook, audit, or graph code.

## Constraints and proposed scope

User decisions: Go now; starter language packs; Superpowers spec and plan;
swarming implementation; at most three workers plus the controller (four
active agents total). Preserve that execution method through reviews.

Proposed engineering choices, subject to this design review:

- Bundle grammars in Rust, adding Go. Independent grammar loading is later.
- JSON manifests, tree-sitter queries, optional sandboxed Rhai.
- Local-directory installation. Authors distribute through Git/archives;
  remote fetching and a hosted registry are later work.
- Prove portability with a second external fixture pack using a bundled
  grammar and unclaimed extension. HTML is not promised in this release.
- Preserve existing native adapters, identities, and rule behavior.
- Support Go static test selection; explicitly report absent Go dynamic
  coverage semantics. A Go coverage collector/importer is later work.

Maintain Rust 1.90 / edition 2024. Preserve unrelated untracked files,
benchmark work, local configuration, and worktree evidence. Public fixtures
use neutral project names. Follow `docs/RELEASING.md`: feature MINOR through
release-plz, reviewed release PR, then publication verification. Implemented,
reviewed, merged, and published are separate states.

## Architecture

Current seams are compiled starter packs (`init/types.rs`), syntax dispatch
(`syntax/mod.rs`), graph extraction (`graph/sync/extract.rs`), and coverage
(`coverage/language.rs`). Toolchains are already declarative. Rhai providers
emit custom facts but cannot impersonate host graph/outcome predicates.

Add a starter-pack registry alongside native adapters. New consumers use a
single validated registry snapshot per operation. Go must use the same
manifest/query/Rhai path exposed to external authors; wrapping a bespoke
Go extractor in a pack does not meet the objective. Keep native adapters
available for deeper language analysis without migrating all of them.

Rust owns bounded parsing/query execution, source access, record validation,
provenance, and storage. Query captures expose node kind, text, byte offsets,
and one-based line spans. Rhai receives immutable per-file context and
bounded captures, and returns structured records. No filesystem, process,
network, or dynamic `eval` API is exposed. Query-only packs work without the
`rhai` feature; script-dependent packs report unsupported capability.

The record vocabulary covers declarations, references, imports, test
identities, and namespaced custom structural facts. Every record has a
validated span. Rust assigns file and adapter provenance, including pack
version/content digest; scripts cannot claim another source or derived
edges. Rust translates records into existing graph/fact relations. Existing
provider reservations remain intact. Shape/provenance validation does not
establish that an author's interpretation is semantically correct.

References retain written targets and resolution status. Only supported,
unambiguous resolution yields call/test relationships. Ambiguous, dynamic,
external, or unknown receiver targets remain unresolved with diagnostics.
Static relationships never imply observed coverage or passing tests.

## Pack format and commands

A directory contains `pack.json`, query files, optional `adapter.rhai`,
optional `rules.json` and `toolchains.json`, and fixtures. Manifest schema
version and adapter API version are both 1. Required metadata: stable pack
ID, pack version, language ID, extensions, bundled grammar ID, capabilities,
and explicit relative resource paths. The plan pins exact field names,
record types, and limits before implementation.

Capabilities are independent: structural facts, graph extraction, and
toolchain recognition. Functions/tests/coverage are not mandatory. Reject
unknown fields, unsupported versions/grammars/capabilities, duplicate IDs,
conflicting extension ownership, invalid assets, and unsafe paths with
specific diagnostics. Installed packs cannot shadow native or built-in
language ownership in v1.

```text
phr-mcp pack list [--json]
phr-mcp pack validate <directory> [--json]
phr-mcp pack install <directory> [--dry-run]
phr-mcp init --packs go
```

The MCP server exposes read-only `list_language_packs {}` for its project
root, using the same discovery/reporting implementation as `pack list
--json`. Return built-in and project-installed packs with their ID, version,
origin, language, extensions, grammar, declared capabilities, effective
activation status per capability, and validation status/diagnostics. Keep
bundled availability distinct from project activation of rules/toolchains.
Include adapter API version and content digest when available; do not expose
script bodies or accept a caller-selected filesystem root.

Listing remains useful when a pack is broken: return identifiable invalid
entries and structured diagnostics rather than silently dropping them or
reporting an empty installation. Unknown metadata stays unknown; include an
overall incomplete/error status when enumeration cannot finish. Inspection
does not execute adapters or fixture tests, install anything, or rebuild the
graph. Refresh discovery on each request so an already-running MCP server
sees installations and removals. Document the tool in server discovery
instructions and the agent guide.

`validate` compiles queries/scripts, validates rules/toolchains, and checks
fixture extraction expectations. Fixtures are inputs and expected records,
never executable commands. `install` validates, copies only declared assets
and fixtures, and atomically publishes `.phronesis/packs/<id>/` with digest
metadata. Identical installation is a no-op; different contents under an
existing ID fail in v1. Upgrade/uninstall commands are deferred. Pack files
are authored configuration and survive `clean`.

Each operation verifies installed contents against recorded digests. Changed
installed assets produce actionable errors. Authors edit source directories
and install into fresh fixture projects. Enumeration is deterministic.

Go's built-in assets ship in the crate and support read-only graph inspection
without `init`, like native languages. `init --packs go` activates starter
rules/toolchains through non-destructive init semantics. External installed
packs activate their capabilities. Both use the same validation/execution.

## Integration, freshness, and errors

Hook and audit structural facts use the same file images, rules, and phases
as native checks. Relevant adapter failures block pre-check, warn in
post-check, and make audit incomplete with a failing exit. Non-tool lifecycle
events retain their fail-open behavior. Invalid registry configuration must
not silently disable checks, even if applicability cannot be established.
Provide a specific diagnostic and repair procedure.

Install does not rewrite project rules. Compose pack rules as lower-priority
layers through shared rule resolution, retain override provenance, require
pack-owned rule IDs, and keep explicit project layers authoritative. Compose
toolchains equivalently with project overrides and origins. Long-running
MCP servers refresh registry changes before relevant operations.

Discovery, incremental updates, rebuild, hydration, and freshness share the
registry's source-file population. Include manifests, queries, scripts,
digest metadata, and identity inputs such as `go.mod` in freshness. Added,
changed, or removed packs invalidate the graph. Adapter errors discard
partial records and obsolete per-file edges; they cannot masquerade as a
successful empty extraction. Rebuild reports incomplete work. Preserve
existing edge consumers while recording adapter provenance.

Bound source bytes, query matches, capture text, Rhai operations, and output
records. Report parse errors and limit exhaustion explicitly. Include tests
for repairing/removing broken pack configuration without disabling unrelated
rules or opening arbitrary source files to unchecked edits.

`coverage select` and MCP `select_tests` report supported Go static tests and
explicitly name absent dynamic coverage support. Do not add a coverage row
that implies an implemented collector/importer.

## Go starter behavior

- Discover `.go` and nearest enclosing `go.mod`; distinguish nested modules.
  Use deterministic repository-relative fallback without a manifest.
  Namespace packages by module/path and declared package, including distinct
  external `*_test` packages.
- Extract functions/methods, spans, imports/aliases, and raw calls. Preserve
  receiver type in identities; normalize pointer/value receiver spelling
  and generic receiver base types.
- Classify `*_test.go` and discover top-level standard `TestXxx` functions
  with supported `testing.T` signatures and import aliases. Helpers remain
  ordinary functions. Benchmarks, fuzz targets, examples, subtests, and
  nonstandard test frameworks are outside starter discovery.
- Resolve unqualified calls to unique package-level functions across files.
  Local/parameter shadowing and build-tag duplicates must not invent calls.
  Receiver/cross-package calls remain raw references in v1.
- Expose definitions/imports/calls/static test relationships; render safe
  anchored `go test <package> -run <test>` commands for supported tests.
- Include a small advisory structural rule, with comment/string/lookalike
  negatives. The plan selects the rule and documents its semantic limits.
- Recognize `go build`, `go vet`, and textual `go test` outcomes with
  unknown/fail/pass semantics. Fixture compile/test failures, successful
  tests, no-tests output, absent exits, and truncation. Missing evidence
  cannot invent a pass. Structured JSON output support is deferred.

This is source-level analysis: build constraints/platform selection, cgo,
generated-code expansion, interface dispatch, and compiler type resolution
are not modeled. Candidate collisions remain explicitly ambiguous.

## Acceptance gates

1. Exercise a real minimal Go module: init, audit, graph queries, pre/post
   hooks, `go test`, outcome ingestion, and static test selection after a
   production edit. Assert no manufactured dynamic coverage.
2. A second external pack with a bundled grammar/unclaimed extension
   installs into a fresh project and contributes rules/facts/edges through
   hook and audit without editing core code. Include useful query-only and
   Rhai fixtures. No language-name switches to implement those fixtures.
3. Direct tests for schema errors, namespace spoofing, traversal/symlinks,
   ownership conflicts, limits, tampering, install idempotency, project
   overrides, repair, registry freshness, and no-default-features behavior.
   Check CLI/MCP pack-list parity, built-in versus installed/active states,
   broken-pack diagnostics, read-only behavior, and install/remove visibility
   through consecutive requests to the same MCP server instance.
4. Independent review of evidence boundaries and contributor instructions;
   controller independently reproduces acceptance rather than accepting
   worker claims as completion evidence.
5. Workspace tests, warning-free Clippy, formatting, applicable MSRV checks,
   and worker targeted checks. Measure warm hook/audit overhead on identical
   inputs with/without packs, including a project with no pack-owned files.
   Resolve material regressions before readiness claims.
6. Update contributor/user docs, schemas/examples, Unreleased changelog,
   generated catalogue, and capability/limitation matrix. Release-plz owns
   the eventual version change; no published-version claim before release.

## Execution

After design approval, write the file-specific Superpowers plan, covering
registry/validation, extraction/Rhai, integration, Go, contributor fixtures,
and independent acceptance. Dependency-order tasks with independent build
and review ownership and at most three concurrent workers.

Use a fresh secure swarm. Authorized worker pool: Codex agents and Claude
Opus/Sonnet; initial deadline two hours; budget USD 30; at most three workers
plus the controller. Verify exact available profiles before dispatch and
record unmetered estimates separately from measured API spend. Do not infer
authorization for additional providers from the user's "etc.". The existing
secure host points at an older integration swarm; it is not authority to
reuse that swarm or its credentials. Scope the operator ceiling to approved
worktrees and routes. Preserve worktrees and evidence. Publication follows
implementation and review as a separate explicit stage.

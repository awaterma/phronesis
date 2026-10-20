# Go and Starter Language Packs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
> **User-selected execution:** Swarming supersedes the execution-method choice
> above. Use its secure contracts, auctions, governed worktrees, and review
> gates; do not substitute an untracked set of implementation agents.

**Goal:** Release starter Go support and independently authorable language
packs, discoverable through CLI and MCP, with structural evidence shared
across hooks, audit, graph queries, and static test selection.

**Architecture:** Add a validated pack registry beside existing native
adapters. Rust owns parsing, resource limits, identities, provenance, and
storage; queries and optional Rhai produce typed per-file records. Go ships
as assets through that same public adapter contract.

**Tech Stack:** Rust 1.90 / edition 2024, existing tree-sitter 0.25 and Rhai,
serde JSON, RETE, rmcp, clap, SHA-256 content digests, bundled Go grammar.

**Spec:** `docs/superpowers/specs/2026-10-17-go-and-language-packs-design.md`

Status (2026-10-18, parked; resumed 2026-10-19; reviewed and fixed 2026-10-20): The Codex-run Swarming campaign stopped when the
Codex account hit its usage limit (resets 2026-10-24). Code for every task
T1-T8 exists in the `feat/go-language-packs` worktree
(`.worktrees/go-packs`) and is committed there as one WIP snapshot; nothing
is pushed and no PR exists. The checkboxes below were never ticked by the
swarm, so use this block, not the boxes, as the record.

Verified on 2026-10-18 after parking (gate target dir `target/swarm/gate`):

- `cargo check -p phronesis-mcp --tests`: clean.
- `cargo fmt --all --check`: clean (formatting was applied when parking).
- `cargo clippy --workspace --all-targets -- -D warnings`: clean after two
  needless-borrow fixes (`graph/sync/extract.rs`, `language_packs/templates.rs`).
- The ten new suites (`language_pack_*`, `go_language_pack`,
  `go_pack_selection`) and `phronesis-rhai --test adapter`: all green after
  two test-side fixes (canonicalize the tempdir on macOS; read `get_rule`'s
  text payload instead of a `structuredContent` field it never had).
- `cargo package -p phronesis-mcp --list --allow-dirty` lists `packs/go/*`.
- `docs/catalogue.html` contains the Go dot-import rule.
- Whole-workspace `cargo test --no-fail-fast`: 115 of 116 test binaries
  green. Two branch regressions were found against existing suites (both
  pass on `main`):
  - `hook_full_file_regions` (`unreadable_file_counts_as_wholly_changed`,
    `oversized_file_is_bounded_and_wholly_changed`): pre-check blocked
    (exit 2) instead of warning when the edited file's whole image could not
    be mapped. FIXED when parking: `hook/pre.rs` now maps the whole image
    only for pack-owned files; non-pack files keep the pre-pack behaviour.
  - `save_rules_integration::mcp_reports_and_rebuilds_the_code_graph_lifecycle`:
    the first `rebuild_code_graph` over MCP reports `generation` 2 instead
    of 1. NOT FIXED. Something on the branch bumps the index generation
    twice on the first rebuild (see `graph/sync/rebuild.rs` `next_index` vs
    the incremental-save bump, and whether the server now persists an index
    before the explicit rebuild). Needs a reviewer who knows the graph
    generation contract.

Resumed 2026-10-19 (Opus 5.5, interactive, no swarm), four commits on top
of the WIP snapshot:

- `c111397` fixes the `save_rules_integration` regression. It was never a
  generation bug: line 336 asserts `files_indexed == 1`, and the index's
  virtual `@pack-builtin/go` freshness key was being counted as a file.
- `4917afc` adds the opt-in `test_counts: "per_test"` toolchain field and
  opts `go.test` into it. Before this, no `go test` run could ground a test
  outcome (Go prints no counts), and a failing assertion read as a build
  failure, against the spec's unknown/fail/pass requirement.
- `03e2d6b` lets `signal ingest` record compile-only evidence (it refused a
  real compile error), and makes the acceptance script assert the verdicts.
- `a616e7e` adds Verus invariant 6 (graded toolchain outcome): 29 verified,
  0 errors.

Verified on 2026-10-19:

- Whole-workspace `cargo test --no-fail-fast`: 118 of 118 test binaries
  green, 3622 passed, 0 failed, 4 ignored. fmt and clippy
  (`--all-targets -D warnings`) clean.
- `scripts/verify-language-packs.py` against real Go 1.26.4: all 23 steps
  pass (init 108 ms on an idle machine; the earlier 180 s timeout was CPU
  starvation from a concurrent test run).

Review and fixes (2026-10-19/20): a read-only Codex review
(`docs/reviews/2026-10-19-go-language-packs/codex-review.md`) raised 14
findings; each was verified against the code before fixing
(`triage.md` there). 13 are fixed test-first and one was refuted (its
sub-claim fixed); every fix commit names its finding. The branch was then
rebased onto `main` (v0.39.0) without conflicts.

The T8 latency gate (warm release builds, one warm-up then 30 samples,
`p50`) found and drove one more fix (`3301a4b`): every hook load compiled
the built-in Go pack, doubling hook time in projects with no Go. Built-in
packs now compile on first use. After the fix:

| project | measure | main | branch |
|---|---|---|---|
| Rust | pre-check | 10.2 ms | 10.6 ms |
| Rust | post-check | 26.9 ms | 27.9 ms |
| Rust | audit | 11.2 ms | 11.7 ms |
| Go | pre-check | 4.3 ms (no Go support) | 14.6 ms |
| Go | post-check | 17.1 ms (no Go support) | 44.1 ms |
| Go | audit | 4.4 ms (no Go support) | 24.9 ms |

Verified on the rebased branch (2026-10-20): whole-workspace `cargo test
--no-fail-fast` 121/121 binaries (3646 passed, 0 failed); fmt, workspace
clippy `-D warnings`, and `--no-default-features` clean; real-Go
`scripts/verify-language-packs.py` 25/25; Verus 31 verified, 0 errors.

Still open before this can become a PR:

- A second Codex pass over the fixes (`brief-fixes.md`).
- Split the WIP snapshot into per-task commits or accept one feature
  commit; squash-merge makes this a PR-title question for the user.
- The user reviews the diff before the one push that opens the draft PR.
- The swarm ledger for the original campaign was not found under
  `/Volumes/Data/Git/phronesis-wt/`; spend and bid records live only in the
  Codex session transcript.

## Global Constraints

- At most three workers plus the controller (four active agents total).
- Codex agents and Claude Opus/Sonnet; USD 30 budget, two-hour initial deadline.
- Bundle grammars in Rust, adding Go. Independent grammar loading is later.
- JSON manifests, tree-sitter queries, optional sandboxed Rhai.
- Preserve existing native adapters, identities, and rule behavior.
- Support Go static test selection; explicitly report absent Go dynamic
  coverage semantics. A Go coverage collector/importer is later work.
- Maintain Rust 1.90 / edition 2024.
- Preserve unrelated files, benchmark work, local configuration, and evidence.
- Public fixtures use neutral project names.
- Follow `docs/RELEASING.md`; implementation does not imply publication.
- No installed-pack script execution during listing; no provider reservation
  relaxation, implicit pack upgrades, remote installation, or hosted registry.

## Review Focus

1. A pack installs while an MCP connection stays open: next list/rule/graph
   operation must see it, without autosaving pack rules into project rules (T4).
2. An installed pack is damaged or removed: listing stays diagnostic,
   evaluation cannot silently allow, and stale graph evidence is rejected (T3/T5).
3. Two Go files/build-tag variants declare the same function: preserve site
   multiplicity and report ambiguity rather than collapse them into one target (T5/T6).
4. Test selection across nested modules, deleted functions, Unicode paths,
   and shell metacharacters must retain scope and safe command arguments (T7).
5. Scriptless builds and projects with no pack-owned files must stay usable;
   scripts, large captures, and query limits cannot bypass the sandbox (T2/T8).

## Shared contracts (pin before workers diverge)

New module root: `crates/phronesis-mcp/src/language_packs/`. Keep each module
under 800 lines; tests may live in sibling test modules. Shared record
validation lives here, not in individual language adapters.

### Manifest and assets

`PackManifest` uses `#[serde(deny_unknown_fields)]` recursively. Fields:
`schema_version: u32` (=1), `adapter_api: u32` (=1), `id: String`,
`version: String` (three numeric components), `language: String`,
`extensions: Vec<String>`, `grammar: String`, `capabilities: Vec<Capability>`,
`queries: Vec<QuerySpec>`, `adapter: Option<String>`,
`rules: Option<String>`, `toolchains: Option<String>`,
`context_files: Vec<String>`, `fixtures: Vec<FixtureSpec>`.

IDs/language names match `[a-z][a-z0-9_]{0,47}`; extensions have no leading
dot, path separator, glob, or whitespace. Ownership matching is exact and
case-sensitive. Capabilities are `structural_facts`, `graph`, `toolchains`.
Reject contradictory declarations (e.g. toolchains resource without capability).
Grammar IDs initially expose the bundled Rust, Python, Swift, TypeScript,
TSX, Java, Lua, and new Go grammars. Unknown grammars fail with available IDs.

`QuerySpec { id: String, path: String, emit: Vec<RecordTemplate> }` identifies
each capture group. Empty `emit` permits Rhai-only interpretation. A template
is a JSON object matching the record schema below; whole-string `${capture}`
values substitute the capture's text, `${file}` substitutes relative file,
and a `span_capture` field chooses a captured node's span. Literal values
remain literal; no evaluation/interpolation inside strings. Missing or
non-unique referenced captures fail that emission explicitly. Repeated
captures remain available as arrays to Rhai. `FixtureSpec { path: String,
expected: String }` compares canonical records/diagnostics; no command fields.

`context_files` contains basenames only, at most four. For each source file,
the host supplies the nearest such file between its parent and repository
root as `{name, file_rel, directory_rel, content}`. No disk API enters Rhai.
This generic context mechanism supplies `go.mod`; extraction can interpret
its module declaration without a Go branch in the host. Added/removed context
files participate in discovery/freshness, including a newly closer manifest.

`PackAssets` contains manifest and a sorted map of relative asset paths to
bytes. `PackError` is a typed `thiserror` enum with pack/path/stage details.
`ValidatedPack` contains immutable assets, digest, compiled queries, and
validated optional script. SHA-256 covers sorted, length-prefixed names and
exact bytes of every declared resource including the manifest and fixtures.
Reject absolute/parent paths, symlink components, nonregular assets, and
duplicate normalized paths. Parse and execute the same bytes that were hashed.

Limits: 64 installed packs; 128 assets/pack; 64 KiB manifest/script/query;
1 MiB source/context/fixture file; 8 MiB total assets/pack; 32 queries/pack;
8,192 matches and 16,384 captures/file; 8 MiB total captured text/file;
4,096 output records/file; 32 custom fact arguments of 4 KiB each. Parsing
and querying each have a 250 ms cancellation deadline, plus match limits.
Rhai gets 2,000,000 operations, call depth 16, and a bounded allowance for
injected captures. These are adapter limits; existing guard/provider limits
remain unchanged. Limit exhaustion discards the extraction, never truncates
it into successful evidence. Test deterministic count limits independently
of timing; wall-clock tests assert cancellation rather than a precise duration.

### Record and runtime contract

`SourceSpan { start_byte: usize, end_byte: usize, start_line: u32,
end_line: u32 }` uses half-open UTF-8 byte boundaries and one-based lines.
`Capture { name, kind, text, span }` and
`QueryMatch { query: String, captures: Vec<Capture> }` preserve match grouping.
`FileContext { file_rel: String, context_files: Vec<ContextFile> }` contains
host-owned paths. Rhai receives constants `context` and `matches` and returns
an array of records as its final expression. It cannot mutate host context.

`PackRecord` is a tagged serde enum (`kind`, snake_case) with:

- `Declaration { name, scope, receiver: Option<String>, span, test: bool,
  command: Option<TestCommand> }`. A command requires `test: true` and
  `TestCommand { program: String, args: Vec<String>, cwd: String }`; cwd is
  a host-validated repository-relative directory. Commands are rendered,
  never executed by pack processing.
- `Reference { owner_start_byte: usize, target: String, scope: String,
  resolution: ReferenceMode, span }`. Modes: `same_scope`, `unresolved`.
  The owner must match a declaration in this file. Only `same_scope` is
  eligible for cross-file resolution against unique free declarations.
- `Import { scope: String, target: String, alias: Option<String>, span }`.
  Imports are written dependencies, not evidence of resolved callable targets.
- `Fact { name: String, args: Vec<String>, span }`. Names are local identifiers;
  the host prepends `pack_<id>_` and the current relative file argument.

All strings are owned Rust Strings in the public types. `Extraction` contains
`records: Vec<PackRecord>` and `diagnostics: Vec<PackDiagnostic>`;
`PackDiagnostic { pack: Option<String>, path: Option<String>, stage: String,
message: String }`. Parse/script/schema errors use `Result::Err`, not an empty
successful record list. Line/byte correspondence is verified against source.

The host qualifies scopes/names under `<language>:<pack-id>::...`, escaping
components deterministically. Keep site identity `(file, start_byte)` as well
as qualified symbol identity: repeated semantic names do not erase ambiguity.
Commands cannot forge outcomes. Records cannot request arbitrary host predicates,
derived edges, source paths, timestamps, or provenance.

## Task 1: Validated manifest and asset loader

**Files:** Create `language_packs/{mod,manifest,assets,records,error}.rs`;
modify `src/lib.rs`, `crates/phronesis-mcp/Cargo.toml`; create
`tests/language_pack_validation.rs` (paths under `crates/phronesis-mcp`).

**Interfaces:** `read_assets(dir: &Path) -> Result<PackAssets, PackError>`;
`validate_manifest(manifest: &PackManifest) -> Result<(), PackError>`;
`digest_assets(assets: &PackAssets) -> String`; types from Shared contracts.
Use a direct SHA-256 dependency compatible with Rust 1.90 rather than the
noncryptographic graph hash. No registry/install effects yet.

- [ ] Write validation tests: `unknown_fields_and_versions_fail`,
  `resources_cannot_escape_pack`, `digest_covers_every_declared_asset`,
  `duplicate_paths_and_ownership_are_rejected`, `asset_caps_are_enforced`.
  Assert errors name field/path/pack; traversal/symlink cases read no outside
  bytes; changing one declared byte changes the digest.
- [ ] Run `cargo test -p phronesis-mcp --test language_pack_validation` and
  record the expected missing implementation failure.
- [ ] Implement the manifest/record types, bounded asset reads, and canonical
  digest; no stringly-typed graph writes or side effects in deserialization.
- [ ] Run the same suite to green and format changed files.
- [ ] Commit only this task's files: `feat(mcp): define starter language pack contract`.

## Task 2: Query execution and sandboxed Rhai interpretation

**Files:** Create `language_packs/{grammar,extract,templates}.rs`,
`crates/phronesis-rhai/src/adapter.rs`,
`crates/phronesis-rhai/tests/adapter.rs`, `tests/language_pack_extraction.rs`;
modify both crate roots and MCP Cargo dependencies.

**Interfaces:** `compile_pack(assets: PackAssets) -> Result<ValidatedPack, PackError>`;
`extract(pack: &ValidatedPack, context: &FileContext, source: &str)
-> Result<Extraction, PackError>`. In phronesis-rhai:
`evaluate_adapter(script: &str, input: &serde_json::Value)
-> Result<serde_json::Value, AdapterError>` and
`validate_adapter(script: &str) -> Result<(), AdapterError>`.
The MCP layer validates returned JSON against `PackRecord`; the Rhai crate
does not depend on MCP. Use the existing sandbox setup without widening other
entry points. Verify the Go grammar release's tree-sitter ABI/MSRV before
pinning its dependency; record the selected version in the task evidence.

- [ ] Write tests asserting query-only and Rhai versions extract identical
  declaration spans from a fixture; strings/comments produce no false
  declarations; malformed source returns error; changed capture arrays cannot
  mutate host input; absent captures and invalid owner/span references fail.
- [ ] Add tests for forbidden filesystem/eval APIs, infinite loops, match and
  record caps, source UTF-8 boundaries, and `--no-default-features` (query-only
  success, script-dependent explicit error).
- [ ] Run `cargo test -p phronesis-rhai --test adapter` and
  `cargo test -p phronesis-mcp --test language_pack_extraction`; record red.
- [ ] Implement grammar registry, parse-once queries, template translation,
  immutable Rhai scope, and host validation. Context files are supplied as
  bytes by later callers; scripts never read the filesystem.
- [ ] Rerun both suites plus
  `cargo test -p phronesis-mcp --no-default-features --test language_pack_extraction`.
- [ ] Commit: `feat(mcp): run bounded query and Rhai language adapters`.

## Task 3: Installation, inventory, CLI, and MCP discovery

**Files:** Create `language_packs/{registry,install,fixtures,cli}.rs`,
`tests/language_pack_install.rs`, `tests/language_pack_discovery.rs`;
modify `src/{main,server,state}.rs` and server discovery tests.

**Interfaces:** `inspect(root: &Path) -> PackInventory` (diagnostic, no scripts);
`load(root: &Path) -> Result<PackRegistry, PackError>` (strict snapshot);
`validate_directory(dir: &Path) -> Result<ValidationReport, PackError>`;
`install(root: &Path, dir: &Path, dry_run: bool) -> Result<InstallReport, PackError>`;
`PackRegistry::for_path(&self, rel: &str)
-> Result<Option<&ValidatedPack>, PackError>`.

Retain unavailable built-in descriptors when a compiled feature is absent;
loading the registry must not break a native-only project just because the
bundled Go adapter requires Rhai. Looking up a source owned by an unavailable
adapter returns an explicit unsupported error. Installing or activating such
a pack fails. An invalid actively installed pack remains a strict load error.

`PackInventory { packs: Vec<PackInfo>, diagnostics: Vec<PackDiagnostic>,
complete: bool }`. `PackInfo` has optional metadata for invalid manifests:
`id, version, language, grammar, adapter_api, digest`; known `origin`
(`built_in`/`installed`), `location`, `extensions`, `capabilities`,
`activation` map (`structural_facts`, `graph`, `rules`, `toolchains`),
`status` (`valid`/`invalid`/`unsupported`), and diagnostics. Activation values
are `active`, `available`, `overridden`, `unsupported`, `invalid`, or
`not_provided`. Sort by origin/id/location. List the new registry's language
packs; explain that legacy rule-only starter selections are not installed
language packs. Never label legacy selections absent merely because they
have no pack manifest.

- [ ] Write tests for atomic install, no-op reinstall, collision refusal,
  dry-run no writes, interrupted staging, and `state`/`clean` preservation.
  Staging is outside the installed directory enumeration; lock concurrent
  installs, recheck target under lock, and atomically rename only validated bytes.
- [ ] Write discovery tests asserting CLI JSON equals the MCP report for one
  project; missing manifests/tampering remain visible; two calls on the same
  server reflect install/remove; `list_language_packs` is registered and
  advertised. Snapshot project bytes before/after listing to prove no writes.
- [ ] Run `cargo test -p phronesis-mcp --test language_pack_install --test language_pack_discovery` (red).
- [ ] Implement `.phronesis/packs/<id>/installed.json` with schema/API version,
  version and digest. Reject a manifest asset named `installed.json`. Ignore
  no arbitrary resource: undeclared installed files are diagnostic errors.
  Expose `list_language_packs {}` with the server's root and shared inventory.
- [ ] Rerun suites (green); verify malformed installations cause strict load
  errors while read-only inventory still returns diagnostic entries.
  In a no-default-features binary, list the Rhai-dependent Go descriptor as
  unsupported, successfully analyze a native fixture, and reject a Go source
  lookup explicitly rather than interpreting it as Rust or an unknown file.
- [ ] Commit: `feat(mcp): install and discover language packs over CLI and MCP`.

## Task 4: Rule/toolchain composition and hook/audit evaluation

**Files:** Create `language_packs/{facts,integration}.rs`,
`tests/language_pack_hooks.rs`, `tests/language_pack_layers.rs`;
modify `src/rule_layers.rs`, `src/outcomes/toolchain.rs`,
`src/hook/{mod,pre,post}.rs`, `src/hook_facts.rs`,
`src/audit/{engine,run,types}.rs`, `src/server.rs`.

**Interfaces:** `facts_for_file(registry: &PackRegistry, context: &FileContext,
source: &str) -> Result<Vec<phr::Fact>, PackError>`;
`pack_rule_layers(registry: &PackRegistry) -> Vec<PackRuleLayer>` where
`PackRuleLayer { name: String, path: PathBuf, rules: Vec<DiskRule> }`;
`pack_toolchains(registry: &PackRegistry) -> Vec<ToolchainDef>`.
Require rule IDs `<pack-id>.…` and toolchain IDs `<pack-id>.…`.

- [ ] Write tests for pack-only rules with no project rules file; explicit
  project overrides and provenance; `save_rules` never copies pack rules;
  refreshing an existing MCP network adds/removes only disk-managed rules
  and preserves asserted session facts. Refresh before rule-consuming or
  rule-writing operations and retain the existing disk-load-error guard.
- [ ] Write a query-backed fixture rule exercised through CLI pre/post and
  audit: same source fact and location, correct phase, comment/string negative,
  multi-file edit whole-image handling, `.phronesisignore` structural behavior.
  Script/parse errors must block pre, warn post, and make audit incomplete.
- [ ] Write repair tests: a shell `phr-mcp pack list` remains inspectable;
  source edits remain governed; repairing a broken asset must be by explicit
  removal/reinstall from trusted local source, not by granting arbitrary
  source writes. Document operator shell recovery for digest-pinned assets.
  Non-tool lifecycle adapters remain fail-open on registry errors.
- [ ] Run `cargo test -p phronesis-mcp --test language_pack_hooks --test language_pack_layers` (red).
- [ ] Compose pack rules below existing project/layer precedence without a
  recursive registry/rule-loader dependency. Keep existing override facts.
  Append custom facts through the shared structural path; audit must evaluate
  actual joined conditions, not treat unknown predicates as lexical matches.
  Use one snapshot per operation; translate errors at existing phase boundaries.
- [ ] Rerun suites and existing hook, rule-layer, save-rule, and audit tests.
- [ ] Commit: `feat(mcp): evaluate pack rules consistently across hooks and audit`.

## Task 5: Graph extraction, generic resolution, and freshness

**Files:** Create `language_packs/{graph,context}.rs`,
`tests/language_pack_graph.rs`; modify `src/graph/sync/{index,extract,rebuild,mod}.rs`,
`src/graph/{hydrate,derive}.rs` and `src/predicate_provider.rs` as needed to
reserve newly host-owned relations (never remove reservations).

**Interfaces:** `file_context(root: &Path, rel: &str, pack: &ValidatedPack)
-> Result<FileContext, PackError>`;
`graph_records(pack: &ValidatedPack, context: &FileContext, extraction: &Extraction)
-> Result<Vec<Edge>, PackError>`;
`resolve_pack_references(base: &[Edge]) -> Vec<Edge>`;
`PackRegistry::input_files(&self) -> Vec<PathBuf>` (complete asset/context inputs).

Preserve `Edge` serialization. Add host-owned relations `pack_declaration`
(site, symbol, scope, name, receiver, test, start, end), `pack_reference`
(owner-site, scope, written-target, mode, start, end), `pack_import`
(scope, written-target, alias), `pack_test_command` (test, cwd, program,
JSON argv), and `pack_file_adapter` (file, pack, version, digest). Each is
attributed through `Edge::src`; sites include escaped file and start byte.
Also emit compatible definitions/module/file-type edges. Expose written
imports through `imports` using a distinct namespaced unresolved target;
retain the raw spelling/alias in `pack_import`. Do not turn an unresolved
import into a resolved module dependency or an invented import cycle.
Keep raw references
out of native bare-name canonicalization. Derive `calls`/`tested_by` only
after unique same-scope resolution, counting sites before deduplication;
transitive reach uses the existing derivation after that step.

- [ ] Write graph tests for disjoint extensions, cross-file same-scope calls,
  unknown receivers, duplicate sites, deleted targets, and two packs with
  same-leaf names. Assert ambiguous sites yield no resolved call/test edge.
- [ ] Write freshness tests for installing/removing packs, changing query or
  script bytes, tampering with locks, changing a context file, adding a nearer
  context file, and source parse failure after previously valid extraction.
  Assert no fresh index with obsolete successful edges.
- [ ] Run `cargo test -p phronesis-mcp --test language_pack_graph` (red).
- [ ] Extend shared walk/snapshot dispatch, context discovery, resolution,
  and graph hydration. On extraction error, invalidate graph completeness
  and remove obsolete edges for that source; return diagnostics to CLI/MCP.
  Native adapters retain existing dispatch and identity. Increment graph
  format for new persisted semantics and require rebuild of older indexes.
- [ ] Rerun suite and existing graph sync/derivation tests; compare a native
  fixture graph before/after with no installed packs for identical identities.
- [ ] Commit: `feat(mcp): index pack sources with provenance and freshness`.

## Task 6: Ship Go through the pack contract

**Files:** Create `crates/phronesis-mcp/packs/go/{pack.json,declarations.scm,calls.scm,imports.scm,adapter.rhai,rules.json,toolchains.json}`,
`packs/go/fixtures/` input/expectation files, `language_packs/builtin.rs`,
`tests/go_language_pack.rs`; modify `src/init/{types,mod,writer_toolchains}.rs`
and package inclusion checks.

**Interfaces:** `builtin_packs() -> Result<Vec<ValidatedPack>, PackError>`;
add `Pack::Go`, aliases `go`/`golang`, and existing compose/init registration.
Embed assets with `include_str!`/`include_bytes!` so installed binaries need
no source checkout. Both embedded and directory assets run through T1/T2.

- [ ] Write fixture expectations for free functions, pointer/value/generic
  methods, aliases, grouped imports, package test helpers, valid/invalid Test
  names and `testing.T` signatures, Unicode names, malformed Go, and source
  lookalikes inside comments/raw strings. Table-driven snapshots pin identities.
- [ ] Write project tests with `module example.com/demo`, sibling packages,
  a nested module, external test package, shadowed functions, and duplicate
  build-tag definitions. A test calls a production function in another file
  in the same package; assert exactly that static relationship.
- [ ] Write outcome fixture tests for `go build`, `go vet`, and textual
  `go test -v`: success, build failure, test failure, no tests, unknown exit,
  truncated output, cached success. No-tests never produces a positive test
  count; a successful command exit alone cannot establish tests were run.
  Preserve build success from an explicit zero exit even when the command
  normally prints nothing; unknown test execution is a separate verdict.
- [ ] Run `cargo test -p phronesis-mcp --test go_language_pack` (red).
- [ ] Implement Go queries/Rhai. Interpret nearest `go.mod` in Rhai; include
  its relative directory and package path/name in scopes. Suppress same-scope
  resolution conservatively when local/parameter/import names shadow a call.
  `init` and blank declarations must not collapse into ordinary callable IDs.
- [ ] Ship advisory `go.warn-dot-import` using `pack_go_dot_import(file, path)`
  from an actual dot import capture. Message: "Dot import of ?path hides where
  names come from; prefer a qualified import." Phase post, audit true. Tests
  prove named/blank imports, comments, and strings do not trigger it.
- [ ] Add Go init rules/toolchains through existing preserve-local-edits
  semantics; rerun suite and `cargo test -p phronesis-mcp --test language_pack_toolchains`.
- [ ] Commit: `feat(mcp): ship Go starter language pack`.

## Task 7: Static test selection independent of dynamic coverage

**Files:** Create `language_packs/selection.rs`, `tests/go_pack_selection.rs`;
modify `src/coverage/select.rs`, its JSON/table rendering, and MCP select tests.

**Interfaces:** `changed_pack_symbols(registry: &PackRegistry, root: &Path,
diffs: &[FileDiff]) -> Result<Vec<PackChangedSymbol>, PackError>` where
`PackChangedSymbol { file: String, symbol: String, region: String }`;
`render_test_command(command: &TestCommand) -> Result<String, PackError>`.
Use existing `function_region_id(file, item_path)` for labels only; do not
add a Go row to the dynamic coverage registry. Compare old/new declaration
spans and text, including removed declarations; join exact symbol IDs against
fresh pack-derived reachability. Retain current native selection path.

- [ ] Write tests asserting a changed Go production body selects its direct
  and transitive supported tests with `evidence == "static_reach"` only;
  unrelated packages/tests are excluded, stale graph omits selection with a
  note, and deleted declarations do not create invented current targets.
- [ ] Test nested-module commands from repository root, anchored test names,
  Unicode and quote/shell metacharacters in paths. Verify argv with a fake
  executable and absence of side effects, not string assertions alone.
- [ ] Run `cargo test -p phronesis-mcp --test go_pack_selection` (red).
- [ ] Implement the separate pack-static join and safe command rendering.
  Extend JSON/table output additively with `dynamic_coverage_unsupported`
  listing affected pack files; preserve `no_coverage_semantics` for evaluated
  languages and explain Go is executable but its collector is unsupported.
- [ ] Rerun suite, existing coverage selection tests, and CLI/MCP parity tests.
- [ ] Commit: `feat(mcp): select Go tests from pack static evidence`.

## Task 8: Contributor workflow, independent acceptance, and release preparation

**Files:** Create `examples/language-packs/python-starter/` (manifest, queries,
optional Rhai variant, fixtures; ID `python_starter`, extension `pystarter`),
`docs/LANGUAGE-PACKS.md`, `tests/language_pack_acceptance.rs`, and
`scripts/verify-language-packs.py`; update README, crate README/CLAUDE.md,
AGENTS.md, `docs/FOR_AGENTS.md`, CHANGELOG.md, docs/catalogue.html, CI.

**Interfaces:** Document and exercise T1–T7's public commands, manifest,
records, template syntax, errors, and MCP responses. The verification script
accepts explicit `--binary`, `--artifacts`, and `--go` paths; uses temp Git
projects; saves commands, exits, outputs, revisions, and durations. It fails
when required Go execution is unavailable rather than reporting a skipped pass.

- [ ] Write acceptance tests that install the external pack, evaluate its
  joined rule in a hook and audit, query a definition, and list it through
  an already-running MCP server. Expected failures must precede integrations.
- [ ] Build the example from documentation alone, with no core edits. Run
  query-only and Rhai variants in separate projects to avoid ID collisions.
  Demonstrate explicit recovery by removing the installed directory and
  reinstalling validated source; never hide digest failures.
- [ ] Implement the real-Go verifier: init, commit fixture sources, graph
  rebuild, edit, pre/post hook replay, audit, static select, real `go test -v`,
  `signal ingest`, and MCP inventory. Include a deliberately failing test
  and compile failure. Save raw output, not only assertion totals.
- [ ] Run controller acceptance independently and assign whole-branch review
  to a worker who did not build the reviewed changes. Re-auction repairs and
  re-review according to the swarming skill; retain failed evidence.
  Independently load and exercise the installed pack using a packaged binary
  outside the source checkout, so embedded assets and project-root selection
  are verified together rather than inferred from unit tests.
- [ ] Run `cargo +1.90 fmt --all --check`,
  `cargo +1.90 clippy --workspace --all-targets -- -D warnings`,
  `cargo +1.90 test --workspace`, and
  `cargo +1.90 test -p phronesis-mcp --no-default-features --test language_pack_extraction`.
  Run the existing no-default-features-with-rhai CI configuration. Add an
  explicit query-only CI check and the real-Go acceptance job with Go 1.26.4,
  matching the locally verified `go version` runtime for this plan.
- [ ] Measure warm hooks and audit: one warm-up then 30 samples per case,
  identical inputs, no packs vs installed pack with no matching files vs
  matching source. Record median/p95 and fixture size. Investigate and resolve
  a no-matching-file regression exceeding both 10% and 5 ms median; report
  matching-file costs rather than asserting a universal latency guarantee.
- [ ] Regenerate catalogue with the tested binary, verify Go rule appears,
  and put Go/language packs first in the Unreleased feature entry. Verify
  `cargo package -p phronesis-mcp --list --allow-dirty` includes embedded pack
  assets. Do not manually bump versions outside release-plz's release PR.
- [ ] Commit explicitly named deliverables: `docs(mcp): prepare Go and language pack release`.

## Swarm dependency schedule and handoff

T1 precedes T2; T2 precedes T3. After T3, T4 (hooks/rules), T5 (graph), and
T6 (Go assets) can be awarded concurrently with disjoint file ownership.
T6 owns `builtin.rs`; T3 defines the registry insertion seam first. Avoid
simultaneous `mod.rs`/Cargo edits by handing them to the controller. T7 waits
for T5/T6. T8 waits for T4/T7. A free worker reviews accepted slices; never
let an implementer approve its own slice. Reviews count toward the three
worker cap. Finish or retire the earlier read-only spec reviewer before
counting new implementation slots.

Before launch: verify exact Codex/Opus/Sonnet profiles, routes, calibration,
and harness availability; no unapproved provider fallback. Start the two-hour
clock at fresh secure-swarm initialization, not during plan review. Record
USD 30 ceiling and reserved/estimated versus measured charges. Obtain live
bids from every eligible approved profile on each first auction, losers
included. Do not equate unavailable billing with zero cost. If calibrated
profiles or safe cost reservation are unavailable, report that specific gap.

Use governed isolated worktrees and distinct external CARGO_TARGET_DIRs.
Credentials stay in private per-role files, never prompts or repository
artifacts; workers receive only their own contract credential path. Include
the skill's targeted-testing paragraph verbatim in each worker brief. Never
hand-edit swarm state/events. Preserve artifacts, branches, and worktrees;
do not dispose of unrelated source-checkout build outputs.

Definition of done is all acceptance gates, independent review, and a
reviewable release change with raw evidence. A two-hour deadline is a stop
and reassessment boundary, not grounds to declare unfinished work done.
Final handoff reports revision, actual checks, failures/limitations, spend
certainty, swarm state, and merge/publication state. Publication follows the
repository's human-reviewed release process after these gates pass.

## Plan self-review

- Spec coverage: T1/T2 format and runtime; T3 inventory/install/MCP/state;
  T4 rules/outcomes/hooks/audit; T5 graph/freshness; T6 Go; T7 selection;
  T8 external contributor path, performance, documentation, and release gates.
- Interface consistency: shared records originate in T1; T2 consumes them;
  T3 owns strict registry versus tolerant inventory; T4/T5/T7 share snapshots.
- Review-focus cases are assigned to explicit tests in their owning tasks.
- Deferred capabilities remain deferred; no hidden Go-only extraction path,
  fake dynamic coverage, or newly introduced external provider.

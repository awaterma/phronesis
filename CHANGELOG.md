# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This project is
pre-1.0: while `0.x`, MINOR versions may carry breaking changes.

## [Unreleased]

### Added

- **Optional Prometheus metrics exporter.** The new `phronesis-metrics` crate
  derives bounded OpenMetrics families from each project's
  `.phronesis/log.jsonl`. Install the CLI with `--features metrics` to enable
  one-shot, atomic textfile, and loopback-only HTTP export through
  `phr-mcp metrics`. Source paths and repository names are never exposed as
  labels, rule-id cardinality is capped, and non-loopback listeners are
  rejected unconditionally.
- **New opt-in `python-patterns` pack** (`phr-mcp init --packs
  python,python-patterns`; alias `py-patterns`). Thirteen advisories derived
  from <https://python-patterns.guide/>, every one backed by a new
  tree-sitter predicate in `syntax/python.rs` — no substring or regex
  conditions anywhere in either Python pack. Warns: `global` rebinding
  (`python_global_statement`), `globals()[...] = ...` introspection
  assignment (`python_globals_subscript_assignment`), three-argument
  `type(...)` dynamic classes (`python_dynamic_class_creation`),
  `__new__`-based singletons (`python_new_override` shape `singleton`),
  containers whose `__iter__` returns `self`
  (`python_container_is_own_iterator`), multiple inheritance of concrete
  classes (`python_multiple_inheritance`), `*Mixin` classes with `__init__`
  (`python_mixin_with_init`), static delegation wrappers of 4+ forwarding
  methods without `__getattr__` (`python_static_delegation_wrapper`),
  mutable containers assigned in a class body
  (`python_mutable_class_attribute`), and `== None` / `!= None`
  (`python_equality_with_none`). Audit-only: other `__new__` overrides
  (Flyweight), `isinstance` dispatch chains (`python_isinstance_chain`),
  and file-local inheritance depth of 3+ (`python_inheritance_depth`). The
  `isinstance` advisory requires a positive `if`/`elif` chain dispatching on
  the same value across non-builtin domain types, excluding independent input
  guards and primitive/container validation.
  Each message cites the guide page and states the limit of its heuristic.
  The base `python` pack is unchanged; the guide remains a secondary source
  there (see the Deviation note in `SPEC-python-pack-expansion.md`).
- **`xcodebuild` and `swift build|test` are built-in toolchains for
  confidence scoring.** A Swift project running `xcodebuild test` through the
  Bash tool saw "tests never registered": cargo was the only built-in def, so
  nothing recognized the command and the `Executed 55 tests, with 0
  failures` result never reached the journal, leaving the subject at `low`
  under `confidence-low-blocks-commit`. Both defs parse XCTest summaries and
  Swift Testing's `Test run with N tests … passed/failed` line, treat
  `** BUILD FAILED **` and `file:line:col: error:` as build failures (an
  XCTest assertion's `file:line: error:` is a test failure, not a broken
  build), and accept `** BUILD SUCCEEDED **` / `** TEST SUCCEEDED **` /
  `Build complete!` as compile evidence when no exit code was captured.
  `phr-mcp toolchains` lists them.
- **`phr-mcp signal <compile|tests> <pass|fail>`** records a confidence
  signal explicitly for the open work unit — the escape hatch for a test
  runner with no toolchain def, or a run that happened outside the hook. It
  journals the same `outcome:*` tag the post-check hook stamps, so
  `phr-mcp confidence` and the commit gate see it identically. Requires the
  `confidence` pack.

- **Swift sources enter the code graph.** `rebuild_code_graph` now indexes
  `.swift` files (`crates/phronesis-mcp/src/graph/swift.rs`), emitting
  `file_type`, `declares_module`, `defines_fn` (methods qualified by their
  type or extension target), `defines_test`/`tested_by` for XCTest `test*`
  methods and Swift Testing `@Test` functions, and one `imports` edge from
  each file to its whole unit. Because every file in a Swift target sees the
  target's entire namespace, `tested_by` resolution now accepts a whole-unit
  import as visibility for every module in that unit. Previously Swift
  was audit-only: `syntax/swift.rs` fed the `audit-swift-*` rules, but
  `query_code_graph` for `file_type * swift` or any Swift `defines_fn`
  returned nothing, so `no_direct_test` could never vouch for Swift code.
  `GRAPH_FORMAT` bumps to 19 so existing graphs rebuild on next use.
  - Swift production functions also emit `calls(caller, callee)` edges,
    canonicalized through the unit-wide import the same way `tested_by` is
    (so `test_reachability` follows Swift calls), and
    `calls_api(function, api)` edges for a small risky-API watchlist
    (`SWIFT_WATCHLIST`: `fatalError`, `preconditionFailure`, `exit`,
    `unsafeBitCast`, the `Unsafe*Pointer` initializers/static members,
    `Thread.sleep`, and semaphore/group `wait`). The structural
    panicking-API rule can now fire for Swift.
  - `Package.swift` targets are units. Discovery parses `.target`,
    `.executableTarget`, `.testTarget` (and `.macro`) declarations —
    honouring an explicit `path:` and SwiftPM's `Sources/<Name>` /
    `Tests/<Name>` defaults otherwise — so a file under `Sources/App`
    is `swift:App::…` rather than `swift:project::Sources::App::…`, and
    every file in a `.testTarget` is `file_type test` whatever it is
    called (`UnitContext::test_target`). `import Foo` / `@testable import
    Foo` naming another target in the repository emits a whole-unit
    `imports(module, swift:Foo)` edge, so `tested_by` and `calls` from a
    test target into its production target canonicalize; imports of
    Foundation, XCTest, or any module the repository does not define emit
    nothing. Xcode `.xcodeproj` projects have no parseable manifest and
    stay on the `swift:project` fallback with the filename/directory test
    heuristic.

### Changed

- **Review hardening makes integration contracts explicit.** CI now runs the
  blocking Phronesis audit in addition to formatting, clippy, and workspace
  tests. Codex documentation and integration coverage pin its structured-JSON
  decision contract (process exit 0, with logical 0/1/2 verdicts retained in
  the action log), and a checked-in v1 bindings fixture proves reconciliation
  remains idempotent before becoming stale. JSON Schema `$ref` resolution now
  rejects repository-root escapes and absolute/drive paths while normalizing
  Windows separators consistently.

- **Rust panic/debug starter rules now use syntax-tree facts.** The existing
  unwrap, empty-message expect, `todo!`, `panic!`, `unimplemented!`, and
  `dbg!` rule IDs now consume `rust_governed_invocation` instead of source
  substrings. Formatting and macro arguments no longer evade the rules, while
  comments, strings, similarly named methods/macros, and non-empty `expect`
  messages no longer cause false positives. Hook and whole-tree audit paths
  share the same producer. The `&Box<T>` parameter rule likewise uses the
  derived `function_param_is_box_ref` fact and accepts whitespace variants
  without matching strings or comments. The audit-only environment-mutation
  rule now recognizes written `env::set_var` and `std::env::set_var` calls
  structurally; arbitrary import aliases remain outside syntax-only evidence.
  The `deny(warnings)` crate-attribute blocker now uses
  `rust_governed_attribute`, accepting whitespace variants without matching
  comments or string literals.
  `impl Deref` detection now consumes `rust_trait_impl`, recognizing qualified
  trait paths and generic implementing types without matching prose.
  The three match-arm audit rules now consume `rust_governed_match_arm`, so
  multiline/whitespace variants of empty `None`/`Err(_)` arms and
  `return Err(...)` are recognized without matching comments or strings.
  Two non-portable blockers for Phronesis-internal sync method names were
  removed from the public Rust pack; downstream Rust projects should not
  receive rules for this repository's private refactor history.
  The `*_id: u64` and `Rc<RefCell<_>>` audit rules now use parsed field/type
  evidence, including whitespace variants and excluding prose. The String-ID
  twin intentionally remains line-oriented so its `///` field exemption keeps
  working.

- **TypeScript `any` enforcement is structural-only.** The TypeScript starter
  pack retires the older `warn-any-in-src` substring rule and keeps
  `warn-ts-explicit-any-ast` as the canonical parser-backed rule. Real `any`
  annotations still warn with function/count evidence; comments and string
  literals containing `: any` no longer produce duplicate or false-positive
  warnings. This intentionally retires the legacy rule ID to avoid emitting
  two consequences for the same annotation.
  The `console.log` warning also uses the parser-backed
  `ts_console_log_call` fact, recognizing whitespace variants while ignoring
  comments, strings, other logging methods, and nested `logger.console.log`
  expressions.

- **Swift crash/legacy rules now use syntax-tree facts.** `try!`, `as!`,
  `fatalError`, mutable `static var shared`, legacy geometry constructors,
  and legacy random APIs now consume `swift_governed_construct`. Comments,
  strings, and neighboring member calls stay silent; the hook and whole-tree
  audit paths honor the fact's construct discriminator.

- **Whole-tree AST audits honor literal fact arguments.** Audit evaluation now
  applies the same constant-argument filtering as hook-time RETE matching, so
  rules sharing a structural predicate report only their intended construct.

- **The verify-before-commit nudge is command-scoped.** It now uses
  `bash_command_matches` instead of generic content matching, so only a Bash
  command containing `git commit -m` can trigger it. This remains lexical
  command recognition, not a language-AST rule.

- **Low-confidence Git gate downgraded from `block` to `warn`.** The
  `confidence-low-blocks-commit` starter rule (id unchanged) no longer exits
  2 on `git (commit|merge|rebase|cherry-pick|revert|pull)` when build/test/
  known-bug evidence is missing or failing — it now warns (exit 1), same as
  the medium band. Incomplete confidence evidence is observability, not
  enforcement: a low-confidence Git mutation now always proceeds. High
  confidence (3/3 signals) still passes clean, and unrelated `block` rules
  still exit 2. See `docs/specs/SPEC-structural-rule-migration.md`
  §"Confidence gate severity" and the migration inventory at
  `docs/specs/INVENTORY-structural-rule-migration.md`. Projects that already
  ran `phr-mcp init --packs confidence` keep the old blocking rule on disk
  until they re-run `phr-mcp init --rules-only --force --packs confidence`
  (existing project rule files are not rewritten automatically).

### Fixed

- **Whole-tree audits no longer silently suppress rules with builtin path
  guards.** `audit_codebase` and `phr-mcp audit` evaluate builtin
  `facts_contain`/`facts_count` `__script__` conditions against fresh per-file
  path and extension facts. Unsupported Rhai or binding-dependent guards now
  produce a diagnostic instead of making the affected audit rule appear
  clean. Fixes #52.

## [0.29.0] - 2026-08-16

### Changed

- **Breaking (library API): `graph::sync::SaveOutcome` gained a `diagnostics`
  field and is now `#[non_exhaustive]`.** Rebuild diagnostics record analysis a
  run did *not* perform — spec §8.2 requires the compiler provider to say that
  build scripts and procedural macros were disabled rather than let a caller
  assume the analysis was macro-complete. The struct is a return value from
  `rebuild`/`on_save`, so this affects only code that constructed it with a
  struct literal. `#[non_exhaustive]` makes future additions non-breaking.
  *Migration:* read the fields you need from the returned value; do not
  construct `SaveOutcome` yourself.

- **A `.rs`/`.json`/`.yaml` save no longer forces a full graph rebuild merely
  because `.phronesis/graph.toml` exists.** The rebuild now triggers when the
  edited file is itself *declared* as a generated artifact. Previously the
  config file's mere presence made every save a whole-repo rebuild, which would
  have made opting into ownership enrichment silently expensive. Projects using
  data contracts should know the trade: an unrelated `.rs` edit no longer
  refreshes *inferred* bindings, which are heuristics recomputed at the next
  rebuild. Explicitly declared bindings are unaffected.

### Added

- **Opt-in Rust ownership evidence (query-only).** The structural graph can
  now record where Rust source clones, filters, awaits, mutates, and acquires
  synchronous locks, plus four bounded relationships between those sites:
  `filter_before_clone`, `clone_before_await`, `read_before_mutation`, and
  `lock_scope_ends_before_await`. Every site carries a source span, an evidence
  level, and a provider, and unavailable analysis is recorded explicitly rather
  than read as a clean result. Enable per project with `[ownership.rust]` in
  `.phronesis/graph.toml`; with it absent the graph is byte-identical to
  before. Query with `phr-mcp graph ownership <function-id-or-glob>` or the
  matching MCP tool. Findings are evidence with stated limits, not verdicts:
  this ships no rule, creates no audit findings, and adds no catalogue entry.
  Graph format 17 -> 18. Design after Schott, *Visualizing Ownership and
  Borrowing in Rust Programs* (Wuerzburg, 2024); see
  `docs/OWNERSHIP-EVIDENCE.md`.

- **Release-ready multilingual structural graph.** CUE packages now use
  canonical package identities with complete import resolution; Lua, JSON,
  YAML, Helm 3, Rhai, Python, TypeScript, and Rust extractors preserve
  repository-local closure and reject ambiguous references. Query arguments
  support embedded `*` and `?` globs consistently in the CLI and MCP tool.
- **Cross-language configuration contracts.** Explicit
  `.phronesis/graph.toml` bindings and bounded inference connect CUE producers
  through tracked YAML/JSON artifacts to deserializing Rust types, including
  wire-key mappings and conservative unconsumed-key evidence.
- **Static test reachability.** Canonical `tested_by` edges record direct
  production-function evidence, while `test_reaches(test, function)` follows
  resolved calls transitively. External `#[path]` unit tests, public
  re-exports, and bounded inherent-method calls participate without guessing
  ambiguous names.
- **Fact and decision provenance.** Consequences retain asserted and derived
  fact origins, and graph-backed rule findings can link their evidence to
  architectural decision records.
- **Local-state housekeeping.** `phr-mcp state` classifies authored, cache,
  history, runtime, backup, and sensitive `.phronesis` state;
  `phr-mcp clean --cache` removes only rebuildable graph artifacts.

### Fixed

- **Graph diagnostics no longer erase valid configuration structure.** YAML
  duplicate-key detection respects sequence-item scope, indentless sequences,
  quoted scalars, and block scalars; CUE built-ins and unresolved imports no
  longer create dangling repository edges.
- **Graph evidence uses one canonical identity space.** Direct test evidence,
  generated artifacts, dynamic-language boundaries, renamed predicates, and
  graph rebuild invalidation now reconcile before derived rules run.
- **ADR graph relations are available to rules.** Decision-to-rule links and
  their lifecycle diagnostics now hydrate into RETE when a project rule names
  them, matching the relations already exposed by graph queries.
- **RETE maintenance paths expose conflicts and stable APIs.** Binding
  conflicts are handled explicitly, profiling uses the public engine surface,
  and internal network state is accessed through supported query methods.
- **Guidance drift discovers project and package instructions.** Consolidated
  `get_drift(source="claude_md")` and `phr-mcp drift --source claude_md` now
  scan root and package-level `CLAUDE.md` and `AGENTS.md` files with bounded,
  exclusion-aware traversal, deduplicate repeated imperatives, and identify
  every source file on each finding. The frozen `phr-mcp claude-md-drift`
  compatibility command remains root-`CLAUDE.md`-only.
- **Scoped audits no longer report out-of-scope debt.** `phr-mcp audit --path
  <dir>` and `audit_codebase(path)` folded in graph-rule findings from the
  whole project, so an audit scoped to one module returned violations from
  unrelated files while `files_scanned` reflected the narrow scope. Graph rules
  still evaluate against the entire graph — a covering test may live anywhere —
  but reported findings are now filtered to the requested path on segment
  boundaries.
- **`use crate::<module>;` no longer collapses onto the crate root.** A `use`
  naming a module directly had its last segment dropped as though it were an
  item, erasing the real dependency and emitting a crate-to-crate self-edge.
  Sibling-crate imports (`use phr::Rule;`) still resolve to the crate root,
  where that is the correct target.
- **Function-local `use` now produces `imports` edges.** The Rust extractor
  never walked function bodies, so a file whose source visibly imported a
  module could show no edge to it — understating fan-in and hiding import
  cycles.

## [0.26.0] - 2026-08-09

### Added

- **Rule-to-code staleness bindings.** Conservative, unqualified call-shaped
  literals in rules bind to locally defined functions in the structural graph.
  When every established referent disappears, a formerly blocking rule warns
  instead of claiming authority from stale evidence. Method calls, namespace
  calls, attributes, prose, foreign symbols, and rules with `binds: false` do
  not bind.
- **Code drift in the consolidated drift surface.** `phr-mcp drift --source
  code` and `get_drift(source="code")` report stale rule bindings alongside
  the existing prose, memory, and decision corpora.
- **MCP graph recovery.** `get_code_graph_status` reports missing, fresh,
  stale, or outdated graph state with generation, edge, file, and binding
  counts. `rebuild_code_graph` performs a server-rooted full rebuild, reconciles
  bindings, and records the generation transition in the action log.
- **GitHub release binaries.** Release automation builds and attaches
  `phr-mcp` archives for Linux x86-64, macOS Apple Silicon, and Windows
  x86-64 after release-plz creates the matching package release.

### Changed

- **The complete language-agnostic platform is now the default.** `init` and
  every non-`none` pack selection include `llm`, `confidence`, `journey`,
  `structural`, and `context`; language packs remain additive. `none` is an
  explicit, mutually exclusive escape hatch.
- **Smaller durable context is the default.** Fresh projects receive the
  bounded kernel/context scaffolding and graph state without a separate pack
  opt-in.
- **MCP collection results use stable object envelopes.** `list_rules`,
  `list_facts`, `get_agenda`, `get_consequences`, and `get_action_log` now
  return named JSON objects through both `structuredContent` and compatibility
  text, avoiding SDK failures on top-level arrays.
- **Documentation is web-native and current.** The GitHub Pages navigation no
  longer sends readers into Markdown sources; the loop guide and changelog now
  have dedicated HTML pages. The README, explainer, and catalogue use a new
  mineral teal/indigo visual system with progressive SVG diagrams for the
  governance boundary, default subsystems, structural code graph, RETE
  internals, and concrete rule behavior. The docs describe the unified
  `drift`, `stats`, default-platform, graph lifecycle, and MCP envelope
  surfaces.

### Safety

- Missing, malformed, stale-graph, or generation-mismatched binding evidence
  never demotes a block. Direct rule-file edits and MCP rule mutations
  reconcile bindings without advancing the graph generation.

## [0.25.0] - 2026-08-01

### Changed

- **`init::Pack` is `#[non_exhaustive]`.** Adding a pack is this crate's most
  routine extension point — four landed in recent releases (`Confidence`,
  `Journey`, `Structural`, `Context`) and more languages are expected — but
  each one was a semver break twice over: a variant added to an exhaustive
  enum, plus a discriminant shift for every variant after the insertion point.
  `cargo-semver-checks` flagged both on the `Context` addition. Marking the
  enum non-exhaustive makes future packs a non-event.

  **Migration:** only affects code outside this workspace that matches on
  `Pack`. Add a `_ => ...` arm. Nothing inside the workspace changes, since
  `#[non_exhaustive]` does not constrain the defining crate.

### Added

- **Token-aware durable context (opt-in).** A project that creates
  `.phronesis/context.json` gets deterministic, budgeted context packing
  instead of unconditional full-file reinjection. Every renderable unit is an
  indivisible item measured with its headings and separators, admitted only
  if it fits its kind ceiling, the shared byte capacity, and — when
  configured — a soft estimated-token budget (`ceil(bytes / 3)`). Current
  enforcement activity gets first claim on the payload, then the kernel, then
  situational nudges, then activity that overflowed its reserve.
- **`.phronesis/kernel.md`, the always-on core.** Written by
  `init --packs context`. `durable.md` keeps its meaning as the session-level
  project document and is never rewritten, repurposed, or shrunk.
- **Situational nudge capsules.** `.phronesis/nudges/*.md` carry strict JSON
  frontmatter and a static Markdown body, selected by positive facts through
  the ordinary RETE engine. Bodies never interpolate fact arguments, so a
  filename or tool payload cannot become a second-order prompt-injection
  channel. Only four allowlisted predicates may trigger a capsule; adding one
  is a reviewed code change, not configuration.
- **`phr-mcp context inspect | predicates | stats`.** `inspect` is a true dry
  run: it writes no observation, so reading the diagnostic cannot contaminate
  the data it reports. It lists candidates, costs, ceilings, per-item omission
  reasons (`kind_ceiling`, `byte_capacity`, `token_capacity`,
  `displaced_by_nudge`), capsule load failures, and fact-hydration failures.
  Human and `--json` output are projections of one value.
- **`--packs base`.** Expands to every language-agnostic pack —
  `llm,confidence,journey,structural,context` — so the usual shape is
  `base,<your language>`. Language packs are deliberately excluded: several
  match raw substrings gated only by path, so composing them produces
  cross-language false positives (the TypeScript `: any` rule fires on Rust's
  `: anyhow::Error`).
- **Context observations.** Bounded `kind: "context"` records in the existing
  rotated log carry bytes, estimated tokens, per-kind omission counts, capsule
  ids, latency, and a raw-truncation flag — no bodies, fact arguments, or user
  content, and no claim about whether the model read or followed anything.

### Changed

- **Context packing splits Markdown on `##` sections, not blank lines.** The
  section — heading, lead-in, and the list under it — is the indivisible unit.
  Blank-line paragraphs let an over-budget list be dropped while its lead-in
  survived, producing text that promised content it did not deliver ("Three
  heuristic tools ...:" followed by nothing). A section is now delivered whole
  or not at all.
- **Context source files are capped at 64 KiB.** These are read on every hook
  invocation, so an accidentally huge file is ignored with a diagnostic rather
  than read and discarded each turn. This is the one place opt-out behavior
  differs from before: a `durable.md` over 64 KiB now yields no context where
  it previously yielded a 4 KiB truncation.

### Fixed

- **cargo-nextest runs grounded no test signal.** The cargo toolchain def
  accepted `cargo nextest`, but its only `test_summary` pattern was libtest's
  `test result:` line, which nextest never emits. The def claimed the command,
  parsed nothing, and recorded no tests signal — so a project gated on
  `cargo nextest run` stayed in the low confidence band and had every commit
  blocked regardless of how green the suite was. `test_summary` now accepts
  one pattern or several (bare string or array, so existing
  `toolchains.json` defs are unaffected); the first pattern that matches wins
  and all of its matches are summed.
- **`.phronesis/nudges/README.md` was parsed as a capsule.** The file `init`
  itself writes produced a load diagnostic on every hook invocation.
- **A limited `read_recent` parsed the entire action log.** Every hook asks for
  a handful of recent entries, and the read parsed every line in the log plus
  its rotated predecessor to return the last few — so the cost grew without
  bound as a project accumulated history. A 3.8 MB log cost 16 ms per hook, and
  a real project measured 30 ms. The read now scans backward from the newest
  record and stops once it has enough *matching* entries (counting matches, not
  lines, so a filter excluding the newest records still reaches back far
  enough), and only consults the rotated file when the current one cannot
  satisfy the limit. This affects every caller, including the legacy context
  path, `stats`, and `trend`.
- **Activity bullets rendered a dangling `in` with no path.** Rules that fire
  on a shell command rather than a file edit log an empty `file`, which
  formatted as `- WARNED 36m ago: some-rule in ` — malformed text in the prompt
  the model reads. The location clause is now omitted when there is no path.
  This changes the legacy renderer's output for command-rule entries.

### Compatibility

- Without `.phronesis/context.json` the session and interaction payloads are
  byte-identical to previous behavior, capsules are not scanned, and no
  context observations are written. Pinned by test. Two deliberate exceptions,
  both listed above: a `durable.md` over 64 KiB is now ignored rather than
  truncated, and command-rule activity bullets no longer carry a dangling
  `in` clause.
- Re-running `init` never overwrites an existing `context.json`, `kernel.md`,
  `durable.md`, or nudges `README.md`.
- `session.charter_max_bytes` is defaulted, so configuration written before
  the charter existed keeps loading.

### Measurements

Measured on a `base,rust` fixture carrying this repository's own 3,292-byte
durable file and five blocked edits, comparing legacy against opted-in:

- interaction payload 3,602 → 680 bytes (81.1% reduction), 1,201 → 227
  estimated tokens;
- session charter 4,062 bytes and truncated mid-rule-list → 2,664 bytes and
  intact;
- all five current blocking items retained in both;
- zero raw truncations across 22 payloads;
- p95 context construction 3.0 ms with no journey/outcomes hydration, on a
  fixture whose action log held a handful of records.

That latency figure describes a fresh project and is not representative on its
own. Context construction reads recent hook decisions, and before the
`read_recent` fix below that read parsed the entire log: a real project with a
3.8 MB log measured 16 ms, and one in the field measured 30 ms. Both are over
the specification's 5 ms target. The fix removes the dependence on log size;
the figures above should be read together with it.

### Limitations

- The specification's measurement gate also asks for a second external corpus
  and a false-relevance review of capsule matches. The corpus measurement has
  not been run; the capsule review is vacuous because no capsules ship by
  default. `context` is therefore opt-in via `--packs`, and is not yet part of
  the default pack set.
- Graph hydration is excluded from capsule selection. The session charter
  reports graph freshness as a state line only.
- `per_test` extraction remains libtest-only, so the known-bug registry does
  not see cargo-nextest per-test results.

## [0.24.0] - 2026-07-31

### Added

- **TypeScript structural code graphs.** The graph now discovers npm package
  units and extracts `.ts`, `.tsx`, `.mts`, and `.cts` modules, functions,
  imports, direct test-call coverage, and non-null assertions. Resolution
  supports relative specifiers, `index` modules, `tsconfig.json` `baseUrl`,
  and `paths` aliases while excluding `node_modules` unconditionally.
- **TypeScript structural warnings.** `warn-import-cycle` applies to resolved
  TypeScript module cycles, and `warn-untested-risky-call` uses the narrow `!`
  watchlist to report untested unchecked type assumptions. Both remain
  advisory.

### Limitations

- TypeScript project references, JavaScript extraction, and monorepo
  cross-unit resolution are not included. Unresolved relative and cross-unit
  imports are counted as skipped evidence rather than silently treated as a
  clean graph.
- Real-corpus validation against tough-cookie measured 1,221 base edges, 54
  derived edges, 105 resolved imports, and zero skipped items across 47 files.

## [0.23.1] - 2026-07-30

### Fixed
- **`query_code_graph` advertised "Rust only" and a stale identity form.**
  The MCP tool description is what a model reads to decide whether a tool
  applies, so both claims changed behavior rather than merely being out of
  date. Python graphs build and query correctly — `init --packs structural`
  in a Python project produces a graph and `graph query defines_fn` returns
  `python:<dist>::<pkg>::<mod>::<fn>` — but the description reported the
  language unsupported. Its worked example also still used the pre-0.23
  `crate::wme` identity form, so a caller following it queried a key nothing
  holds, received zero results, and could reasonably read that as an empty
  graph rather than a malformed query. The description now names both
  languages, gives the current identity form with an example per language,
  and marks `calls_api` as the Rust-only relation it is.

## [0.23.0] - 2026-07-30

### Added
- **Structural code-graph facts** (`structural` pack, alias `graph`). A
  durable, gitignored graph of architectural relations at
  `.phronesis/graph.jsonl`, extracted by the `PostToolUse` sensor and
  hydrated into the RETE network at `PreToolUse`. Ships two `warn` rules:
  `warn-untested-risky-call` (a production function calling a panicking API
  with no direct test) and `warn-import-cycle` (a module in an import
  cycle). Both join `edited_file`, so they report the file in front of you
  rather than the whole repository on every edit. See
  `docs/specs/SPEC-triple-store-rete.md`.
- **Rust and Python extractors.** Entity identity is
  `<lang>:<package>[#<target>]::<module path>`. Rust resolves Cargo packages,
  compilation targets, and dependency aliases including
  `[workspace.dependencies]` inheritance; Python resolves distributions from
  `pyproject.toml` (PEP 621 and Poetry), both `src/` and flat layouts, and
  imports across sibling distributions in one repository.
- **Graph CLI and MCP surface.** `phr-mcp graph rebuild`, `graph status`, and
  `graph query`, plus the `query_code_graph` MCP tool.
- **`event.file_rel` for Rhai predicate providers** — the edited path in the
  repo-relative form the graph keys files by, so provider-emitted facts can
  join graph facts on a path. `event.file_path` remains the host's absolute
  path.

### Changed
- **Graph identity carries an explicit format stamp.** `.phronesis/graph.index`
  records the identity scheme it was built under. A graph built by an older
  version is reported as outdated rather than fresh, and the next save
  rebuilds it — content hashes cannot detect an identity change, because the
  files themselves do not change.

### Fixed
- **The `PostToolUse` graph sensor never ran through a real hook.** A
  traversal guard rejected absolute paths, which is the only form hosts send;
  the sensor was additionally gated behind post-phase rules, which a
  pre-phase-only pack never has. Both are fixed, and `repo_relative` now
  resolves symlinked project roots (`/var` vs `/private/var` on macOS).
- **A parse failure destroyed the file's graph evidence and reported
  success.** An empty extraction was indistinguishable from "this file
  defines nothing", so a malformed mid-edit save erased every function, call,
  and import the file had and recorded its hash as successfully indexed.
  Parse failure now preserves prior evidence and leaves the file reported
  stale.
- **`#[cfg(not(test))]` and `#[cfg(feature = "test-utils")]` were classified
  as test attributes**, dropping production functions from `defines_fn` and
  turning their calls into coverage edges. `cfg` predicates are now parsed
  rather than token-scanned.
- **A deleted file kept its edges and its index entry**, leaving the graph
  permanently reporting drift.
- **The Codex adapter was not wired for the code graph** — no sensor, no
  hydration, and no agenda update before firing, so every purely
  RETE-derived verdict was computed and discarded.
- **The MCP `audit_codebase` tool omitted the graph merge** the CLI performs,
  reporting zero structural debt regardless of the graph's contents and
  writing that zero into the debt trend.
- **The sensor built a graph in every project, opted in or not.** Moving it
  ahead of rule loading (necessary, since the structural pack ships `phase:
  "pre"` rules exclusively) removed an accidental gate without adding a
  deliberate one, so a project on `--packs llm` gained an unasked-for
  `.phronesis/graph.jsonl` and a per-save extraction pass. The graph's own
  presence is now the opt-in signal.

### Known limits
- `calls_api` is deliberately empty for Python: there is no defensible
  equivalent of Rust's closed panic watchlist. Python projects therefore fire
  `warn-import-cycle` only.
- `tested_by` matches by bare short name and over-approximates coverage, so
  `untested` under-approximates. That direction is chosen — a missed warning
  is recoverable, a false "untested" verdict is not.
- Both rules `warn`. Promotion to `block` awaits a second measured corpus.

### Migration
- The graph and its index are derived, gitignored state; nothing needs to be
  committed or hand-edited. An existing graph is detected as outdated and
  rebuilt automatically on the next save, or on demand with
  `phr-mcp graph rebuild`.

## [0.22.0] - 2026-07-25

### Added
- **Codex lifecycle integration.** `phr-mcp codex-hook` now implements the
  current `PreToolUse`, `PostToolUse`, session, prompt, compaction, and
  subagent contracts for Bash and `apply_patch`; `phr-mcp init` safely merges
  project hooks and project-scoped stdio MCP registration without bypassing
  Codex's `/hooks` trust review.
- **Extensible predicates.** Project-owned Rhai providers under
  `.phronesis/predicates/*.rhai` can derive new RETE facts from normalized hook
  events. MCP tools add, inspect, test, list, and remove providers so agents can
  evolve the rule vocabulary alongside rules. Multi-file operations expose a
  once-per-operation `event.files` batch context before per-file evaluation;
  the repository includes a dogfood `change_set.rhai` classifier.

### Changed
- **Interaction context terminology.** The per-prompt context command is now
  `interaction-context`; `turn-context` remains a compatible CLI alias and the
  old Rust helpers remain deprecated wrappers. The unrelated markdown
  `set_section_context` MCP workflow is unchanged.

### Fixed
- **Contract-grounded hook behavior.** Current snake-case payload fields and
  PascalCase event names are decoded, pre-action violations return a real deny
  decision, post-action feedback remains advisory, patch paths are validated,
  and executed calls retain action-log, journey, and grounded outcome data.
- **Honest Codex fixtures.** Schema-authored fixtures are labeled `authored`
  and use the payload-corpus envelope instead of claiming unverified runtime
  capture provenance.

## [0.20.0] - 2026-07-13

### Changed
- **Context-struct API migration.** Four call surfaces now group related
  arguments into explicit input types: `Consequence::from_rule_firing` takes
  `RuleFiringContext`; `journey::derive::assert_facts` takes `DeriveInput`;
  `outcomes::extract` takes `ExtractInput`; and
  `outcomes::adapter::extract_from` takes `ExtractFromInput`. These are
  breaking Rust API changes; construct the corresponding context/input struct
  and pass it in place of the former positional arguments.
- **Audit precision and maintainability.** Rust parameter-count auditing now
  evaluates each function independently instead of combining same-named
  methods, and high-complexity paths across the engine, hook, journey, and
  outcome layers use focused helpers and input types.

### Fixed
- **Honest payload scrubbing.** Scrubber construction rejects empty, relative,
  and filesystem-root scrub roots. Scrubbing recognizes colon-delimited bearer
  credentials and credential URLs with an empty username, detects residual
  sensitive material, and aborts before backup or overwrite when safety cannot
  be established.
- **Evidence integrity.** Residual-risk failures are surfaced rather than
  silently treated as successful anonymization, scrub integration tests are
  top-level tests that are actually discovered, and the checked-in toolchain
  definitions are regression-tested against the generated scaffold.

### Migration

```rust
let consequence = Consequence::from_rule_firing(
    RuleFiringContext {
        rule_id,
        predicate,
        bound_facts,
        kind,
    },
    &payload,
)?;

journey::derive::assert_facts(&mut network, DeriveInput {
    project_root,
    rules: &rules,
    config: &config,
    scope: WindowScope {
        current_sid: session_id,
        now_ts: now,
    },
}).await?;

let facts = outcomes::extract(outcomes::adapter::ExtractInput {
    root,
    subject,
    command,
    output,
    command_exit,
});

let (tags, subject) = outcomes::adapter::extract_from(ExtractFromInput {
    project_root,
    tool_name,
    command,
    output,
    command_exit,
});
```

## [0.19.0] - 2026-07-12

### Added
- **`PHRONESIS_CAPTURE_DIR`** — when set, pre-check/post-check tee the
  raw stdin payload to `<dir>/payloads.jsonl` before parsing (flock'd
  for concurrent-hook safety, best-effort and off by default). This is
  how the payload-contract corpus below gets refreshed against a real
  CLI's current payload shape.
- **`payload_scrub` module + `phr-mcp scrub-payload <path> [--write] [--project-root DIR]`** —
  anonymizes captured payloads for committing as fixtures. Operates on
  JSONL in/out; `--write` backs the original up to `<path>.bak` before
  overwriting; `--project-root` defaults to the current working
  directory so in-project paths survive scrubbing while `$HOME`,
  username, `session_id`, `transcript_path`, and other out-of-project
  paths are rewritten to deterministic, indexed placeholders. A
  residual leak or unrecognized shape aborts the run before anything
  is written.
- **Payload-contract corpus.** Fixture payloads for Claude Code and
  Gemini CLI hook events under
  `crates/phronesis-mcp/tests/fixtures/payloads/`, each tagged
  `provenance: "authored"` — hand-written approximations of the real
  envelopes pending supersession by live captures via the tee above.
  A contract runner replays every fixture through the real binary and
  asserts rule liveness (a hook or tagger that silently no-ops now
  fails CI) and journey-journal outcome tags. A companion hook-event
  registry test suite pins `init`'s hook wiring to event names that
  actually exist on each host CLI, including a regression pin on the
  0.17.1 `BeforeModelRequest` incident.

## [0.18.0] - 2026-07-11

### Added
- **Neutral toolchain outcomes.** Build/test grading is no longer
  cargo-specific: declarative `ToolchainDef`s (built-ins ∪ project
  `.phronesis/toolchains.json`, project ids overriding built-ins)
  drive outcome detection via named-capture regexes, with the command
  exit code as the authoritative build signal and regex refinement
  layered on top. `phr-mcp init` scaffolds example pytest/tsc defs;
  `phr-mcp toolchains [--json]` lists the effective registry.
- **`command_exit` capture.** PostToolUse payloads from shell tools
  (`Bash`, `run_shell_command`) are probed for a numeric exit code
  (`exit_code`/`exitCode`/`returncode`/`code`/`status`, then a
  trailing `exit code: N` text fallback), journaled as
  `command_exit`, and used to grade outcomes — a non-zero exit with a
  test summary grades build-pass/test-fail (the pytest exit-1 case).
- **Journal compaction.** `.phronesis/journey/events.jsonl` is
  bounded (16 MiB default, `PHRONESIS_MAX_JOURNAL_BYTES` override,
  1 GiB ceiling): compaction retains the 10k-record tail plus the
  latest `outcome:*` record per subject, atomically via temp+rename
  with fd/inode revalidation so concurrent appends never land in a
  stale file.

### Removed
- `CargoAdapter` — cargo grading now flows through the same
  toolchain-def registry as every other toolchain.

## [0.17.1] - 2026-07-06

### Fixed
- **Gemini CLI turn-context hook never fired.** `init` wired the
  turn-context hook into `.gemini/settings.json` under
  `BeforeModelRequest`, which is not a Gemini CLI hook event — Gemini
  silently ignored it, so per-turn context injection (recent hook
  decisions + durable directives) never ran in Gemini sessions. Now
  wired under `BeforeAgent`, the per-prompt analogue of Claude Code's
  `UserPromptSubmit`. Re-running `init` (or `init --hooks-only`) also
  removes the dead legacy `BeforeModelRequest` key from existing
  settings. The emitted `hookEventName` stays `"UserPromptSubmit"` for
  both CLIs: Claude Code validates the field, Gemini reads only
  `additionalContext` and ignores the echo.

## [0.17.0] - 2026-07-04

phr-mcp, phr, and phronesis-rhai all release as **0.17.0** — the workspace
adopts lockstep versioning (`[workspace.package] version`); from this
release one number covers all three crates. (Previous: phr-mcp 0.16.2,
phr 0.14.0, phronesis-rhai 0.1.0; the jumps are version-line unification,
not breaking changes.)

### Changed
- **MCP-crate decomposition.** `hook.rs` (1764 LOC) and `syntax/rust.rs`
  (1622 LOC) split into focused submodules; `main`, `audit::run`/`run_profiled`
  (deduped via a shared core), and ~30 further functions decomposed below the
  let-count audit thresholds. Audit debt drops 59 → 8 hits; the remaining 8
  are core-engine functions deferred to the embedded-consumer-gated engine
  spec. Behavior-preserving; no public API changes. Implements
  `docs/superpowers/specs/2026-06-28-mcp-crate-decomposition-design.md`.

## [0.16.2] - 2026-07-03

### Added
- **`phr-mcp migrate-extracted-rules <path> [--dry-run]`** — the salvage
  command deferred from 0.14.0. Rewrites pre-0.14.0 `extract_rules` output
  in place (with a `.bak` backup): strips the bracketed extraction-time
  prefixes (`[pattern]`, `[anti_pattern]`, `[context]`, `[problem]`,
  `[directive]`) from messages, demotes `block` actions to `warn`, and
  demotes to `log` any extracted rule duplicating a structural Rust-pack
  rule (the SPEC's static keyword table: unwrap, clone, Deref, &String,
  &Vec, thiserror). Extracted rules are detected by their `markdown_rule`
  condition, so hand-written rules are never touched. Idempotent.
  Implements the salvage path in `docs/specs/SPEC-extract-rules-defaults.md`.

## [0.16.1] - 2026-07-03

### Added
- **Named function detail on AST-predicate audit hits.** The audit
  table/JSON previously rendered whole-function hits (let-binding
  counts, etc.) as `lines: 1, 1, 1` — the placeholder line number.
  `FileAudit` gains a `details` field parallel to `lines`, rendered as
  `audit.rs — run (26 let bindings), run_profiled (32 let bindings)`
  in both output formats.

### Changed
- **Rust pack: audit let-rules scope to `src/`.**
  `audit-rust-let-binding-count-high` / `-let-mut-count-high` gain a
  `file_path_matches: "src"` gate so examples, benches, and tests are
  no longer flagged for let-count debt.
- **Rust pack: `audit-newtype-id-string` honors doc exceptions.** The
  rule gains `doc_excepted: true`, so a `///` field doc marks an
  intentional string ID as an accepted exception.

### Security
- **Migrate `serde_yml` → `serde_norway`.** `serde_yml 0.0.12` and its
  `libyml` backend are archived and flagged unsound
  (RUSTSEC-2025-0068 / RUSTSEC-2025-0067) with no fix coming.
  `serde_norway` is the RustSec-recommended maintained `serde_yaml`
  fork with the same API; the only call site (wiki frontmatter
  parsing) changes crate path only. Removes `libyml` from the
  dependency tree entirely.

### Fixed
- **Wiki frontmatter closing fence must be exactly `---` on its own
  line.** The parser previously accepted any line beginning with three
  dashes (`----`, `--- see appendix`) as the closing fence, silently
  truncating the YAML and leaking the line's tail into the body. The
  fence search now skips lookalikes; a page with no true fence reports
  "missing closing `---` fence" instead of parsing corrupted content.
  Pinned by five new parser tests (lookalike lines, fence at EOF,
  CRLF endings).

## [0.16.0] - 2026-07-03

phr-mcp 0.16.0; phr library bumps to 0.14.0 (engine changes this round —
new scripting trait, a removed method, and a new feature gate); new
`phronesis-rhai` 0.1.0.

Three changes that tighten the engine/embedding-host boundary ahead of a
1.0 line: an expressive scripting layer, removal of the last
consumer-specific engine API, and a feature gate that makes the default
public surface equal what the bundled MCP consumes.

### Added
- **`phronesis-rhai` crate + `ScriptEval` trait.** The core
  `__script__` evaluator now lives behind a `ScriptEval` trait
  (`ReteNetwork::with_script_evaluator`). The new `phronesis-rhai` crate
  provides `RhaiScriptEvaluator`, a sandboxed Rhai implementation
  (`Engine::new_raw` + StandardPackage, operation/call-depth/string/array/map
  caps, `sync`) supporting numeric comparisons and boolean combinators over
  fact arguments — the guard expressions the builtin two-primitive DSL
  can't express. Scripts see `facts` (array of `#{predicate, args}`) and
  `bindings` (map) and must return `bool`; errors/non-bool are treated as a
  blocked guard. `CompositeScriptEvaluator` routes builtin-DSL forms
  (`facts_contain`/`facts_count`) to the builtin evaluator and everything
  else to Rhai, so bundled packs and Rhai guards coexist in one rules.json.
  Wired into `phronesis-mcp` behind an off-by-default `rhai` feature (server
  + pre/post hooks via a `net::build_network` seam). Implements
  `docs/superpowers/specs/2026-06-01-rhai-script-evaluator-design.md`.
- **`embedding-host` cargo feature on `phronesis`** (off by default). Gates
  the ~10 public `ReteNetwork` methods only an external embedding host needs
  (`restore_persistent_facts*`, `execute_next_agenda_item`,
  `fact_ids_matching`, `fact_count`, `facts_matching_predicate`,
  `get_rules_count`, `get_wmes_by_condition`, and the instrumentation
  getters). The default surface equals what the bundled MCP consumes, so the
  compiler enforces the symmetry. CI exercises the feature config. Implements
  `docs/superpowers/specs/2026-06-13-embedding-host-feature-gate-design.md`.

### Changed
- **`ScriptEvaluator` renamed to `BuiltinScriptEvaluator`** (implements
  `ScriptEval`). `ScriptEvaluator` remains as a backwards-compatible alias
  and the inherent `evaluate` still returns `ReteError`, so existing callers
  are unaffected. The misleading "Rhai" docstrings in core (the builtin is a
  hand-rolled DSL, not Rhai) are corrected.

### Removed
- **`ReteNetwork::get_persistent_facts` and its hardcoded
  `PERSISTENT_PREDICATES`** — a downstream consumer's game-state vocabulary
  baked into a "domain-neutral" engine, deprecated since 0.11 and now that
  the consumer has migrated onto `facts_matching_predicates`, deleted. The
  remaining consumer-flavored doc/example vocabulary in the engine and MCP
  fixtures is neutralized. `restore_persistent_facts*` stay (generic
  bulk-assert; now behind `embedding-host`). Implements
  `docs/superpowers/specs/2026-06-13-domain-neutral-persistent-facts-design.md`.

## [0.15.0] - 2026-06-24

### Added
- **Loop-based agent programming guide**
  (`docs/loop-programming-guide.md`) — writing recurring /loop-driven
  agent workflows against phronesis, with captures from live sessions
  in this repo.
- **`journey_derive` scaling bench** plus an ADR recording the
  scaling behavior of journey fact derivation.

### Fixed
- **Journey rules with undefined selectors fail closed.** A rule
  referencing a tag absent from `.phronesis/journey.json` was
  fail-open: a stderr warning, then the rule loaded anyway — and for
  absence-style rules (`== 0`) the missing tagger looked like zero
  occurrences, so the rule fired on every call. Configuration errors
  (`BadWindow`, `UndefinedSelector`) now propagate — the hook exits 2
  (pre-check) / 1 (post-check) naming the offending rule id and
  missing selector — while transient journal I/O errors stay
  fail-open. See the decision page
  `2026-06-23-undefined-selector-rejection.md`.

## [0.14.0] - 2026-06-21

Dogfooding-driven polish. The 0.13.x patch line was driven by playtest bugs
visible only after install; 0.14.0 closes the four next-deepest friction
points the same playtests surfaced. Compiled under
`docs/specs/SPEC-0.14.0-dogfooding-polish.md`.

### Added
- **`journey_filtered_since_ge(target, counted, k)` aggregator** — the
  existing `journey_since_ge` counts distance over every record; a long
  Bash session could trip "8+ tool calls since build" with no writes.
  The new aggregator emits a k-ladder up to the count of `counted`
  records appearing after the most recent `target` record. Rules can now
  express "8 writes since last build" directly:
  `facts_count('journey_filtered_since_ge', ['build','write','8']) >= 1`
  with a `write` tagger keying on
  `change_type=edit|write|multiedit|replace|write_file`. The existing
  five aggregators are unchanged. See
  `docs/specs/SPEC-journey-filtered-since.md`.
- **`confidence_enabled` zero-arg marker fact** — asserted at every hook
  fire when `.phronesis/confidence.json` exists, mirroring the
  `clock_facts.rs::business_hours_local` pattern. Lets rules condition
  on opt-in state via the existing `facts_count('confidence_enabled',
  []) == 0` absence form. Generalizable: future packs can ship
  `journey_enabled`, `wiki_present`, etc. using the same shape. See
  `docs/specs/SPEC-pack-opt-in-facts.md`.

### Changed
- **Confidence gate broadens to all commit-producing porcelain commands.**
  `confidence-low-blocks-commit` and `confidence-medium-warns-commit` now
  match `bash_command_matches: "git
  (commit|merge|rebase|cherry-pick|revert|pull)"` instead of the literal
  `"git commit"`. Closes the gate-bypass-by-merge hole surfaced during
  the journey-facts merge night (5 of 6 commit-producing commands
  silently bypassed the gate). See `docs/specs/SPEC-gate-merge-commits.md`.
- **`nudge-verify-before-commit` self-deactivates when confidence is on.**
  The rule gained a second `when` clause:
  `{ "__script__": "facts_count('confidence_enabled', []) == 0" }`. The
  confidence gate enforces the same call-chain-tracing discipline by
  counting `signal_pass` facts; the nudge was redundant in that mode and
  was double-warning on every `git commit`. Projects without confidence
  are unaffected.
- **`extract_rules` defaults action `warn`, not `block`.** A live
  invocation added 27 block-action rules to a project rules.json
  overnight; with any section context set, every pre-check fired 6
  simultaneous `constraint_violation`s and exited 2 on every tool call.
  Block is reserved for known-bad code shapes; pattern reminders are
  advisory. See `docs/specs/SPEC-extract-rules-defaults.md`.
- **`extract_rules` strips the bracketed metadata prefix** (`[pattern]`,
  `[anti_pattern]`, `[context]`, `[problem]`) from the user-facing
  message. Those were extraction-time discriminators leaking into prose.

### Migration
- Projects that ran `phr-mcp init --packs confidence` before 0.14.0
  carry the narrow gate pattern in `.phronesis/rules.json`. Either re-run
  `phr-mcp init --rules-only --force --packs confidence` (rewrites the
  rule pack with the broadened pattern, backs up to `.bak`) or
  hand-edit the two `bash_command_matches` clauses.
- Projects with the old `nudge-verify-before-commit` rule should add the
  second `when` clause to opt into the supersession. Same
  `--rules-only --force` flow works.
- Projects that already invoked `extract_rules` and want to salvage
  their extracted rules can apply the in-tree recipe in
  `docs/specs/SPEC-extract-rules-defaults.md` §"Salvage path." A
  `phr-mcp migrate-extracted-rules` command is deferred to a follow-up
  PATCH.

### Deferred (intentional, with specs on disk for future work)
- **`extract_rules`**: per-pattern marker conditions (Problem 3b),
  structural-rule skip-list (Problem 4a), and the
  `migrate-extracted-rules` command. The umbrella spec scopes 0.14.0 to
  the action/prefix defaults; the rest rides a follow-up PATCH.
- **Subject inheritance across merge commits.** Real design surface; the
  `SPEC-gate-merge-commits` open question flags it.
- **Repo-lifetime journey windows (`r`)** — still phase 2 of
  `SPEC-journey-facts`.

### Notes
- **Coverage.** Workspace lines at 86.20%+ across the four implementations,
  up from the post-0.13.x baseline of 85.94%.
- **`phr` library version unchanged at 0.13.3.** The engine wasn't
  touched in 0.14.0; only `phr-mcp` bumps. `phr-mcp`'s `phr` dep stays
  pinned at `0.13.3`.

## [0.13.2] - 2026-06-20

### Fixed
- **`bash_command_matches` taggers actually fire.** `journey::tagger::tagger_facts`
  built only file/content facts and relied on a "tagger regex pass" implied by a
  misleading comment but never implemented. The default `build` tagger
  (`{ "bash_command_matches": "cargo (build|check|test)" }`) silently no-fired on
  every `cargo` invocation. `tagger_facts` now walks `taggers[*].when[*]`
  (including nested `or` clauses) collecting `bash_command_matches` patterns,
  regex-matches each against the bash command, and asserts one synthetic
  `bash_command_matches:<pattern>` Fact per match — the same pattern
  `check_bash_command_patterns` uses for top-level rules
  (`hook_facts.rs:316`). Surfaced in a live playtest, not in unit tests.
- **`HookPayload.tool_output` accepts `tool_response` as a serde alias.** Claude
  Code's PostToolUse hook delivers Bash output under `tool_response`, not
  `tool_output`. Without the alias, the field was `None` / empty string, so
  `compiled("")` returned true (no error patterns match → spurious
  `outcome:compile_ok`) and `TEST_RESULT.captures_iter("")` returned nothing
  (`outcome:test_pass` never fired). Net effect: confidence-scoring was wedged at
  "low / compile" for every real `cargo` run, even when tests were green —
  the whole gate-by-band feature was non-functional in production. Tests and
  fixtures all passed because they synthesized payloads under `tool_output`;
  only a live hook payload surfaced it. Backward compatible with Gemini and
  existing fixtures.

## [0.13.1] - 2026-06-20

### Fixed
- **Same-day sid fallback collision.** When `.phronesis/journey/session` was
  missing, the journey fallback was the literal placeholder
  `s-YYYY-MM-DD-fallback`, collapsing distinct sessions to the same id. Now
  `journey::current_sid` reads-or-creates atomically in the
  `context::ensure_session_id` format (`s-YYYY-MM-DD-<6 hex>`); the placeholder
  is gone.
- **Triple-duplicated `current_sid` consolidated.** Three independent
  implementations (in `hook`, `main`, and `server::get_journey`) coalesced into
  a single `journey::current_sid(project_root)` helper. Same semantics, one
  source of truth.

### Changed
- **CLAUDE.md packs list now includes `confidence`** alongside `journey`. The
  scaffolded CLAUDE.md previously enumerated `journey` only.
- **`phr-mcp journey` nudges on empty config.** When `.phronesis/journey.json`
  is missing or empty, the CLI emits a stderr suggestion
  ("run `phr-mcp init --packs journey` to scaffold one") before falling back
  to an empty config. The hook stays silent — fail-open is advisory there, not
  user-facing.

### Fixed (engine)
- **Pure-script rules now fire.** Rules whose `when` was entirely `__script__`
  clauses had no alpha state, no terminal id, no p-state — they never reached
  the agenda, because `__script__` clauses are post-filters on activations and
  with no other clause there were no activations to filter. `update_agenda`
  now branches on `real_condition_count == 0` (count of non-`__script__`
  conditions per loaded rule) and, for pure-script rules, evaluates the script
  clauses against the current fact base with empty bindings, emitting an
  activation when every clause passes. Dedupe key is `<rule_id>` —
  fire-once-ever, the right semantics for threshold rules. Alpha/beta network
  and the production network shape are unmodified; mixed-script behaviour is
  unchanged. Surfaced by the journey-facts SPEC's headline
  `auth-churn-without-tests` rule, which is naturally two `__script__` clauses
  (`facts_count(...) >= 5` AND `facts_count(...) == 0`). The
  `journey_seen` anchor leaf added as a workaround is no longer required.

## [0.13.0] - 2026-06-20

### Added
- **Journey facts (new fact family + new hook stage)** — call-window and
  session-scale predicates that summarise *trajectory*, not the current diff.
  Five aggregators over project-defined tags (`journey_occurrence`,
  `journey_count`, `journey_seen`, `journey_since_ge`, `journey_distinct`)
  with windowed selectors (`5c` for last 5 calls, `30m`/`2h`/`7d` wall-clock,
  `s` for session; repo-lifetime `r` is phase 2). Rule-driven derivation;
  the journal is the substrate, the predicates are recomputed each cycle.
  See `docs/specs/SPEC-journey-facts.md`.
  - **Append-only journal** (`.phronesis/journey/events.jsonl`) writes a
    record per post-check with subject + tags + monotonic seq. Tail-read
    for hot queries (`SUFFIX_HARD_CAP = 10_000` lines) and per-subject read
    for outcomes folding.
  - **Taggers reuse the predicate engine** — `taggers[*].when` clauses are the
    same DSL as rule conditions. `bash_command_matches`, `new_content_contains`,
    `file_path_matches` all available. Project-defined via
    `.phronesis/journey.json`.
  - **Derivation pass** runs at every pre-check and post-check via
    `journey::derive::assert_facts`; selector validation rejects malformed
    journey config without exit-2.
  - **Outcomes ledger folded into the journey journal** (the notable storage
    change of 0.13.0). `outcomes/ledger.rs` is gone; `outcomes/cargo.rs` now
    returns `(tags, subject)` and the hook stamps them on a single journal
    record. `outcomes/derive::signals` reads via
    `journey::journal::read_recent_subject`. Confidence-scoring behaviour is
    byte-identical; the storage is unified.
  - **`SessionStart` stamps** `.phronesis/journey/session`; pre/post-check
    read it. `PHRONESIS_NO_JOURNEY=1` disables both paths. Fail-open
    throughout — corrupt `journey.json` or missing journal degrades to "no
    journey facts," never exit 2.
  - **`phr-mcp journey [--json] [--explain <rule-id>]`** renders the
    `journey_*` facts a derivation pass would assert against the current
    journal, with `--explain` filtering to a single rule's dependencies.
  - **MCP tool `get_journey`** mirrors the same table/JSON view so the agent
    can ask "what does my trajectory look like" mid-conversation.
  - **`phr-mcp init --packs journey`** writes a starter `journey.json` and
    ensures it is tracked.

### Changed
- **Workspace bumps to 0.13.0.** `phr` and `phr-mcp` move together; `phr-mcp`'s
  `phr` dep bumps to match.

### Notes
- **Coverage discipline.** The workspace stayed at or above the pre-feature
  baseline of 85.4% lines across the journey-facts merges; the journal and
  tagger modules sit near ~90%.

## [0.12.0] - 2026-06-19

### Added
- **Confidence scoring (first milestone)** — gate LLM output on three grounded
  outcomes before a `git commit`: does it compile, do the tests pass, does it
  catch a known bug (a TDD test red on the buggy baseline that goes green).
  See `docs/specs/SPEC-confidence-scoring.md`.
  - Domain-neutral outcome facts (`build_outcome`, `test_outcome`,
    `bug_check_outcome`) behind a per-toolchain **adapter** layer (`cargo`
    first; pytest/tsc/go later emit the same neutral facts).
  - A per-subject **ledger** (`.phronesis/outcomes/<subject>.jsonl`) bridges the
    stateless hook invocations; the pre-check re-derives `signal_pass` facts and
    gate rules count them with the existing `facts_count(...)` DSL
    (`<=1` blocks, `==2` warns, 3 passes clean).
  - Post-check parses a build/test command's captured output into the ledger;
    a `git commit` settles the open work unit.
  - Known-bug registry in `.phronesis/bugs.json`.
  - `phr-mcp confidence [--subject <id>] [--json]` — read-only band/signals
    report for the open work unit.
  - `phr-mcp init --packs confidence` — writes the commit-gate rules plus the
    `.phronesis/confidence.json` opt-in marker and `.phronesis/bugs.json`
    registry, and carves both back into `.gitignore` as tracked config.
  - MCP tools `get_confidence` (band/signals report) and `submit_suggestion`
    (declare an explicit work unit, e.g. a translation, and accrue signals to
    it).
  - **Opt-in per project** via `.phronesis/confidence.json`; fail-open
    throughout, so projects that haven't enabled it are unaffected.

## [0.11.0] - 2026-06-13

### Added
- **Public fact-query API** on `ReteNetwork` — `facts_snapshot`,
  `facts_matching_predicate`, `facts_matching_predicates` (predicate-set
  membership), `facts_matching` (positional-arg filters), `fact_ids_matching`,
  `get_fact_by_id`, `fact_count`. Sync, owned results sorted by fact id, so
  embedding hosts need not reach into `wme_manager`.
- **Richer `list_facts` MCP tool** — the existing `predicate` filter plus new
  `predicates` (set membership) and `arg_filters` (positional `arg = value`)
  params, backed by the fact-query API. Lets coding agents query working
  memory by predicate set or argument, not just list-all.
- **`bash_command_matches` predicate** — regex rules over Bash/command-tool
  text, gated to command tools (file content quoting the same text never
  fires). Ships two LLM-pack guard rules (stage-explicitly, don't-kill-build).
- **Tree-sitter AST predicates for Python and TypeScript** — Python:
  `python_bare_except`, `python_mutable_default_arg`,
  `python_function_param_count_high`, `python_function_missing_docstring`;
  TypeScript (TSX grammar included): `ts_explicit_any`,
  `ts_non_null_assertion`, `ts_suppression_comment`,
  `ts_function_param_count_high`.
- **Silent zero-result audit diagnostics** — `phr-mcp audit` and the
  `audit_codebase` tool now explain a no-hits result when the cause is
  recoverable (no rules carry `audit: true`, or the walker scanned 0 files)
  instead of returning an empty shape indistinguishable from a failure.
- **CI** — GitHub Actions workflow (fmt + clippy `-D warnings` + tests, on
  MSRV 1.90 and stable).
- Typed-error, retraction-semantics, and salience-order test suites.

### Changed (breaking)
- **`Result<_, ReteError>` replaces `Result<_, String>`** across the engine
  crate. `ReteError` is a matchable enum (`FactNotFound`, `LockPoisoned`,
  `DuplicateFactId`, `BindingConflict`, …) implementing `std::error::Error`;
  `From<ReteError> for String` eases migration for string-carrying hosts.
- **Duplicate fact ids are rejected.** Asserting an id already present with
  *different* content errors (`DuplicateFactId`); an identical re-assert is an
  idempotent no-op. Previously a duplicate silently corrupted the predicate
  index (the same fact was returned twice from `get_by_predicate`).
- **Same-salience agenda items fire in FIFO (insertion) order.** Previously
  tie order was `BinaryHeap`-arbitrary; firing order is now deterministic.

### Deprecated
- **`ReteNetwork::get_persistent_facts`** — it hardcodes consumer-specific
  predicates, which don't belong in a domain-neutral engine. Define your own
  predicate set and call `facts_matching_predicates(&YOUR_SET)`. Slated for
  removal in 0.12.

### Fixed
- **Retraction purges stale agenda items** referencing the retracted fact, so
  a pending rule can no longer fire against a fact that is no longer true.
- **Refraction keys compare exact WME ids** — retracting `f1` no longer
  clobbers the refraction state of `f10` (was a substring match).
- **`get_memory_drift`** marks guidance *actionable* only when it maps to an
  expressible predicate (named command, file/path/code shape, or function
  shape); operational prose is bucketed *ambient*. Actionable entries now also
  register coverage from `durable.md`, so the drift list converges.

## Known follow-ups (specs landed, implementation deferred)

- **`SPEC-gate-merge-commits.md`** — broaden the confidence gate's
  `bash_command_matches` pattern from `"git commit"` to
  `"git (commit|merge|rebase|cherry-pick|revert|pull)"`. Five of six
  commit-producing porcelain commands currently bypass the gate. Live-tested
  during the journey-facts merge night. PATCH-shaped change for 0.13.x.
- **`SPEC-pack-opt-in-facts.md`** — pack-level supersession via zero-arg
  marker facts. When `confidence` is opted in, assert `confidence_enabled`
  at hook fire (mirroring `clock_facts`) and condition
  `nudge-verify-before-commit` on its absence via the existing
  `facts_count(...) == 0` form. Removes the double-warn on every `git commit`
  for projects running both `llm` and `confidence` packs. PATCH for 0.13.x.

## Earlier releases

Pre-0.11 history (0.10.0 and earlier) is recorded in the git log and
`docs/specs/`. Notably, 0.10.0 added wiki-drift, the block-pattern rules, and
the v2 rule schema.

[0.17.0]: https://github.com/awaterma/phronesis/releases/tag/v0.17.0
[0.16.2]: https://github.com/awaterma/phronesis/releases/tag/v0.16.2
[0.14.0]: https://github.com/awaterma/phronesis/releases/tag/v0.14.0
[0.13.3]: https://github.com/awaterma/phronesis/releases/tag/v0.13.3
[0.13.2]: https://github.com/awaterma/phronesis/releases/tag/v0.13.2
[0.13.1]: https://github.com/awaterma/phronesis/releases/tag/v0.13.1
[0.13.0]: https://github.com/awaterma/phronesis/releases/tag/v0.13.0
[0.12.0]: https://github.com/awaterma/phronesis/releases/tag/v0.12.0
[0.11.0]: https://github.com/awaterma/phronesis/releases/tag/v0.11.0

# phronesis-mcp

MCP server wrapping the phronesis RETE rules engine for rules-bounded LLM interaction. Builds the `phr-mcp` binary and uses the `phr` library (the `phronesis` crate, imported under that alias).

## Build & Run

```
cargo build
cargo run -- serve       # MCP stdio server (default)
cargo run -- pre-check   # PreToolUse hook (blocks violations)
cargo run -- post-check  # PostToolUse hook (warns on violations)
cargo run -- codex-hook PreToolUse  # Codex protocol adapter (event varies by hook)
cargo run -- claude-hook UserPromptSubmit  # Claude Code / Gemini CLI lifecycle adapter (event varies by hook)
cargo run -- init             # One-command setup for a project
cargo run -- session-context  # SessionStart hook (injects active rules + durable directives)
cargo run -- interaction-context # UserPromptSubmit / BeforeAgent hook (injects recent activity + durable directives)
cargo run -- context inspect   # dry-run the configured context payload (writes nothing)
cargo run -- context predicates # allowlisted predicates for nudge capsules
cargo run -- context stats     # observed context cost, omissions, latency
cargo run -- stats             # Read-only per-rule summary of .phronesis/log.jsonl
cargo run -- stats --kalpa lifecycle-events  # ...with the lifecycle section restricted to one kalpa
cargo run -- audit            # Whole-tree audit of rule violations (CI-friendly: --fail-on block)
cargo run -- trend            # Debt-over-time view comparing audit snapshots
cargo run -- confidence       # Confidence band + grounded signals for the open work unit
cargo run -- toolchains        # List active toolchain defs (built-in + project); --json for machine output
cargo run -- journey   # what journey_* facts assert right now
cargo run -- journey --lifecycle    # only the lifecycle records (sub-agent start/stop, prompts, interrupts, stops, commits)
cargo run -- journey --corrections  # the prompts that followed an interrupt, oldest first, with their scrubbed text
cargo run -- kalpa start <name>     # Name the theme this run of sessions belongs to (also: kalpa end, kalpa show [name])
cargo run -- unit start [<id>] [--spec <path>|--bug <id>]  # Name the work item being built (also: unit end, unit show [<id>])
cargo run -- drift            # Multi-source drift across CLAUDE.md, memory, wiki decisions, and code
cargo run -- decision new <slug>  # Scaffold a new ADR page at .phronesis/wiki/decisions/<today>-<slug>.md
cargo run -- graph rebuild        # Rescan every Rust file into .phronesis/graph.jsonl (resync after git checkout/rebase)
cargo run -- graph status         # Does the code graph still match the working tree?
cargo run -- graph ownership <fn> # Grouped Rust ownership evidence for a function (or glob): sites, spans, relationships, evidence level/provider, type & MIR availability, and the limit of each claim. Evidence with stated limits — never proof. Opt-in via `[ownership.rust]` in `.phronesis/graph.toml`.
cargo run -- state                # Classify authored/cache/history/sensitive .phronesis state
cargo run -- clean --cache        # Remove only rebuildable graph cache files
cargo run -- migrate-rules <path>  # Convert a rules.json from the old (v1) shape to the v2 shape
cargo run -- migrate-extracted-rules <path>  # Salvage pre-0.14.0 extract_rules output: strip prefixes, demote actions
cargo run -- catalogue        # Regenerate docs/catalogue.html from the shipped packs (run from repo root)
cargo run -- scrub-payload <path> [--write] [--home DIR] [--project-root DIR]  # Anonymize captured payloads for committing as fixtures
cargo run -- coverage import <export.jsonl>  # Import a normalized per-test coverage export into the evidence store
cargo run -- coverage select [--change <id>] [--json]  # Select tests relevant to the current change (dynamic coverage + static graph reach)
```

### Payload-contract corpus

Committed fixtures of CLI hook payloads under
`tests/fixtures/payloads/<cli>/<name>.json` (claude-code and gemini) —
replayed verbatim through the real binary by
`cargo test --test payload_contract`, which asserts exit codes,
stdout-JSON (the Gemini exit-0 contract), action-log consequences, and
journey-journal tags (with a freshness guard so `init` scaffolding can't
produce false-greens, and a non-empty-corpus assert so a missing fixture
tree can't pass vacuously). Each fixture is self-describing with `source`
(cli, event, provenance) and `expect` (exit, stdout_json, log_rule_fired,
journal_tag_new, journal_tag_from_output, stderr_contains). The replayable
fixtures are still `provenance: "authored"`, but they are no longer
guesses: `payloads/claude/raw/` holds **real** headless Claude Code
captures (2.1.270, taken through `PHRONESIS_CAPTURE_DIR` and scrubbed) and
`payloads/gemini/raw/` real Gemini CLI captures, and the authored
envelopes are modelled on them. `raw/` is evidence, not contract —
`collect_fixtures` walks exactly one level, so nothing under `raw/` is
replayed; `payload_contract.rs` pins its *field sets* instead. One
lifecycle fixture is deliberately not a capture:
`tests/fixtures/transcripts/claude/interrupted-tail.jsonl` is
hand-written to Claude Code's transcript line shape so the
interrupt-marker branch of `classify_prompt` has the exact
`[Request interrupted by user]` text to match. Its README says so; replace
it if a scrubbed real tail is ever captured.

The same test file consumes `tests/fixtures/hook_events.json`, the
hook-event-name registry: `init_wires_hooks_only_under_event_names_that_exist`
checks every event name `init` wires (Claude Code and Gemini) against the
registry, and `before_model_request_never_reappears` pins the 0.17.1
`BeforeModelRequest` incident by name. New host events must be added to
the registry in the same PR that adds wiring. The lifecycle work grew it:
`SubagentStart`, `SubagentStop`, `Stop`, and `SessionEnd` for
`claude-code`; `AfterAgent` and `SessionEnd` for `gemini`; `Interrupt` and
`SessionEnd` for `codex`.

**Refresh workflow** (when a CLI changes its payload shape):
1. Set `PHRONESIS_CAPTURE_DIR=/tmp/cap` in the shell that launches the CLI.
2. Work normally — captures are written to `<dir>/payloads.jsonl`.
3. `phr-mcp scrub-payload /tmp/cap/payloads.jsonl [--write] [--home DIR] [--project-root DIR]`
   to anonymize. Run it from the project root (or pass `--project-root`)
   so in-project paths are preserved. Output is JSONL; `--write` backs the
   original up to `<path>.bak` first, and a residual leak or corrupt line
   aborts the run before anything is written.
4. Human-review the scrubbed output for semantic leaks.
5. Place reviewed records under `tests/fixtures/payloads/<cli>/` with
   proper `source.provenance` and `expect` blocks.

### Durable directives (`.phronesis/durable.md`)

Optional file. When present, its contents are re-injected into the model's
context at every SessionStart AND every UserPromptSubmit. CLAUDE.md gets
loaded once at session start and then fades as the conversation fills the
context window; `durable.md` stays live across the whole session.

Use it for the small subset of project guidance that absolutely must
survive context compression — typically a few hundred words. CLAUDE.md
remains the human-facing onmaping doc; `durable.md` is the "this must
not fade" subset that the model re-reads every turn.

### Token-aware durable context (enabled by default)

Reinjecting the whole `durable.md` on every turn costs a fixed slice of
the payload whether it is relevant or not, and it spends that budget
*before* the active rules and recent decisions. On a project with a
3.3 KB durable file that left roughly 780 bytes of the 4 KiB cap for
everything else, and the session charter truncated mid-rule-list.

A project opts in by creating `.phronesis/context.json` (written by
`phr-mcp init --packs context`). That splits the one file into two roles:

- **`.phronesis/kernel.md`** — the always-on core, injected every turn.
  Keep it small; it is paid for on every single interaction.
- **`.phronesis/durable.md`** — the session-level project document,
  rendered once at SessionStart (and PostCompact where the host has one).
  Opting in never rewrites, repurposes, or shrinks it.

Packing is deterministic and stateless: every unit is measured with its
headings and separators, and admitted only if it fits its kind ceiling,
the shared byte capacity, and the optional soft token budget. Markdown
splits on `##` sections, so an over-budget section drops whole rather
than leaving an orphaned lead-in. Current enforcement activity gets first
claim, then the kernel, then nudges, then overflow activity.

Without `context.json` nothing changes: payloads stay byte-identical,
capsules are not scanned, and no observations are written.

```
phr-mcp context inspect --event interaction   # dry run: what is selected, and why not
phr-mcp context inspect --event session --json
phr-mcp context predicates                    # what may trigger a nudge capsule
phr-mcp context stats --since 7d              # observed cost, omissions, latency
```

`inspect` writes nothing, so reading the diagnostic cannot contaminate
the data it reports on. It names every candidate with its cost, the
reason each omitted item was dropped (`kind_ceiling`, `byte_capacity`,
`token_capacity`, `displaced_by_nudge`), every capsule that failed to
load, and every demanded fact that could not be hydrated — which is how
you tell "the facts were false" apart from "the selector has a typo".

Situational nudge capsules live in `.phronesis/nudges/*.md` (see the
scaffolded README there for the schema). Bodies are static: runtime facts
select a capsule but are never interpolated into it, which is what stops
a filename or tool payload becoming a second-order prompt-injection
channel. Only the compiled predicate allowlist may trigger one.

Context source files are capped at 64 KiB — they are read on every hook,
so an oversized one is ignored with a diagnostic rather than re-read and
discarded each turn.

### Drift detection — guidance and code ↔ rules

Three frozen compatibility CLI commands and one consolidated MCP tool surface
the gap between guidance or code and enforced rules. The MCP surface is
`get_drift(source)`, where `source` is `claude_md`, `memory`, `wiki`, `code`,
or `all` (the default). It also accepts `limit` (default 5, maximum 50),
`format` (`json` by default or `table`), `suggest` (default false),
`memory_dir`, and `wiki_dir`. The equivalent multi-source CLI is `phr-mcp
drift [--source S] [--limit N] [--json] [--suggest] [--memory-dir P]
[--wiki-dir P]`.

The consolidated `claude_md` source discovers root and package-level
`CLAUDE.md` and `AGENTS.md` files within three directory levels, excluding
`.git/`, `.worktrees/`, `target/`, and `node_modules/`. Repeated imperatives
are deduplicated and each finding names every guidance file it came from. The
source is missing only when no guidance file is found anywhere in that bounded
search.

`phr-mcp claude-md-drift` is a frozen compatibility command retaining its
existing single-source output and `--suggest` behavior. It extracts imperative
bullets from CLAUDE.md ("Don't X", "Always Y", "Prefer Z") and matches each
one against the current rule pack by token overlap. Output flags bullets with
no confident match — candidates that either should become rules or should be
marked as "non-lintable by design" so future audits don't re-flag them.

`phr-mcp memory-drift` is a frozen compatibility command retaining its
existing single-source output and `--suggest` behavior. It walks Claude Code's
per-project auto-memory directory (default
`~/.claude/projects/<encoded-cwd>/memory/`), parses the YAML frontmatter on
each entry, and classifies it into one of three buckets per
`docs/specs/SPEC-memory-to-rules.md`: `actionable` (should become a rule),
`ambient` (should be in durable.md), or `personal` (stays in MEMORY.md).
Non-personal entries are scored against rules.json and durable.md by token
overlap; uncovered ones are surfaced for porting. `--suggest` emits draft rule
JSON on stderr.

`phr-mcp wiki-drift` is a frozen compatibility command retaining its existing
single-source output and `--suggest` behavior. It walks
`.phronesis/wiki/decisions/`, parses ADR-style frontmatter on each page, and
classifies decisions into `covered` / `likely-covered` / `uncovered` /
`superseded` against the current rule pack. Explicit `enforces: [rule-id]`
frontmatter beats the Jaccard fallback — authors who list which rules enforce
a decision get a deterministic match. `--suggest` emits draft v2 rule JSON on
stderr for uncovered decisions. Pair with `phr-mcp decision new <slug>` to
scaffold new ADR pages from a template.

All drift surfaces are heuristic (no LLM call) — output is a triage list, not
ground truth.

## One-time global install (recommended)

After `cargo install --path .`, register the MCP server at user scope so
*every* project Claude Code opens can call `mcp__phronesis__*` tools without
per-project `.mcp.json`:

```
phr-mcp install              # writes to ~/.claude.json::mcpServers.phronesis and ~/.gemini/settings.json::mcpServers.phronesis
phr-mcp install --dry-run    # preview
phr-mcp uninstall            # remove the user-level entry
```

This is idempotent. Other entries in `~/.claude.json` (other MCP servers,
theme, etc.) are preserved.

After install, restart Claude Code and Gemini CLI (any project) to pick it up. You still
need per-project `init` for hooks and rules — see below.

Codex registration is deliberately project-scoped: `phr-mcp init` merges
`.codex/config.toml` and `.codex/hooks.json`. Review new or changed commands
with Codex `/hooks`; Phronesis never marks its own hooks trusted.

Codex hook decisions are a structured-JSON contract: after emitting valid
JSON, `phr-mcp codex-hook` exits 0 even when its `PreToolUse` response contains
`permissionDecision: "deny"`. Warnings use `additionalContext` or
`systemMessage`. Claude-compatible `pre-check`/`post-check` remain a process
exit contract (0 clean, 1 advisory, 2 pre-tool block). The shared action log
records the logical 0/1/2 verdict for either host. External automation must
therefore parse Codex stdout rather than interpreting its process exit as the
rule verdict.

## Setting up a project

For hooks and project-specific rules, in any project:

```
phr-mcp init                          # complete language-agnostic platform
phr-mcp init --packs rust             # defaults + Rust enforcement
phr-mcp init --packs rust,rhai        # defaults + Rust and Rhai enforcement
phr-mcp init --packs python           # defaults + Python enforcement
phr-mcp init --packs python,python-patterns  # + opt-in python-patterns.guide advisories
phr-mcp init --packs typescript       # defaults + TS/JS enforcement
phr-mcp init --packs none             # no defaults or starter rules
phr-mcp init --dry-run                # preview without writing
phr-mcp init --force                  # overwrite existing rules.json (backs up to .bak)
phr-mcp init --hooks-only             # refresh hook wiring without touching rules.json
```

`setup` and `configure` are aliases for `init` if those feel more natural.

### Extensible predicates

Project-defined Rhai providers in `.phronesis/predicates/*.rhai` run before
RETE matching and may call `emit_fact(predicate, args)` against a normalized
read-only `event`. Multi-file operations first provide batch context through
`event.files`, then per-file context through `event.file_path`; the unused
field is empty in each view. When a requested rule needs LHS vocabulary that
does not exist yet:

1. Call `test_predicate_provider` with the proposed script and representative
   event.
2. Call `add_predicate_provider` to write the validated provider.
3. Call `add_rule` with a condition matching the emitted predicate.

Use `list_predicate_providers`, `get_predicate_provider`, and
`remove_predicate_provider` for review and lifecycle management. Existing
providers require explicit `replace: true`; pre-hook provider failures block,
while post-hook failures warn because the action has already executed.

The packs are composable and **independent**:
- `llm` — LLM-behavior rules. Blocks the deflection family (disclaimers
  that shift blame to pre-existing code or to "the test environment")
  plus unverified completion claims. Warns on `git commit -m` to nudge
  end-to-end verification before reporting done, on a sweeping
  `git add -A` / `git add .`, and on `pkill`/`kill` of a `cargo`/`rustc`
  build (the last two via the `bash_command_matches` regex predicate).
  These rules fire from disk at every hook invocation, so they remain
  active even when CLAUDE.md content has been compressed out of context.
- `rust` — Rust code-shape enforcement. Blocks: `.unwrap()` /
  `todo!()` / `panic!()` / `unimplemented!()` in src/,
  `Result<_, String>` returns, `#![deny(warnings)]` (breaks on toolchain
  upgrade), panicking constructs (unwrap / `.expect("")` / panic! /
  todo! / unimplemented!) inside a `Drop::drop` body (a panic during
  unwind there aborts the whole process — log and swallow instead).
  Warns: public fn taking `&String`, `&Vec<T>`, or
  `&Box<T>`, functions with 3+ `.clone()` calls, functions with 5+
  parameters, `impl Deref for` (Deref polymorphism anti-pattern),
  `#[test]` functions with no assertions or `?` operator,
  `cargo build/test/check/clippy` without `--workspace`, `dbg!()` in
  src/, `.expect("")` with an empty message in src/, public fns in src/
  without a `///` doc comment (API Guidelines C-DOC). Audit-only
  (silent at hook time, surfaced by `phr-mcp audit`): files exceeding
  800 lines (god-file signal), manual `=> return Err(...)` match arms
  (use `?`), `*_id: String` / `*_id: u64` fields (newtype
  opportunity), `None => {}` and `Err(_) => {}` match arms (if-let
  opportunity / silent error-swallowing), `Rc<RefCell<...>>` in src/
  (fighting-the-borrow-checker shape), `" + &` string concatenation
  (prefer `format!`), `#[allow(dead_code)]` in src/, `env::set_var(`
  in src/ (unsound under concurrent reads — and unsafe in edition
  2024), functions with 3+ outer-scope `let mut` declarations
  (block-pattern candidate: erasure of mutability via
  `let x = { let mut tmp = ...; tmp }`), functions with 8+
  outer-scope `let` bindings (block-pattern candidate: scope
  intermediate temporaries into a block), synchronous lock guards whose
  lexical scope crosses `.await`, unsafe blocks without a nearby `SAFETY:`
  explanation, and known blocking filesystem/thread calls made directly
  inside `async fn` (warning at hook time). The rust-unofficial/patterns
  book is the upstream source for
  the borrow-types, deny-warnings, and string-concat rules; the
  Rc/RefCell rule is a more general Rust-idiom observation.
- `rhai` — discipline for projects that embed the Rhai scripting
  language. Blocks: `engine.eval(<string literal>)` in Rust source
  (precompile to AST via `compile_file` / `eval_ast` instead), and
  `print(` in `.rhai` scripts (use the host-registered output channel,
  whatever your `Engine` exposes via `register_fn`). Generic messages
  — layer project-specific guidance into your own `.phronesis/rules.json`.
- `python` — bare-except blocked; warnings for `print()`, mutable default
  arguments, import-time I/O, and identity comparisons against value
  literals; audits for function calls in defaults, swallowed typed
  exceptions, high parameter counts, missing public docstrings, mutated
  module-level containers, and star imports. The last two remain audit-only
  because name shadowing and package `__init__.py` re-exports can be
  intentional. Every rule consumes a tree-sitter predicate; none matches
  substrings.
- `python-patterns` (alias `py-patterns`) — opt-in, opinionated advisories
  derived from <https://python-patterns.guide/>, each backed by its own
  tree-sitter predicate and citing the guide page plus the heuristic's
  limit. Warns on `global` rebinding and `globals()[...]` introspection
  assignment (Prebound Methods / Global Object), `type(name, bases, ns)`
  dynamic classes, multiple inheritance of concrete classes, and `*Mixin`
  classes with `__init__` (Composition Over Inheritance), `__new__`-based
  singletons (Singleton), containers whose `__iter__` returns `self`
  (Iterator), static delegation wrappers of 4+ forwarding methods without
  `__getattr__` (Decorator), mutable class-body containers (shared state),
  and `== None` (Sentinel Object). Audit-only: other `__new__` overrides
  (Flyweight), positive same-subject `if`/`elif` dispatch across non-builtin
  domain types (Composite), and file-local inheritance depth of 3+. The
  Composite predicate excludes independent guards, negative checks, and
  primitive/container validation. Not enabled by the plain `python` pack.
- `typescript` — `: any` and `console.log` warnings, plus tree-sitter AST
  rules (explicit `any`, `@ts-ignore`/`@ts-expect-error`/`@ts-nocheck`
  suppressions, non-null `!` assertions, functions with 5+ parameters)
- `swift` — Swift-specific advisories: force-unwrap warning, try! warning,
  `throws` functions that force-unwrap instead of throwing
- `confidence` — confidence-band gate (SPEC-confidence-scoring), enabled by default.
  Writes `.phronesis/confidence.json` and ships two advisory gate rules over
  `git (commit|merge|rebase|cherry-pick|revert|pull)`: low confidence warns
  that build/test/known-bug evidence is incomplete or failing, medium warns
  that one grounded signal is missing, and high (3/3 signals) passes clean —
  neither band blocks the Git command (SPEC-structural-rule-migration
  §"Confidence gate severity"). Pair with `.phronesis/bugs.json` (known-bug
  registry) and `phr-mcp confidence` for the report surface. Also scaffolds
  `.phronesis/toolchains.json` (pytest/tsc example defs). Confidence signals
  are toolchain-neutral: any command matched by a toolchain def grounds a
  `build_outcome` from its exit code (`command_exit`, captured on every shell
  journal record), with optional per-toolchain regex refinement for test
  counts and per-test results. Cargo, `xcodebuild`, and SwiftPM ship as
  built-in defs; project defs in `toolchains.json` extend or override them.
  (One behavior refinement
  vs. pre-0.18: a build/test command that exits non-zero with no test
  summary and no compile-error text is now graded build-fail when the CLI
  supplies the exit code.) Journal growth is bounded by
  write-side compaction (`PHRONESIS_MAX_JOURNAL_BYTES`, default 16 MiB) that
  preserves each subject's latest grounded outcome.
- `journey` — project-defined taggers + journey_* aggregator facts (cross-call temporal predicates)
- `structural` (alias `graph`) — rules over the code graph
  (SPEC-triple-store-rete). Warns on a production function that calls a
  panicking API with no direct test, and on a module in an import cycle.
  `init` builds `.phronesis/graph.jsonl` for you; the `PostToolUse` sensor
  keeps it current thereafter. Edits that bypass the hook (`git checkout`,
  rebase, shell edits) mark the graph stale, which downgrades these rules to
  warnings until `phr-mcp graph rebuild`. Every hooked document save triggers
  a complete graph rebuild; `on_save` remains available as a lower-level
  incremental API for explicit callers and performance comparisons. Rust,
  Python, TypeScript, Swift, and Java produce
  graph facts. Rust's risky-call watchlist covers panic-at-call-site APIs;
  TypeScript's narrower `!` watchlist means "unchecked type assumption," not
  that failure occurs at the assertion site. Both structural rules can fire
  for Rust and TypeScript. Python has no defensible risky-call watchlist, so
  only its import-cycle rule can fire. Java uses Maven/Bazel discovery and
  package-level modules; the same `warn-import-cycle` rule reports package
  cycles. Java has no packaged risky-call rule. Java source and build-metadata
  edits refresh the repository-wide declaration index and unchanged importers.
  A content-validated `.phronesis/java-declarations.json` cache reuses parses
  across hook processes; `state` lists it and `clean --cache` removes it.
  Discovery diagnostics expose unsupported build constructs and classpath
  approximations. Both rules remain `warn`; measured
  precision is recorded in the spec and promotion to `block` requires broader
  corpus evidence.
  Rule staleness uses the same graph, but accepts only conservative evidence:
  an unqualified function-call-shaped `new_content_contains` literal such as
  `legacy_call(` must first resolve to a local definition. Prose, attributes,
  method calls, and namespace-qualified calls do not bind. Once bound, losing
  every definition demotes that rule from block to warn until review. Set
  `"binds": false` on a disk rule to disable this behavior explicitly.
- `context` — token-aware durable context (see above), enabled by default. Writes
  `.phronesis/context.json`, `.phronesis/kernel.md`, and a
  `.phronesis/nudges/README.md` documenting the capsule schema. Ships no
  rules.
- `none` — empty rules array (hooks still wired)

`base` is shorthand for every language-agnostic pack —
`llm,confidence,journey,structural,context`. This includes graph construction,
graph-derived rules, and every other language-neutral subsystem. It is included automatically for
every selection except `none`, so name only the language additions:

```
phr-mcp init --packs rust
phr-mcp init --packs typescript
```

Language packs are deliberately **not** in `base`. Several of their rules
match raw substrings gated only by path, so composing every language at
once produces cross-language false positives — the TypeScript `: any`
rule fires on Rust's `: anyhow::Error`, for instance. Name the language
you actually want.

`init` writes/merges seven files:
- `.claude/settings.local.json` — hook config (preserves existing permissions/hooks).
  `PreToolUse`/`PostToolUse` keep the `Edit|Write|MultiEdit|Bash` matcher.
  `SessionStart`, `SessionEnd`, `UserPromptSubmit`, `SubagentStart`,
  `SubagentStop`, and `Stop` are registered empty-matcher against
  `phr-mcp claude-hook <Event>` — one adapter for the whole lifecycle
  surface. Replacement on those events is keyed on the **command**, not the
  matcher, so a hook of your own on the same event survives `init`; an entry
  counts as ours whether the binary is invoked bare, by absolute path, or
  through a wrapper. Settings files written by older versions keep working:
  `session-context` and `interaction-context` are unchanged subcommands.
- `.mcp.json` — MCP server registration
- `.phronesis/rules.json` — starter rule pack (left alone on re-run unless --force)
- `.phronesis/durable.md` — default re-injected directives, including drift-discipline nudges that point the model at `get_drift`. Left alone on re-run; edit in place to customize.
- `.phronesis/wiki/decisions/README.md` — wiki scaffold; the directory is un-ignored from the broad `.phronesis/` gitignore. Left alone on re-run.
- `.gemini/settings.json` — MCP server registration; `BeforeTool`/`AfterTool`
  hooks, whose matcher is now **anchored**
  (`^(replace|write_file|run_shell_command|invoke_agent)$`) because Gemini
  treats it as an unanchored regex, and which now include `invoke_agent`
  because that is where Gemini sub-agent start/stop pairs are derived from;
  plus `SessionStart`, `SessionEnd`, `BeforeAgent`, and `AfterAgent` against
  `phr-mcp claude-hook <Event>`. `init` notes that Gemini HTML-escapes
  injected context and skips project hooks until the folder is trusted.
- `.gitignore` — log/backup paths + `!.phronesis/wiki/**` exception so the decisions tree is versioned

`.codex/hooks.json` and `.codex/config.toml` are merged as well (see
*One-time global install* above). Codex now gets `Interrupt` and
`SessionEnd` alongside the tool phases, and the `SessionStart` matcher is
empty rather than `startup|resume|clear`, so compact and fork sessions get
context too. Codex skips new or changed project hooks **silently** until
you review and trust them with `/hooks` — a Codex session that records no
lifecycle events is the trust gate, not a bug.

Re-running is idempotent: existing config is preserved; only our entries are added.

### Refreshing just the rules pack

When you've added new predicates upstream and want to pull the latest rule
pack into an existing project — without touching its hook config, MCP
registration, or gitignore — use `--rules-only`:

```
phr-mcp init --rules-only --packs llm,rust
```

Sync adds new starter rules and updates previously installed definitions that
have not been edited locally. Custom rules, local edits, remembered deletions,
rules from other packs, and top-level metadata are preserved. Conflicting
local definitions are reported. Older projects without a baseline preserve
all existing definitions on their first sync; differing starter IDs produce
warnings because their ownership cannot be inferred safely.

`.phronesis/starter-rules.json` records starter definitions for future syncs.
Keep this file with the project rules; if it is missing, sync falls back to
conservative conflict handling. Comparisons use JSON values, so rules rewritten
into another schema are also conservatively preserved. Removed upstream rules
are retained. `--packs none` preserves existing rules during sync.

Changed rules are backed up to `rules.json.bak`; dry runs write neither rules,
baselines, nor backups. Invalid rule files or baselines fail without replacement.
Use `--force` for an explicit replacement of all rules with the selected packs.

### Refreshing just the hooks

When you've added new context-injection hooks upstream (e.g. SessionStart or
BeforeAgent entries that didn't ship in older versions of init) and
want to pull them into an existing project without touching its rules pack,
use `--hooks-only`:

```
phr-mcp init --hooks-only
```

Writes `.claude/settings.local.json`, `.mcp.json`, and `.gemini/settings.json`
only. `.phronesis/rules.json` and `.gitignore` are left exactly as they were.

### Looking at activity

`phr-mcp stats` reads `.phronesis/log.jsonl` and prints a per-rule
summary of blocked/warned counts and the last time each rule fired.

```
phr-mcp stats                 # all time, terminal table
phr-mcp stats --since 7d      # last week only
phr-mcp stats --rule no-unwrap-in-src
phr-mcp stats --json          # machine-readable, pipeable into jq
```

Read-only. Useful for spotting noisy rules to silence (`silent: true` on
the rule), dead rules to delete, or for confirming a tuning change had the
effect you wanted.

### Lifecycle events

Journey facts record what the agent did to files and shells. Lifecycle
events record the shape of the conversation around them: when a sub-agent
was spawned and when it returned, when the human spoke while the agent was
still working, when a turn was aborted, and when `HEAD` moved. That is
where the governance signal is densest — an interrupt followed by a new
prompt is a correction, and corrections are the raw material for
friction-driven rule proposals. Full design:
[SPEC-agent-lifecycle-events](../../docs/specs/SPEC-agent-lifecycle-events.md).

Ten record kinds reach both the journey journal and `.phronesis/log.jsonl`:
`subagent_start`, `subagent_stop`, `prompt`, `interrupt`, `stop`, `commit`,
`unit_start`, `unit_end`, `kalpa_start`, `kalpa_end`. Claude Code and Gemini
CLI feed them through `phr-mcp claude-hook <Event>`, Codex through
`phr-mcp codex-hook <Event>`; Gemini has no sub-agent event, so its pair is
derived from the `invoke_agent` tool at pre-check and post-check.

A `prompt` record carries a **mode**:

| mode | meaning |
|---|---|
| `fresh` | the previous turn ended with a `stop`, or this is the session's first prompt |
| `mid_turn` | the turn is still open and no interrupt was seen — the human added context while the agent worked |
| `correction` | an `interrupt` immediately precedes this prompt in the same session |

**An intervention is the human changing the plan, not merely replying.**
That is why `fresh` is not one: it arrives after the agent stopped, and as
far as a hook can tell it is a reply. `mid_turn` and `correction` arrive
while the agent was executing its plan, or after the human stopped it, so
those records also carry the tag `lifecycle:intervention` — **but only when
the prompt is top-level**. A prompt record carrying an `agent_id` (a prompt
delivered inside a sub-agent) is never tagged and never counted as a
correction, because the human did not speak. The number undercounts plan
changes delivered as a fresh prompt after a natural stop; reading intent is
a non-goal.

`commit` is detected from ground truth rather than command text:
`pre-check` records `HEAD` before a shell call that passes a cheap text
pre-filter, `post-check` compares it after. **`HEAD` movement is the ground
truth and the exit code is only a veto**, so a host that reports no exit
code at all — Claude Code's `Bash` is one — still gets its commits, marked
`detection: "no_exit_code"`. A sha the host reports itself is kept as
`host_sha` and stands in as `detection: "host_reported"` when the probe
found no baseline. Commits are undercounted, never overcounted: an alias, a
wrapper script, `git pull`, or a commit made outside a tool call is missed,
and the reports say so.

**Kalpas and work items.** A *kalpa* is a named theme spanning sessions
(`[a-z0-9][a-z0-9-]{0,63}`); a *work item* is the existing work unit
(`outcomes::subject`) made explicit.

```
phr-mcp kalpa start lifecycle-events   # ends any open kalpa first
phr-mcp kalpa show [name]              # open kalpa, or a named one
phr-mcp kalpa end

phr-mcp unit start --spec docs/specs/SPEC-agent-lifecycle-events.md
phr-mcp unit start --bug 42            # names it bug-42 from .phronesis/bugs.json
phr-mcp unit start                     # mints a fresh unit-<nanos>
phr-mcp unit end
phr-mcp unit show [<id>] [--json]
```

`--bug` resolves against the known-bug registry and carries that entry's
cargo test name (and its spec, if it has one); an unknown id is an error,
not a fresh unit. The agent can name the item from inside the conversation
instead: `submit_suggestion` gained optional `spec` and `bug_id` parameters
and records the same `unit_start` through the same code path, so a rule can
nudge it to ask which bug or spec the session is for. Implicit units keep
working exactly as before and get no `unit_start` record; the reports show
the explicit/implicit split rather than hiding it.

**Reports.** `phr-mcp stats` grows a lifecycle section (and `--kalpa <name>`
restricts it); `phr-mcp kalpa show` prints the same counts for one kalpa:

```
kalpa: lifecycle-events      started 2026-09-18 (3d)      counts since log entry 2026-09-17 14:02
sessions        4
prompts        61   fresh 44   mid_turn 9   correction 8
interventions  17   (mid_turn + correction)
interrupts      8
sub-agents     12   starts, 11 matched   median 3m40s
commits         7   (shell tool calls only)   confidence at commit: high 5  medium 2  low 0
interventions / commit   2.43   (retained window)
work items      9   explicit 6   implicit 3
governed        7   (commit + rules evaluated + band ≥ medium)
interventions / work item   1.89
```

`phr-mcp unit show` joins the journal and the action log on `subject` and
prints one item's spec, window, kalpa, rules evaluated/fired/blocked/warned
with per-rule counts, grounded evidence and band, interventions with their
scrubbed text, and commits. Read every ratio against the header: the action
log rotates at 50 MiB keeping one predecessor, so **`interventions / commit`
is computed over the retained window, not the kalpa's full span**, and a
closed kalpa whose `kalpa_start` has rotated off prints `start not
retained`. `phr-mcp journey` renders lifecycle records in a table below the
facts (`⟂` marker); `--lifecycle` shows only those, `--corrections` lists
the post-interrupt prompts oldest first with their text.

**Selectors.** Lifecycle records carry a closed set of built-in tags, plus
two open-ended patterns:

```
lifecycle:subagent_start   lifecycle:subagent_stop
lifecycle:prompt           lifecycle:prompt:fresh
lifecycle:prompt:mid_turn  lifecycle:prompt:correction
lifecycle:intervention     lifecycle:interrupt
lifecycle:stop             lifecycle:commit
lifecycle:unit_start       lifecycle:unit_end
lifecycle:kalpa_start      lifecycle:kalpa_end
lifecycle:agent:<agent_type>   kalpa:<name>
```

**Agent types are lowercased**, whatever the host sent: Claude Code's
`Explore` is `lifecycle:agent:explore`. Write selectors in lower case. The
set is closed on purpose — `lifecycle:prompt:corection` still fails as
`UndefinedSelector` rather than validating and matching nothing — and both
namespaces are reserved, so a tagger tag beginning `lifecycle:` or `kalpa:`
is rejected at config load. Two rules worth copying:

```json
{ "id": "warn-many-interventions-since-last-commit",
  "when": [
    { "__script__": "facts_count('journey_filtered_since_ge', ['lifecycle:commit','lifecycle:intervention',3]) >= 1" }
  ],
  "then": { "warn": "Three interventions since the last commit. Stop and re-plan before continuing." } }

{ "id": "suggest-rule-after-two-corrections",
  "when": [
    { "__script__": "facts_count('journey_count', ['lifecycle:prompt:correction','s']) >= 2" }
  ],
  "then": { "suggestion": "Two corrections this session. `phr-mcp journey --corrections` lists them; consider a rule." } }
```

**Journal v2 and the tool projection.** `JournalRecord` bumps `v` to 2 and
gains optional `kind`, `mode`, `host`, `turn`, `agent`, `agent_type`, and
`kalpa`; readers accept v1 and v2. A lifecycle record uses the sentinel
`tool: "__lifecycle"` and `path: ""`, and the tagger and module resolution
are **not** invoked for it, so no user-defined tag can land on one. The
derive pass splits the records it read into `tool_records` (`kind` absent)
and `all_records`: positional `Nc` windows and `distinct` count tool
records only, while `s`/time windows, `since_ge`, and `filtered_since_ge`
filter over everything. **Existing journey rules are therefore byte-identical
before and after** — that is the point of the split, and a determinism test
pins it. The corollary for new rules: a `lifecycle:*` selector paired with
an `Nc` window yields no facts, so use `s` or a time window;
`validate_selectors` prints one stderr warning naming a rule that does it.
Compaction additionally retains `lifecycle:commit`, `lifecycle:interrupt`,
`lifecycle:prompt:correction`, `lifecycle:kalpa_start`, and
`lifecycle:kalpa_end`, so "two corrections this session" cannot stop firing
because the journal compacted.

**Privacy.** Prompt text goes **only** to `.phronesis/log.jsonl` (gitignored,
along with its rotated `.1` predecessor, which `init` now appends if the
project's `.gitignore` is missing them), and only through
`lifecycle::scrub::scrub_prompt`, which strips UUIDs, phronesis sids, and
host transcript paths before the ordinary payload scrubber runs. The
journal never receives it, and no fact, context render, or stats line
carries it. Set `"lifecycle": { "prompt_text": "none" }` in
`.phronesis/journey.json` to suppress it; the switch is enforced at read
time too, so it also hides prompts already written, and it fails **closed**
— an unreadable or malformed block is treated as `"none"`.
`get_journey` never returns prompt text either way.

**Metrics.** Two bounded families, behind `--features metrics`:
`phronesis_lifecycle_events_total{host,event,mode}` and
`phronesis_subagent_duration_seconds{host}` (13 exponential buckets, 1 s to
~68 min). No kalpa label and no `agent_type` label — both are free text,
user-typed and model-supplied respectively.

### Sweeping the existing tree

The hook only sees diffs — it catches new violations as you write them,
but can't see what's already in the tree. `phr-mcp audit` (and the
`audit_codebase` MCP tool) does a whole-tree pass against opted-in rules,
reporting per-rule hit counts plus the affected files and line numbers.

```
phr-mcp audit                          # table summary
phr-mcp audit --rule no-unwrap-in-src  # expand to file:line detail
phr-mcp audit --json                   # machine-readable
phr-mcp audit --fail-on block          # exit 1 on any blocked violation (CI gate)
```

Rules opt in via `audit: true` on the disk rule. Diff-only rules and
LLM-deflection rules don't participate by default. The audit engine
honors `new_content_contains` predicates plus `file_path_matches` and
`file_extension_is` as gates, and evaluates registered AST predicates.
Builtin `__script__` guards using `facts_contain` or `facts_count` may inspect
fresh per-file `file_path`, path-component `file_path_matches`, and
`file_extension_is` facts. Audit does not support arbitrary Rhai or binding
variables in scripts; unsupported guards are skipped with a diagnostic rather
than silently producing a clean result.

A rule's `phase` field is consulted by the hook (which only loads rules
whose phase equals `"pre"` or `"post"`). Setting `phase: "audit"` on a
rule makes it **audit-only**: the hook silently skips it at edit time,
but `phr-mcp audit` still surfaces it (assuming `audit: true` is
also set). Use this for patterns where every-edit warnings would be
disruptive but a periodic debt sweep is valuable — e.g. manual
`=> return Err(...)` arms that `?` could replace.

Each audit run writes a `kind:"mcp" event:"audit_codebase"` snapshot to
`.phronesis/log.jsonl`. `phr-mcp trend` (and the `get_debt_trend`
MCP tool) reads those back and reports per-rule counts across snapshots
with a delta column — useful for confirming that a cleanup sweep
actually shrank the pile.

```
phr-mcp trend                # last 5 snapshots, table
phr-mcp trend --since 30d    # all snapshots in the last month
phr-mcp trend --rule no-unwrap-in-src
```

## Rule file format (v2)

Rules are stored in `.phronesis/rules.json`. The current (v2) shape uses readable
`when`/`then`/predicate-as-key syntax. Both v1 and v2 files are parsed on load;
only v2 is written. Existing v1 files continue to work — run `migrate-rules` to
convert them.

### Layered rule loading

Projects can opt into layered rules with `.phronesis/loader.json`. Layers are
processed in declaration order. When two layers contain the same rule ID, the
later definition wins. Without `loader.json`, Phronesis retains the legacy
behavior and reads only `.phronesis/rules.json`.

```json
{
  "version": 1,
  "layers": [
    { "name": "project", "path": ".phronesis/rules.json" },
    {
      "name": "team",
      "path": ".phronesis/team-rules.json",
      "decision": ".phronesis/wiki/decisions/ADR-team-policy.md",
      "optional": true
    },
    {
      "name": "personal",
      "path": "~/.config/phronesis/rules.json",
      "decision": "~/.config/phronesis/decisions/ADR-personal-policy.md",
      "optional": true
    }
  ]
}
```

Paths relative to the project root, absolute paths, and `~/` paths are
supported. `optional: true` skips an absent layer; malformed files still fail
closed. An override asserts this working-memory fact:

```text
rule_overridden(rule_id, old_layer, old_path, new_layer, new_path, decision)
```

The fact's source is `rule_layers`. A third override records another fact, so
the full resolution chain remains visible. The winning layer's `decision`
supplies the ADR provenance (or an empty final argument when omitted).

MCP autosave remains project-scoped: it never copies winning team or personal
rules into `.phronesis/rules.json`.

Override facts are ordinary RETE facts, so rules can govern the layering
process itself. This example fires once `policy` has been replaced by three
successive layers:

```json
{
  "id": "too-many-policy-overrides",
  "phase": "pre",
  "priority": 20,
  "when": [
    {
      "__script__": "facts_count('rule_overridden', ['policy','*','*','*','*','*']) >= 3"
    }
  ],
  "then": { "warn": "policy has been overridden at least three times" }
}
```

### Condition shape — `when`

Each element of `when` is a single-key object: `{ "<predicate>": <arg> }`.

- **String** — one argument: `{ "new_content_contains": ".unwrap()" }`
- **Array** — two or more arguments: `{ "function_param_count_high": ["?file", "?fn", "?count"] }`
- **`true`** — zero arguments (predicate has no parameters): `{ "some_flag_predicate": true }`
- **`__script__`** — inline Rhai expression: `{ "__script__": "rank > 5" }`

### Action shape — `then`

`then` is a single-key object mapping an action verb to its message string:

| Verb | Internal action type |
|------|---------------------|
| `block` | `constraint_violation` — hook exits 2, Claude sees the message |
| `warn` | `constraint_warning` — hook exits 1, advisory |
| `log` | `log` — recorded in the log, not surfaced to the model |

Any other verb is passed through as its own `action_type` for forward compatibility.

### Full example

```json
{
  "id": "enforce-no-unwrap-in-src",
  "phase": "pre",
  "priority": 10,
  "audit": true,
  "when": [
    { "rust_governed_invocation": ["?file", "?fn", "unwrap"] },
    { "file_path_matches": "src" }
  ],
  "then": { "block": "Avoid .unwrap() in src/ — use ? for error propagation, or expect() with a clear message if truly unreachable." }
}
```

### `or` operator

An `or` clause inside `when` expresses disjunction:

```json
{ "or": [ { "new_content_contains": "cargo test" }, { "new_content_contains": "cargo nextest" } ] }
```

At load time, `read()` expands `or` into separate OR-free rules using disjunctive
normal form (DNF). A rule with an OR fires if **any** branch matches. Expanded
rules get deterministic ids like `<base-id>#or0`, `<base-id>#or1`, etc. Multiple
OR positions produce a cartesian product of variants.

`not` is **not** supported yet — planned for a later release.

### `migrate-rules`

Converts a v1 rules.json (using `conditions`/`actions`/`predicate`/`action_type` keys)
to v2 in place. Preserves `or` clauses on disk without expanding them.

```
phr-mcp migrate-rules <path>            # convert in place (backs up to .bak)
phr-mcp migrate-rules --dry-run <path>  # print converted JSON to stdout; write nothing
phr-mcp migrate-rules --check <path>    # exit 0 if already v2, exit 1 if needs migration (no writes; CI gate)
```

Idempotent — running on an already-v2 file re-writes it in canonical form (no loss).

### `migrate-extracted-rules`

Salvages pre-0.14.0 `extract_rules` output in place (with a `.bak` backup).
Detected by the `markdown_rule` condition — hand-written rules are never touched.

```
phr-mcp migrate-extracted-rules <path>            # rewrite in place (backs up to .bak)
phr-mcp migrate-extracted-rules --dry-run <path>  # print what would change to stdout; write nothing
```

Three transformations applied to every extracted rule:
- Strip bracketed extraction-time prefixes (`[pattern]`, `[anti_pattern]`, `[context]`,
  `[problem]`, `[directive]`) from the message.
- Demote `block` actions to `warn`.
- Demote to `log` any rule whose message matches a structural Rust-pack keyword
  (`unwrap`, `clone`, `Deref`, `&String`, `&Vec`, `thiserror`). Rust
  panic/debug invocations are emitted as `rust_governed_invocation` facts so
  comments and strings do not trigger those starter rules.

Idempotent.

## Development

```
cargo fmt --all
cargo clippy --workspace -- -D warnings
cargo test
```

## Versioning

Semver, automated by **release-plz** (see `docs/RELEASING.md`). Version
bumps, git tags, and crates.io publishing are no longer done by hand:
release-plz watches conventional commits on `main`, opens/updates a
Release PR (the human approval gate), and merging that PR publishes all
three crates via crates.io trusted publishing and tags `vX.Y.Z`.

We are pre-1.0, so:
- **MINOR** (`0.X.0`) — new features (new subcommand, new pack, new hook
  surface, anything user-visible). Bump and reset PATCH to 0.
- **PATCH** (`0.X.Y`) — bug fixes, internal refactors, doc-only changes,
  rule pack tweaks that don't add a new rule kind.
- **MAJOR** (`1.0.0`) — reserved for the first "I'd recommend this to
  someone else" milestone.

**Still manual** (release-plz does NOT do these):
- **CHANGELOG.md entry** — hand-written in (or before) the release PR;
  `changelog_update = false`, the Keep a Changelog format with Migration
  sections stays human-curated.
- **Catalogue regeneration** — if the release touched any pack rules,
  run `phr-mcp catalogue` from the repo root and commit the regenerated
  `docs/catalogue.html` before merging the release PR — the page is a
  generated artifact and drifts otherwise.
- **Local reinstall** — after each release, `cargo install --path
  crates/phronesis-mcp` so the user-level binary (the one hooks invoke)
  matches. `phr-mcp --version` prints the installed version — that's how
  you check whether a project's hooks are running fresh code.
- **Conventional-commit PR titles** — squash merges make the PR title
  the commit message release-plz parses, so PR titles must be
  conventional-commit shaped (`feat:`, `fix:`, `chore:`, ...).

## Coding Standards

Follow patterns in `docs/RUST-PATTERNS-GUIDE.md`. Key points:
- Use `?` for error propagation, not manual match
- Prefer `&str` over `&String`, `&[T]` over `&Vec<T>`
- Use `thiserror`/`anyhow` for errors, not raw strings where possible
- Avoid unnecessary `.clone()` — work with references
- No `.unwrap()` in production paths

## Architecture

- `src/main.rs` — CLI entry point (clap). Dispatches one `handle_<variant>` fn per subcommand: `serve`, `pre-check`, `post-check`, `session-context`, `interaction-context` (legacy alias: `turn-context`), `stats`, `confidence`, `journey`, `audit`, `trend`, `drift`, `claude-md-drift`, `migrate-rules`, `migrate-extracted-rules`, `memory-drift`, `wiki-drift`, `decision`, `init` (aliases: `setup`, `configure`), `install`, `uninstall`, `codex-hook`, `claude-hook`, `kalpa`, `unit`, `coverage` (import, select).
- `src/server.rs` — `EpistemeMcp` with MCP tools via rmcp macros (rules, facts, fire/agenda, predicate-provider create/read/test/list/remove, graph query/status/rebuild, get_stats, audit_codebase, get_debt_trend, get_drift, get_confidence, submit_suggestion — which takes optional `spec` / `bug_id` and records a `unit_start` through `lifecycle::unit_cli::start` — and get_journey, whose optional `include_lifecycle` (default `false`) switches the bare fact array for `{"facts": [...], "lifecycle": [...]}`; prompt text is never included)
- `src/wiki.rs` — Page primitives: Decision struct, YAML-frontmatter parser, `walk_decisions` iterator. Shared by wiki_drift and future wiki-consuming modules.
- `src/wiki_drift.rs` — Drift extractor: scores decisions vs rules.json, surfaces `Uncovered` ones; `enforces:` frontmatter shortcut beats Jaccard.
- `src/clock_facts.rs` — Local-clock-derived facts (`business_hours_local`, `weekday_local`, `hour_local`) asserted at every hook invocation; lets rules condition on the wall clock.
- `src/memory_drift.rs` — Walks the Claude Code auto-memory directory, classifies entries by `metadata.type`, and scores them against rules.json + durable.md.
- `src/hook/{mod,pre,post,journey_record,seq,lifecycle_wiring}.rs` — Pre/post hook subcommands; reads `.phronesis/rules.json`, fires rules, exits 0/1/2. Split: `pre`/`post` are the hook runners, `journey_record` stamps the journey journal, `seq` sequences the pre-check pipeline, `lifecycle_wiring` is the one place pre and post share for pushing/popping the in-flight entry, deriving Gemini's `invoke_agent` sub-agent pair, and recording commits.
- `src/claude_hook.rs` — `phr-mcp claude-hook <Event>`, the Claude Code and Gemini CLI lifecycle adapter. Tool phases delegate to `pre-check`/`post-check` unchanged. Non-tool events fail open: a stdin read or parse failure prints `{}` and exits 0, because a `UserPromptSubmit` hook exiting 2 would discard the human's prompt. A *blocked* `Stop`/`SubagentStop` is not a stop — the turn stays open and the record is written when the turn really ends.
- `src/lifecycle/` — Agent lifecycle events (SPEC-agent-lifecycle-events). `event.rs` is the single place that decides both on-disk shapes (`LifecycleEvent::to_journal_record` / `to_log_entry`, with a closed `extra` vocabulary that never carries message or transcript content); `state.rs` holds the locked correlation files and `classify_prompt`; `record.rs` is the only writer (bump seq, append journal, append log, update state; failures are swallowed onto stderr with event names and error kinds only); `outcome.rs` is `detect_commit`; `scrub.rs` is `scrub_prompt`; `inflight.rs` pairs a pre-check with its post-check; `kalpa_cli.rs`, `unit_cli.rs`, and `unit_report.rs` back `phr-mcp kalpa`, `phr-mcp unit start|end`, and `phr-mcp unit show`.
- `src/init.rs` — `phr-mcp init` one-command project setup
- `src/context.rs` — Formatters for SessionStart / UserPromptSubmit hook payloads (active-rules summary, recent-activity summary) plus legacy renderers for projects initialized before context support became the default
- `src/context/` — Token-aware context (SPEC-token-aware-durable-context).
  `render.rs` builds one side-effect-free `RenderResult` that the live hook
  path, `context inspect`, and the metrics all project from — which is why
  inspect reports exactly what the hook would emit and writes nothing.
  `packing.rs` is the deterministic packer (kind ceilings, shared byte and
  soft token budgets, omission reasons); `config.rs` parses
  `.phronesis/context.json`; `capsule.rs` loads and compiles nudge capsules
  into ordinary RETE rules; `metrics.rs` writes the bounded
  `kind:"context"` observations and aggregates them for `context stats`.
- `src/stats.rs` — Aggregates `.phronesis/log.jsonl` per rule and renders as table or JSON
- `src/audit.rs` — Whole-tree rule audit + debt-over-time aggregation. Provides `run` (file scan), `render_table/json`, `compute_trend` (reads `audit_codebase` snapshots), `render_trend_table/json`, `resolve_scan_root` (shared by MCP and CLI), `audit_snapshot_entry` (shared log-snapshot builder).
- `src/action_log.rs` — Append-only `.jsonl` log of hook decisions and MCP events
- `src/rules_file.rs` — Disk format for rules.json: v2 `SourceRule`/`WhenClause` types, v1+v2 deserialization, `unfold_or` (DNF expansion), `read`/`write_atomic`/`read_source`/`write_source`
- `src/security.rs` — Path canonicalization, size caps, input validators
- `src/diff_extract.rs` — Regex-based diff facts (function_added, import_added, etc.)
- `src/syntax/` — Tree-sitter AST predicates for Rust, Swift, Python, and TypeScript (e.g. function_returns_result_string, rust_governed_invocation, rust_governed_attribute, rust_trait_impl, rust_governed_match_arm, rust_primitive_id_field, rust_rc_refcell_type, swift_governed_construct, python_bare_except, ts_explicit_any, ts_console_log_call). The Rust analyzer is split across `src/syntax/rust/{mod,walk,derives,counts,signatures,docs,assertions,eval,hazards,invocations,attributes,impls,match_arms,types}.rs`.
- `src/outcomes/` — Confidence scoring (SPEC-confidence-scoring). Grounded
  `build_outcome`/`test_outcome`/`bug_check_outcome` facts behind declarative
  toolchain defs (built-in cargo def + `.phronesis/toolchains.json`, one
  generic parse engine in `outcomes/toolchain.rs`), reading per-subject
  history from the journey journal (keyed by subject), and `signal_pass`
  derivation. The hook captures outcomes at post-check (stamping `subject` +
  `outcome:*` tags on the journal record) and asserts signals at pre-check so
  gate rules (`facts_count('signal_pass', ['*','*']) <op> N`) block/warn a
  `git commit` by confidence band. Opt-in via
  `.phronesis/confidence.json`; known bugs in `.phronesis/bugs.json`;
  report via `phr-mcp confidence`.
- `src/journey/` — Journey facts (SPEC-journey-facts). Durable per-call
  journal at `.phronesis/journey/events.jsonl` plus project-defined taggers
  in `.phronesis/journey.json`; derivation recomputes `journey_*`
  aggregator facts from a bounded suffix of the journal every pre- and
  post-check, so rules can match cross-call temporal patterns (auth
  churn over a session, recent SQL in the last 5 calls, build staleness)
  without any in-memory accumulation. The outcomes storage layer above
  also lives here — folded in at 0.13.0 so there's one storage seam.
  Surface: `phr-mcp journey [--json] [--explain <rule-id>]` and the
  `get_journey` MCP tool.
- `docs/RUST-PATTERNS-GUIDE.md` — Rust coding standards (source for `extract_rules`)
- `docs/PATTERNS-WORKFLOW.md` — End-user workflow guide

### Emitted capsule lifecycle

`then.emit_capsule` is structured action data, never JSON embedded in an action
parameter. Its `id` is static; only string leaves such as `body` receive RETE
binding substitution. Interaction rendering packs emitted IDs as `emitted:<id>`
beside static `nudge:<id>` items. SessionStart never renders emitted capsules.
For `next_interaction`, selection leases rather than consumes the record;
acknowledgement removes it, and an unacknowledged five-minute lease retries.
Use `phr-mcp context list|acknowledge|retract` or the corresponding MCP tools.

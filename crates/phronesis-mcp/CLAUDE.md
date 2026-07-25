# phronesis-mcp

MCP server wrapping the phronesis RETE rules engine for rules-bounded LLM interaction. Builds the `phr-mcp` binary and uses the `phr` library (the `phronesis` crate, imported under that alias).

## Build & Run

```
cargo build
cargo run -- serve       # MCP stdio server (default)
cargo run -- pre-check   # PreToolUse hook (blocks violations)
cargo run -- post-check  # PostToolUse hook (warns on violations)
cargo run -- codex-hook PreToolUse  # Codex protocol adapter (event varies by hook)
cargo run -- init             # One-command setup for a project
cargo run -- session-context  # SessionStart hook (injects active rules + durable directives)
cargo run -- turn-context     # UserPromptSubmit / BeforeAgent hook (injects recent activity + durable directives)
cargo run -- stats             # Read-only per-rule summary of .phronesis/log.jsonl
cargo run -- audit            # Whole-tree audit of rule violations (CI-friendly: --fail-on block)
cargo run -- trend            # Debt-over-time view comparing audit snapshots
cargo run -- confidence       # Confidence band + grounded signals for the open work unit
cargo run -- toolchains        # List active toolchain defs (built-in + project); --json for machine output
cargo run -- journey   # what journey_* facts assert right now
cargo run -- claude-md-drift  # Heuristic: which CLAUDE.md imperatives lack a matching rule?
cargo run -- memory-drift     # Heuristic: which auto-memory entries lack a matching rule or durable.md paragraph?
cargo run -- wiki-drift      # Heuristic: which .phronesis/wiki/decisions/ ADRs lack rule coverage?
cargo run -- decision new <slug>  # Scaffold a new ADR page at .phronesis/wiki/decisions/<today>-<slug>.md
cargo run -- migrate-rules <path>  # Convert a rules.json from the old (v1) shape to the v2 shape
cargo run -- migrate-extracted-rules <path>  # Salvage pre-0.14.0 extract_rules output: strip prefixes, demote actions
cargo run -- catalogue        # Regenerate docs/catalogue.html from the shipped packs (run from repo root)
cargo run -- scrub-payload <path> [--write] [--home DIR] [--project-root DIR]  # Anonymize captured payloads for committing as fixtures
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
journal_tag_new, journal_tag_from_output, stderr_contains). All current
fixtures are `provenance: "authored"` — hand-written approximations of
the real envelopes, to be superseded by real captures.

The same test file consumes `tests/fixtures/hook_events.json`, the
hook-event-name registry: `init_wires_hooks_only_under_event_names_that_exist`
checks every event name `init` wires (Claude Code and Gemini) against the
registry, and `before_model_request_never_reappears` pins the 0.17.1
`BeforeModelRequest` incident by name. New host events must be added to
the registry in the same PR that adds wiring.

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

### Drift detection — CLAUDE.md and auto-memory ↔ rules

Two CLI tools (also exposed as MCP tools, so the model can invoke
them in conversation) surface the gap between prose guidance and
enforced rules:

`phr-mcp claude-md-drift` (MCP: `get_claude_md_drift`) extracts
imperative bullets from CLAUDE.md ("Don't X", "Always Y", "Prefer Z")
and matches each one against the current rule pack by token overlap.
Output flags bullets with no confident match — candidates that
either should become rules or should be marked as "non-lintable by
design" so future audits don't re-flag them.

`phr-mcp memory-drift` (MCP: `get_memory_drift`) walks Claude Code's
per-project auto-memory directory (default
`~/.claude/projects/<encoded-cwd>/memory/`), parses the YAML
frontmatter on each entry, and classifies it into one of three
buckets per `docs/specs/SPEC-memory-to-rules.md`: `actionable`
(should become a rule), `ambient` (should be in durable.md), or
`personal` (stays in MEMORY.md). Non-personal entries are scored
against rules.json and durable.md by token overlap; uncovered ones
are surfaced for porting. `--suggest` emits draft rule JSON on
stderr.

`phr-mcp wiki-drift` (MCP: `get_wiki_drift`) walks
`.phronesis/wiki/decisions/`, parses ADR-style frontmatter on each
page, and classifies decisions into `covered` / `likely-covered` /
`uncovered` / `superseded` against the current rule pack. Explicit
`enforces: [rule-id]` frontmatter beats the Jaccard fallback —
authors who list which rules enforce a decision get a deterministic
match. `--suggest` emits draft v2 rule JSON on stderr for uncovered
decisions. Pair with `phr-mcp decision new <slug>` to scaffold new
ADR pages from a template.

Both tools are heuristic (no LLM call) — output is a triage list,
not ground truth.

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

## Setting up a project

For hooks and project-specific rules, in any project:

```
phr-mcp init                          # just the LLM-behavior pack (deflection rules)
phr-mcp init --packs llm,rust         # LLM rules + Rust enforcement
phr-mcp init --packs rust             # Rust enforcement only, no deflection
phr-mcp init --packs llm,rust,rhai    # Rust + Rhai for embedded-scripting projects
phr-mcp init --packs llm,python       # LLM rules + Python enforcement
phr-mcp init --packs llm,typescript   # LLM rules + TS/JS enforcement
phr-mcp init --packs none             # no starter rules; you add them
phr-mcp init --dry-run                # preview without writing
phr-mcp init --force                  # overwrite existing rules.json (backs up to .bak)
phr-mcp init --hooks-only             # refresh hook wiring without touching rules.json
```

`setup` and `configure` are aliases for `init` if those feel more natural.

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
  `Result<_, String>` returns,
  `.execute_all_agenda_items().await` / `.fire_all_consequences().await`
  on now-sync methods, `#![deny(warnings)]` (breaks on toolchain
  upgrade). Warns: public fn taking `&String`, `&Vec<T>`, or
  `&Box<T>`, functions with 3+ `.clone()` calls, functions with 5+
  parameters, `impl Deref for` (Deref polymorphism anti-pattern),
  `#[test]` functions with no assertions or `?` operator,
  `cargo build/test/check/clippy` without `--workspace`, `dbg!()` in
  src/, `.expect("")` with an empty message in src/. Audit-only
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
  intermediate temporaries into a block). The rust-unofficial/patterns
  book is the upstream source for
  the borrow-types, deny-warnings, and string-concat rules; the
  Rc/RefCell rule is a more general Rust-idiom observation.
- `rhai` — discipline for projects that embed the Rhai scripting
  language. Blocks: `engine.eval(<string literal>)` in Rust source
  (precompile to AST via `compile_file` / `eval_ast` instead), and
  `print(` in `.rhai` scripts (use the host-registered output channel,
  whatever your `Engine` exposes via `register_fn`). Generic messages
  — layer project-specific guidance into your own `.phronesis/rules.json`.
- `python` — bare-except blocked, `print()` warning, mutable-default-arg
  warning, plus tree-sitter AST audits (bare-`except:` by enclosing
  function, public `def`s missing a docstring)
- `typescript` — `: any` and `console.log` warnings, plus tree-sitter AST
  rules (explicit `any`, `@ts-ignore`/`@ts-expect-error`/`@ts-nocheck`
  suppressions, non-null `!` assertions)
- `swift` — Swift-specific advisories: force-unwrap warning, try! warning
- `confidence` — opt-in confidence-band gate (SPEC-confidence-scoring).
  Writes `.phronesis/confidence.json` (the opt-in marker) and ships three
  commit-gate rules: low confidence blocks `git commit -m`, medium warns,
  high passes clean. Pair with `.phronesis/bugs.json` (known-bug
  registry) and `phr-mcp confidence` for the report surface. Also scaffolds
  `.phronesis/toolchains.json` (pytest/tsc example defs). Confidence signals
  are toolchain-neutral: any command matched by a toolchain def grounds a
  `build_outcome` from its exit code (`command_exit`, captured on every shell
  journal record), with optional per-toolchain regex refinement for test
  counts and per-test results. Cargo ships as a built-in def; project defs
  in `toolchains.json` extend or override it. (One behavior refinement
  vs. pre-0.18: a build/test command that exits non-zero with no test
  summary and no compile-error text is now graded build-fail when the CLI
  supplies the exit code.) Journal growth is bounded by
  write-side compaction (`PHRONESIS_MAX_JOURNAL_BYTES`, default 16 MiB) that
  preserves each subject's latest grounded outcome.
- `journey` — project-defined taggers + journey_* aggregator facts (cross-call temporal predicates)
- `none` — empty rules array (hooks still wired)

`init` writes/merges seven files:
- `.claude/settings.local.json` — hook config (preserves existing permissions/hooks)
- `.mcp.json` — MCP server registration
- `.phronesis/rules.json` — starter rule pack (left alone on re-run unless --force)
- `.phronesis/durable.md` — default re-injected directives, including drift-discipline nudges that point the model at `get_claude_md_drift` / `get_memory_drift` / `get_wiki_drift`. Left alone on re-run; edit in place to customize.
- `.phronesis/wiki/decisions/README.md` — wiki scaffold; the directory is un-ignored from the broad `.phronesis/` gitignore. Left alone on re-run.
- `.gemini/settings.json` — MCP server registration + BeforeTool/AfterTool hooks for Gemini CLI
- `.gitignore` — log/backup paths + `!.phronesis/wiki/**` exception so the decisions tree is versioned

Re-running is idempotent: existing config is preserved; only our entries are added.

### Refreshing just the rules pack

When you've added new predicates upstream and want to pull the latest rule
pack into an existing project — without touching its hook config, MCP
registration, or gitignore — use `--rules-only`:

```
phr-mcp init --rules-only --force --packs llm,rust
```

This writes only `.phronesis/rules.json` (with a `.bak` of the prior version
since `--force` is set). Everything else in the project stays exactly as it
was — custom permissions, custom hooks, custom gitignore entries, none of
it touched. Useful when you've changed `--packs` or just want to pick up
a new rule that ships in `llm` after upgrading the binary.

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
`file_extension_is` as gates; rules using AST predicates are deferred to
a follow-up.

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
    { "new_content_contains": ".unwrap()" },
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
  (`unwrap`, `clone`, `Deref`, `&String`, `&Vec`, `thiserror`).

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

- `src/main.rs` — CLI entry point (clap). Dispatches one `handle_<variant>` fn per subcommand: `serve`, `pre-check`, `post-check`, `session-context`, `turn-context`, `stats`, `confidence`, `journey`, `audit`, `trend`, `claude-md-drift` (alias: `drift`), `migrate-rules`, `migrate-extracted-rules`, `memory-drift`, `wiki-drift`, `decision`, `init` (aliases: `setup`, `configure`), `install`, `uninstall`.
- `src/server.rs` — `EpistemeMcp` with MCP tools via rmcp macros (rules, facts, fire/agenda, get_stats, audit_codebase, get_debt_trend, get_claude_md_drift, get_memory_drift, get_wiki_drift, get_confidence, submit_suggestion, get_journey)
- `src/wiki.rs` — Page primitives: Decision struct, YAML-frontmatter parser, `walk_decisions` iterator. Shared by wiki_drift and future wiki-consuming modules.
- `src/wiki_drift.rs` — Drift extractor: scores decisions vs rules.json, surfaces `Uncovered` ones; `enforces:` frontmatter shortcut beats Jaccard.
- `src/clock_facts.rs` — Local-clock-derived facts (`business_hours_local`, `weekday_local`, `hour_local`) asserted at every hook invocation; lets rules condition on the wall clock.
- `src/memory_drift.rs` — Walks the Claude Code auto-memory directory, classifies entries by `metadata.type`, and scores them against rules.json + durable.md.
- `src/hook/{mod,pre,post,journey_record,seq}.rs` — Pre/post hook subcommands; reads `.phronesis/rules.json`, fires rules, exits 0/1/2. Split: `pre`/`post` are the hook runners, `journey_record` stamps the journey journal, `seq` sequences the pre-check pipeline.
- `src/init.rs` — `phr-mcp init` one-command project setup
- `src/context.rs` — Formatters for SessionStart / UserPromptSubmit hook payloads (active-rules summary, recent-activity summary)
- `src/stats.rs` — Aggregates `.phronesis/log.jsonl` per rule and renders as table or JSON
- `src/audit.rs` — Whole-tree rule audit + debt-over-time aggregation. Provides `run` (file scan), `render_table/json`, `compute_trend` (reads `audit_codebase` snapshots), `render_trend_table/json`, `resolve_scan_root` (shared by MCP and CLI), `audit_snapshot_entry` (shared log-snapshot builder).
- `src/action_log.rs` — Append-only `.jsonl` log of hook decisions and MCP events
- `src/rules_file.rs` — Disk format for rules.json: v2 `SourceRule`/`WhenClause` types, v1+v2 deserialization, `unfold_or` (DNF expansion), `read`/`write_atomic`/`read_source`/`write_source`
- `src/security.rs` — Path canonicalization, size caps, input validators
- `src/diff_extract.rs` — Regex-based diff facts (function_added, import_added, etc.)
- `src/syntax/` — Tree-sitter AST predicates for Rust, Swift, Python, and TypeScript (e.g. function_returns_result_string, python_bare_except, ts_explicit_any). The Rust analyzer is split across `src/syntax/rust/{mod,walk,derives,counts,signatures,docs,assertions,eval}.rs`.
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

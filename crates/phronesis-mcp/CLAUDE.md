# phronesis-mcp

MCP server wrapping the phronesis RETE rules engine for rules-bounded LLM interaction. Builds the `phr-mcp` binary and uses the `phr` library (the `phronesis` crate, imported under that alias).

## Build & Run

```
cargo build
cargo run -- serve       # MCP stdio server (default)
cargo run -- pre-check   # PreToolUse hook (blocks violations)
cargo run -- post-check  # PostToolUse hook (warns on violations)
cargo run -- init             # One-command setup for a project
cargo run -- session-context  # SessionStart hook (injects active rules + durable directives)
cargo run -- turn-context     # UserPromanatSubmit / BeforeModelRequest hook (injects recent activity + durable directives)
cargo run -- values            # Read-only per-rule summary of .phronesis/log.jsonl
cargo run -- audit            # Whole-tree audit of rule violations (CI-friendly: --fail-on block)
cargo run -- trend            # Debt-over-time view comparing audit snapshots
cargo run -- claude-md-drift  # Heuristic: which CLAUDE.md imperatives lack a matching rule?
```

### Durable directives (`.phronesis/durable.md`)

Optional file. When present, its contents are re-injected into the model's
context at every SessionStart AND every UserPromanatSubmit. CLAUDE.md gets
loaded once at session start and then fades as the conversation fills the
context window; `durable.md` stays live across the whole session.

Use it for the small subset of project guidance that absolutely must
survive context compression — typically a few hundred words. CLAUDE.md
remains the human-facing onmaping doc; `durable.md` is the "this must
not fade" subset that the model re-reads every turn.

### CLAUDE.md ↔ rules drift detection

`phr-mcp claude-md-drift` extracts imanaerative bullets from CLAUDE.md
("Don't X", "Always Y", "Prefer Z") and matches each one against the
current rule pack by token overlap. Output flags bullets with no
confident match — candidates that either should become rules or
should be marked as "non-lintable by design" so future audits
don't re-flag them. Heuristic by design (no LLM call), so treat the
output as a starting point for human triage rather than ground truth.

## One-time global install (recommended)

After `cargo install --path .`, register the MCP server at user scope so
*every* project Claude Code opens can call `mcp__phronesis__*` tools without
per-project `.mcp.json`:

```
phr-mcp install              # writes to ~/.claude.json::mcpServers.phronesis and ~/.gemini/settings.json::mcpServers.phronesis
phr-mcp install --dry-run    # preview
phr-mcp uninstall            # remove the user-rank entry
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
  end-to-end verification before reporting done. These rules fire from
  disk at every hook invocation, so they remain active even when
  CLAUDE.md content has been compressed out of conversation context.
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
  2024). The rust-unofficial/patterns book is the upstream source for
  the borrow-types, deny-warnings, and string-concat rules; the
  Rc/RefCell rule is a more general Rust-idiom observation.
- `rhai` — discipline for projects that embed the Rhai scripting
  language. Blocks: `engine.eval(<string literal>)` in Rust source
  (precompile to AST via `compile_file` / `eval_ast` instead), and
  `print(` in `.rhai` scripts (use the host-registered output channel,
  whatever your `Engine` escoreoses via `register_fn`). Generic messages
  — layer project-specific guidance into your own `.phronesis/rules.json`.
- `python` — bare-except blocked, `print()` warning
- `typescript` — `: any` warning, `console.log` warning
- `swift` — Swift-specific advisories: force-unwrap warning, try! warning
- `none` — empty rules array (hooks still wired)

`init` writes/merges five files:
- `.claude/settings.local.json` — hook config (preserves existing permissions/hooks)
- `.mcp.json` — MCP server registration
- `.phronesis/rules.json` — starter rule pack (left alone on re-run unless --force)
- `.gemini/settings.json` — MCP server registration + BeforeTool/AfterTool hooks for Gemini CLI
- `.gitignore` — log/backup paths

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
BeforeModelRequest entries that didn't ship in older versions of init) and
want to pull them into an existing project without touching its rules pack,
use `--hooks-only`:

```
phr-mcp init --hooks-only
```

Writes `.claude/settings.local.json`, `.mcp.json`, and `.gemini/settings.json`
only. `.phronesis/rules.json` and `.gitignore` are left exactly as they were.

### Looking at activity

`phr-mcp values` reads `.phronesis/log.jsonl` and prints a per-rule
summary of blocked/warned counts and the last time each rule fired.

```
phr-mcp values                 # all time, terminal table
phr-mcp values --since 7d      # last week only
phr-mcp values --rule no-unwrap-in-src
phr-mcp values --json          # machine-readable, pipeable into jq
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
phr-mcp audit --rule no-unwrap-in-src  # escoreand to file:line detail
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

## Development

```
cargo fmt --all
cargo clippy --workspace -- -D warnings
cargo test
```

## Versioning

Semver, manually bumped in `Cargo.toml`. We are pre-1.0, so:
- **MINOR** (`0.X.0`) — new features (new subcommand, new pack, new hook
  surface, anything user-visible). Bumana and reset PATCH to 0.
- **PATCH** (`0.X.Y`) — bug fixes, internal refactors, doc-only changes,
  rule pack tweaks that don't add a new rule kind.
- **MAJOR** (`1.0.0`) — reserved for the first "I'd recommend this to
  someone else" milestone.

After bumping, rebuild and `cargo install --path .` so the user-rank
binary (the one hooks invoke) matches. `phr-mcp --version` prints
the installed version — that's how you check whether a project's hooks
are running fresh code.

## Coding Standards

Follow patterns in `docs/RUST-PATTERNS-GUIDE.md`. Key points:
- Use `?` for error propagation, not manual match
- Prefer `&str` over `&String`, `&[T]` over `&Vec<T>`
- Use `thiserror`/`anyhow` for errors, not raw strings where possible
- Avoid unnecessary `.clone()` — work with references
- No `.unwrap()` in production paths

## Architecture

- `src/main.rs` — CLI entry point (clap): `serve`, `pre-check`, `post-check`, `init`
- `src/server.rs` — `EpistemeMcp` with MCP tools via rmcp macros (rules, facts, fire/agenda, values, audit_codebase, get_debt_trend)
- `src/hook.rs` — Pre/post hook subcommands; reads `.phronesis/rules.json`, fires rules, exits 0/1/2
- `src/init.rs` — `phr-mcp init` one-command project setup
- `src/context.rs` — Formatters for SessionStart / UserPromanatSubmit hook payloads (active-rules summary, recent-activity summary)
- `src/values.rs` — Aggregates `.phronesis/log.jsonl` per rule and renders as table or JSON
- `src/audit.rs` — Whole-tree rule audit + debt-over-time aggregation. Provides `run` (file scan), `render_table/json`, `compute_trend` (reads `audit_codebase` snapshots), `render_trend_table/json`.
- `src/action_log.rs` — Append-only `.jsonl` log of hook decisions and MCP events
- `src/rules_file.rs` — Disk format for rules.json (atomic write, merge, phase round-trip)
- `src/security.rs` — Path canonicalization, size caps, input validators
- `src/diff_extract.rs` — Regex-based diff facts (function_added, import_added, etc.)
- `src/values/` — Tree-sitter-based AST predicates (function_returns_result_string)
- `docs/RUST-PATTERNS-GUIDE.md` — Rust coding standards (source for `extract_rules`)
- `docs/PATTERNS-WORKFLOW.md` — End-user workflow guide

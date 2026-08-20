# phronesis-mcp

MCP server and CLI (`phr-mcp`) wrapping the
[phronesis](https://crates.io/crates/phronesis) RETE rules engine for
durable enforcement of project conventions in LLM-assisted work.

Rules live on disk in `.phronesis/rules.json`, fire from outside the
context window at every tool call, and cannot be compressed away.

## Quick start

```sh
# Install the binary
cargo install phronesis-mcp

# Register globally for Claude Code + Gemini CLI.
# Codex receives project-scoped MCP registration during init.
phr-mcp install

# Initialize in your project
cd /your/project
phr-mcp init --packs rust
```

## What it does

- **Pre/post hooks** fire rules against every file edit, blocking violations
  and warning on anti-patterns.
- **MCP tools** let the model query rules, fire the engine, audit the tree,
  detect drift between prose guidance and enforced rules, and test/manage
  sandboxed project predicate providers.
- **Graph recovery over MCP** lets shell-less agents inspect freshness with
  `get_code_graph_status` and safely rebuild derived graph and binding state
  with `rebuild_code_graph`; neither tool accepts an arbitrary path.
- **Stable MCP envelopes** keep collection results compatible across SDKs:
  `list_rules` returns `{"rules": [...]}`, `list_facts` returns
  `{"facts": [...]}`, `get_agenda` returns `{"agenda": [...]}`,
  `get_consequences` returns `{"consequences": [...]}`, and
  `get_action_log` returns `{"entries": [...]}` in both structured and text
  output.
- **Extensible predicates** — Rhai providers derive project vocabulary from
  normalized events. Codex `apply_patch` supplies a batch `files` change set
  before the existing per-file `file_path` evaluation.
- **Starter packs** ship rules for Rust (including async/unsafe hazards),
  Python (including import-time I/O, identity comparisons, mutable globals,
  and star imports), TypeScript, Swift, Rhai, and LLM behavior.
- **Journey facts** — durable per-call journal + project-defined taggers
  in `.phronesis/journey.json` let rules match cross-call temporal
  patterns (`auth-churn-without-tests`, `build-staleness`, recent SQL)
  without any in-memory accumulation.
- **Confidence scoring** — declarative toolchain definitions (built-in
  Cargo plus project definitions in `.phronesis/toolchains.json`) feed one
  generic parser that turns build / test / known-bug outcomes into grounded
  signals; gate rules block or warn a `git commit` by confidence band.
  Enabled by default through `.phronesis/confidence.json`.
- **Structural graph and graph rules, compact durable context, journey facts,
  confidence, and LLM rules are defaults.** Every language-neutral capability
  belongs to `base`; language packs are additive. `--packs none` is the
  only way to initialize without the default platform.
- **Rule staleness** records conservative bindings from unqualified
  function-call patterns such as `legacy_call(` to graph definitions. If every
  bound definition later disappears, a blocking rule warns instead until it
  is reviewed. Prose, attributes, method/namespace calls, and rules with
  `"binds": false` never bind.
- **Wiki decisions** — ADR-style pages in `.phronesis/wiki/decisions/`
  travel with the repo and are scored against rules for coverage.

## Commands

```
phr-mcp serve             # MCP stdio server
phr-mcp pre-check         # PreToolUse hook
phr-mcp post-check        # PostToolUse hook
phr-mcp codex-hook        # Codex lifecycle adapter (JSON stdin/stdout)
phr-mcp init              # One-command project setup
phr-mcp audit             # Whole-tree rule sweep
phr-mcp stats             # Per-rule activity summary
phr-mcp journey           # journey_* facts asserted right now
phr-mcp confidence        # Confidence band + grounded signals
phr-mcp drift             # Guidance/rule gaps (`--source wiki`, memory, claude_md, code)
phr-mcp decision new <s>  # Scaffold an ADR page
```

## Codex setup

`phr-mcp init` merges project hooks into `.codex/hooks.json` and registers the
stdio MCP server in `.codex/config.toml`, preserving unrelated settings. It
wires `Bash` and `apply_patch` pre/post events plus session, prompt, compaction,
and subagent context events. Re-running is idempotent; `--dry-run` writes
nothing, and `--hooks-only` refreshes integration files without touching rules.

Codex deliberately does not trust new or changed project hooks automatically.
Open `/hooks`, inspect the exact commands, and trust them explicitly. Phronesis
hooks are deterministic guardrails, not a complete security boundary: some
specialized or hosted tool paths may not traverse local lifecycle hooks.

Codex observes unified shell execution as `Bash`, file edits as `apply_patch`,
and can expose other local or MCP tools. Phronesis v0.26 governs only `Bash` and
`apply_patch`; unsupported tools are safe no-ops.

## Rule-emitted context capsules

Rules may use an object-valued `emit_capsule` action with a static `id`, a
binding-substituted `body`, and a `next_interaction`, `session`, or `persistent`
lifecycle. Emitted records are stored in `.phronesis/emitted-capsules.json` and
are merged with static `.phronesis/nudges/*.md` only for interaction context.
`next_interaction` delivery is at least once: selection creates a five-minute
lease, and the host acknowledges successful delivery with
`phr-mcp context acknowledge <id> [--lease-token <token>]` (or the matching MCP
tool). Failed delivery retries after lease expiry. `context inspect` is
read-only.

Storage permits at most 128 records, 8 KiB per final body/record, and 256 KiB
aggregate. Only 96 records may have priority below 50; the remaining 32 slots
are reserved for governance capsules at priority 50 or above. Persistence is
advisory: an invalid, conflicting, or unwritable capsule is diagnosed but does
not change a hook's allow/block result. IDs use an `emitted:` packing namespace,
distinct from static `nudge:` IDs.

Manage emitted records with `phr-mcp context list [--json]`, `context
acknowledge`, and `context retract`; equivalent MCP tools are
`list_emitted_capsules`, `acknowledge_emitted_capsule`, and
`retract_emitted_capsule`.

## Documentation

- [The Explainer](https://awaterma.github.io/phronesis/explainer.html) — how the engine works
- [The Catalogue](https://awaterma.github.io/phronesis/catalogue.html) — starter rules reference
- [Command Reference](https://github.com/awaterma/phronesis/blob/main/crates/phronesis-mcp/CLAUDE.md) — full CLI surface

## License

MIT

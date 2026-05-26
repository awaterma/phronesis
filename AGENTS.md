# Agents.md - Phronesis Codebase Guide

## Quick Overview

This is a Rust workspace with two crates:

- **`phronesis`** - Core RETE rules engine library
- **`phronesis-mcp`** - MCP server for LLM-agent governance (CLAUDE.md hook integration)

**Project type:** Rust workspace with MCP (Model Context Protocol) integration for AI agent governance.

---

## Essential Commands

### Build & Test
```bash
# Build entire workspace
cargo build --workspace

# Run all tests
cargo test --workspace

# Run clippy with warnings as errors
cargo clippy --workspace -- -D warnings

# Format code
cargo fmt --all
```

### Development Server
```bash
# Start MCP server (stdio)
cargo run -- -p phronesis-mcp serve

# Pre-tool-use hook (blocks violations)
cargo run -- -p phronesis-mcp pre-check

# Post-tool-use hook (warns on violations)
cargo run -- -p phronesis-mcp post-check
```

### CLI Commands (after cargo install)
```bash
# Project setup
phr-mcp init                    # LLM rules only
phr-mcp init --packs llm,rust   # LLM + Rust rules
phr-mcp init --packs none       # No starter rules

# Configuration refresh
phr-mcp init --rules-only --force --packs llm,rust   # Update rules only
phr-mcp init --hooks-only                                            # Update hooks only

# Activity inspection
phr-mcp values                    # Per-rule summary from log
phr-mcp values --since 7d
phr-mcp values --rule no-unwrap-in-src
phr-mcp values --json

phr-mcp audit                    # Whole-tree rule audit
phr-mcp audit --fail-on block    # Exit 1 on blocked violations

phr-mcp trend                    # Debt-over-time view
phr-mcp trend --rule no-unwrap-in-src

# Global MCP server registration
phr-mcp install                  # Register globally (user scope)
phr-mcp uninstall
```

### Development Examples
```bash
# Profile assert_fact performance
cargo run --example profile_assert_fact -p phronesis

# Profile audit performance
cargo run --example profile_audit -p phronesis-mcp

# Run specific test
cargo test -p phronesis -- rete_smoke
cargo test -p phronesis-mcp -- action_log
```

---

## Architecture Overview

### RETE Engine (`phronesis` crate)

The core is a **domain-neutral RETE rules engine** based on the Forgy algorithm (1982). Key concepts:

- **Facts** → asserted into working memory
- **Rules** → fire when facts match conditions
- **Consequences** → emitted by firing rules (Event, Snapshot, Constraint, Affordance)
- **Provenance** → tracks rule firing or lookup source

#### Two Transport Modes

| Mode | Description | Use Case |
|------|-------------|----------|
| **Push** | Rules fire, consequences emitted to Actor | Long-running sessions where LLM acts as narrator |
| **Pull** | Actor asks, deterministic Lookup returns consequence | Ad-hoc queries, hook-time validation |

See `crates/phronesis/src/pull.rs` and `crates/phronesis/src/push.rs`.

#### Core Types
```rust
// Consequence - what consumers actually see
pub struct Consequence {
    pub kind: ConsequenceKind,      // Event/Snapshot/Constraint/Affordance
    pub predicate: String,
    pub payload: serde_json::Value,
    pub provenance: Provenance,     // RuleFiring/Lookup/Asserted
}

// Actor - consumes consequences
#[async_trait]
pub trait Actor: Send + Sync {
    async fn act(&self, consequences: &[Consequence]) -> anyhow::Result<ActorOutput>;
}

// Lookup - deterministic queries
pub trait Lookup {
    type Request;
    type Response: Serialize;
    fn name(&self) -> &'static str;
    fn schema_version(&self) -> u8;
    fn invoke(&self, req: Self::Request) -> anyhow::Result<Self::Response>;
}
```

#### RETE Network Structure

- **Alpha Network** - indexed by predicate key, stores WMEs (Working Memory Elements)
- **Beta Network** - stores tokens (partial matches), join states for multi-condition rules
- **Production Network** - stores rules, fires when complete matches occur
- **Agenda** - activated rules waiting to fire

See `crates/phronesis/src/{alpha,beta,production,network}.rs`.

---

### MCP Server (`phronesis-mcp` crate)

#### CLI Commands

| Command | Purpose |
|---------|---------|
| `serve` | Start MCP stdio server (default) |
| `pre-check` | Block edits that violate rules |
| `post-check` | Warn about edits that violate rules |
| `session-context` | Inject active rules summary on SessionStart |
| `turn-context` | Inject recent hook activity on UserPromptSubmit |
| `values` | Read `.phronesis/log.jsonl`, show per-rule summary |
| `audit` | Scan whole tree for rule violations |
| `trend` | Debt-over-time from audit snapshots |
| `claude-md-drift` | Find CLAUDE.md imperatives without matching rules |
| `init` | One-command project setup (hooks + rules) |
| `install` | Register MCP server at user scope |

#### Hook Behavior

**`pre-check`**: Blocks edits with violations → exit 2
**`post-check`**: Warns about violations → exit 1
**`session-context`**: Emits JSON with active rules + durable directives
**`turn-context`**: Emits JSON with last N hook decisions

#### Rules File (`.phronesis/rules.json`)

```json
{
  "rules": [
    {
      "id": "no-unwrap-in-src",
      "phase": "pre",
      "priority": 10,
      "audit": true,
      "conditions": [
        {
          "predicate": "new_content_contains",
          "args": ["\\.unwrap\\(\\)"]
        },
        {
          "predicate": "file_path_matches",
          "args": ["src/"]
        }
      ],
      "actions": [
        {
          "action_type": "constraint_violation",
          "params": ["Found .unwrap() in src/"]
        }
      ]
    }
  ]
}
```

**Rule Fields:**
- `id`: Unique identifier
- `phase`: `"pre"` (block), `"post"` (warn), `"audit"` (audit-only)
- `priority`: Higher fires first
- `audit`: Whether this rule participates in `phr-mcp audit`
- `silent`: Suppress console output (default false)
- `conditions`: Array of predicate+args
- `actions`: Array of action_type+params

#### Available Predicates

| Predicate | Description |
|-----------|-------------|
| `file_path_matches(?path)` | Path substring match |
| `file_extension_is(?ext)` | Extension check |
| `new_content_contains(?pattern)` | Regex substring in new content |
| `function_added(?file, ?name)` | Function introduced in diff |
| `function_removed(?file, ?name)` | Function removed in diff |
| `import_added(?file, ?target)` | Import introduced |
| `import_removed(?file, ?target)` | Import removed |
| `test_exists_for(?name)` | Test function exists |
| `no_test_for(?name)` | No test found |
| `function_returns_result_string(?file, ?fn)` | Rust AST: return type is `Result<_, String>` |

**Adding new predicates:** Write tree-sitter queries in `src/values/rust.rs`, assert facts in `assert_values_facts` in `src/hook.rs`.

#### Packaged Rules

| Pack | Contents |
|------|----------|
| `llm` | Deflection rules (blocks blame-shifting, unverified completion claims), `git commit -m` warning |
| `rust` | `.unwrap()`/`todo!()`/`panic!()`/`unimplemented!()` in src/, `Result<_, String>`, `.execute_all_agenda_items().await` on sync, `#![deny(warnings)]`, public fn with `&String`/`&Vec<T>`/`&Box<T>`, 3+ `.clone()` calls, 5+ params, `impl Deref`, `#[test]` without assertions, `cargo build/test/check/clippy` without `--workspace`, `dbg!()` in src/, `.expect("")` empty message, god-file (>800 LOC), manual `=> return Err(...)`, `*_id: String`/`u64`, `None => {}`/`Err(_) => {}`, `Rc<RefCell<...>>` in src/, string concatenation with `+ &`, `#[allow(dead_code)]`, `env::set_var` |
| `rhai` | `engine.eval(<string literal>)` (use `compile_file`), `print(` in `.rhai` scripts |
| `python` | Bare `except:`, `print()` warning |
| `typescript` | `: any` warning, `console.log` warning |
| `swift` | Force-unwrap warning, `try!` warning |

---

## Code Patterns & Conventions

### Rust Coding Standards

See `crates/phronesis-mcp/docs/RUST-PATTERNS-GUIDE.md`:

- Use `?` for error propagation (not manual match)
- Prefer `&str` over `&String`, `&[T]` over `&Vec<T>`
- Use `thiserror`/`anyhow` for errors
- Avoid unnecessary `.clone()` - work with references
- No `.unwrap()` in production paths
- Use `if let` for single pattern matching
- Prefer `Default` trait + builder pattern
- Use newtype pattern for type safety (`UserId(u64)`)
- Block pattern: `{ let x = ...; x }` to close scope/mutability

### Key File Roles

| File | Purpose |
|------|---------|
| `crates/phronesis/src/lib.rs` | Library root, re-exports |
| `crates/phronesis/src/network.rs` | `ReteNetwork` struct + methods (RETE engine core) |
| `crates/phronesis/src/alpha_network.rs` | Alpha network, predicate-indexed WME storage |
| `crates/phronesis/src/beta_network.rs` | Beta network, tokens, join states |
| `crates/phronesis/src/production.rs` | Production network, rule storage, agenda |
| `crates/phronesis/src/actor.rs` | `Actor` trait, `ActorOutput` |
| `crates/phronesis/src/consequence.rs` | `Consequence`, `ConsequenceKind`, `Provenance` |
| `crates/phronesis/src/pull.rs` | `Lookup`, `DynLookup` for deterministic queries |
| `crates/phronesis/src/push.rs` | Push transport (rule firing → Actor) |
| `crates/phronesis/src/wme.rs` | `WorkingMemoryElement`, `WmeManager` |
| `crates/phronesis/src/variable_binding.rs` | Variable substitution in actions |
| `crates/phronesis-mcp/src/lib.rs` | Library root, module exports |
| `crates/phronesis-mcp/src/server.rs` | `EpistemeMcp` struct, MCP tools (rmcp macros) |
| `crates/phronesis-mcp/src/hook.rs` | `pre-check`/`post-check` hooks, rule evaluation |
| `crates/phronesis-mcp/src/init.rs` | `phr-mcp init` project setup |
| `crates/phronesis-mcp/src/context.rs` | SessionStart/BeforeModelRequest payload formatters |
| `crates/phronesis-mcp/src/values.rs` | Aggregate log entries per rule |
| `crates/phronesis-mcp/src/audit.rs` | Whole-tree audit + debt-over-time |
| `crates/phronesis-mcp/src/action_log.rs` | Append-only JSONL log |
| `crates/phronesis-mcp/src/rules_file.rs` | Disk format for `.phronesis/rules.json` |
| `crates/phronesis-mcp/src/security.rs` | Path canonicalization, size caps, validators |

### Action Log (`.phronesis/log.jsonl`)

**Entry format:**
```json
{
  "ts": 1715717111,
  "kind": "hook",
  "event": "pre_check",
  "phase": "pre",
  "tool": "Edit",
  "file": "src/foo.rs",
  "exit": 2,
  "violations": ["Avoid .unwrap() in src/"],
  "rules_fired": ["constraint_violation"]
}
```

**Entry kinds:**
- `kind: "hook"` - Hook invocation (pre_check, post_check)
- `kind: "mcp"` - MCP tool call (add_rule, remove_rule, fire_rules, audit_codebase)

**Read-only tools:**
- `get_action_log` - Filter entries by kind/event/since
- `phr-mcp values` - Per-rule summary
- `phr-mcp trend` - Debt-over-time

**Rotation:** At 50 MB, renamed to `log.jsonl.1`. Maximum 100 MB per project.

### Durable Directives (`.phronesis/durable.md`)

Optional file. Contents are re-injected at every `SessionStart` AND `BeforeModelRequest`. Use for project guidance that **must not fade** from context window (typically a few hundred words).

---

## Important Gotchas & Non-Obvious Patterns

### 1. Rule Phase Behavior

| Phase | Hook Behavior | Audit Behavior |
|-------|---------------|----------------|
| `"pre"` | Blocks edits (exit 2) | Included by default |
| `"post"` | Warns (exit 1) | Included by default |
| `"audit"` | Silently skipped at hook time | Only shown by `phr-mcp audit` |
| `"none"` | Never fires | Not in audit |

### 2. Hook vs Server Interaction

- **Hook** (`phr-mcp pre-check`) reads `.phronesis/rules.json` from disk and fires rules in a fresh network
- **Server** (`phr-mcp serve`) maintains in-memory network, hydrates from disk on startup
- **Autosave** on every `add_rule`/`extract_rules`/`remove_rule` keeps disk and memory in sync
- **Autoload** on server startup keeps session in sync with disk

### 3. Context Injection Hooks

**SessionStart** (Claude) / **ConfigUpdate** (Gemini):
- Injects active rules summary + durable directives
- No context about recent activity (fresh session)

**UserPromptSubmit** (Claude) / **BeforeModelRequest** (Gemini):
- Injects last N hook decisions + durable directives
- Helps LLM understand what's been blocked/warned recently

### 4. Pattern-Guide Rules (`extract_rules`)

Rules extracted from markdown (e.g., `docs/RUST-PATTERNS-GUIDE.md`) use:

```json
{
  "predicate": "markdown_rule",
  "args": ["docs/RUST-PATTERNS-GUIDE.md", "Error Handling"]
}
```

Must be paired with `set_section_context` before `fire_rules` to activate section-specific reminders.

### 5. Performance Gotchas

Current known hot paths (from May 2026 audit):

| Issue | Status | Impact |
|-------|--------|--------|
| `aho-corasick` for audit match loop | Proposed | 3-10× on audit for large repos |
| `Vec<Arc<WME>>` in beta tokens | Proposed | Reduces clone cost at high preload |
| Inner mutexes in `ReteNetwork` | Verified overhead | Microsecond range per call |
| `.ok()` error swallowing in bindings | Tech debt | Rare binding conflicts |

**Performance guidance:** Profile with `profile_assert_fact.rs` and `profile_audit.rs` before optimizing. `criterion` + `tokio::block_on` gives misleading numbers for routines <10µs.

### 6. God-File Exemptions

Three files exceed 800 LOC with intentional exemptions (see `SPEC-god-file-decomposition.md`):

| File | Why Exempt | Decomposition Plan |
|------|-----------|-------------------|
| `server.rs` (~1110 LOC) | `rmcp` macro requires single `#[tool]` impl block | Delegation pattern: thin wrappers in server.rs, bodies in `server_handlers/` modules |
| `network.rs` (~817 LOC) | `ReteNetwork` is single coherent engine surface | Split `impl` blocks across `network/rules.rs`, `network/facts.rs`, `network/firing.rs`, `network/script.rs` |
| `audit.rs` (~817 LOC) | Single cohesive audit engine + types + render + trend | Split into `audit/types.rs`, `audit/engine.rs`, `audit/render.rs`, `audit/trend.rs` |

**Note:** `init.rs` at 2281 LOC has **no exemption** - highest priority for decomposition.

### 7. Rule Persistence Model

**Autopersist** (default): Every `add_rule`/`extract_rules`/`remove_rule` writes to disk atomically. Hooks see changes within milliseconds.

**Disable:** Set `PHRONESIS_NO_AUTOPERSIST=1` in server environment (hooks still read from disk).

**Manual control:**
- `save_rules { "dry_run": true }` - Preview without writing
- `save_rules { "merge": false }` - Replace (autosave does same)
- `load_rules_file` - Hot-reload after external edit

### 8. Security Constraints

- Path canonicalization prevents directory traversal
- Size caps on files read (configurable via `PHRONESIS_LOG_MAX_BYTES`)
- Input validation in `security.rs`
- `.phronesisignore` support in `phronesis-mcp/.phronesisignore`

---

## Workflow Patterns

### Common Agent Workflows

#### 1. Adding a New Rule

```bash
# 1. Add rule to .phronesis/rules.json (or use MCP tool)
# 2. Test with hook
cargo run -- -p phronesis-mcp pre-check  # or post-check

# 3. If rule fires, check log
cat .phronesis/log.jsonl | jq 'select(.rules_fired | contains(["my-new-rule"]))'

# 4. Add test case if needed (see crates/phronesis-mcp/tests/)
```

#### 2. Auditing Existing Code

```bash
# Scan project
phr-mcp audit

# Expand specific rule
phr-mcp audit --rule no-unwrap-in-src

# Fail CI on violations
phr-mcp audit --fail-on block

# Track debt over time
phr-mcp trend
phr-mcp trend --rule no-unwrap-in-src
```

#### 3. Setting Up a New Project

```bash
# 1. Install binary (one-time)
cargo install --path crates/phronesis-mcp

# 2. Register globally (one-time per machine)
phr-mcp install

# 3. Initialize project (per-project)
cd /my/project
phr-mcp init --packs llm,rust

# 4. Restart Claude Code / Gemini CLI in this project
```

#### 4. Extracting Rules from Documentation

```bash
# 1. Extract rules from markdown (one-time per session)
extract_rules { "file_path": "docs/RUST-PATTERNS-GUIDE.md" }

# 2. Before editing in a section, declare context
set_section_context { "file": "docs/RUST-PATTERNS-GUIDE.md", "section": "Error Handling" }

# 3. Fire rules to see reminders
fire_rules

# 4. Move to next section
set_section_context { "file": "docs/RUST-PATTERNS-GUIDE.md", "section": "API Design" }
```

### CI/CD Integration

```bash
# Fail build on blocking violations
phr-mcp audit --fail-on block

# Check for CLAUDE.md drift (heuristic, never fails)
phr-mcp claude-md-drift
```

---

## Testing Approach

### Test Structure

```
crates/phronesis/tests/          # Core engine tests
├── rete_smoke.rs               # Basic RETE functionality
├── push_smoke.rs               # Push transport (actor consumption)
├── pull_smoke.rs               # Pull transport (lookup queries)
├── compose_smoke.rs            # Composed lookups
└── types_smoke.rs              # Type serialization round-trips

crates/phronesis-mcp/tests/     # MCP server tests
├── features/                   # BDD-style feature tests
│   ├── facts_management.feature
│   ├── hooks.feature
│   ├── markdown_extraction.feature
│   ├── rule_firing.feature
│   └── rules_management.feature
├── action_log_integration.rs   # Log file operations
├── hook_integration.rs         # Hook behavior
├── init_integration.rs         # Project initialization
├── save_rules_integration.rs   # Rule persistence
├── section_context_integration.rs # Section context flow
├── values_integration.rs       # AST predicates
├── extraction.rs               # Markdown extraction
└── security_tests.rs           # Security edge cases
```

### Writing Tests

**Unit tests:** Use `cargo test -p phronesis` or `cargo test -p phronesis-mcp`

**BDD tests:** Add to `crates/phronesis-mcp/tests/features/*.feature`

**Integration tests:** Add to `crates/phronesis-mcp/tests/*_integration.rs`

### Benchmarking

```bash
# Run criterion benches
cargo bench -p phronesis

# Profile specific scenarios
cargo run --example profile_assert_fact -p phronesis
cargo run --example profile_audit -p phronesis-mcp
```

---

## Documentation

### Primary Sources

| Document | Purpose |
|----------|---------|
| `README.md` | Project overview, quick start |
| `crates/phronesis-mcp/CLAUDE.md` | CLI reference, hook details, pack descriptions |
| `crates/phronesis-mcp/docs/PATTERNS-WORKFLOW.md` | Working with patterns-guide rules |
| `crates/phronesis-mcp/docs/RUST-PATTERNS-GUIDE.md` | Rust coding standards (source for `extract_rules`) |
| `docs/specs/SPEC-god-file-decomposition.md` | File size exemptions and decomposition plan |
| `docs/specs/SPEC-next-round-perf.md` | Performance items, verified vs. false leads |
| `crates/phronesis/README.md` | Core engine overview |

### Generated Documentation

GitHub Pages: https://awaterma.github.io/phronesis/

- **Explainer** - Technical essay on RETE algorithm and design intent
- **Catalogue** - Visual reference of starter rules with rationale
- **Command Reference** - CLI surface and hook wiring details

---

## Versioning & Releases

**Current version:** `0.6.1` (workspace-wide)

**Semver (pre-1.0):**
- **MINOR** (`0.X.0`) - New features (subcommand, pack, hook surface, user-visible)
- **PATCH** (`0.X.Y`) - Bug fixes, internal refactors, doc-only, rule pack tweaks
- **MAJOR** (`1.0.0`) - First "production ready" release

**After bumping:**
```bash
# Update Cargo.toml versions
# Rebuild and reinstall
cargo install --path crates/phronesis-mcp

# Verify installed version
phr-mcp --version
```

---

## MCP Tools Reference

The `phr-mcp serve` command exposes these tools:

| Tool | Purpose | Return Type |
|------|---------|-------------|
| `add_rule` | Add a rule to RETE network | `CallToolResult` |
| `list_rules` | List all rules | `{ "rules": [...] }` |
| `get_rule` | Get single rule by ID | `{ "rule": {...} }` |
| `remove_rule` | Remove rule by ID | `CallToolResult` |
| `extract_rules` | Extract rules from markdown | `CallToolResult` |
| `save_rules` | Persist in-memory rules to disk | `CallToolResult` |
| `load_rules_file` | Load rules from disk | `CallToolResult` |
| `assert_fact` | Assert fact into working memory | `CallToolResult` |
| `retract_fact` | Retract fact by ID | `CallToolResult` |
| `list_facts` | List all facts | `{ "facts": [...] }` |
| `get_fact` | Get single fact by ID | `{ "fact": {...} }` |
| `fire_rules` | Fire all pending rules | `{ "actions_fired": N, "consequences": [...] }` |
| `check_constraints` | Check active constraint consequences | `{ "constraint_violations": [...] }` |
| `get_consequences` | Get accumulated consequences | `{ "consequences": [...] }` |
| `get_agenda` | Get pending agenda items | `{ "agenda": [...] }` |
| `clear_consequences` | Clear accumulated consequences | `{ "success": true }` |
| `set_section_context` | Set markdown section context | `{ "success": true }` |
| `clear_section_context` | Clear section context | `{ "success": true }` |
| `get_action_log` | Read action log entries | `[ {...}, ... ]` |
| `get_values` | Get per-rule statistics | `{ "rules": [...] }` |
| `audit_codebase` | Scan project for violations | `{ "per_rule": [...] }` |
| `get_debt_trend` | Get debt-over-time from audit snapshots | `{ "trend": [...] }` |

---

## Known Limitations

1. **No TMS (Truth Maintenance System)** - Derived facts don't automatically retract when premises change. Design choice: current use case (hook-time validation) doesn't need it.

2. **String allocation** - Predicates and IDs use `String` instead of `SmolStr`/interning. Premature optimization until malloc-profile shows it's dominant.

3. **Single-threaded** - No `tokio::spawn`/`thread::spawn` against engine. The `Arc<Mutex<ReteNetwork>>` is a single mutex protecting everything.

4. **Beta token copying** - `Token.wmes: Vec<WME>` uses deep copy. Could switch to `Vec<Arc<WME>>` for large preloads.

5. **Binding `.ok()` swallowing** - Four sites silently drop binding conflicts. Benign in current workloads, but could cause subtle bugs with complex rules.

---

## Contributing

### Before Committing

```bash
# 1. Run tests
cargo test --workspace

# 2. Run clippy
cargo clippy --workspace -- -D warnings

# 3. Format code
cargo fmt --all
```

### Code Review Checklist

- [ ] Tests pass (`cargo test --workspace`)
- [ ] Clippy passes (`cargo clippy --workspace -- -D warnings`)
- [ ] Code formatted (`cargo fmt --all`)
- [ ] Performance profiled if changing hot paths (`profile_assert_fact.rs`, `profile_audit.rs`)
- [ ] Documentation updated (README, CLAUDE.md, AGENTS.md)
- [ ] Version bumped if user-visible change (see Versioning section)

### Breaking Changes

If a change breaks the wire format (JSON schema for MCP tools, disk format for rules.json), increment MINOR version and document in release notes.

---

## Quick Reference

### Common Agent Tasks

| Task | Command |
|------|---------|
| Build | `cargo build --workspace` |
| Test | `cargo test --workspace` |
| Lint | `cargo clippy --workspace -- -D warnings` |
| Format | `cargo fmt --all` |
| Profile assert | `cargo run --example profile_assert_fact -p phronesis` |
| Profile audit | `cargo run --example profile_audit -p phronesis-mcp` |
| Run bench | `cargo bench -p phronesis` |
| CLI help | `phr-mcp --help` |
| Project init | `phr-mcp init --packs llm,rust` |
| Audit | `phr-mcp audit --fail-on block` |
| Values | `phr-mcp values --rule no-unwrap-in-src` |
| Trend | `phr-mcp trend --rule no-unwrap-in-src` |

### MCP Tools (when server is running)

| Tool | Use |
|------|-----|
| `add_rule` | Add new enforcement rule |
| `extract_rules` | Extract rules from markdown guide |
| `assert_fact` | Inject fact for rule evaluation |
| `fire_rules` | Trigger rule evaluation |
| `get_action_log` | Review hook decisions |
| `audit_codebase` | Scan project for violations |
| `get_values` | Per-rule activity summary |

---

*Last updated: 2026-05-25*
*Based on phronesis v0.6.1*

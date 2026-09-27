# Handover: swarm governance, Crush/OpenCode hosts, self-properties

**From:** a cloud Claude Code session (no access to your Mac)
**For:** your local Claude Code, running in your `phronesis` checkout, which can also reach `/Volumes/Data/agent-skills`
**Repo state when written:** `main` at `d015c3a` (#102), with PR #77 (`chore: release v0.36.0`) the only open PR.

Start by pulling `main`. There are three workstreams, in the order below. Workstream 3 is the largest, so write its spec first and review it before any code.

---

## Background (why this exists)

- `findings.md` (repo root) showed that the coverage-evidence swarm (`.scratch/swarm_report/`) left **no Phronesis trace**: `phr-mcp init` was never run in the worker worktrees, so there were no rules, no log, no journey records and no confidence data. `audit --fail-on block` exited 0 only because no rules existed.
- Since #81, a root counts as governed only when `.phronesis/rules.json` or `.phronesis/loader.json` exists. Hooks write nothing in an ungoverned root. A worktree without `init` is silently ungoverned.
- The swarm workers ran on **Crush** and **OpenCode**. Phronesis only has adapters for Claude Code, Gemini CLI (`claude-hook`) and Codex (`codex-hook`). `docs/specs/SPEC-agent-lifecycle-events.md:85` lists "Crush and other hosts without a hook protocol" as out of scope. **That statement is now out of date:** both hosts have hooks (see Workstream 3).

---

## Workstream 1: swarming skill (`/Volumes/Data/agent-skills`)

Find the swarm skill (the one that drives `swarmctl` and worker worktrees). Make these changes:

1. **Govern every worker worktree before spawning.** In each worktree, run:
   ```bash
   phr-mcp init --packs llm,<lang>,<lang>...
   ```
   - Use commas with **no spaces**, or quote the list. `parse_packs` trims spaces, but an unquoted `llm, rust` splits into two shell arguments.
   - `llm` is already in the default base (`BASE_PACKS` = llm, confidence, journey, structural, context), so listing it does no harm.
   - Language packs that exist: `rust`, `rhai`, `python`, `python-patterns` (opt-in), `typescript`, `swift`, `lua`, `cue`, `json`, `yaml`, `helm3`. Pick the ones for the languages the project uses. For Phronesis itself: `--packs llm,rust,rhai`.
   - Run `init` with the host flags for the worker's harness. Once Workstream 3 lands, that means Crush and OpenCode too.
2. **Refuse to spawn into an ungoverned worktree.** After `init`, check that `.phronesis/rules.json` exists and that `phr-mcp audit` exits 0. If either check fails, fail the contract loudly. Never spawn silently.
3. **Gate at hand-off, regardless of harness.** Before the orchestrator accepts a worker's commits, run `phr-mcp audit --fail-on block` and `phr-mcp confidence` in the worker's worktree. Record the result in the contract and the swarm report. This is the backstop until every harness has hook support, and it still matters afterwards because OpenCode sub-agent hooks have had gaps (see below).
4. **Open a work unit per task** (`phr-mcp unit start --spec <spec>`) and optionally a kalpa per swarm run (`phr-mcp kalpa start <name>`). Then `stats` and `journey` can attribute each worker's activity.
5. Add a line to the swarm report template: "Governance trace: N hook decisions, M blocks, confidence band X." That way a missing trace shows up in the report.

---

## Workstream 2: stage 1 of self-properties (`.phronesis/properties.json`)

Goal: register real Phronesis invariants as properties, so the property ontology is used on Phronesis itself. **Add no code and no encodings in this stage.**

Facts about the current file and schema:
- Today the file holds only two `safe_divide` properties from the coverage test fixture (`crates/phronesis-mcp/tests/fixtures/coverage-sample/`). Keep them or move them, but they don't describe Phronesis.
- Schema: `crates/phronesis-mcp/src/properties/store.rs`.
  - Fields: `id`, `subject`, `kind`, optional `condition`/`guarantee`, `depends_on` (region ids), `source`, `status`, `corroborated_by`, `encodings`.
  - `source` must be one of: `explicit_spec`, `existing_verifier_contract`, `test_assertion`, `documentation`, `code_inference`, `runtime_observation`, `agent_inference`.
  - `status` must be one of: `observed`, `candidate`, `corroborated`, `accepted`, `verified`, `rejected`, `superseded`.
  - Identifier charset: `[A-Za-z0-9_:./-]`.
- **Use `status: "candidate"`, not `accepted`.** Promotion is a recorded human act via `set_property_status ... --because` (#98). An agent must not write `accepted`.
- **Leave `encodings` empty.** An encoding is a verifier artifact whose results must be bound (allowlisted SHA-256, commit, tier; see SPEC-property-ontology §2). Tests aren't encodings in that model. Put the test files in `corroborated_by` instead.
- Region ids in `depends_on` follow the grammar in `crates/phronesis-mcp/src/coverage/region_map.rs`: `fn:<repo-relative file>::<item-path>`, with the impl type as a segment. **Check each id against the region map** before committing, for example with a unit test that calls the region-map builder on the file. A mismatched id never joins `changed_region`.

Proposed properties (ids are suggestions):

| id | subject / depends_on | guarantee | corroborated_by |
|---|---|---|---|
| `rete.refraction_key_injective` | `fn:crates/phronesis/src/network.rs::ReteNetwork::retract_fact` (plus the firing path that inserts `ActivationKey`) | distinct (rule, facts) activations never share a refraction key; retraction clears exactly the keys using that fact (#80) | `crates/phronesis/tests/retraction_semantics.rs` |
| `journey.compaction_preserves_signals` | `fn:crates/phronesis-mcp/src/journey/journal.rs::compacted_content_with_cap` | every subject's derived confidence signals are identical before and after compaction (#82) | `journal.rs` test `compaction_preserves_signals_for_every_subject_on_random_journals` |
| `hook.codex_precheck_parity` | `fn:crates/phronesis-mcp/src/hook/mod.rs::build_rule_network` | Codex PreToolUse denies wherever `pre-check` blocks; PostToolUse warns wherever `post-check` warns (#83, #101) | `crates/phronesis-mcp/tests/codex_fail_closed_parity.rs` |
| `scrub.no_residual_home_paths` | `fn:crates/phronesis-mcp/src/payload_scrub.rs::Scrubber::scrub_value` | after scrubbing, no `$HOME` path remains in any spelling (spaces, escaped slashes) (#84) | `crates/phronesis-mcp/tests/scrub_payload_integration.rs` |
| `rules.malformed_never_allows` | `fn:crates/phronesis-mcp/src/rules_file.rs::read` | a rules file the engine cannot honor fails closed at load and never loads as "allow" (#91) | rules-load tests added in #91 |
| `coverage.identifier_charset` | `fn:crates/phronesis-mcp/src/coverage/store.rs::validate_identifier_field` | accepts exactly `[A-Za-z0-9_:./-]` | `crates/phronesis-mcp/verification/harness.rs` (Verus, proves a **mirror**, not the real fn) |

Use `kind: "invariant"`, except where `postcondition` reads better. Use `source: "test_assertion"` for the first five and `existing_verifier_contract` for the Verus one.

Validate the file by loading it through `properties::store::load_properties` in a test, or run any hook with a property rule loaded. A corrupt store reports `store_corrupt(properties, …)`. Then run `cargo test --workspace`, `cargo clippy --workspace -- -D warnings` and `cargo fmt --all`.

Next stages, for later:
- **Stage 2:** give `coverage.identifier_charset` a Verus encoding and make the harness prove the *real* functions, not copies.
- **Stage 3:** write the first Rhai render template (`verification/templates/verus-postcondition.rhai`, per SPEC-verification-artifact-generation). **No templates exist yet.**

---

## Workstream 3: Crush and OpenCode host adapters in Phronesis

Both hosts have hooks. Checked against upstream docs on 2026-09-27; **re-verify against the versions installed on your machine**:

### Crush (charmbracelet/crush): `docs/hooks/README.md`
- Config: `hooks.PreToolUse[]` in `crush.json` / `.crush.json` (project level overrides global), with entries `{name, matcher (regex on tool name), command, timeout}`.
- **Only `PreToolUse` exists.** There's no PostToolUse, prompt, session or sub-agent hook yet. A prompt-context hook is proposed in crush PR #3825.
- Stdin: `{"event":"PreToolUse","session_id","cwd","tool_name","tool_input"}`.
- Exit codes and output:
  - exit 0: stdout is JSON `{"version":1,"decision":"allow"|"deny"|null,"halt","reason","context","updated_input"}`
  - exit 2: block, with stderr as the reason
  - exit 49: halt the turn
  - any other exit: a non-blocking error, **so the tool proceeds**
- Env vars: `CRUSH=1`, `CRUSH_EVENT`, `CRUSH_TOOL_NAME`, `CRUSH_SESSION_ID`, `CRUSH_CWD`, `CRUSH_PROJECT_DIR`, `CRUSH_TOOL_INPUT_COMMAND`, `CRUSH_TOOL_INPUT_FILE_PATH`.

### OpenCode (sst/opencode, a.k.a. anomalyco/opencode): plugin API
- There are no shell hooks. Plugins are TS/JS files in `.opencode/plugin/` (project) or `~/.config/opencode/plugin/` (global).
- Hooks: `tool.execute.before` (throw an error to block), `tool.execute.after`, `event` (`session.created`, `session.idle`, `session.deleted`, `session.compacted`, `message.updated`, `file.edited`, …), `stop`, and `experimental.chat.system.transform` (inject context).
- Input shape: `input.tool`, `input.sessionID`, and `output.args` (mutable).
- Known gaps to test for: sub-agent (`task` tool) calls used to bypass `tool.execute.before` (issue #5894, since closed; confirm the fix is in your version), and parallel tool calls reporting only the first thrown error (issue #50331).

### Design notes for the spec (`docs/specs/SPEC-crush-opencode-hosts.md`)
- **Crush:** add `phr-mcp crush-hook PreToolUse`, modeled on `codex_hook.rs`.
  - Map Crush tool names and input fields (`bash` → shell, plus its edit/write/multiedit tools; get the exact names and `tool_input` fields from Crush's source) onto the normalized event that `pre-check` uses.
  - Build the network with `hook::build_rule_network`, so fail-closed parity holds.
  - **Fail closed explicitly.** Any internal error must exit 2 or print `decision: "deny"`, because Crush treats unknown exit codes as "proceed".
  - Post-check, lifecycle and commit detection aren't possible yet. Document that, and use the swarm hand-off gate (Workstream 1, step 3) instead.
- **OpenCode:** `phr-mcp init --opencode` writes `.opencode/plugin/phronesis.ts`, a thin shim that shells out to `phr-mcp opencode-hook <event>` with JSON on stdin.
  - Put all logic in Rust. The shim only marshals and throws.
  - `tool.execute.before` maps to pre-check (throw on deny), `tool.execute.after` to post-check, and `event` to lifecycle (session start/end, idle as turn stop, `task` tool as sub-agent start/stop).
  - The shim must throw if `phr-mcp` is missing or errors, so it fails closed.
- `init`: add host detection and flags for both hosts. Replace entries keyed on the command, as the other hosts do, so users' own hooks survive.
- Update `SPEC-agent-lifecycle-events.md:85`, `AGENTS.md` (the lifecycle-events section and the CLI table) and `crates/phronesis-mcp/CLAUDE.md`.
- Tests:
  - Captured real payloads from both hosts, via `PHRONESIS_CAPTURE_DIR` if you add capture.
  - A parity test in the style of `codex_fail_closed_parity.rs`, asserting that Crush and OpenCode deny wherever `pre-check` blocks.
  - A test that an internal error in the adapter denies.
- This is a `feat:` change (MINOR).

---

## Housekeeping noticed along the way (separate small PRs)

- The `CHANGELOG.md` `[Unreleased]` section has `Fixed` and `Upgrading` but **no `Added`**. The #75 (typed self-receiver resolution), #78 (per-file call-resolution accounting) and #79 (coverage evidence and property ontology, experimental) features are undocumented. Fix this before merging release PR #77.
- `findings.md` and `.scratch/swarm_report/` are in the repo root. Decide whether they belong in `docs/`.
- The `AGENTS.md` footer says "Based on phronesis v0.26.0", the file lists three crates when there are four (`phronesis-metrics`), and two workflows are both numbered "1.".
- Delete stray `.phronesis/` dirs locally, using the `find` command in the #81 CHANGELOG entry.

# Validation brief: durable capsules and layered rules

You are a read-only validation reviewer. Inspect the current branch and
working tree. Do not edit files, create markers, commit, switch branches, or
run destructive commands.

The feature has two connected parts:

1. Any matching rule may emit a structured `emit_capsule` consequence. Validate
   rule-file parsing, RETE binding substitution, core push consequences, MCP
   `fire_rules`, Claude hooks, Codex hooks, provenance, and persistence.
2. `.phronesis/loader.json` defines ordered repository/team/personal rule
   layers. Validate later-ID-wins precedence, `~/.phronesis` and
   `~/.config/phronesis` path expansion, `rule_overridden` facts with layer,
   path, and ADR provenance, rules governing override facts, and project-only
   autosave boundaries.

Check capsule lifecycle behavior:

- `next_interaction`, `session`, and `persistent` lifecycles;
- expiry, five-minute leases, acknowledgement, retry after lease expiry;
- same-rule upsert and cross-rule ID conflict;
- bounded body/provenance, atomic writes, symlink rejection, capacity limits,
  and concurrent writers;
- deterministic context packing and separate `emitted:` / `nudge:` IDs;
- MCP and CLI list/acknowledge/retract operations and idempotence.

Check both success and failure paths, including empty/non-empty cases and
budget-omitted capsules. Tests must exercise real entry points, not only
reconstructed expected values. Preserve and identify unrelated pre-existing
worktree changes; do not attribute them to this feature.

Run these checks and report their actual results:

```sh
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check
cargo test -p phronesis --test push_smoke
cargo test -p phronesis-mcp capsule --lib
cargo test -p phronesis-mcp --test hook_integration later_rule_layer_wins
cargo test -p phronesis-mcp --test hook_integration rules_can_govern_a_three_override_chain
cargo test -p phronesis-mcp --test hook_integration pre_hook_persists_emit_capsule
cargo test -p phronesis-mcp --test save_rules_integration layered_autosave
cargo test -p phronesis-mcp --test save_rules_integration mcp_fire_rules
```

Report concrete findings with file paths and test names. End with exactly one
of:

```text
VERDICT: PASS
```

or

```text
VERDICT: BLOCKERS
```

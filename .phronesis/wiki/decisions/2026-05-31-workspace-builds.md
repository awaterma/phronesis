---
id: workspace-builds
date: 2026-05-31
status: accepted
enforces:
  - warn-cargo-build-without-workspace
superseded_by: null
tags: [rust, cargo, workspace]
---

# Always build and test with --workspace

## Context

Cargo workspaces contain multiple crates. Running `cargo build`,
`cargo test`, `cargo check`, or `cargo clippy` without `--workspace`
operates on only the current crate (or the workspace default
members). In a multi-crate project like phronesis (library `phr` +
binary `phronesis-mcp`), this silently skips half the code.

LLMs routinely forget `--workspace` because the bare command
"works" — it just checks less than intended.

## Decision

Warn whenever `cargo build`, `cargo test`, `cargo check`, or
`cargo clippy` appears without `--workspace`. The model should use
the `--workspace` flag for all four commands.

## Enforcement

One pre-tool-use hook rule (`warn-cargo-build-without-workspace`)
matching the command patterns. Warning severity (exit 1).

## Consequences

- Cross-crate breakage is caught at edit time instead of CI.
- Slightly longer build times when only one crate changed, but
  incremental compilation makes this negligible.

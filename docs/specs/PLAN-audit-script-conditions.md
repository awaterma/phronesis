# Builtin file-scoped audit script implementation plan

**Spec:** `docs/specs/SPEC-audit-script-conditions.md`  
**Issue:** [#52](https://github.com/awaterma/phronesis/issues/52)  
**Target:** v0.31.x

## Goal and architecture

Fix false-clean audits by teaching the existing file scanner to evaluate the
core builtin script DSL against fresh path facts. Keep the change inside the
audit module plus its two presentation wrappers; do not change report or
snapshot schemas.

## Task 1 — Regression tests

- [x] Add a failing test with matching files under `commands/` and
  `integration/` and a `facts_contain('file_path_matches', ...)` exclusion.
- [x] Require only the unrelated file to be reported.
- [x] Cover reversed file order, multiple ANDed scripts, and a script-only
  whole-file rule.
- [x] Cover malformed, binding-dependent, and non-builtin diagnostics.

## Task 2 — Per-file builtin guards

- [x] Treat `__script__` as a supported deferred condition in the ordinary
  applicability gate.
- [x] Build repository-relative `file_path`, component
  `file_path_matches`, and lowercase `file_extension_is` facts per file.
- [x] Evaluate each script with `phr::BuiltinScriptEvaluator` and empty
  bindings before hit collection.
- [x] Short-circuit on false or error without changing hit multiplicity.
- [x] Keep facts local to one `scan_file_into_accum` call.

## Task 3 — Focused diagnostics

- [x] Add an audit preflight helper that returns one message per opted-in rule
  with malformed, binding-dependent, or non-builtin scripts.
- [x] Filter diagnostics consistently with `--rule`.
- [x] Print diagnostics to CLI stderr for both table and JSON modes.
- [x] Prepend diagnostics to MCP text responses.
- [x] Leave `AuditReport`, `render_json`, snapshots, trend parsing, and exit
  codes unchanged.

## Task 4 — Documentation and verification

- [x] Document builtin-only audit script support in
  `crates/phronesis-mcp/CLAUDE.md`.
- [x] Add a v0.31.x changelog entry when the patch release is cut; do not bump
  the workspace version on an unreleased implementation branch unless the
  release workflow requires it.
- [x] Run:

```bash
cargo test -p phronesis-mcp --lib audit::tests
cargo test -p phronesis-mcp
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check
```

## Review checklist

- [x] No fact or evaluator state crosses file boundaries.
- [x] The implementation reuses the core builtin evaluator; audit contains no
  parser for `facts_contain` or `facts_count`.
- [x] Unsupported scripts cannot silently make a rule look clean.
- [x] Non-script audit counts are unchanged.
- [x] No JSON or snapshot compatibility surface changed.

# Local developer state

This repository keeps its root `.phronesis/` directory local. Reviewed
presets live in [`config/phronesis/`](../../config/phronesis/), and public
architecture decisions live in [`docs/decisions/`](../decisions/).
Runtime journals, captures, caches, and customized rules are not published.

From the repository root, preview and provision local configuration:

```sh
cargo run -p phronesis-mcp -- provision --dry-run
cargo run -p phronesis-mcp -- provision
cargo run -p phronesis-mcp -- init --rules-only --packs llm,rust,rhai,typescript,confidence
```

`provision` copies missing regular files from `config/phronesis/` into
`.phronesis/`, leaves identical files alone, and reports differing files as
conflicts without overwriting them. It adopts the anchored `/.phronesis/`
ignore entry. `--dry-run` reports the same intended changes without writing.
Use `--path <project>` to select a different project explicitly.

Resolve reported conflicts by reviewing the preset and local file yourself;
re-running provisioning does not resolve them automatically. Symbolic links
in preset or destination paths are rejected, including dangling links.
Files outside the preset set, such as local journals, are left untouched.
Illustrative samples in [`examples/phronesis/`](../../examples/phronesis/)
are not provisioned. The CI audit uses the same explicit provisioning and
starter-pack commands, so a clean clone does not depend on local rules.

Ordinary `init` does not migrate another project's state policy. It preserves
an existing anchored whole-directory ignore. It scaffolds ADRs in
`docs/decisions/` only when that directory already exists; otherwise it keeps
the legacy `.phronesis/wiki/decisions/` location. Creating the canonical
directory alone does not change that project's ignore policy.

Decision discovery reads both locations. Canonical IDs take precedence over
legacy copies; differing copies produce diagnostics. Unique legacy decisions
remain visible. Duplicate IDs within one location are rejected by drift
commands and excluded from graph governance with diagnostics. Explicit
`--wiki-dir` overrides continue to select only the requested directory.
ADRs retain their existing IDs, dates, status vocabulary, and enforcement
links when moved.

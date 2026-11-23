# First governed session — disposable practice project

This folder is a throwaway mini-project used by the tutorial
[docs/tutorials/first-governed-session.md](../../../docs/tutorials/first-governed-session.md).
Copy it into a fresh `mktemp -d` scratch directory, govern the copy, walk
through one allowed edit and one blocked edit in Claude Code, then delete the
scratch directory.

Contents:

- `.phronesis/rules.json` — one deterministic teaching rule,
  `no-tutorial-blocked-marker`: an edit whose new content contains the
  literal plain-text marker `TUTORIAL-BLOCKED-MARKER` is blocked by the
  `pre-check` hook, on any file. It is a teaching fixture, not a real coding
  policy; no language packs, no extras. This exact rule set is what the
  tutorial's checker asserts.
- `src/visit.rs` — a small non-compiled playground file to edit.
- `notes.md` — the edit target; a plain line is allowed, a line with the
  marker is blocked.

The sibling script `check_tutorial.py` verifies the whole tutorial flow in
fresh temporary directories: fixtures, setup commands, hook-level
allowed/blocked evidence, failure handling for missing executables, malformed
and stale rule configurations, doctor's read-only behavior, doctor's exact
finding schema (including `host_trust` and `log_evidence` staying
`not_observed` even with forged or stale local log records), the disposable
`--probe` fixture contract, and doctor's response to malformed rules.

Doctor checks require the installed `phr-mcp` binary to implement the
`doctor` subcommand. When it does not, those checks **fail by default** — a
missing required feature is never turned green; pass
`--allow-missing-doctor` to explicitly downgrade them to SKIP on a legacy
binary. Feature availability of your own binary is authoritative — check
`phr-mcp doctor --help`.

Run it:

```sh
python3 examples/first-governed-session/check_tutorial.py
```

Useful flags: `--phr-mcp /path/to/phr-mcp` to point at a specific binary,
`--allow-missing-doctor` for the explicit legacy skip, `--keep` to keep the
temporary directories for inspection, and `--list` to show the individual
checks. The script uses only the Python 3 standard library.

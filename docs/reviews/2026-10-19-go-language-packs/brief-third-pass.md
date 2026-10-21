# Review brief, third pass: fixes for the second-pass findings

You are an independent reviewer. Review read-only. Do not edit, create, or
delete any file in the repository, and do not run git commands that change
state.

Branch `feat/go-language-packs`. Your second-pass review is
`docs/reviews/2026-10-19-go-language-packs/codex-review-fixes.md`; our triage
of it (section "Second pass" of `triage.md`) names the fix commit for each of
your seven findings: `b29272a`, `5485638`, `8bab8fe`, `1b0af39`, `58d0850`.
Read both first; `git show <commit>` each fix.

## Check, in priority order

1. **Does each fix close its finding?** Retry your own counterexamples:
   the project-local `./phr-mcp`, `--path` to another project, the clean
   summary with exit 1, the earlier pass surviving a failed run, the local
   `type Work func()` conversion, the ancestor swap during `pack remove`,
   and the oversized pack source in audit.
2. **The repair exception again** (`language_packs/repair.rs`). It now
   requires the executable to canonicalize to the running hook binary and
   `--path` to canonicalize to the project. Try to get anything else past
   it: symlinks, hard links, `PATH` tricks, argument smuggling, `--path`
   forms, and subcommand arguments that reach outside the project.
3. **The exit veto** (`outcomes/toolchain.rs`, `test_run_failed`): any case
   where a run that should pass is now recorded as failed (a false
   negative), or one that still passes wrongly. Check the Verus model
   (`verification/harness.rs`, invariant 6) mirrors the new rule.
4. **The anchored removal** (`install.rs`, `mod anchored`): correctness of
   the descriptor handling and the `unsafe` blocks (each must be sound and
   carry an accurate `SAFETY:` comment), and anything it deletes or fails to
   delete.
5. Regressions anywhere else these commits touch.

Ignore style; fmt and clippy are clean. You may run read-only commands and
targeted `cargo test` with `CARGO_TARGET_DIR=target/swarm/gate`.

## Output

Write nothing to disk. Print Markdown to stdout: a one-paragraph verdict
(ready for a PR, ready after named fixes, or not ready); a table of your
seven second-pass findings with closed / partly / not closed and evidence;
then any new findings, most severe first, with severity, `file:line`, the
defect, a concrete failure scenario, and a suggested fix. Mark anything
inferred rather than confirmed as `(unconfirmed)`.

# Review brief, fourth pass: fixes for the third-pass findings

You are an independent reviewer. Review read-only: do not edit, create, or
delete repository files, and do not run git commands that change state.

Branch `feat/go-language-packs`. Your third-pass review is
`docs/reviews/2026-10-19-go-language-packs/codex-review-third.md`; the
"Third pass" section of `triage.md` names the fixes: `938ac5a` (findings 1
and 2), `389b5db` (3), `458b9f9` (4). `git show` each.

Check, in priority order:

1. Does each fix close its finding? Retry your counterexamples.
2. The repair exception (`language_packs/repair.rs`) now requires an
   absolute executable path that canonicalizes to the running binary, and an
   absolute `--path` naming the project for `install`/`remove`. Try once more
   to get any other command, binary, or pack store past it. Also check that
   the load error's printed repair command is one the check admits.
3. `test_started` (`outcomes/toolchain.rs`; cargo built-in patterns; Go's
   `packs/go/toolchains.json`): can it now make a genuine compile failure
   read as a compiled run, or make any run that should fail read as a pass?
   Does the Verus model (`verification/harness.rs`, invariant 6) match?
4. Regressions elsewhere in these three commits.

You may run read-only commands and targeted `cargo test` with
`CARGO_TARGET_DIR=target/swarm/gate`.

Print Markdown to stdout, writing nothing to disk: a one-paragraph verdict
(ready for a PR, ready after named fixes, or not ready); a table of the four
third-pass findings with closed / partly / not closed and evidence; then new
findings, most severe first, with severity, `file:line`, the defect, a
concrete failure scenario, and a suggested fix. Mark inferences
`(unconfirmed)`.

# Review brief, second pass: fixes for the Codex findings

You are an independent reviewer. Review read-only. Do not edit, create, or
delete any file in the repository, and do not run git commands that change
state (no commit, checkout, reset, stash, rebase).

## Context

Branch `feat/go-language-packs`, in the worktree you are running in, rebased
onto `main` (v0.39.0). You reviewed this branch earlier; your review is
`docs/reviews/2026-10-19-go-language-packs/codex-review.md`, and our triage of
it, with a verdict and fix commit per finding, is `triage.md` in the same
directory. Read both first.

The fixes are the commits from `7ca493f` (finding 2) and `ba87199` through
`8693a3a` (findings 1, 3-14). `git log --format='%h %s' origin/main..HEAD`
lists them; each commit message names its finding.

## What to check, in priority order

1. **Does each fix close its finding?** For every finding, say whether the
   fix closes it fully, partly, or not, with `file:line` evidence. Try the
   failure scenario from your first review against the new code.
2. **New attack surface.** Finding 9's fix lets a single command through
   while a pack fails to load (`language_packs/repair.rs`,
   `is_pack_repair_command`, used in `hook/pre.rs` and `codex_hook.rs`).
   Governance is down while this applies. Try to get any command other than
   the pack CLI past it, or to make the pack CLI itself do something
   harmful. Also review `install::remove` (symlinks, races, built-ins,
   non-identifier ids) and the new install lock (`lock_pack_store`).
3. **Regressions.** Did a fix change behaviour for native languages, for
   projects without packs, or for packs that were valid before? In
   particular: capability gating (`for_path_with`), the freshness refactor
   in `graph/sync/index.rs` (must report exactly what it reported before),
   and the incremental-save rebuild in `graph/sync/rebuild.rs`.
4. **Evidence integrity.** The per-test count rule (`outcomes/toolchain.rs`,
   finding 2) and its Verus model (`verification/harness.rs`, invariant 6):
   can any output still produce a test pass the run does not support?
5. **Tests.** Does each new test fail without its fix? Name any that would
   pass anyway.

Ignore style and formatting; fmt and clippy are clean.

## Evidence gathered (verify, don't trust)

- Whole-workspace `cargo test --no-fail-fast` on the rebased branch.
- `scripts/verify-language-packs.py` against real Go 1.26.4: 25/25 steps,
  including a test that calls `os.Exit`.
- Verus: 31 verified, 0 errors; three deliberate mutations each fail.

You may run read-only commands and targeted `cargo test` with
`CARGO_TARGET_DIR=target/swarm/gate`.

## Output

Write nothing to disk. Print Markdown to stdout:

1. A one-paragraph verdict: ready for a PR, ready after named fixes, or not
   ready.
2. A table: finding number, closed / partly / not closed, evidence.
3. New findings, most severe first: severity (BLOCKER / MAJOR / MINOR /
   NIT), `file:line`, the defect in one sentence, a concrete failure
   scenario, and a suggested fix.
4. What you did not check, and why.

Mark anything inferred rather than confirmed as `(unconfirmed)`.

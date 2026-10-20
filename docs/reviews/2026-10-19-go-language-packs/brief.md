# Review brief: branch feat/go-language-packs

You are an independent reviewer. Review the whole branch, read-only. Do not
edit, create, or delete any file in the repository, and do not run git
commands that change state (no commit, checkout, reset, stash, rebase).

## What to review

- Branch: `feat/go-language-packs`, worktree you are running in.
- Diff: `git diff 01513a7..HEAD` (103 files, about 13k lines added). Base
  `01513a7` is the commit the branch started from.
- Spec: `docs/superpowers/specs/2026-10-17-go-and-language-packs-design.md`.
- Plan: `docs/superpowers/plans/2026-10-17-go-and-language-packs.md`. Its
  Status block, not its checkboxes, records what was verified.
- Project conventions: `AGENTS.md`.

Commit `b8a336a` is a WIP snapshot of Tasks 1-8, built by an earlier swarm.
**No reviewer has seen Tasks 4-8.** The four commits after it
(`dde8fd1`, `da10577`, `163066e`, `d6acfce`) are fixes from 2026-10-19 and
also need review.

## Focus, in priority order

1. **Correctness against the spec.** Does the code do what the spec and plan
   require? Name any requirement that is missing, partial, or contradicted.
2. **Security and confinement.** Pack manifests and assets are untrusted
   input: path traversal, symlinks, namespace spoofing of host-owned
   predicates, resource limits, Rhai sandboxing, anything a malicious pack
   could use to escape or to impersonate host evidence.
3. **Evidence integrity.** Phronesis must never invent evidence: an absent
   signal is unknown, never a pass. Check outcome parsing (including the new
   `test_counts: "per_test"` field), graph facts, static test selection, and
   coverage: can any path manufacture a pass, a test relationship, or
   coverage that did not happen?
4. **Fail-closed behaviour.** Where the hook cannot evaluate, does it block
   or warn as the conventions in `AGENTS.md` require, rather than allowing
   silently?
5. **Hook-path cost.** The pre/post-check hooks run on every tool call. Flag
   work added to that path that scales badly (whole-tree scans, repeated
   parsing, unbounded reads).
6. **Tests.** Tests that pass without testing their claim, assertions that
   pin a defect, missing negative cases.

Ignore pure style and formatting; clippy and rustfmt are clean.

## Evidence already gathered (verify, don't trust)

- `cargo test --workspace --no-fail-fast`: 3622 passed, 0 failed.
- `scripts/verify-language-packs.py` against real Go 1.26.4: 23/23 steps.
- `crates/phronesis-mcp/verification/harness.rs` under Verus: 29 verified.

You may run read-only commands (`git diff`, `git log`, `rg`, `cargo test`
for a specific test, `cargo check`). Use target dir `target/swarm/gate`.

## Output

Write nothing to disk. Print your review as Markdown to stdout:

1. A one-paragraph verdict: ready for a PR, ready after named fixes, or not
   ready.
2. Findings, most severe first. Each with: severity (BLOCKER / MAJOR / MINOR
   / NIT), `file:line`, the defect in one sentence, a concrete failure
   scenario (input or state, then the wrong result), and the fix you
   suggest.
3. Spec requirements you checked and found satisfied (one line each), so
   the coverage of the review is visible.
4. What you did not review, and why.

Mark anything you inferred rather than confirmed by reading code or running
a command as `(unconfirmed)`.

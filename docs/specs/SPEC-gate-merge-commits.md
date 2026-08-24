# SPEC: gate merge / rebase / cherry-pick commits, not just `git commit`

**Status:** draft
**Authors:** Claude, Andrew Waterman
**Date:** 2026-06-20
**Target release:** phronesis-mcp 0.13.x (PATCH — rule pack tweak; no new
              fact family, no new hook surface, no schema change)
**Affects:** `crates/phronesis-mcp/src/init.rs::confidence_rules`,
              `crates/phronesis-mcp/tests/confidence_gate_integration.rs`,
              the live `.phronesis/rules.json` of any project that already
              opted into the `confidence` pack.

## The problem

The 0.12.0 confidence-scoring gate matches `bash_command_matches: "git commit"`.
This was the intended trigger when the SPEC was written — the headline
scenario was a developer typing `git commit -m "..."` to claim a unit done.

The journey-facts merge night surfaced the gap. `git merge --no-ff <branch>`
also lands work in main, also produces a commit, and is **not matched** by
the existing pattern. Same for `git rebase`, `git cherry-pick`, and any
porcelain command that produces a commit without containing the literal
substring `git commit`. Concretely, on this branch we did:

```
$ git merge --no-ff claude/confidence-scoring-0-12-0 -m "Merge ..."
$ git merge --no-ff claude/journey-facts-0-13-0    -m "Merge ..."
```

Neither merge invoked the pre-check gate, even though both produced a real
commit on `main`. The confidence-low-blocks-commit rule sat silent, and the
medium-warn rule sat silent.

This is the gate-bypass-by-merge hole.

## Why the current design is wrong (but not by much)

The intent of the gate is "no commit reaches main without grounded
validation." The implementation matches a narrow textual pattern that
captures only one of the many commands that produce commits. The hole is:

| Command | Produces commit on disk | Matches `git commit` pattern | Gate fires |
|---|---|---|---|
| `git commit -m "..."` | yes | yes | ✓ |
| `git merge --no-ff <b>` | yes | no | ✗ |
| `git merge --squash <b>` then `git commit` | yes (in two steps) | yes (step 2) | ✓ (lucky) |
| `git rebase --interactive <ref>` | yes (potentially many) | no | ✗ |
| `git cherry-pick <sha>` | yes | no | ✗ |
| `git revert <sha>` | yes | no | ✗ |
| `git pull` (with default merge) | yes (when ff fails) | no | ✗ |

Five of six paths bypass the gate. The invariant the SPEC promises — *gate
all commit-producing operations* — is honored for one shape and quietly
violated for the rest. That's a worse outcome than "no gate at all,"
because it gives a false sense of coverage.

## What we're not changing

- The **subject lifecycle** stays as-is. Subjects open on a build/test and
  settle on a successful gate-firing commit. We are not redesigning subject
  semantics for merge commits in this SPEC.
- The **signal_pass derivation** stays as-is. Gate rules continue to
  count `signal_pass('*','*')`.
- The **band thresholds** stay as-is. ≤1 blocks, ==2 warns, ==3 passes
  silently.
- The **gate placement** stays as-is. Pre-check, not post-check; the gate
  blocks the work from happening rather than recording its happening.

## What we are changing

The two rules in `confidence_rules()` get their `bash_command_matches`
pattern broadened from `"git commit"` to a regex that matches every
commit-producing porcelain command. Concretely:

```diff
- { "bash_command_matches": "git commit" }
+ { "bash_command_matches": "git (commit|merge|rebase|cherry-pick|revert|pull)" }
```

That's the entire substantive change. Two rules × one clause each = four
lines of JSON across `confidence-low-blocks-commit` and
`confidence-medium-warns-commit`.

### Why include `git pull`

`git pull` defaults to a merge when the local branch can't fast-forward.
That merge produces a commit. A `git pull --ff-only` that succeeds fast-
forwards (no commit produced) and the gate fires harmlessly on a no-op
(the pull command runs, the hook checks signals, no new commit happens,
the gate has nothing to block). `git pull --rebase` produces a rebase
(covered by the `rebase` token). So including `pull` over-fires
slightly on the fast-forward case and correctly fires otherwise.

The alternative — leaving `pull` out — leaves a hole every time someone
pulls without `--ff-only`. We pick over-fire over under-cover; the cost
of over-fire is a fast-forward `pull` warning, the cost of under-cover
is the same gate-bypass we're trying to close.

### Subject availability across merge sessions

A real-world friction: merges typically come in bursts (cut a release,
merge N branches). Each successful gated commit settles the subject. The
next merge then sees "no open subject → 0 signals → low confidence warning"
(per `SPEC-structural-rule-migration.md` §"Confidence gate severity", low
confidence warns rather than blocks).

The expected workflow is:

```
$ cargo test                       # open subject, accumulate signals
$ git merge --no-ff feat-a         # gate fires (medium/high), commit lands, subject settles
$ cargo test                       # reopen subject
$ git merge --no-ff feat-b         # gate fires (medium/high), commit lands
$ cargo test                       # reopen
$ git merge --no-ff feat-c         # ...
```

This is the friction the status-quo design accepted by NOT gating merges.
We are explicitly accepting that friction in exchange for closing the
hole. The mitigation is process — encourage a "cargo test → merge"
cadence per branch — not engineering. A "subject inheritance from the
merged branch" design would remove the friction but introduces
significantly more design surface; it's a v2 question.

## Migration for existing projects

Projects that ran `phr-mcp init --packs confidence` before this change
have the narrow pattern in their `.phronesis/rules.json`. Two paths:

1. **`phr-mcp init --rules-only --force --packs confidence`** — overwrites
   the rule pack, restoring the broadened pattern. Backs up to `.bak`.
2. **Hand-edit `rules.json`** — change the two `bash_command_matches`
   patterns in `confidence-low-blocks-commit` and
   `confidence-medium-warns-commit`.

There is a related, broader composability issue surfaced the same night:
running `phr-mcp init --packs <new-pack>` against an existing project
**does not merge** the new pack's rules into the existing `rules.json`
(only the marker files are written). That's tracked as its own follow-up
and is out of scope here. For now, the migration story for this SPEC is
"existing projects need `--rules-only --force` or a hand-edit."

## Tests

`crates/phronesis-mcp/tests/confidence_gate_integration.rs` should add a
new test per command shape:

```
gate_fires_on_git_merge_at_low_confidence
gate_fires_on_git_merge_at_medium_confidence
gate_fires_on_git_rebase
gate_fires_on_git_cherry_pick
gate_fires_on_git_revert
gate_fires_on_git_pull_when_default_merge
gate_does_not_fire_on_unrelated_git_command  // sanity: `git status`, `git log` are safe
```

Each test seeds the journal with the appropriate signal_pass count and
asserts pre-check exit code (2 for low, 1 for medium, 0 for high) for the
shaped command. Mirror the existing low/medium/high test pattern.

## Out of scope

- **Subject inheritance from merged branches.** A merge commit could in
  principle inherit the signals of the branch being merged, removing the
  need to re-test. Real design surface (which branch when octopus-merging?
  what if multiple subjects were open on the source branch?); deferred.
- **Init-pack composability** — adding `--packs <new>` to an existing
  project should merge the new pack's rules into the existing
  `rules.json`. Tracked separately; the migration story for this SPEC
  uses `--rules-only --force` as the workaround.
- **A first-class `commit_producing` predicate.** Long-term, a predicate
  shape like `{ "produces_commit": true }` would be cleaner than a
  command-regex; the engine evaluates "this Bash invocation creates a
  commit" via direct inspection. Deferred to engine-feature work.
- **Hook integration with non-Bash tools.** If a future runtime delivers
  a git operation via something other than Bash (e.g. a hypothetical
  `git` tool with its own MCP integration), the `bash_command_matches`
  pattern won't help. Out of scope; deal with when the surface exists.

## Open questions

- **`git commit --amend`.** Amending changes a commit in place; is that
  "producing a commit"? Argument for: the resulting commit is a new
  object in main. Argument against: amending typically polishes an
  existing claim rather than making a new one. The pattern `"git commit"`
  matches `--amend`; the broadened pattern still matches via the `commit`
  token. Stay matched. Document.
- **Worktree operations.** `git worktree add` followed by work inside the
  worktree doesn't bypass the hook (the hook fires on every Bash call
  regardless of cwd). Probably no change needed; flag if the pattern
  emerges differently.

## Rollout

1. Land the rule-pattern change in a PATCH bump (0.13.x → 0.13.y).
2. Update the confidence_gate_integration test set per §Tests.
3. Note the migration step (`--rules-only --force --packs confidence`) in
   the CHANGELOG and CLAUDE.md.
4. Mention the gate-bypass-by-merge hole in the release note so users on
   0.12.x / 0.13.x without the fix know to upgrade or hand-edit.

## Why this isn't a 0.14.0

Pre-1.0 semver: MINOR is "new feature surface" (subcommand, pack, hook
stage). This change is "the existing confidence pack now also fires on
operations it always should have fired on." That's a bug-fix-shaped
change in semantic intent, even though it widens what trips the gate.
PATCH. The headline behavior — gate by band, on commit-producing
operations — is unchanged.

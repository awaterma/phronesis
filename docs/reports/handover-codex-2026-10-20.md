# Handover to Codex (Astra): Go language packs, 2026-10-20

Written by the outgoing controller (Claude Opus 5.5) at the end of the
2026-10-19/20 session. Nothing from that conversation survives except this
file, the files it points to, and git history. Read this first, then the
plan's Status block and the review triage (paths below).

## 1. Where things stand

- **Branch:** `feat/go-language-packs`, checked out in the worktree
  `/Volumes/Data/Git/phronesis/.worktrees/go-packs`.
- **Head:** `a95a815` (plus the commit that adds this file), 53 commits on
  top of `main` at `7ac6695` (v0.39.0). Rebased cleanly; `main` has not moved
  since.
- **Not pushed, no PR.** The owner reviews the diff before the one push that
  opens a draft PR (section 4).
- **State:** the feature is complete and every gate below passed at the final
  code. Twelve read-only review passes by you (Codex) converged: the twelfth
  reported no new BLOCKER or MAJOR. One open item awaits the owner (6.3).

### What the branch delivers

- Go support (`phr-mcp init --packs go`), built as a bundled language pack on
  the same manifest/query/Rhai contract external packs use.
- Installable language packs under `.phronesis/packs/<id>/`, with
  `phr-mcp pack list|validate|install|remove` and MCP `list_language_packs`.
- Toolchain fields `test_counts: "per_test"` and `test_started`; a
  run-level `test_run_failed` fact (a failed run withdraws an earlier test
  pass); manifest `test_file_suffixes`.
- A Verus model of the toolchain outcome rules (invariant 6).

The `[Unreleased]` section of `CHANGELOG.md` describes all of it for users.

## 2. Read these, in order

1. `docs/superpowers/plans/2026-10-17-go-and-language-packs.md`, its
   **Status** block at the top. It is the record of what is verified; the
   task checkboxes below it were never ticked and are not the record.
2. `docs/reviews/2026-10-19-go-language-packs/triage.md`: every finding from
   all twelve of your passes, the verdict after checking it against the code,
   and the fix commit. Your own reviews are the `codex-review*.md` files
   beside it, with the briefs that produced them.
3. `docs/superpowers/specs/2026-10-17-go-and-language-packs-design.md`: the
   approved design.
4. `crates/phronesis-mcp/verification/RESULT.md`, section 6: what Verus
   proves about outcome classification, and what it does not.
5. `AGENTS.md` and `docs/LANGUAGE-PACKS.md` for conventions and the pack
   contract.

## 3. Evidence at the final code (2026-10-20, 04:20 local)

| Gate | Result |
|---|---|
| `cargo test --workspace --no-fail-fast` | 121/121 binaries; 3658 passed, 0 failed, 4 ignored |
| `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo check -p phronesis-mcp --no-default-features` | clean |
| `scripts/verify-language-packs.py` against real Go 1.26.4 | 27/27 steps |
| Verus (`bash crates/phronesis-mcp/verification/run-verification.sh`) | 35 verified, 0 errors; each modelled rule mutation-checked |
| Warm latency, release builds, p50 (one warm-up, 30 samples) | Rust project within 3% of `main` (pre-check 9.2 vs 9.2 ms); Go project 12.6 / 40.2 / 22.8 ms for pre-check / post-check / audit (new work: `main` has no Go) |

Treat these as of that time. Rerun before you claim anything (section 6).

## 4. Decisions that belong to the owner

Ask; do not decide these yourself.

1. **6.3, non-Unix `pack remove`.** On Unix, removal is anchored to a
   directory descriptor (`crates/phronesis-mcp/src/language_packs/install.rs`,
   `mod anchored`), so swapping an ancestor directory cannot redirect it. On
   other platforms it is still path-based. Options: keep it as a documented
   limitation, or refuse `pack remove` on non-Unix.
2. **Commit shape.** Keep the 53 commits, or squash into one feature commit.
   The first commit, `f3e3770`, is a WIP snapshot of plan tasks 1-8; the rest
   are reviewed fixes, each naming its finding.
3. **The PR.** The owner looks at the diff (137 files, about 17k lines
   added) first. Then: one push, and open the PR as a **draft**
   (`gh pr create --draft`). Do not push incrementally, do not promote the
   PR (`gh pr ready`), and do not merge: the owner playtests and promotes.
   The PR title must be conventional-commit shaped (squash-merge makes it the
   commit release-plz reads), for example
   `feat(mcp): Go support and installable language packs`. End the PR
   description with the attribution your harness uses.

## 5. Constraints that are easy to miss

- **This repository is public.** Never write the downstream consumer
  project's name into any file here. A local rule blocks it (including in
  commands), and a pre-commit hook refuses staged content that names it.
  Use "downstream consumer" in prose and `example-app` in fixtures. Raw
  review transcripts can carry it: do not commit them (keep them under a
  scratch directory, as the triage notes).
- **Never pipe a gating command.** `cargo clippy ... | tail` hides its exit
  status; a hook (`warn-piped-verification-masks-exit-status`) warns, and a
  piped clippy once let a lint failure into a commit this session. Run gates
  unpiped and check the exit code.
- **Run the real-Go acceptance script and latency samples on an idle
  machine.** A concurrent workspace test run once starved `init` past the
  script's 180 s timeout (it takes about 100 ms idle).
- **Your sandbox.** In a read-only Codex sandbox, integration tests that
  spawn the `phr-mcp` binary stalled at `_dyld_start`, and some needed temp
  directories outside the repository. Run those suites with a sandbox that
  can write temp directories and spawn processes, or report them as not run.
- **Use separate target directories** so parallel builds do not contend:
  `target/swarm/gate` for targeted runs, `target/swarm/final` for the full
  gate, `target/swarm/release` for latency.
- **Test-first.** Every fix on this branch starts with a test that fails for
  the right reason. When a defect is claimed, reproduce it (with real `go` or
  `cargo` where the claim is about their output) before changing code.
- **Hooks govern this checkout.** Phronesis's own pre/post-check hooks run
  on your edits. A block names its rule; read it rather than routing around
  it.

## 6. To re-establish the evidence

```sh
cd /Volumes/Data/Git/phronesis/.worktrees/go-packs
cargo fmt --all --check
CARGO_TARGET_DIR=target/swarm/final cargo clippy --workspace --all-targets -- -D warnings
CARGO_TARGET_DIR=target/swarm/final cargo check -p phronesis-mcp --no-default-features
CARGO_TARGET_DIR=target/swarm/final cargo test --workspace --no-fail-fast
CARGO_TARGET_DIR=target/swarm/gate cargo build -p phronesis-mcp
python3 scripts/verify-language-packs.py --binary target/swarm/gate/debug/phr-mcp \
  --go "$(command -v go)" --artifacts <scratch-dir>/go-acceptance
bash crates/phronesis-mcp/verification/run-verification.sh
```

The latency comparison used a throwaway script that is not in the repo. To
redo it: build the branch with `--release`, then time `pre-check`,
`post-check` and `audit --json` (one warm-up, then 30 runs, report p50 and
p95) for a small Go project initialized with `--packs go` and a small Rust
project initialized with `--packs rust`, against the installed `main` binary
(`~/.cargo/bin/phr-mcp`, 0.39.0) on equivalent projects.

## 7. Known limits, stated plainly

- Test outcomes are recognized by patterns over tool output. Verus proves
  the classification logic over the evidence the patterns supply, not that
  each pattern recognizes the right line. Twelve passes hardened the cargo
  and Go patterns against real output (build-script payloads, colored
  output, quiet and verbose runs, compiler excerpts, unusual paths); the
  last four produced no false pass. New output shapes can still be missed;
  a miss costs a stale or wrongly withdrawn pass, not a fabricated one, in
  every case found so far.
- Go analysis is source-level: build tags, cgo, generated code, interface
  dispatch and type resolution are not modelled. Unresolved calls are left
  unresolved, never guessed.
- Go has no dynamic coverage collector; selection is static reach only.
- The original swarm campaign's ledger (spend and bids) was not found; its
  records live only in an earlier Codex session transcript.

## 8. After the PR merges (owner's call, not yours to trigger)

Release follows `docs/RELEASING.md`: merge the release-plz Release PR with a
merge commit, then reinstall locally (`cargo install --path
crates/phronesis-mcp`) and check `phr-mcp --version`.

## 9. Queued next, once the Go PR is ready

The owner deferred this until the Go work is ready for a PR:
`docs/superpowers/specs/2026-10-19-verification-scopes-and-rete-proofs-design.md`
(untracked, in the **main checkout**, not this branch). It specifies a rule
that new functions in declared verification scopes must carry a property
with a verifier encoding (owner decisions: declared scopes only, warn at
post-check, satisfied only by an encoded property), and a RETE verification
goal with six named properties, starting with refraction. The owner chose
"spec and plan first, no proofs in the same change". It awaits the owner's
review before any plan is written.

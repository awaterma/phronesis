# First governed session: finish a task with TDD

Build a tiny visit counter with an agent: **write a failing test, observe a
prohibited implementation being blocked, then finish with passing tests**.
Tests describe the behavior you need; Phronesis protects a coding constraint
while you work. Allow about 20 minutes after installing the tools.

The task is small: `visits_today("12")` returns `12`; malformed input such as
`"many"` returns `0`. The starting implementation returns zero for everything.
A project rule prohibits `.unwrap()` calls under `src/`, because malformed
input must not panic. This is a structural Rust rule, not a search for words
in comments or strings.

This exercise **demonstrates TDD; it does not enforce test chronology**.
Phronesis blocks the prohibited call. You and the agent preserve the failing
and passing test results. The TDD reminder available in Phronesis is guidance,
not proof that tests came first.

## Prerequisites

- Rust 1.90 or newer (`cargo --version`), Git, and a Phronesis checkout.
- `phr-mcp` on PATH, with `phr-mcp doctor --help` working.
- Claude Code installed and authenticated. See its
  [setup instructions](https://code.claude.com/docs/en/setup).

**While this tutorial is under review, check out its PR branch before
installing:** `git fetch origin feat/first-governed-session`, then
`git switch feat/first-governed-session` in a clean checkout. An older release
may not include `doctor`. Source installation uses Cargo's installation
location; only the practice project and hook settings below are disposable.

If building Phronesis from source, run these commands in its checkout:

```sh
cargo install --locked --path crates/phronesis-mcp
export PATH="$HOME/.cargo/bin:$PATH"
phr-mcp doctor --help
```

For installation help, detailed doctor findings, or a language-independent
marker exercise, see [Verify your installation](verify-installation.md).

## Step 1 — Set up the practice copy

Run this from the Phronesis checkout root and keep the same shell open:

<!-- check: tdd-setup -->
```sh
REPO="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
SCRATCH="$(mktemp -d)"
cp -R "$REPO/examples/first-governed-session/tdd-practice" "$SCRATCH/practice"
cd "$SCRATCH/practice"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/.cache/phronesis-tutorial-target}"
phr-mcp init --hooks-only
phr-mcp doctor --host claude --path .
```

The crate has no dependencies and no tests yet. `init --hooks-only` preserves
its one teaching rule and writes project hook/MCP configuration. Keep the
Cargo target path absolute; the default above preserves build artifacts
outside the scratch directory, including after cleanup.

Expect doctor to report valid rules and local hook wiring. Trust and log
observations may remain `not_observed`; a zero exit code is not a host verdict.
Resolve any warnings before continuing. `doctor --probe` is optional and
exercises only a temporary fixture through `pre-check`, not your live host.

## Step 2 — Start the agent and check the hooks

From the same shell:

```sh
claude
```

Review the folder trust prompt if shown. Use `/hooks` to confirm
`phr-mcp pre-check` and `phr-mcp post-check`. Use `/mcp` to inspect and approve
the project Phronesis server if prompted. Hooks enforce edits; the MCP server
supplies management tools. See [host setup troubleshooting](verify-installation.md#troubleshooting)
if either is missing. This walkthrough targets Claude Code; it does not
establish equivalent behavior in other hosts.

## Step 3 — Write the tests, then observe red

Ask the agent:

> Read src/lib.rs and Cargo.toml. Add tests/visits.rs with the two tests below,
> using Edit or Write. Do not change src/lib.rs yet. Run the test command and
> show the failing assertion before proposing any implementation change.

<!-- check: tdd-tests -->
```rust
use visit_counter::visits_today;

#[test]
fn parses_valid_count() {
    assert_eq!(visits_today("12"), 12);
}

#[test]
fn malformed_count_is_zero() {
    assert_eq!(visits_today("many"), 0);
}
```

Use this command for both the red and green runs:

<!-- check: tdd-test-command -->
```sh
cargo test --offline -j 1 --test visits
```

Expected: `parses_valid_count` fails with actual `0` and expected `12`;
`malformed_count_is_zero` passes. The command exits nonzero (Cargo exit 101).
A compiler error, missing toolchain or missing test target is **not** the
intended red result. Preserve this output and confirm `src/lib.rs` is still
the original stub. Having one test already pass is fine: the other exposes
the missing behavior.

## Step 4 — Observe a meaningful policy refusal

For a controlled demonstration, deliberately ask for the prohibited shortcut:

> Replace src/lib.rs with the implementation below using Edit or Write.
> If Phronesis blocks it, report the rule message and stop. Do not retry via
> Bash, weaken the rule or modify the tests. This is an intentional policy
> exercise, not a suggested solution.

<!-- check: tdd-blocked -->
```rust
pub fn visits_today(count: &str) -> usize {
    count.parse::<usize>().unwrap()
}
```

The hook should block this real tool call and name
`tutorial-no-unwrap-in-src`. The implementation would panic for `"many"`;
Phronesis rejects it before it reaches the file. Ask the agent to read
`src/lib.rs` again: **the original zero-returning stub must still be there**.
A refusal without inspecting the resulting file is incomplete evidence.

The tests and rule serve different purposes. The malformed-input test would
catch this bug at runtime; the rule rejects this prohibited coding construct
at edit time. One does not replace the other. A real task may never trigger a
block if the agent proposes a compliant solution immediately.

## Step 5 — Recover, finish the task, and observe green

Now ask:

> Keep both tests unchanged. Handle parse failure explicitly so malformed
> input returns zero. Use Edit or Write to update src/lib.rs, then rerun the
> same test command. Show the result and the final file.

One acceptable implementation is:

<!-- check: tdd-green -->
```rust
pub fn visits_today(count: &str) -> usize {
    match count.parse::<usize>() {
        Ok(value) => value,
        Err(_) => 0,
    }
}
```

The edit should be allowed, and the same command should report
`2 passed; 0 failed`. Read the source and tests to confirm the desired behavior
was implemented without deleting assertions or weakening the policy. There
is no necessary refactor for this tiny function; if you make one, keep the
tests green.

**Success means all four observations:** the intended assertion failed
before implementation; the host refused the `.unwrap()` proposal and left
the stub intact; the corrected edit succeeded; both unchanged tests passed.
You have completed a useful requirement, not merely collected a refusal.

To inspect the rule decision, in another terminal opened in the practice
copy (or after exiting Claude Code), run:

```sh
cat .phronesis/log.jsonl
```

Look for a `pre_check` record with `exit: 2` and `blocked_by` naming
`tutorial-no-unwrap-in-src`, alongside allowed edits. Logs corroborate the
observed host action; they do not independently prove enforcement or TDD
chronology. Keep the red/green output separately if you want a durable record.

## Step 6 — Optional: find tests for a changed function

Phronesis can also help choose what to run after an edit. This extension uses
**static call relationships**, not imported runtime coverage. Keep working
in the same practice copy; no additional packages are needed.

First ask the agent to append this unrelated control test to `tests/visits.rs`.
It checks package metadata, never calls `visits_today`, and makes it visible
whether selection narrows the work:

<!-- check: select-control -->
```rust
#[test]
fn package_name_is_stable() {
    assert_eq!(env!("CARGO_PKG_NAME"), "visit-counter");
}
```

In a terminal in the practice copy, run all three tests and record a local
Git baseline. This creates one commit **only in the disposable example**;
there is no remote and nothing is pushed. The command-scoped identity avoids
requiring or changing your personal Git identity. Do this before the next edit:

<!-- check: select-baseline -->
```sh
cargo test --offline -j 1 --test visits
git init -q
git add Cargo.toml src/lib.rs tests/visits.rs
git -c user.name="Tutorial" -c user.email="tutorial@example.invalid" -c commit.gpgsign=false commit -m "Record passing tutorial baseline"
```

Expect `3 passed; 0 failed`. Now ask the agent to make this small equivalent
refactor in `src/lib.rs`, leaving all tests unchanged:

<!-- check: select-refactor -->
```rust
pub fn visits_today(count: &str) -> usize {
    count.parse::<usize>().unwrap_or(0)
}
```

`unwrap_or(0)` handles failure by returning zero; it is not the prohibited
`.unwrap()` call. This is a refactor of already-tested behavior, not a new
requirement needing another red test. Leave this source edit **unstaged**:
selection currently examines unstaged changes to tracked files. Staging or
committing it now removes the diff this example needs.

Rebuild the graph after the edit, then inspect the selected tests:

<!-- check: select-select -->
```sh
phr-mcp graph rebuild
phr-mcp coverage select --json > ../selected-tests.json
cat ../selected-tests.json
```

Keep this output beside the practice copy, not inside it: a newly created
JSON file in the project can itself make the graph stale before selection.

Expect `changed_functions` to contain `fn:src/lib.rs::visits_today`,
`static_reach_available` to be `true`, and two entries with
`evidence: "static_reach"`:

- `rust:visit-counter#test:visits::parses_valid_count`
- `rust:visit-counter#test:visits::malformed_count_is_zero`

`package_name_is_stable` should not be selected. The graph connects the two
visit tests to the changed function; it does not infer that the unrelated
package check exercises it. If the result is empty or says the graph is
stale, inspect `phr-mcp graph status` and `git diff`, then rebuild. An empty
selection is not evidence that no tests are needed.

Rust selections currently report `command: null`. Here `#test:visits` names
the Cargo integration-test target, and the final component names the test.
Run the two identified tests explicitly:

<!-- check: select-selected-tests -->
```sh
cargo test --offline -j 1 --test visits parses_valid_count -- --exact
cargo test --offline -j 1 --test visits malformed_count_is_zero -- --exact
```

Each command should report `1 passed` and `2 filtered out`. Selection helps
focus feedback; it does not replace your full-suite gate or prove sufficient
coverage. No runtime coverage was imported in this exercise, and Phronesis
labels that limit instead of presenting static reach as observed execution.

## Step 7 — Clean up and take it to your project

Exit Claude Code, then return to the original shell:

```sh
cd "$REPO"
rm -rf "$SCRATCH"
```

This removes only the disposable practice copy. The external Cargo build
cache and installed tools remain. This exercise does not require
`phr-mcp install` or changing user-scope MCP configuration.

For a real project, start with its behavioral test and relevant coding
policies. The Rust starter pack provides a broader `.unwrap()` rule among
other checks; the [README quick start](../../README.md#quick-start) explains
`phr-mcp init --packs rust`. The teaching rule here is deliberately narrow.

## Executable checks and their limits

From the checkout, with Python 3 and Cargo available:

```sh
python3 examples/first-governed-session/check_tutorial.py --phr-mcp "$(command -v phr-mcp)" -v --keep
```

The checker executes the published setup and code examples, requires the
specific failing assertion, tests block/allow decisions, and runs both tests
green without changing them. The optional extension verifies that static
selection excludes an unrelated test and that each exact Cargo command runs
only its named test. `--keep` retains fixtures and red/green/selection logs;
build caches are retained separately even without that option. Existing
marker and diagnostic checks remain included.

The checker sends hook payloads directly and applies only allowed writes.
It does **not** run an agent or establish host trust. The interactive Steps
2–5 are the host acceptance exercise. A passing automated check must never
be reported as an observed live-agent walkthrough.

Back to the [tutorials index](README.md).

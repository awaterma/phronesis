# Verify your installation

An installation check for Phronesis: set up a **disposable** practice
project, run a pre-flight check, then watch a governed agent make one edit
that sails through and one that gets blocked — in a real Claude Code session,
not a simulation. Allow about 20 minutes after installation. The practice edits and project
hook configuration live in a scratch directory. Installing the binary writes
to Cargo’s installation directory; this exercise does not modify your existing
project configuration.

For a practical test-first coding task, follow [First governed session](first-governed-session.md).

> **Binary note.** This tutorial requires the `doctor` feature. Verify it
> with `phr-mcp doctor --help`; a version string alone is not sufficient for
> development builds. If your installed release lacks it, build from a
> source checkout containing this tutorial with
> `cargo install --path crates/phronesis-mcp`, then check `doctor --help`
> again.

**Prerequisites**

- Claude Code installed (this tutorial's primary host). Codex and Gemini CLI
  are governed the same way, but the walkthrough below is Claude Code's.
- `phr-mcp` on your `PATH`. Check with `phr-mcp --version`.
- A checkout of the Phronesis repository (the practice example and the
  checker script ship with it). No Rust toolchain is needed at runtime
  unless you are building from source.

If you are starting without a checkout or binary, install Rust 1.90 or newer
and Git. While the change is under review, select the PR branch before the
install command (in a clean clone: `git fetch origin feat/first-governed-session`
then `git switch feat/first-governed-session`). Then run:

```sh
git clone https://github.com/awaterma/phronesis.git
cd phronesis
cargo install --locked --path crates/phronesis-mcp
export PATH="$HOME/.cargo/bin:$PATH"
phr-mcp --version
phr-mcp doctor --help
```

The last command must list `--path`, `--host`, `--probe`, and `--json`.
If this change is still under review, check out the PR branch before the
install command; the released binary may not contain `doctor` yet. If you
already have this checkout, start at the install command in its root.
Python 3 is needed for the optional executable checker. Claude Code also
needs its own authenticated account; install and sign in using its
[official setup instructions](https://code.claude.com/docs/en/setup).

**What "governed" means here.** Phronesis does not live in the model's
context window. Rules sit in `.phronesis/rules.json` on disk, and a **hook**
— a tiny command your host runs before every tool call — re-reads them and
returns allow, warn, or block. The MCP server (`phr-mcp serve`) is the
interactive half: management tools an agent may call. Hooks are the
enforcement half: the host runs them whether or not the agent cooperates.
This tutorial wires the hooks and observes them working.

Doctor reports configuration, an optional isolated probe, and host-observation
limits separately. Its JSON fields are described below; human-readable
message wording may change between versions.

## Step 0 — Pin your variables

Run this from the top level of your Phronesis checkout, and keep the same
shell open for the whole tutorial:

```sh
REPO="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
SCRATCH="$(mktemp -d)"
```

`$REPO` is the absolute checkout path (used by every later step, so nothing
breaks after `cd`), and `$SCRATCH` is a fresh, collision-free scratch
directory that disappears when you delete it at the end.

## Step 1 — Copy the disposable practice project

The shipped example is a mini-project with exactly one deterministic
teaching rule: any edit whose new content contains the plain-text marker
`TUTORIAL-BLOCKED-MARKER` is blocked by the `pre-check` hook, on any file.
It is a teaching fixture, not a real coding policy — the point is a
perfectly predictable rule, not a Rust best practice.

```sh
cp -R "$REPO/examples/first-governed-session/practice" "$SCRATCH/practice"
```

The practice copy contains one rule and no host wiring yet. These setup
steps change only the disposable copy. It contains:

- `.phronesis/rules.json` — the deterministic marker rule,
  `no-tutorial-blocked-marker`.
- `src/visit.rs` — an additional source fixture used by the checker.
- `notes.md` — an ordinary file a plain edit passes and a marker edit fails.

## Step 2 — Govern the copy

```sh
cd "$SCRATCH/practice"
phr-mcp init --hooks-only
```

That is the whole step: `init --hooks-only` writes the host wiring and
leaves rules files untouched, so the deterministic rule set the copy already
contains stays exactly as shipped. The wiring for Claude Code is:

- `.claude/settings.local.json` — registers `phr-mcp pre-check` on
  `PreToolUse` (matcher `Edit|Write|MultiEdit|Bash`) and `phr-mcp
  post-check` on `PostToolUse`, plus lifecycle hooks
  (`phr-mcp claude-hook <Event>`) for session context.
- `.mcp.json` — registers the `phronesis` MCP server (`phr-mcp serve`) for
  this project.

Optionally confirm the wiring with the checker script from the checkout
(the checker runs this same setup in its own scratch directory and asserts
the hook-level outcomes shown in Step 5):

```sh
python3 "$REPO/examples/first-governed-session/check_tutorial.py"
```

## Step 3 — Pre-flight: doctor (configuration observation)

Before opening a session, ask doctor to inspect this directory's
configuration — plain doctor performs filesystem reads only; it never
writes, never spawns a process, and never appends to your log:

```sh
phr-mcp doctor --host claude --path "$SCRATCH/practice"
phr-mcp doctor --host claude --path "$SCRATCH/practice" --json
```

Doctor reports one finding per configured concern, each with a status of
`ok`, `warn`, `fail`, or `not_observed`:

| Finding id | What it reports |
|---|---|
| `binary_on_path` | whether the `phr-mcp` your `PATH` finds is the same binary that ran doctor (a `warn` here is normal until you reinstall a new build) |
| `project_governed` | whether `.phronesis/rules.json` (or a loader) exists at all |
| `rules_load` | whether the rules file parses cleanly |
| `host_hooks` | whether `.claude/settings.local.json` wires `pre-check` into `PreToolUse` and `post-check` into `PostToolUse` (only this generated local file is inspected; shared-only settings report `not_observed`, and other host scopes are not merged) |
| `host_trust` | **always `not_observed`** — doctor cannot see live host trust from local files and never claims it |
| `log_evidence` | **always `not_observed`** — local log lines are evidence to orient you, never proof the host currently runs these hooks |

Exit codes: `0` when every finding is `ok`/`not_observed`, `1` when the
worst finding is a `warn`, and `2` when any finding is `fail` or an
explicit probe fails. Read each warning and resolve it before relying on
this setup. A different PATH binary means the host may run a different
version than the one you just checked.

Keep the three kinds of evidence separate in your head — this is the core
lesson of the tutorial:

| Evidence | Source | What it can tell you |
|---|---|---|
| Configuration observation | `doctor` (plain, read-only) | the wiring exists and parses |
| Probe | `doctor --probe` (explicit; writes its own rule into a disposable OS-temp fixture, calls this executable’s `pre-check` directly twice, deletes the fixture) | this binary’s rule-evaluation path blocks and allows what its fixture demands |
| Host observation | Step 5, your actual Claude Code session | a real host let one edit through and blocked another |

A probe is never a host observation, and a local log is never proof of
either. Doctor's `--probe` is optional here; the walkthrough in Step 5 is
the part that matters, and nothing can substitute for it. When you do run
it, the observed output looks like this (from an actual doctor build):

> probe: rule=doctor-probe-no-unwrap ok=true
>   blocked_edit_contains_unwrap expect_block=true  exit=2    match=true  rule=doctor-probe-no-unwrap decision=block
>   allowed_edit_no_unwrap       expect_block=false exit=0    match=true  rule=- decision=allow
>   note: evidence from a temporary isolated fixture, exercising only this executable's own `pre-check` code path directly (not through any host adapter); not a claim about the real project's hook wiring, host trust, or that any specific host (Claude, Gemini, Codex, or otherwise) actually invokes it this way.

The probe leaves the real project untouched; its evidence stays in the
`probe` field of `--json` output, never merged into `findings`, and its
"ok" never upgrades `host_trust` or `log_evidence` past `not_observed`.

## Step 4 — Open Claude Code in the copy

```sh
cd "$SCRATCH/practice"
claude
```

Start Claude Code from the same shell so it inherits the `PATH` used above.
A GUI-launched host may have a different `PATH`; doctor inspects its own
environment and cannot verify the environment of a separate host process.

On first interactive open, review and accept Claude Code’s folder-trust
prompt if shown. Headless invocation has different trust behavior and does
not validate this interactive step. See the official
[hook reference](https://code.claude.com/docs/en/hooks) and
[MCP approval instructions](https://code.claude.com/docs/en/mcp).
Then confirm the wiring from inside the session:

- `/mcp` — the `phronesis` server from `.mcp.json` should appear; approve
  the project MCP server if Claude Code asks.
- `/hooks` — you should see the `PreToolUse` hook (`phr-mcp pre-check`) and
  the `PostToolUse` hook (`phr-mcp post-check`) from
  `.claude/settings.local.json`.

If `/mcp` or `/hooks` shows nothing, see Troubleshooting below. Do not
proceed to Step 5 on a hunch; the point of this tutorial is to observe, not
assume.

## Step 5 — The governed walkthrough

Stay in the Claude Code session and prompt your agent twice.

**First, the allowed edit.** Prompt something like:

> Read notes.md, then use Edit or Write to append a line saying the
> governance setup works. Do not use a shell command.

The agent edits `notes.md` with its normal Edit/Write tool. No rule matches
an ordinary note, so the `pre-check` hook allows it and the edit goes
through — governance stays out of the way. That quiet success is half the
lesson.

**Then, the blocked edit.** Prompt something like:

> Read notes.md, then use Edit or Write to append exactly:
> TUTORIAL-BLOCKED-MARKER
> If a hook blocks it, report the rule message and stop. Do not retry with
> another tool or change the rules.

The agent proposes an edit whose new content contains the literal marker —
exactly what the practice rule matches. The host runs the `pre-check` hook
before the tool call; the hook exits with a block decision, and Claude Code
**refuses the edit** and shows you the rule's message ("Tutorial rule
no-tutorial-blocked-marker: …"). This is the boundary doing its job from
outside the model's context window.

**Do not substitute a shell command for this walkthrough.** Piping a fake
hook payload into `phr-mcp pre-check` yourself (as the checker script does)
tests the plumbing, not the host: it proves the rule engine fires, but it is
not an observation of Claude Code enforcing anything. The actual host
observation — the host declining a real tool call from a real agent — is the
only evidence that counts for "my session is governed."

**The evidence trail.** After the blocked edit, look at the local action
log from a terminal:

```sh
cat .phronesis/log.jsonl
```

You should find a `pre_check` entry with `"exit": 2` and a `blocked_by`
array naming rule `no-tutorial-blocked-marker`, plus the exit-0 entries for
the allowed edit. Read that log as **unauthenticated evidence, not proof**:
it is a local file the hook wrote, useful for orienting yourself, and
trivially fakeable — which is why Step 5's host observation, not the log, is
the tutorial's success criterion. The same honesty applies in the other
direction: nothing in the log, and nothing doctor says, should be quoted as
"my host verified enforcement" unless you actually watched the host block
the edit in Step 5. When evidence is missing or ambiguous — an unobserved
trust decision, a host you never watched — the honest verdict is "unknown,"
and a system that reports unknown is more trustworthy than one that reports
false positives.

## Step 6 — What just happened

**The rule, anatomized** (`.phronesis/rules.json`): phase `pre` means the
`pre-check` hook may block; `new_content_contains "TUTORIAL-BLOCKED-MARKER"`
matches the literal marker substring in the proposed edit's new content
(not a regex); the `constraint_violation` action is the message the host
shows. Real rules compose this with path and other predicates — for
instance the shipped Rust pack blocks `.unwrap()` only under `src/` — and
the [rule catalogue](https://awaterma.github.io/phronesis/catalogue.html)
shows how the mechanics scale from one marker to full packs.

**Hooks vs MCP, again:** the MCP server is a set of tools an agent *may*
call (`add_rule`, `audit_codebase`, journey and confidence surfaces…). The
hooks are the part the host *must* run on every Edit/Write/Bash call. If you
unregister the MCP server, enforcement continues; if you remove the hook
registrations, enforcement stops — which is why Step 4 had you verify
`/hooks` specifically.

**Doctor's role:** a read-only pre-flight that inspects configuration so
you catch wiring mistakes before a session, not during one. It is the
configuration row of the evidence table, and deliberately nothing more: it
never observes live host trust (its `host_trust` finding is `not_observed`
by design), never upgrades local log lines into proof (`log_evidence` is
`not_observed` even when the log has entries), and never lets probe
success stand in for a host observation.

## Step 7 — Clean up

```sh
cd "$REPO"
rm -rf "$SCRATCH"
```

The hooks and rules lived in the scratch copy; deleting it removes
everything. Nothing was written to your home configuration unless you chose
to run `phr-mcp install` (user-scope MCP registration in `~/.claude.json`),
which you did not need for this tutorial. In your real projects later,
`phr-mcp init --packs <languages>` writes the same wiring plus starter
rules — see the [README quick start](../../README.md#quick-start).

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| `phr-mcp: command not found` | binary not installed or not on `PATH` | install with `cargo install --path crates/phronesis-mcp`; check `echo $PATH`; the checker's `missing_executable` case shows the loud failure you should expect |
| `phr-mcp doctor` → "unrecognized subcommand" | this binary lacks the doctor feature | check feature availability with `phr-mcp doctor --help`; if absent, install from a source checkout that has it (see the binary note) |
| doctor exits 1 with a `binary_on_path` warn | the `phr-mcp` on `PATH` differs from the doctor binary you ran | resolve the mismatch before the host walkthrough; reinstall the intended build or correct `PATH`, then run doctor again |
| `/hooks` shows no Phronesis hooks | `init` was not run in this directory, or the session predates the wiring | re-run `phr-mcp init --hooks-only` in `$SCRATCH/practice`, then start a **new** session there |
| `/mcp` shows no `phronesis` server | project MCP servers need approval | approve the project server when prompted, or re-check `.mcp.json` |
| Claude Code asks to trust the folder | normal first-open trust flow | trust it; hooks from `.claude/settings.local.json` are part of that project trust decision |
| Edits that should block do not | rules file missing or unparseable, or hooks not firing | the checker's `malformed_rules` and `stale_config` cases show the fail-closed behavior to expect: a bad rules file makes `pre-check` exit 2 with an error naming the rule and field; run `python3 "$REPO/examples/first-governed-session/check_tutorial.py"` |
| Doctor seems to claim things you never observed | configuration findings can be mistaken for host observations | re-read Step 3's finding table: `host_trust` and `log_evidence` are always `not_observed`, and only the Step 5 walkthrough is a host observation |

## Where to next

- [Loop-Based Agent Programming](../guides/loop-programming-guide.md) — keep
  a long propose/act/observe loop on the rails.
- [Phronesis for Coding Agents](../guides/FOR_AGENTS.md) — evaluate Phronesis
  read-only on your own repository.
- The [rule catalogue](https://awaterma.github.io/phronesis/catalogue.html)
  and the [visual explainer](https://awaterma.github.io/phronesis/explainer.html).

Back to the [tutorials index](README.md).

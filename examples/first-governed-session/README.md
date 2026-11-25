# First governed session — disposable examples

[First governed session](../../../docs/tutorials/first-governed-session.md)
uses **`tdd-practice/`**, a standalone Rust crate with no dependencies. Start
from its deliberately incomplete `visits_today` implementation, add the
published behavioral tests, observe the intended failure, then finish the task
with a compliant implementation and passing tests. Its one structural rule,
`tutorial-no-unwrap-in-src`, blocks actual `.unwrap()` invocations under
`src/`. Comments containing that spelling are allowed. No tests ship inside
the starter crate: the reader adds them first.

[Verify your installation](../../../docs/tutorials/verify-installation.md)
uses **`practice/`**, the original language-independent marker fixture:

- `.phronesis/rules.json` blocks proposed content containing
  `TUTORIAL-BLOCKED-MARKER`, with rule `no-tutorial-blocked-marker`.
- `notes.md` is the interactive edit target.
- `src/visit.rs` is an additional non-compiled hook-check fixture.

Copy either fixture into a scratch directory before using it. Neither
requires user-scope MCP registration. TDD build caches live outside the
scratch copy and are retained after cleanup.

## Executable checks

From the Phronesis checkout, with Python 3, Cargo (Rust 1.90+) and a binary
supporting `doctor`:

```sh
python3 examples/first-governed-session/check_tutorial.py --phr-mcp /path/to/phr-mcp -v --keep
```

The checker uses the Python standard library. The TDD case executes the
published setup, test source, implementation snippets and Cargo command;
it checks the specific assertion failure, the structural rule's block,
unchanged source after denial, allowed recovery, and both unchanged tests
passing. Comments do not spuriously trigger the structural rule. Cargo runs
offline with one job and no crate dependencies. Missing Cargo is a failure,
not a skipped success.

The existing fixture, marker, doctor and negative-configuration checks remain.
Doctor checks fail by default when the binary lacks the command;
`--allow-missing-doctor` explicitly permits legacy skips, including the TDD
case because its setup runs doctor. `--list` shows all cases. `--keep` retains
the scratch projects and TDD `setup.log`, `red.log`, `blocked-write.log`, and
`green.log`; Cargo caches are retained separately even without `--keep`.

These are direct hook checks, **not a live agent session**: the checker sends
proposals to `pre-check` and applies only allowed edits itself. Follow the
interactive tutorial to observe the host enforce a real tool invocation.
Neither this checker nor the example rule proves an agent followed TDD; the
walkthrough explicitly records that chronology.

# Coverage sample fixture

The `safe_divide` sample from `docs/specs/SPEC-coverage-evidence.md` §1,
verbatim: one function, three tests, one interesting branch.

`export.jsonl` is **regenerated** by `./regenerate-export.sh` (requires
`cargo-llvm-cov` 0.8.x) from real per-test coverage runs and is committed
as the golden input for the importer and golden-trace tests. Regenerating
is one command; the committed file is derived, but committing it keeps
the default test path free of any toolchain dependency (SPEC §8:
opt-in integration runs the real toolchain so this command cannot rot).
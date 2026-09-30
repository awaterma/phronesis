# TypeScript lcov fixture

Neither vitest nor c8 is installed in this repository's development
environment. The lcov and manifest are hand-crafted from a real
vitest 2.1.9 + @vitest/coverage-v8 run performed while this fixture was
written (observed shape: flat V8 `FN`/`FNDA` names such as `load`,
relative `SF:` paths, an empty `TN:` line, and `FNDA:0` for a one-liner
whose declaration line is marked by module evaluation). The integration
test replaces the manifest revision and digest after copying the fixture
into a temporary git repository. The fixture pins Review Focus 1:
`Store::load` is attributed from its executed body lines, while
`oneLiner` — a single-line arrow whose declaration line has a positive
`DA` but whose `FNDA` is zero — is reported unattributable, never guessed
from the declaration line.

To regenerate with vitest installed, run:

    npx vitest run tests/store.test.ts -t 'Store loads' --coverage.enabled --coverage.provider=v8 --coverage.reporter=lcov --coverage.reportsDirectory=cov/1

then prepend the `TN:typescript:ts-store#test:store.test.ts::tests::store.test::Store loads`
and `# node: tests/store.test.ts -t Store loads` lines, move the file to
`cov/typescript_ts-store_test_store.test.ts__tests__store.test__Store_loads.lcov`, and write
`cov/manifest.json` with `git rev-parse HEAD` and the SHA-256 of
`src/store.ts` (plus `"runner": "vitest"`).
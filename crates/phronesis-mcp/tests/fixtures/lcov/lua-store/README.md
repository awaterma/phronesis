# Lua lcov fixture

The environment used for this fixture did not have `busted`, `luacov`, or
the `luacov-reporter-lcov` rock installed (only the plain `lua`
interpreter is on PATH). The LCOV and manifest are therefore hand-crafted
from `luacov-reporter-lcov`'s documented record shape: `SF`, `DA`
(optionally with a source-md5 third field, which the importer tolerates),
`LH`, `LF`, and `end_of_record` — no `TN`, `FN`, or `FNDA`, which is why
`Store.reset`, a one-line function, imports as unattributable rather than
hit. The integration test replaces the manifest revision and digest after
copying the fixture into a temporary git repository.

To regenerate with busted and luacov installed, run the isolated
collection the collector emits (`phr-mcp coverage collect --tool lua-cov
--emit-script`) for the single test in `spec/store_spec.lua`, then
`luacov -r lcov`, and prepend the `TN:` line naming the graph test id
`lua:project::spec::store_spec::store::loads the stored value`.

# Python lcov fixture

The environment used for this fixture did not have `python3 -m coverage` or
`python3 -m pytest` installed (both version probes reported `No module named`).
The LCOV and manifest are therefore hand-crafted from coverage.py's documented
`TN`, `SF`, `FN`, `FNDA`, and `DA` record format. The integration test replaces
the manifest revision and digest after copying the fixture into a temporary git
repository. The fixture pins the expected body-line behavior: `load` is hit,
while the imported `save` definition line alone does not make its body hit.

To regenerate with coverage.py 7.x and pytest when available, run isolated
coverage collection for `tests/test_store.py::test_load`, then
`python3 -m coverage lcov -o cov/test_load.lcov`.

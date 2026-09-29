# pytest collection fixture

The current environment does not have pytest installed (`python3 -m pytest
--version` reports `No module named pytest`), so this committed output is
hand-crafted to match `python -m pytest --collect-only -q`, including a
parameterized id, class method, warning summary, and collection trailer.

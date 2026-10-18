# Python-starter external pack

This pack recognizes Python grammar in a deliberately unclaimed `.pystarter`
file. The query-only variant emits a declaration and two namespaced facts for
each real function definition. Its rule joins the two facts on file and name
and blocks a function named `danger`. A comment or string with that text does
not match the query. The `rhai/` variant adds a bounded script that emits
`pack_python_starter_script_seen` from the same captures.

Use **separate throwaway projects** for the two variants; both claim ID
`python_starter` and extension `.pystarter`.

```sh
phr-mcp pack validate examples/language-packs/python-starter/query-only --json
cd /tmp/my-pystarter-project
phr-mcp pack install /path/to/examples/language-packs/python-starter/query-only
phr-mcp pack list --json
cp /path/to/examples/language-packs/python-starter/sample.pystarter .
phr-mcp audit --rule python_starter.review_function
phr-mcp graph rebuild
phr-mcp graph query defines_fn danger
```

To try the Rhai variant, use another project and replace `query-only` with
`rhai`. Both variants have a non-executable empty-source fixture checked by
`pack validate`. The sample file is intentionally outside the pack source
folders so only manifest-declared assets are copied at install. For recovery
from a digest mismatch, remove `.phronesis/packs/python_starter` in that
project and reinstall from validated source; never edit the installed bytes.

The [language-pack guide](../../../docs/LANGUAGE-PACKS.md) documents the
manifest, record templates, sandbox, evidence limits, and Go starter.

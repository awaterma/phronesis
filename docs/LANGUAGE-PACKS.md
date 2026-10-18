# Language packs

Language packs add bounded source-level structure to Phronesis without adding a native language adapter. The host compiles a declared tree-sitter query against one of its bundled grammars, runs optional sandboxed Rhai, validates records, and assigns pack ID, version, content digest, and source spans. An installed pack can contribute graph declarations/references/imports, namespaced facts, rules, and toolchain definitions. These are advisory source observations: they do not prove compilation, dynamic coverage, or runtime behavior.

## Install and inspect

```sh
phr-mcp pack validate examples/language-packs/python-starter/query-only --json
cd /path/to/project
phr-mcp pack install /absolute/path/to/python-starter/query-only
phr-mcp pack list --json
phr-mcp graph rebuild
```

`validate` compiles the declared query/script, checks resource IDs and fixture expectations, and does not install. `install --dry-run` previews. Installed bytes live under `.phronesis/packs/<id>/` with digest metadata. `pack list --json` reports built-in and installed packs, capability activation, diagnostics, and an overall `complete` flag. A running MCP server exposes the same inventory through read-only `list_language_packs {}` and refreshes it on each request. If an installed file changes, validation fails with a digest error; remove the installed directory and reinstall from validated source rather than editing it in place.

`phr-mcp init --packs go` selects the bundled Go starter rules and toolchains for a Go project. Built-in structural/graph support is available without copying rules. Project rules and toolchains with the same exact ID override a pack resource; `rule_overridden` records provenance. Pack rules never become project `rules.json` entries through server autosave. A script pack requires the `rhai` build feature; the query-only variant works without it.

## Author a pack

The complete external example is [`../examples/language-packs/python-starter/`](../examples/language-packs/python-starter/). Its `.pystarter` extension deliberately avoids native `.py` ownership. Install `query-only/` and `rhai/` into **separate projects** because they share the same ID and extension. Both contain a `pack.json`, query, rules, and fixture; the Rhai variant adds `adapter.rhai` and emits one extra fact. The rule joins two query facts for the same file and function name. A comment or string spelling `def danger` does not produce a function-definition capture.

`pack.json` schema/API version is 1. Required fields are `id`, `version`, `language`, `extensions`, `grammar`, `capabilities`; explicit relative paths declare `queries`, optional `adapter`, `rules`, `toolchains`, `context_files`, and fixture source/expected pairs. IDs use lowercase letters, digits, and underscores; rule and toolchain IDs start with `<pack-id>.`. Valid grammars are compiled into the binary (`go`, `java`, `lua`, `python`, `rust`, `swift`, `tsx`, `typescript`). Installed packs cannot claim an extension owned by a built-in or native language. Resource paths cannot escape the pack source directory; duplicate, unknown, oversized, and malformed resources fail validation. Only declared assets are copied.

A query capture exposes its node kind, text, byte span, and one-based lines. An `emit` entry is a record-shaped JSON object. `${name}` and `${file}` substitute only as an entire string value; `${name.start_byte}` yields the capture's byte offset. `span_capture` names the capture providing the record span. The supported record kinds are `declaration`, `reference`, `import`, and `fact`. A fact's local name becomes `pack_<id>_<name>` and receives the repository-relative file as its first argument. References retain their written target and resolution status; unresolved or ambiguous references do not create call edges. Tests require an explicit declaration test identity and command; a static relationship is not an observed coverage hit.

Optional Rhai receives immutable `context` and `matches` values, returns an array of record maps, and cannot access a filesystem, process, network, or dynamic `eval`. The host owns provenance and validates every record and span. Keep scripts small and deterministic. Fixture expected files contain canonical `{"records": [...], "diagnostics": [...]}`; fixtures are data, never executable commands. Run `pack validate` before install, then exercise hooks, audit, and graph in a throwaway project.

## Go and evidence limits

The bundled Go pack extracts package-scoped declarations, imports, calls, and `testing.T` tests from source. Same-scope, unambiguous free-function calls may resolve; receiver/interface dispatch, external targets, ambiguity, build tags, cgo, generated expansion, and compiler type resolution remain outside this source-level model. The starter's dot-import policy is an advisory post-check/audit warning. `go build`, `go vet`, and textual `go test` outputs are separate outcome signals; an absent exit or truncated output must not become a pass. Use real `go test` for validation. `coverage select` labels static reach separately from dynamic observations, and no pack manufactures dynamic coverage.

The reproducible Go smoke is `scripts/verify-language-packs.py --binary /absolute/path/to/phr-mcp --artifacts /absolute/path/to/evidence --go /absolute/path/to/go`. It requires a real Go executable and records commands, revisions, exits, raw output, and durations. A passing test, deliberately failing test, and compile failure are separate evidence. An unavailable Go toolchain fails the verifier.

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

`validate` compiles the declared query/script, checks resource IDs and fixture expectations, and does not install. `install --dry-run` previews. Installed bytes live under `.phronesis/packs/<id>/` with digest metadata. `pack list --json` reports built-in and installed packs, capability activation, diagnostics, and an overall `complete` flag. A running MCP server exposes the same inventory through read-only `list_language_packs {}` and refreshes it on each request. If an installed file changes, validation fails with a digest error. On Unix, remove it with `phr-mcp pack remove <id>` and reinstall from validated source rather than editing it in place; other platforms require the owner recovery described below. A pack that fails to load blocks every governed tool call, like a malformed `rules.json`: the error names the broken pack's directory, and on Unix the only commands the hooks admit until it loads are lone `phr-mcp pack install|remove|list|validate` invocations: the hook's own binary written as an absolute path (a bare `phr-mcp` or a relative path is resolved by the shell, which the hook cannot see), exactly that subcommand's arguments, and for `install` and `remove` an absolute `--path` naming this project. Only plain characters and single-quoted literal words are accepted, so no chaining, pipes, redirects, substitutions, or expansions. On Unix, the error message prints the exact repair command, single-quoting paths that need it. `pack remove` refuses built-in packs, symlinks, and anything that is not a directory under `.phronesis/packs/`. Removal is supported on Unix only, where operations are anchored to an opened directory descriptor. On non-Unix platforms (including Windows), it exits with an unsupported-platform error before touching the filesystem; removal and automatic remove/reinstall recovery remain unavailable until equivalent anchoring is implemented. While an installed pack is broken, non-Unix hooks admit only the narrowly validated read-only `pack list` and `pack validate` diagnostics; they refuse `pack install` and `pack remove` as automatic recovery. The message asks the project owner to restore the exact installed directory from a trusted, digest-consistent backup or quarantine it outside agent hooks before reinstalling from validated source.

`phr-mcp init --packs go` selects the bundled Go starter rules and toolchains for a Go project. Built-in structural/graph support is available without copying rules. Project rules and toolchains with the same exact ID override a pack resource; `rule_overridden` records provenance. Pack rules never become project `rules.json` entries through server autosave. A script pack requires the `rhai` build feature; the query-only variant works without it.

## Author a pack

The complete external example is [`../examples/language-packs/python-starter/`](../examples/language-packs/python-starter/). Its `.pystarter` extension deliberately avoids native `.py` ownership. Install `query-only/` and `rhai/` into **separate projects** because they share the same ID and extension. Both contain a `pack.json`, query, rules, and fixture; the Rhai variant adds `adapter.rhai` and emits one extra fact. The rule joins two query facts for the same file and function name. A comment or string spelling `def danger` does not produce a function-definition capture.

`pack.json` schema/API version is 1. Required fields are `id`, `version`, `language`, `extensions`, `grammar`, `capabilities` (which decide what runs: facts reach hooks and audit only with `structural_facts`, declarations, references, imports and static selection only with `graph`, and a pack with neither is never parsed); explicit relative paths declare `queries`, optional `adapter`, `rules`, `toolchains`, `context_files`, and fixture source/expected pairs. Optional `test_file_suffixes` (Go: `["_test.go"]`) classifies a matching file as a test file even when it declares only helpers; each suffix must end in one of the pack's extensions. IDs use lowercase letters, digits, and underscores; rule and toolchain IDs start with `<pack-id>.`. Because facts are named `pack_<id>_<name>`, two packs cannot coexist when one id is the other plus `_…` (`demo` and `demo_x`), and an id that would open a host-owned `pack_*` relation (`file`, `test`) is reserved. Valid grammars are compiled into the binary (`go`, `java`, `lua`, `python`, `rust`, `swift`, `tsx`, `typescript`). Installed packs cannot claim an extension owned by a built-in or native language. Resource paths cannot escape the pack source directory; duplicate, unknown, oversized, and malformed resources fail validation. Only declared assets are copied.

A query capture exposes its node kind, text, byte span, and one-based lines. An `emit` entry is a record-shaped JSON object. `${name}` and `${file}` substitute only as an entire string value; `${name.start_byte}` yields the capture's byte offset. `span_capture` names the capture providing the record span. The supported record kinds are `declaration`, `reference`, `import`, and `fact`. A fact's local name becomes `pack_<id>_<name>` and receives the repository-relative file as its first argument. References retain their written target and resolution status; unresolved or ambiguous references do not create call edges. Tests require an explicit declaration test identity and command; a static relationship is not an observed coverage hit.

Optional Rhai receives immutable `context` and `matches` values, returns an array of record maps, and cannot access a filesystem, process, network, or dynamic `eval`. The host owns provenance and validates every record and span. Keep scripts small and deterministic. Fixture expected files contain canonical `{"records": [...], "diagnostics": [...]}`; fixtures are data, never executable commands. Bundled packs are held to their fixtures by a unit test; after a deliberate query or adapter change, `PHR_BLESS_FIXTURES=1 cargo test -p phronesis-mcp --lib bundled_pack_fixtures` rewrites the expectations from the actual extraction, and the diff must be reviewed before it is committed. Run `pack validate` before install, then exercise hooks, audit, and graph in a throwaway project.

## Go and evidence limits

The bundled Go pack extracts package-scoped declarations, imports, calls, and `testing.T` tests from source. Same-scope, unambiguous free-function calls may resolve; receiver/interface dispatch, external targets, ambiguity, build tags, cgo, generated expansion, and compiler type resolution remain outside this source-level model. The starter's dot-import policy is an advisory post-check/audit warning. `go build`, `go vet`, and textual `go test` outputs are separate outcome signals; an absent exit or truncated output must not become a pass. Use real `go test` for validation. `coverage select` labels static reach separately from dynamic observations, and no pack manufactures dynamic coverage.

The reproducible Go smoke is `scripts/verify-language-packs.py --binary /absolute/path/to/phr-mcp --artifacts /absolute/path/to/evidence --go /absolute/path/to/go`. It requires a real Go executable and records commands, revisions, exits, raw output, and durations. A passing test, deliberately failing test, and compile failure are separate evidence. An unavailable Go toolchain fails the verifier.

### Structural demand and lexical limits

Hooks activate a source pack's structural extractor only when an active rule
in the current phase requests that pack's namespaced predicates. Empty rule
sets, lexical-only rules, and another pack's predicates do not make ordinary
Go edits depend on Go syntax, script budgets, or optional grammar support.
Opaque `__script__` guards conservatively demand structural extraction because
they can inspect facts and compute predicate names.
Requested structural evidence still fails closed when unavailable or invalid;
installed-pack integrity validation remains mandatory.

Lexical-only audit reads of pack-owned source use `PHRONESIS_MAX_FILE_BYTES`
(default 1 MiB, bounded at 64 MiB). Files refused by that read are listed in
`lexical_excluded`. Raising this limit permits a complete lexical scan; it does
not raise a structural extractor's limit. A source that cannot be examined for
requested pack predicates makes the audit incomplete.

Native graph source symlinks are ignored consistently by rebuild, freshness,
and save handling. Pack-owned graph source symlinks remain unsafe inputs and
are reported rather than followed. Saves through links rebuild the graph to
purge any previously indexed regular-file evidence; repeated linked-path saves
therefore incur a full rebuild.

### Pack-load failures and cached graph state

A failed installed-pack load is reported against that pack's exact directory
when known; inventory-wide failures name the pack store. Native-file freshness
checks continue, so an independent native edit or deletion remains visible.
Pack sources and context inputs whose ownership cannot be loaded are unverified,
not falsely reported as deleted. A failed rebuild leaves the previous graph and
index intact, including evidence from healthy packs; restoring the original pack
bytes does not require undoing destructive cache changes.

The preserved graph is still an untrusted cached snapshot until validation
succeeds. Hooks continue to fail closed: an installed pack can contribute rules
for any file, so a damaged policy cannot safely be ignored by extension. Audit
retains native findings it can compute but reports the inventory failure as
incomplete rather than certifying unknown policy. This failure does not mean
that every native source file changed.

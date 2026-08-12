# SPEC: Lua language pack and code-graph extractor

**Status:** draft, revision 1, 2026-08-11
**Target release:** a future MINOR release
**Parent spec:** `docs/specs/SPEC-triple-store-rete.md` (revision 7)
**Affects:** `crates/phronesis-mcp/src/graph/{unit,lua,sync,mod}.rs`,
`crates/phronesis-mcp/src/syntax/{facts,lua,mod}.rs`, init, tests, catalogue,
and pack documentation

## Summary

Add an opt-in `lua` pack and a Lua structural extractor. Lua participates in
the same durable graph as Rust, Python, TypeScript, and Java by emitting the
existing closed relation vocabulary. A Lua module has a language-qualified
identity, functions are qualified beneath it, and statically resolvable
`require` calls emit ordinary `imports` edges. No Lua-only graph or derivation
path is introduced.

The pack begins with a small, warning-first set of syntax rules. Its primary
value is graph participation: mixed repositories can query Lua definitions,
tests, imports, and cycles alongside every other supported language.

## Authority and compatibility target

The language baseline is Lua 5.4. Module loading follows the default behavior
documented in the [Lua 5.4 reference manual, section 6.3](https://www.lua.org/manual/5.4/manual.html#6.3):
`require` first checks `package.loaded`, then delegates to configurable
`package.searchers`, including Lua and C loaders searched through
`package.path` and `package.cpath`.

LuaJIT, Lua 5.1 through 5.3, OpenResty, Neovim, and game engines remain useful
corpora, but their custom loaders and globals are not assumed to have standard
resolution semantics. Project configuration may add roots; it may not turn a
dynamic module expression into a static edge.

## Goals

1. Add `.lua` ownership, extraction, freshness tracking, and graph rebuilds.
2. Resolve repository-local literal `require` calls with explicit,
   deterministic search roots.
3. Extract functions and direct test calls without claiming full Lua name
   resolution.
4. Coexist with other language identities in one graph and allow genuine
   cross-language edges only when configuration explicitly maps them.
5. Ship a conservative base pack whose warnings are AST-backed.
6. Count every unsupported or ambiguous dependency shape.

## Non-goals

- Executing Lua, rockspecs, build scripts, or custom searchers.
- Inferring modules loaded through computed strings, `dofile`, `loadfile`,
  `load`, FFI, C loaders, or framework resource APIs in v1.
- Whole-program table, metatable, alias, or call-target inference.
- Treating every global access as a defect.
- Assuming Busted, LuaUnit, Telescope, or a game engine is installed.
- Adding a Lua-specific cycle algorithm.

## Shared graph contract

The extractor emits only existing base relations:

| Relation | Lua meaning |
|---|---|
| `graph_file(file)` | A tracked `.lua` source file. |
| `file_type(file, kind)` | `production`, `test`, `example`, or `build`. |
| `declares_module(file, module)` | The resolved Lua module owning the file. |
| `graph_module(module)` | The language-qualified module node. |
| `defines_fn(file, function)` | A named or assigned function definition. |
| `graph_function(function)` | A production callable. |
| `graph_test(test)` | A recognized test callable. |
| `element_in_file(element, file)` | Exact file containment. |
| `element_in_module(element, module)` | Exact module containment. |
| `imports(from, to)` | A literal `require` resolved to a tracked repository module. |
| `tested_by(function, test)` | A recognized test directly calls a named function. |
| `calls_api(function, api)` | A call to the closed Lua risk watchlist. |

`imports` has the same meaning in every extractor: evaluating or building the
left module has a statically resolvable dependency on the right repository
module. Both arguments remain opaque, language-qualified identities, so the
existing `in_cycle` derivation requires no special case. A future explicit
loader map may point a Lua `require` at a generated or embedded node owned by
another language, but v1's default resolver produces Lua-to-Lua edges.

## Identity and discovery

```text
LuaUnitId     = lua:<unit>
LuaModuleId   = lua:<unit>::<module segments>
LuaFunctionId = lua:<unit>::<module segments>::<function segments>
```

Segments use `::`, matching the graph-wide identity contract. Dots and slashes
in source module names normalize to segments: `require("a.b")` and
`require("a/b")` target the same identity only if ordered probing selects the
same file. Identity comes from the selected repository file, not directly from
the spelling passed to `require`.

Unit selection, in order:

1. The nearest containing rockspec supplies its literal `package` value.
2. A project override in `.phronesis/graph.toml` supplies a stable unit id.
3. Otherwise use `lua:project`.

Rockspecs are Lua programs. Discovery must not execute them. V1 reads only a
literal top-level `package = "..."`; computed forms are
`manifest_dynamic`. Duplicate unit ids are rejected and counted, never merged
by traversal order.

### Module paths and search roots

The canonical module path is the repo-relative file path beneath the selected
import root, without `.lua`; trailing `/init.lua` names its parent. Default
roots, in order, are the unit root, `<unit>/lua`, and `<unit>/src`. Projects may
configure ordered additions in `.phronesis/graph.toml`:

```toml
[languages.lua]
roots = ["lua", "src"]
patterns = ["?.lua", "?/init.lua"]
```

Only `?` substitution is supported. Absolute paths and paths leaving the
repository are rejected. Resolution probes patterns in order and accepts an
edge only when the first matching tier selects exactly one tracked `.lua`
file. The resolver never consults the host's `LUA_PATH`, current directory,
user cache, or installed rocks because those make derived state
machine-dependent.

### File classification

Production wins over test when claims conflict. A file is `test` under
`test/`, `tests/`, or `spec/`, or with `_spec.lua` / `_test.lua`; `example`
under `example/` or `examples/`; `build` for rockspecs and explicit build/tool
directories; otherwise `production`. Configuration may extend these patterns.

## Extraction

Use a maintained tree-sitter Lua grammar compatible with the workspace's
tree-sitter version. Pin its version and corpus before implementation. If no
grammar passes the gate, use a small parser for the required subset; regex
extraction is not acceptable.

Recognized definitions:

- `function f(...) ... end`
- `local function f(...) ... end`
- `function M.f(...) ... end`
- `M.f = function(...) ... end`
- `local f = function(...) ... end`

Nested named functions append lexical segments. Anonymous callbacks assigned
inside table constructors, returned, or passed as arguments do not receive a
fabricated stable identity. Repeated assignment to one name deduplicates the
element and increments `redefinition` because runtime meaning is ordered.

### Import extraction

Recognize literal calls in ordinary forms:

```lua
local x = require("a.b")
local y = require "a.b"
require('a/b') -- accepted extension; not standard Lua module spelling
```

Aliases of `require`, reassignment of `require`, computed arguments, protected
loads such as `pcall(require, name)`, and custom loaders are not resolved in
v1. Literal `dofile` and `loadfile` are counted as
`dynamic_loader_literal`; they do not emit `imports` until their execution-root
semantics are separately specified.

Slash-form module names are accepted for compatibility with custom loaders but
counted as `import_slash_form`; standard Lua path substitution is defined for
dot-separated module names.

An unresolved literal increments `import_not_found`, `import_ambiguous`,
`import_external`, or `import_unsupported`. The aggregate `skipped` field
remains for existing status surfaces; detailed counters appear in rebuild
diagnostics.

### Tests and direct coverage

Recognize conservatively:

- Busted `it("title", function() ... end)` / `test(...)` callbacks inside a
  `describe` tree;
- LuaUnit functions named `test*` in a test-class table and global/local
  functions named `test_*`;
- project-configured test-call names.

Test identity is the module plus normalized suite titles and test title or
function name. Only calls in the test callback/body produce `tested_by`.
Helpers are not proof of verification. Coverage uses the existing conservative
short-name bridge, so same-named methods may be over-covered rather than
falsely reported untested.

### Risk watchlist

V1 emits `calls_api` only for bare `error`, through an explicit Lua watchlist
entry. Bare `load`, `loadfile`, and `dofile` instead emit syntax facts consumed
by the dynamic-load rule; they do not also produce `calls_api`. `assert` is
excluded because it is Lua's ordinary input-validation idiom. Member calls do
not match. The graph configuration grows a language-scoped
`[languages.lua.calls_api]` list rather than reusing Rust's default names.

## Starter pack

| Rule id | Phase | Audit | Predicate/shape |
|---|---:|---:|---|
| `warn-lua-dynamic-code-load` | pre | yes | Static `loadfile` / `dofile` / legacy `loadstring` call in a production Lua module; v1 evidence is module-scoped. |
| `audit-lua-unresolved-require` | deferred | no | Requires repository-index resolution; a per-file extractor must not guess that a static `require` is unresolved. |
| `warn-lua-untested-abort` | deferred | no | Requires the coverage derivation to expose Lua production/test ownership. |
| `warn-import-cycle` | pre | yes | Shared structural rule over Lua or mixed-language SCCs. |

The pack must not use substring rules. `assert` is common and legitimate; the
untested join supplies useful context. No v1 rule blocks.

## Incremental invalidation

Edits to `.lua` files compact by provenance. Changes to a rockspec,
`.phronesis/graph.toml`, or configured root/pattern can reclassify many files
and require `rebuild()`, not `on_save`. Adding, deleting, or renaming `.lua`
files can change resolution of existing requires and also rebuilds unless the
synchronizer gains a reverse unresolved-import index.

The coordinated language-pack release adopts parent-spec graph format 5 because
the shared `imports` meaning and definition contracts change. Lua alone would
otherwise be additive, but it must not write a format-4 graph with revision-7
semantics.

## Pack mechanics

Add `Pack::Lua`, `rules()`, `label()`, and `Pack::ALL`. Register `.lua` in
language dispatch, unit discovery, rebuild enumeration, and graph status.
Update CLI valid-pack output, catalogue, README/CLAUDE/AGENTS pack tables, and
examples. `base` remains language-agnostic.

## Testing and evidence gate

- Parser fixtures for every definition and require shape, malformed input,
  comments/strings, shadowed `require`, and supported Unicode identifiers.
- Discovery with zero, one, nested, dynamic, and colliding rockspecs.
- Resolution for dot/slash names, `init.lua`, ordered roots, ambiguity,
  external modules, traversal attempts, and file add/delete.
- Busted and LuaUnit extraction, including nested-helper negatives.
- Integration through rebuild, save, hydrate, query, audit, and a real
  mixed-language cycle created through an explicit loader mapping.
- A real-project corpus from one library and one application. Report tracked
  files, imports by outcome, functions, tests, edges, rebuild time, and manual
  inspection of every cycle.
- Workspace format, tests, clippy, and `git diff --check`.

## Risks and honest limits

Lua deliberately permits runtime replacement of loaders and namespaces. The
graph describes the default static convention plus explicit configuration; it
is not proof of what an embedded runtime loads. Findings must preserve that
limit. A missing edge with a counted reason is preferable to an invented one.
